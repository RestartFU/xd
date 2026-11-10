//! Pinned TLS transport shared by the desktop and daemon.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rustls::{
    ClientConfig, ClientConnection, Connection, DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

pub use rustls;
const CHUNK: usize = 64 * 1024;
const QUEUE: usize = 256;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCredentials {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub token: String,
}

impl std::fmt::Debug for RemoteCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteCredentials")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// The port a host's remote listener normally uses.
pub const DEFAULT_PAIRING_PORT: u16 = 4001;

/// What a host hands out: `xd2:<one-use code>:<base64url certificate SHA-256>`.
///
/// The address is left out on purpose, because only the person pairing knows
/// which route reaches the host. The certificate pin has to stay: it is what
/// lets a client verify the host before sending it the one-use code.
// Intentionally no Debug: pairing codes contain secrets.
#[derive(Clone)]
pub struct PairingCode {
    pub code: String,
    pub fingerprint: String,
}

impl PairingCode {
    pub fn encode(&self) -> Result<String, String> {
        validate_pin(&self.fingerprint)?;
        validate_code(&self.code)?;
        let pin = (0..self.fingerprint.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&self.fingerprint[index..index + 2], 16))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "Invalid host certificate fingerprint.")?;
        Ok(format!("xd2:{}:{}", self.code, URL_SAFE_NO_PAD.encode(pin)))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 4096 {
            return Err("Pairing code is too long.".into());
        }
        // Long codes can pick up line breaks when copied out of a terminal.
        let text: String = text.split_whitespace().collect();
        if text.starts_with("xd1:") {
            return Err(
                "This pairing code is from an older version of xd. Update the host and create a new code."
                    .into(),
            );
        }
        let (code, pin) = text
            .strip_prefix("xd2:")
            .ok_or("Paste the full pairing code copied from the host.")?
            .split_once(':')
            .ok_or("Invalid pairing code.")?;
        let pin = URL_SAFE_NO_PAD
            .decode(pin)
            .map_err(|_| "Invalid pairing code.")?;
        if pin.len() != 32 {
            return Err("Invalid pairing code.".into());
        }
        validate_code(code)?;
        Ok(Self {
            code: code.to_owned(),
            fingerprint: pin.iter().map(|byte| format!("{byte:02x}")).collect(),
        })
    }
}

// Intentionally no Debug: invitations and credentials contain secrets.
#[derive(Clone)]
pub struct PairingInvite {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub code: String,
}

impl PairingInvite {
    /// Combines the address the user chose with the code from the host.
    pub fn new(address: &str, pairing_code: &str) -> Result<Self, String> {
        let (host, port) = parse_address(address)?;
        let PairingCode { code, fingerprint } = PairingCode::parse(pairing_code)?;
        Ok(Self {
            host,
            port,
            fingerprint,
            code,
        })
    }
}

/// Reads `host`, `host:port`, an IPv6 literal, or `[ipv6]:port`.
pub fn parse_address(address: &str) -> Result<(String, u16), String> {
    let address = address.trim();
    if address.is_empty() {
        return Err("Enter the host's LAN or Tailscale address.".into());
    }
    let invalid = || "Invalid host address or port.".to_owned();
    let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
        let (host, rest) = bracketed.split_once(']').ok_or_else(invalid)?;
        let port = (!rest.is_empty())
            .then(|| rest.strip_prefix(':').ok_or_else(invalid))
            .transpose()?;
        (host, port)
    } else {
        match address.split_once(':') {
            Some((host, port)) if !port.contains(':') => (host, Some(port)),
            // No colon at all, or an unbracketed IPv6 literal.
            _ => (address, None),
        }
    };
    let port = match port {
        Some(port) => port.parse().map_err(|_| invalid())?,
        None => DEFAULT_PAIRING_PORT,
    };
    validate_address(host, port)?;
    Ok((host.to_owned(), port))
}

pub fn validate_endpoint(host: &str, port: u16, pin: &str) -> Result<(), String> {
    validate_address(host, port)?;
    validate_pin(pin)
}

fn validate_address(host: &str, port: u16) -> Result<(), String> {
    if host.trim().is_empty()
        || host.len() > 253
        || host.chars().any(char::is_whitespace)
        || port == 0
    {
        return Err("Invalid host address or port.".into());
    }
    Ok(())
}

fn validate_pin(pin: &str) -> Result<(), String> {
    if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid host certificate fingerprint.".into());
    }
    Ok(())
}

fn validate_code(code: &str) -> Result<(), String> {
    if code.is_empty() || code.len() > 64 || code.contains(':') {
        return Err("Invalid pairing code.".into());
    }
    Ok(())
}

pub fn fingerprint(certificate: &[u8]) -> String {
    Sha256::digest(certificate)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug)]
struct PinnedCertificate {
    fingerprint: String,
}
impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if fingerprint(leaf.as_ref()) != self.fingerprint {
            return Err(rustls::Error::General(
                "Host certificate changed. Pair with the host again.".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn connect(host: &str, port: u16, pin: &str) -> io::Result<(TlsReader, TlsWriter, TlsSession)> {
    validate_endpoint(host, port, pin).map_err(io::Error::other)?;
    let mut last = io::Error::other("Host address did not resolve.");
    for address in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, Duration::from_secs(10)) {
            Ok(socket) => {
                let config = ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .map_err(io::Error::other)?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(PinnedCertificate {
                    fingerprint: pin.to_lowercase(),
                }))
                .with_no_client_auth();
                let connection = ClientConnection::new(
                    Arc::new(config),
                    ServerName::try_from("xd.local").unwrap(),
                )
                .map_err(io::Error::other)?;
                return split(Connection::Client(connection), socket);
            }
            Err(error) => last = error,
        }
    }
    Err(last)
}

pub struct TlsSession {
    socket: TcpStream,
}
impl TlsSession {
    pub fn close(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}
impl Drop for TlsSession {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct TlsReader {
    incoming: Receiver<Vec<u8>>,
    buffered: io::Cursor<Vec<u8>>,
    timeout: Option<Duration>,
}
impl TlsReader {
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }
}
impl Read for TlsReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.buffered.read(bytes)?;
            if count > 0 {
                return Ok(count);
            }
            let received = match self.timeout {
                Some(timeout) => match self.incoming.recv_timeout(timeout) {
                    Ok(chunk) => Some(chunk),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Host did not authenticate in time",
                        ));
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => None,
                },
                None => self.incoming.recv().ok(),
            };
            match received {
                Some(chunk) => self.buffered = io::Cursor::new(chunk),
                None => return Ok(0),
            }
        }
    }
}
pub struct TlsWriter {
    outgoing: SyncSender<Vec<u8>>,
}
impl Write for TlsWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = bytes.len().min(CHUNK);
        if count == 0 {
            return Ok(0);
        }
        self.outgoing
            .send(bytes[..count].to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "TLS connection closed"))?;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One driver owns rustls so reads never prevent concurrent event writes.
pub fn split(
    mut connection: Connection,
    mut socket: TcpStream,
) -> io::Result<(TlsReader, TlsWriter, TlsSession)> {
    socket.set_read_timeout(Some(Duration::from_secs(10)))?;
    socket.set_write_timeout(Some(Duration::from_secs(10)))?;
    while connection.is_handshaking() {
        connection.complete_io(&mut socket)?;
    }
    socket.set_nonblocking(true)?;
    connection.set_buffer_limit(Some(128 * 1024));
    let session = TlsSession {
        socket: socket.try_clone()?,
    };
    let (incoming, received) = mpsc::sync_channel(QUEUE);
    let (outgoing, sent) = mpsc::sync_channel(QUEUE);
    thread::Builder::new()
        .name("xd-tls".into())
        .spawn(move || {
            let _ = drive(&mut connection, &mut socket, incoming, sent);
            let _ = socket.shutdown(Shutdown::Both);
        })?;
    Ok((
        TlsReader {
            incoming: received,
            buffered: io::Cursor::new(Vec::new()),
            timeout: None,
        },
        TlsWriter { outgoing },
        session,
    ))
}

fn drive(
    connection: &mut Connection,
    socket: &mut TcpStream,
    incoming: SyncSender<Vec<u8>>,
    outgoing: Receiver<Vec<u8>>,
) -> io::Result<()> {
    let mut pending = Vec::new();
    let mut offset = 0;
    let mut blocked_since = None;
    let mut last_progress = Instant::now();
    loop {
        let mut progress = false;
        match connection.read_tls(socket) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                connection.process_new_packets().map_err(io::Error::other)?;
                progress = true;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        let mut bytes = [0u8; CHUNK];
        loop {
            match connection.reader().read(&mut bytes) {
                Ok(0) => return Ok(()),
                Ok(count) => {
                    incoming
                        .try_send(bytes[..count].to_vec())
                        .map_err(|_| io::Error::other("TLS reader stopped draining"))?;
                    progress = true;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        if offset == pending.len() {
            match outgoing.try_recv() {
                Ok(bytes) => {
                    pending = bytes;
                    offset = 0;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if offset < pending.len() {
            match connection.writer().write(&pending[offset..]) {
                Ok(count) => {
                    offset += count;
                    progress |= count > 0;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        while connection.wants_write() {
            match connection.write_tls(socket) {
                Ok(count) => {
                    progress |= count > 0;
                    blocked_since = None;
                    if count == 0 {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let since = blocked_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_secs(15) {
                        return Err(io::Error::other("TLS peer stopped reading"));
                    }
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if progress {
            last_progress = Instant::now();
        } else {
            thread::sleep(idle_wait(last_progress.elapsed()));
        }
    }
}

/// How long the driver sleeps when nothing moved. Traffic keeps it quick;
/// a connection quiet for a while, as most are most of the time, wakes far
/// less often, at the cost of a few milliseconds on the first frame after.
fn idle_wait(quiet: Duration) -> Duration {
    if quiet < Duration::from_secs(1) {
        Duration::from_millis(5)
    } else {
        Duration::from_millis(50)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample_code() -> PairingCode {
        PairingCode {
            code: "ABCD-EFGH".into(),
            fingerprint: "ab".repeat(32),
        }
    }

    #[test]
    fn an_idle_connection_wakes_less_often() {
        assert_eq!(idle_wait(Duration::ZERO), Duration::from_millis(5));
        assert_eq!(
            idle_wait(Duration::from_millis(900)),
            Duration::from_millis(5)
        );
        assert_eq!(idle_wait(Duration::from_secs(5)), Duration::from_millis(50));
    }

    #[test]
    fn pairing_codes_carry_the_pin_but_no_address() {
        let encoded = sample_code().encode().unwrap();
        assert_eq!(
            encoded,
            "xd2:ABCD-EFGH:q6urq6urq6urq6urq6urq6urq6urq6urq6urq6urq6s"
        );
        let parsed = PairingCode::parse(&encoded).unwrap();
        assert_eq!(parsed.code, "ABCD-EFGH");
        assert_eq!(parsed.fingerprint, "ab".repeat(32));
    }

    #[test]
    fn pairing_codes_survive_line_wrapping() {
        let encoded = sample_code().encode().unwrap();
        let (start, end) = encoded.split_at(20);
        let parsed = PairingCode::parse(&format!("  {start}\n{end}\n")).unwrap();
        assert_eq!(parsed.fingerprint, "ab".repeat(32));
    }

    #[test]
    fn malformed_pairing_codes_are_rejected() {
        for text in [
            "",
            "ABCD-EFGH",
            "xd2:ABCD-EFGH",
            "xd2::q6urq6urq6urq6urq6urq6urq6urq6urq6urq6urq6s",
            "xd2:ABCD-EFGH:q6ur",
            "xd2:ABCD-EFGH:not base64!",
        ] {
            assert!(PairingCode::parse(text).is_err(), "{text:?}");
        }
        assert!(
            PairingCode::parse("xd1:eyJob3N0IjoiYSJ9")
                .err()
                .unwrap()
                .contains("older version")
        );
        let mut bad_pin = sample_code();
        bad_pin.fingerprint = "zz".repeat(32);
        assert!(bad_pin.encode().is_err());
    }

    #[test]
    fn addresses_default_to_the_pairing_port() {
        for (text, host, port) in [
            ("100.64.0.1", "100.64.0.1", 4001),
            (" 100.64.0.1:4444 ", "100.64.0.1", 4444),
            ("vps.tailnet.ts.net", "vps.tailnet.ts.net", 4001),
            ("vps:9", "vps", 9),
            ("fd7a:115c::1", "fd7a:115c::1", 4001),
            ("[fd7a:115c::1]", "fd7a:115c::1", 4001),
            ("[fd7a:115c::1]:4444", "fd7a:115c::1", 4444),
        ] {
            assert_eq!(
                parse_address(text).unwrap(),
                (host.to_owned(), port),
                "{text}"
            );
        }
        for text in [
            "",
            "   ",
            "host:0",
            "host:70000",
            "host:port",
            "a b",
            "[::1",
            "[::1]4001",
            "[]:4001",
            ":4001",
        ] {
            assert!(parse_address(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn invites_combine_the_chosen_address_with_the_code() {
        let code = sample_code().encode().unwrap();
        let invite = PairingInvite::new("100.64.0.1", &code).unwrap();
        assert_eq!(invite.host, "100.64.0.1");
        assert_eq!(invite.port, DEFAULT_PAIRING_PORT);
        assert_eq!(invite.code, "ABCD-EFGH");
        assert_eq!(invite.fingerprint, "ab".repeat(32));
        assert!(PairingInvite::new("", &code).is_err());
        assert!(PairingInvite::new("100.64.0.1", "ABCD-EFGH").is_err());
        assert!(validate_endpoint("host", 0, &invite.fingerprint).is_err());
        assert!(validate_endpoint("host", 4001, "").is_err());
    }
}
