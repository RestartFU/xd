use std::{
    env, fs,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{local_socket::UnixStream, private_fs::*};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{
        CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
    },
};

mod client;
mod local_socket;
mod private_fs;

const IO_TIMEOUT: Duration = Duration::from_millis(50);
const WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(30);
const COPY_BUFFER: usize = 64 * 1024;
const MAX_IDENTITY_BYTES: u64 = 1024 * 1024;
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
            eprintln!("xd-tls-proxy: {error}");
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
    let identity = ensure_identity(certificate, private_key)?;
    let provider = rustls::crypto::ring::default_provider();
    let mut config = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("cannot configure TLS: {error}"))?
        .with_no_client_auth()
        .with_single_cert(
            identity
                .certificates
                .into_iter()
                .map(CertificateDer::from)
                .collect(),
            identity.private_key.into_der(),
        )
        .map_err(|error| format!("cannot load the TLS identity: {error}"))?;
    config.send_tls13_tickets = 0;
    Ok(config)
}

struct Identity {
    certificates: Vec<Vec<u8>>,
    private_key: StoredPrivateKey,
}

enum StoredPrivateKey {
    Pkcs8(Vec<u8>),
    Pkcs1(Vec<u8>),
    Sec1(Vec<u8>),
}

impl StoredPrivateKey {
    fn into_der(self) -> PrivateKeyDer<'static> {
        match self {
            Self::Pkcs8(bytes) => PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(bytes)),
            Self::Pkcs1(bytes) => PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(bytes)),
            Self::Sec1(bytes) => PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(bytes)),
        }
    }
}

fn ensure_identity(certificate_path: &Path, private_key_path: &Path) -> Result<Identity, String> {
    match (
        read_regular(certificate_path),
        read_regular(private_key_path),
    ) {
        (Ok(Some(certificate)), Ok(Some(private_key))) => {
            secure_file(private_key_path, 0o600).map_err(|error| {
                format!("cannot secure {}: {error}", private_key_path.display())
            })?;
            return parse_identity(&certificate, &private_key);
        }
        (Err(error), _) | (_, Err(error)) => return Err(error),
        (Ok(None), Ok(None)) => {}
        _ => {
            return Err(format!(
                "TLS certificate and private key must both exist: {} and {}",
                certificate_path.display(),
                private_key_path.display()
            ));
        }
    }

    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(["localhost".to_owned(), "xd".to_owned()])
            .map_err(|error| format!("cannot generate the TLS identity: {error}"))?;
    let certificate_der = cert.der().to_vec();
    let private_key_der = signing_key.serialize_der();
    write_private_atomic(private_key_path, &private_key_der, 0o600)?;
    write_private_atomic(certificate_path, &certificate_der, 0o644)?;
    Ok(Identity {
        certificates: vec![certificate_der],
        private_key: StoredPrivateKey::Pkcs8(private_key_der),
    })
}

fn parse_identity(certificate: &[u8], private_key: &[u8]) -> Result<Identity, String> {
    let certificates = if contains_pem_marker(certificate) {
        pem_blocks(certificate, "CERTIFICATE")?
    } else {
        vec![certificate.to_vec()]
    };
    if certificates.is_empty() || certificates.iter().any(Vec::is_empty) {
        return Err("the TLS certificate file contains no certificates".into());
    }
    let private_key = if contains_pem_marker(private_key) {
        if let Some(key) = pem_blocks(private_key, "PRIVATE KEY")?.into_iter().next() {
            StoredPrivateKey::Pkcs8(key)
        } else if let Some(key) = pem_blocks(private_key, "RSA PRIVATE KEY")?
            .into_iter()
            .next()
        {
            StoredPrivateKey::Pkcs1(key)
        } else if let Some(key) = pem_blocks(private_key, "EC PRIVATE KEY")?
            .into_iter()
            .next()
        {
            StoredPrivateKey::Sec1(key)
        } else {
            return Err("the TLS private-key file contains no supported private key".into());
        }
    } else {
        StoredPrivateKey::Pkcs8(private_key.to_vec())
    };
    Ok(Identity {
        certificates,
        private_key,
    })
}

fn contains_pem_marker(contents: &[u8]) -> bool {
    contents
        .windows(b"-----BEGIN ".len())
        .any(|window| window == b"-----BEGIN ")
}

fn pem_blocks(contents: &[u8], label: &str) -> Result<Vec<Vec<u8>>, String> {
    let contents = std::str::from_utf8(contents)
        .map_err(|_| "TLS PEM identity is not valid UTF-8".to_owned())?;
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut remaining = contents;
    let mut blocks = Vec::new();
    while let Some(start) = remaining.find(&begin) {
        let encoded = &remaining[start + begin.len()..];
        let finish = encoded
            .find(&end)
            .ok_or_else(|| format!("TLS PEM {label} block is incomplete"))?;
        let compact = encoded[..finish]
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>();
        let decoded = STANDARD
            .decode(compact)
            .map_err(|_| format!("TLS PEM {label} block is not valid base64"))?;
        if decoded.is_empty() {
            return Err(format!("TLS PEM {label} block is empty"));
        }
        blocks.push(decoded);
        remaining = &encoded[finish + end.len()..];
    }
    Ok(blocks)
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
    if metadata.len() == 0 || metadata.len() > MAX_IDENTITY_BYTES {
        return Err(format!(
            "TLS identity at {} must be from 1 byte to {MAX_IDENTITY_BYTES} bytes",
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
    secure_directory(parent)
        .map_err(|error| format!("cannot secure {}: {error}", parent.display()))?;

    let temporary = parent.join(format!(
        ".xd-tls-{}-{}.tmp",
        std::process::id(),
        TEMPORARY_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = create_private_file(&temporary, mode)
            .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
        file.write_all(contents)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("cannot install {}: {error}", path.display()))?;
        secure_file(path, mode)
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
            Ok(count) => write_all_retrying(&mut upstream, &from_tls[..count])?,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) if retryable(&error) => {}
            Err(error) => return Err(error),
        }
        match upstream.read(&mut from_upstream) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                write_all_retrying(&mut tls, &from_upstream[..count])?;
                flush_retrying(&mut tls)?;
            }
            Err(error) if retryable(&error) => {}
            Err(error) => return Err(error),
        }
    }
}

/// The proxy polls both directions with short socket timeouts, but a short
/// write timeout must not sever a large frame after forwarding only its prefix.
/// Keep retrying while the peer makes progress and bound only a true stall.
fn write_all_retrying(writer: &mut impl Write, mut bytes: &[u8]) -> io::Result<()> {
    let mut last_progress = Instant::now();
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => {
                bytes = &bytes[count..];
                last_progress = Instant::now();
            }
            Err(error) if retryable(&error) && last_progress.elapsed() < WRITE_STALL_TIMEOUT => {
                thread::yield_now();
            }
            Err(error) if retryable(&error) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "proxy write made no progress for 30 seconds",
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn flush_retrying(writer: &mut impl Write) -> io::Result<()> {
    let started = Instant::now();
    loop {
        match writer.flush() {
            Ok(()) => return Ok(()),
            Err(error) if retryable(&error) && started.elapsed() < WRITE_STALL_TIMEOUT => {
                thread::yield_now();
            }
            Err(error) if retryable(&error) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "proxy flush made no progress for 30 seconds",
                ));
            }
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
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct TemporarilyBlockedWriter {
        blocked: bool,
        bytes: Vec<u8>,
    }

    impl Write for TemporarilyBlockedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.blocked {
                self.blocked = true;
                return Err(io::ErrorKind::TimedOut.into());
            }
            let count = bytes.len().min(3);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn temporary_write_backpressure_does_not_truncate_a_frame() {
        let mut writer = TemporarilyBlockedWriter {
            blocked: false,
            bytes: Vec::new(),
        };

        write_all_retrying(&mut writer, b"complete frame\n").unwrap();

        assert_eq!(writer.bytes, b"complete frame\n");
    }

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
        let first = ensure_identity(&certificate, &key).unwrap();
        let first_certificate = first.certificates[0].clone();
        let second = ensure_identity(&certificate, &key).unwrap();
        assert_eq!(first_certificate, second.certificates[0]);
        assert!(!first_certificate.is_empty());
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
    fn loads_a_pem_identity_without_changing_its_certificate() {
        let root = fixture();
        let certificate_path = root.join("server-cert.pem");
        let key_path = root.join("server-key.pem");
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(["xd".to_owned()]).unwrap();
        let certificate_der = cert.der().to_vec();
        let key_der = signing_key.serialize_der();
        fs::write(&certificate_path, pem("CERTIFICATE", &certificate_der)).unwrap();
        fs::write(&key_path, pem("PRIVATE KEY", &key_der)).unwrap();

        let identity = ensure_identity(&certificate_path, &key_path).unwrap();
        assert_eq!(identity.certificates, vec![certificate_der]);
        load_server_config(&certificate_path, &key_path).unwrap();
        assert_eq!(
            fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_partial_and_oversized_tls_identities() {
        let root = fixture();
        let certificate = root.join("server-cert.pem");
        let key = root.join("server-key.pem");
        fs::write(&certificate, "certificate without its key").unwrap();
        assert!(ensure_identity(&certificate, &key).is_err());
        fs::write(&key, vec![0_u8; MAX_IDENTITY_BYTES as usize + 1]).unwrap();
        assert!(ensure_identity(&certificate, &key).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    fn pem(label: &str, der: &[u8]) -> String {
        let encoded = STANDARD.encode(der);
        let body = encoded
            .as_bytes()
            .chunks(64)
            .map(|line| std::str::from_utf8(line).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n")
    }

    #[test]
    fn proxies_a_real_tls_session_to_the_private_unix_socket() {
        let root = fixture();
        let certificate = root.join("certificate.der");
        let key = root.join("private-key.der");
        let certificate_der = ensure_identity(&certificate, &key).unwrap().certificates[0].clone();
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
