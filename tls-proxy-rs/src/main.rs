use std::{
    env, fs,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};

mod client;

const IO_TIMEOUT: Duration = Duration::from_millis(50);
const COPY_BUFFER: usize = 64 * 1024;
static TEMPORARY_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct Options {
    listen: SocketAddr,
    upstream: PathBuf,
    certificate: PathBuf,
    private_key: PathBuf,
}

fn main() -> ExitCode {
    let command_arguments = env::args().skip(1).collect::<Vec<_>>();
    let result = if command_arguments.first().map(String::as_str) == Some("connect") {
        client::arguments(command_arguments.into_iter().skip(1)).and_then(client::run)
    } else {
        arguments(command_arguments).and_then(run)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xd-tls-proxy-dev: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(options: Options) -> Result<(), String> {
    thread::Builder::new()
        .name("xd-tls-parent-watch".into())
        .spawn(|| {
            let mut buffer = [0_u8; 1];
            while io::stdin().read(&mut buffer).is_ok_and(|count| count > 0) {}
            std::process::exit(0);
        })
        .map_err(|error| format!("cannot monitor the daemon process: {error}"))?;
    let config = Arc::new(load_server_config(
        &options.certificate,
        &options.private_key,
    )?);
    let listener = TcpListener::bind(options.listen)
        .map_err(|error| format!("cannot listen on {}: {error}", options.listen))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("cannot inspect the TLS listener: {error}"))?;
    println!("{{\"port\":{}}}", address.port());
    io::stdout()
        .flush()
        .map_err(|error| format!("cannot report the TLS listener: {error}"))?;

    for connection in listener.incoming() {
        let Ok(connection) = connection else {
            break;
        };
        let config = config.clone();
        let upstream = options.upstream.clone();
        let _ = thread::Builder::new()
            .name("xd-tls-remote-session".into())
            .spawn(move || {
                let _ = proxy_connection(connection, &upstream, config);
            });
    }
    Ok(())
}

fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut arguments = arguments.into_iter();
    if arguments.next().as_deref() != Some("serve") {
        return Err("expected the serve command".into());
    }
    let mut listen = None;
    let mut upstream = None;
    let mut certificate = None;
    let mut private_key = None;
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("{argument} needs a value"))?;
        match argument.as_str() {
            "--listen" => {
                listen = Some(
                    value
                        .parse()
                        .map_err(|_| "--listen needs an IP address and port".to_owned())?,
                );
            }
            "--upstream" => upstream = Some(PathBuf::from(value)),
            "--certificate" => certificate = Some(PathBuf::from(value)),
            "--private-key" => private_key = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown argument {argument}")),
        }
    }
    Ok(Options {
        listen: listen.ok_or_else(|| "--listen is required".to_owned())?,
        upstream: upstream.ok_or_else(|| "--upstream is required".to_owned())?,
        certificate: certificate.ok_or_else(|| "--certificate is required".to_owned())?,
        private_key: private_key.ok_or_else(|| "--private-key is required".to_owned())?,
    })
}

fn load_server_config(certificate: &Path, private_key: &Path) -> Result<ServerConfig, String> {
    let (certificate, private_key) = ensure_certificate(certificate, private_key)?;
    let provider = rustls::crypto::ring::default_provider();
    let mut config = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("cannot configure TLS: {error}"))?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificate)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key)),
        )
        .map_err(|error| format!("cannot load the TLS identity: {error}"))?;
    config.send_tls13_tickets = 0;
    Ok(config)
}

fn ensure_certificate(
    certificate_path: &Path,
    private_key_path: &Path,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    match (
        read_regular(certificate_path),
        read_regular(private_key_path),
    ) {
        (Ok(Some(certificate)), Ok(Some(private_key))) => {
            fs::set_permissions(private_key_path, fs::Permissions::from_mode(0o600)).map_err(
                |error| format!("cannot secure {}: {error}", private_key_path.display()),
            )?;
            return Ok((certificate, private_key));
        }
        (Err(error), _) | (_, Err(error)) => return Err(error),
        _ => {}
    }

    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(["localhost".to_owned(), "xd".to_owned()])
            .map_err(|error| format!("cannot generate the TLS identity: {error}"))?;
    let certificate_der = cert.der().to_vec();
    let private_key_der = signing_key.serialize_der();
    write_private_atomic(private_key_path, &private_key_der, 0o600)?;
    write_private_atomic(certificate_path, &certificate_der, 0o644)?;
    Ok((certificate_der, private_key_der))
}

fn read_regular(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing non-file TLS identity at {}",
            path.display()
        ));
    }
    fs::read(path)
        .map(Some)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn write_private_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} must have a parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot secure {}: {error}", parent.display()))?;

    let temporary = parent.join(format!(
        ".xd-tls-{}-{}.tmp",
        std::process::id(),
        TEMPORARY_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)
            .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
        file.write_all(contents)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("cannot install {}: {error}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| format!("cannot secure {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn proxy_connection(
    connection: TcpStream,
    upstream_path: &Path,
    config: Arc<ServerConfig>,
) -> io::Result<()> {
    connection.set_nodelay(true)?;
    connection.set_read_timeout(Some(IO_TIMEOUT))?;
    connection.set_write_timeout(Some(IO_TIMEOUT))?;
    let server = ServerConnection::new(config).map_err(io::Error::other)?;
    let mut tls = StreamOwned::new(server, connection);
    let mut upstream = UnixStream::connect(upstream_path)?;
    upstream.set_read_timeout(Some(IO_TIMEOUT))?;
    upstream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut from_tls = [0_u8; COPY_BUFFER];
    let mut from_upstream = [0_u8; COPY_BUFFER];

    loop {
        match tls.read(&mut from_tls) {
            Ok(0) => return Ok(()),
            Ok(count) => upstream.write_all(&from_tls[..count])?,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) if retryable(&error) => {}
            Err(error) => return Err(error),
        }
        match upstream.read(&mut from_upstream) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                tls.write_all(&from_upstream[..count])?;
                tls.flush()?;
            }
            Err(error) if retryable(&error) => {}
            Err(error) => return Err(error),
        }
    }
}

fn retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::{ClientConfig, ClientConnection, RootCertStore, pki_types::ServerName};
    use std::{
        net::IpAddr,
        sync::atomic::{AtomicU64, Ordering},
    };

    static FIXTURE: AtomicU64 = AtomicU64::new(1);

    fn fixture() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "xd-tls-proxy-test-{}-{}",
            std::process::id(),
            FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn parses_the_serve_contract() {
        let options = arguments([
            "serve".into(),
            "--listen".into(),
            "127.0.0.1:0".into(),
            "--upstream".into(),
            "/tmp/xd.remote".into(),
            "--certificate".into(),
            "/tmp/xd.der".into(),
            "--private-key".into(),
            "/tmp/xd.key".into(),
        ])
        .unwrap();
        assert_eq!(options.listen.ip(), IpAddr::from([127, 0, 0, 1]));
        assert_eq!(options.listen.port(), 0);
        assert_eq!(options.upstream, PathBuf::from("/tmp/xd.remote"));
    }

    #[test]
    fn certificate_identity_is_persistent_and_private() {
        let root = fixture();
        let certificate = root.join("certificate.der");
        let key = root.join("private-key.der");
        let first = ensure_certificate(&certificate, &key).unwrap();
        let second = ensure_certificate(&certificate, &key).unwrap();
        assert_eq!(first, second);
        assert!(!first.0.is_empty());
        assert!(!first.1.is_empty());
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&certificate).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn proxies_a_real_tls_session_to_the_private_unix_socket() {
        let root = fixture();
        let certificate = root.join("certificate.der");
        let key = root.join("private-key.der");
        let (certificate_der, _) = ensure_certificate(&certificate, &key).unwrap();
        let server_config = Arc::new(load_server_config(&certificate, &key).unwrap());
        let unix_path = root.join("daemon.remote");
        let unix_listener = std::os::unix::net::UnixListener::bind(&unix_path).unwrap();
        let tcp_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = tcp_listener.local_addr().unwrap();

        let upstream = thread::spawn(move || {
            let (mut stream, _) = unix_listener.accept().unwrap();
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").unwrap();
        });
        let proxy = thread::spawn(move || {
            let (stream, _) = tcp_listener.accept().unwrap();
            proxy_connection(stream, &unix_path, server_config).unwrap();
        });

        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(certificate_der)).unwrap();
        let provider = rustls::crypto::ring::default_provider();
        let client_config = ClientConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connection = ClientConnection::new(
            Arc::new(client_config),
            ServerName::try_from("localhost").unwrap(),
        )
        .unwrap();
        let mut stream = StreamOwned::new(connection, TcpStream::connect(address).unwrap());
        stream.write_all(b"ping").unwrap();
        let mut response = [0_u8; 4];
        stream.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        drop(stream);
        proxy.join().unwrap();
        upstream.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
