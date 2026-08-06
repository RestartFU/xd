use std::{
    fmt, fs,
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error as TlsError, SignatureScheme,
    StreamOwned,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_millis(50);
const COPY_BUFFER: usize = 64 * 1024;
const MAX_CERTIFICATE_BYTES: usize = 64 * 1024;

pub(crate) struct Options {
    host: String,
    port: u16,
    socket: PathBuf,
    expected: Option<ExpectedCertificate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExpectedCertificate {
    Der(Vec<u8>),
    Fingerprint([u8; 32]),
}

pub(crate) fn arguments(arguments: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut arguments = arguments.into_iter();
    let mut host = None;
    let mut port = None;
    let mut socket = None;
    let mut expected = None;
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("{argument} needs a value"))?;
        match argument.as_str() {
            "--host" => host = Some(value),
            "--port" => {
                port = Some(
                    value
                        .parse::<u16>()
                        .ok()
                        .filter(|port| *port > 0)
                        .ok_or_else(|| "--port must be from 1 to 65535".to_owned())?,
                )
            }
            "--socket" => socket = Some(PathBuf::from(value)),
            "--pin" => {
                let decoded = STANDARD
                    .decode(value)
                    .map_err(|_| "--pin is not valid base64".to_owned())?;
                if decoded.is_empty() || decoded.len() > MAX_CERTIFICATE_BYTES {
                    return Err("--pin is not a valid certificate".into());
                }
                if expected.is_some() {
                    return Err("provide only one certificate pin".into());
                }
                expected = Some(ExpectedCertificate::Der(decoded));
            }
            "--fingerprint" => {
                if expected.is_some() {
                    return Err("provide only one certificate pin".into());
                }
                expected = Some(ExpectedCertificate::Fingerprint(parse_fingerprint(&value)?));
            }
            _ => return Err(format!("unknown argument {argument}")),
        }
    }
    let host = host
        .filter(|host| !host.is_empty() && host.len() <= 253)
        .ok_or_else(|| "--host is required".to_owned())?;
    Ok(Options {
        host,
        port: port.ok_or_else(|| "--port is required".to_owned())?,
        socket: socket.ok_or_else(|| "--socket is required".to_owned())?,
        expected,
    })
}

fn parse_fingerprint(value: &str) -> Result<[u8; 32], String> {
    let compact = value
        .bytes()
        .filter(|byte| *byte != b':')
        .collect::<Vec<_>>();
    if compact.len() != 64 {
        return Err("--fingerprint must contain exactly 64 hexadecimal digits".into());
    }
    let mut fingerprint = [0_u8; 32];
    for (index, pair) in compact.chunks_exact(2).enumerate() {
        let high = hexadecimal(pair[0])
            .ok_or_else(|| "--fingerprint contains a non-hexadecimal digit".to_owned())?;
        let low = hexadecimal(pair[1])
            .ok_or_else(|| "--fingerprint contains a non-hexadecimal digit".to_owned())?;
        fingerprint[index] = (high << 4) | low;
    }
    Ok(fingerprint)
}

fn hexadecimal(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn certificate_fingerprint(certificate: &[u8]) -> [u8; 32] {
    Sha256::digest(certificate).into()
}

fn fingerprint_text(fingerprint: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(64);
    for byte in fingerprint {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 0x0f) as usize] as char);
    }
    text
}

pub(crate) fn run(options: Options) -> Result<(), String> {
    watch_parent()?;
    let config = Arc::new(client_config(options.expected.clone())?);
    let (_, certificate) = connect_tls(&options.host, options.port, config.clone())?;
    let listener = bind_private_socket(&options.socket)?;
    println!(
        "{{\"certificate\":\"{}\",\"fingerprint\":\"{}\"}}",
        STANDARD.encode(&certificate),
        fingerprint_text(&certificate_fingerprint(&certificate))
    );
    io::stdout()
        .flush()
        .map_err(|error| format!("cannot report the remote certificate: {error}"))?;

    for connection in listener.incoming() {
        let Ok(connection) = connection else {
            break;
        };
        let host = options.host.clone();
        let port = options.port;
        let config = config.clone();
        let _ = thread::Builder::new()
            .name("xd-tls-client-session".into())
            .spawn(move || {
                if let Ok((tls, _)) = connect_tls(&host, port, config) {
                    let _ = proxy_connection(connection, tls);
                }
            });
    }
    let _ = fs::remove_file(&options.socket);
    Ok(())
}

fn watch_parent() -> Result<(), String> {
    thread::Builder::new()
        .name("xd-tls-client-parent-watch".into())
        .spawn(|| {
            let mut buffer = [0_u8; 1];
            while io::stdin().read(&mut buffer).is_ok_and(|count| count > 0) {}
            std::process::exit(0);
        })
        .map(|_| ())
        .map_err(|error| format!("cannot monitor the desktop process: {error}"))
}

fn client_config(expected: Option<ExpectedCertificate>) -> Result<ClientConfig, String> {
    let provider = rustls::crypto::ring::default_provider();
    let algorithms = provider.signature_verification_algorithms;
    ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("cannot configure remote TLS: {error}"))
        .map(|builder| {
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(CertificateVerifier {
                    expected,
                    algorithms,
                }))
                .with_no_client_auth()
        })
}

fn connect_tls(
    host: &str,
    port: u16,
    config: Arc<ClientConfig>,
) -> Result<(StreamOwned<ClientConnection, TcpStream>, Vec<u8>), String> {
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("cannot resolve {host}: {error}"))?
        .collect::<Vec<_>>();
    let mut last_error = None;
    let mut socket = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(connected) => {
                socket = Some(connected);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let mut socket = socket.ok_or_else(|| {
        format!(
            "cannot connect to {host}:{port}: {}",
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "no address was found".into())
        )
    })?;
    socket
        .set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|error| format!("cannot configure remote TLS: {error}"))?;
    socket
        .set_write_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|error| format!("cannot configure remote TLS: {error}"))?;
    socket
        .set_nodelay(true)
        .map_err(|error| format!("cannot configure remote TLS: {error}"))?;
    let name = ServerName::try_from("localhost")
        .map_err(|_| "cannot configure the remote TLS server name".to_owned())?;
    let mut connection = ClientConnection::new(config, name)
        .map_err(|error| format!("cannot initialize remote TLS: {error}"))?;
    while connection.is_handshaking() {
        connection
            .complete_io(&mut socket)
            .map_err(|error| format!("remote TLS handshake failed: {error}"))?;
    }
    let certificate = connection
        .peer_certificates()
        .and_then(|certificates| certificates.first())
        .map(|certificate| certificate.as_ref().to_vec())
        .filter(|certificate| !certificate.is_empty() && certificate.len() <= MAX_CERTIFICATE_BYTES)
        .ok_or_else(|| "remote TLS did not provide a valid certificate".to_owned())?;
    socket.set_read_timeout(Some(IO_TIMEOUT)).ok();
    socket.set_write_timeout(Some(IO_TIMEOUT)).ok();
    Ok((StreamOwned::new(connection, socket), certificate))
}

fn bind_private_socket(path: &Path) -> Result<UnixListener, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            if UnixStream::connect(path).is_ok() {
                return Err(format!(
                    "a remote bridge is already listening on {}",
                    path.display()
                ));
            }
            fs::remove_file(path)
                .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
        }
        Ok(_) => {
            return Err(format!(
                "refusing to replace non-socket path {}",
                path.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    let listener = UnixListener::bind(path)
        .map_err(|error| format!("cannot bind {}: {error}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot secure {}: {error}", path.display()))?;
    Ok(listener)
}

fn proxy_connection(
    mut local: UnixStream,
    mut tls: StreamOwned<ClientConnection, TcpStream>,
) -> io::Result<()> {
    local.set_read_timeout(Some(IO_TIMEOUT))?;
    local.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut from_local = [0_u8; COPY_BUFFER];
    let mut from_tls = [0_u8; COPY_BUFFER];
    loop {
        match local.read(&mut from_local) {
            Ok(0) => return Ok(()),
            Ok(count) => tls.write_all(&from_local[..count])?,
            Err(error) if retryable(&error) => {}
            Err(error) => return Err(error),
        }
        match tls.read(&mut from_tls) {
            Ok(0) => return Ok(()),
            Ok(count) => local.write_all(&from_tls[..count])?,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
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

struct CertificateVerifier {
    expected: Option<ExpectedCertificate>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl fmt::Debug for CertificateVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CertificateVerifier")
            .field("pinned", &self.expected.is_some())
            .finish()
    }
}

impl ServerCertVerifier for CertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let matches = match self.expected.as_ref() {
            None => true,
            Some(ExpectedCertificate::Der(expected)) => {
                constant_time_equal(expected, end_entity.as_ref())
            }
            Some(ExpectedCertificate::Fingerprint(expected)) => {
                constant_time_equal(expected, &certificate_fingerprint(end_entity.as_ref()))
            }
        };
        if !matches {
            return Err(TlsError::General("remote certificate changed".into()));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, certificate, signature, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, certificate, signature, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::{ServerConnection, StreamOwned};
    use std::{
        net::TcpListener,
        sync::atomic::{AtomicU64, Ordering},
    };

    static FIXTURE: AtomicU64 = AtomicU64::new(1);

    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "xd-tls-client-test-{}-{}",
            std::process::id(),
            FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn tls_server(root: &Path) -> (u16, Vec<u8>, thread::JoinHandle<()>) {
        let certificate_path = root.join("certificate.der");
        let key_path = root.join("private-key.der");
        let (certificate, _) = crate::ensure_certificate(&certificate_path, &key_path).unwrap();
        let config = Arc::new(crate::load_server_config(&certificate_path, &key_path).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let connection = ServerConnection::new(config).unwrap();
            let mut stream = StreamOwned::new(connection, socket);
            let mut request = [0_u8; 4];
            if stream.read_exact(&mut request).is_ok() {
                assert_eq!(&request, b"ping");
                stream.write_all(b"pong").unwrap();
            }
        });
        (port, certificate, server)
    }

    #[test]
    fn parses_pinned_and_unpinned_client_modes() {
        let unpinned = arguments([
            "--host".into(),
            "desktop.local".into(),
            "--port".into(),
            "4001".into(),
            "--socket".into(),
            "/tmp/xd-remote.sock".into(),
        ])
        .unwrap();
        assert_eq!(unpinned.host, "desktop.local");
        assert_eq!(unpinned.port, 4001);
        assert!(unpinned.expected.is_none());

        let pinned = arguments([
            "--host".into(),
            "desktop.local".into(),
            "--port".into(),
            "4001".into(),
            "--socket".into(),
            "/tmp/xd-remote.sock".into(),
            "--pin".into(),
            STANDARD.encode(b"certificate"),
        ])
        .unwrap();
        assert_eq!(
            pinned.expected,
            Some(ExpectedCertificate::Der(b"certificate".to_vec()))
        );
        let fingerprint = arguments([
            "--host".into(),
            "desktop.local".into(),
            "--port".into(),
            "4001".into(),
            "--socket".into(),
            "/tmp/xd-remote.sock".into(),
            "--fingerprint".into(),
            "00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff".into(),
        ])
        .unwrap();
        assert_eq!(
            fingerprint.expected,
            Some(ExpectedCertificate::Fingerprint([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb,
                0xcc, 0xdd, 0xee, 0xff,
            ]))
        );
        assert!(
            arguments([
                "--host".into(),
                "host".into(),
                "--port".into(),
                "0".into(),
                "--socket".into(),
                "/tmp/socket".into(),
            ])
            .is_err()
        );
        assert!(
            arguments([
                "--host".into(),
                "host".into(),
                "--port".into(),
                "4001".into(),
                "--socket".into(),
                "/tmp/socket".into(),
                "--fingerprint".into(),
                "not-a-fingerprint".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn tofu_returns_the_leaf_and_reconnects_only_with_the_exact_pin() {
        let root = fixture();
        let (port, certificate, server) = tls_server(&root);
        let config = Arc::new(client_config(None).unwrap());
        let (mut stream, observed) = connect_tls("127.0.0.1", port, config).unwrap();
        assert_eq!(observed, certificate);
        stream.write_all(b"ping").unwrap();
        let mut response = [0_u8; 4];
        stream.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        drop(stream);
        server.join().unwrap();

        let (port, certificate, server) = tls_server(&root);
        let config =
            Arc::new(client_config(Some(ExpectedCertificate::Der(certificate.clone()))).unwrap());
        let (_, observed) = connect_tls("127.0.0.1", port, config).unwrap();
        assert_eq!(observed, certificate);
        server.join().unwrap();

        let (port, _, server) = tls_server(&root);
        let config = Arc::new(
            client_config(Some(ExpectedCertificate::Der(
                b"different certificate".to_vec(),
            )))
            .unwrap(),
        );
        assert!(connect_tls("127.0.0.1", port, config).is_err());
        server.join().unwrap();

        let (port, certificate, server) = tls_server(&root);
        let config = Arc::new(
            client_config(Some(ExpectedCertificate::Fingerprint(
                certificate_fingerprint(&certificate),
            )))
            .unwrap(),
        );
        assert!(connect_tls("127.0.0.1", port, config).is_ok());
        server.join().unwrap();

        let (port, _, server) = tls_server(&root);
        let config =
            Arc::new(client_config(Some(ExpectedCertificate::Fingerprint([0x42; 32]))).unwrap());
        assert!(connect_tls("127.0.0.1", port, config).is_err());
        server.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
