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

// Intentionally no Debug: invitations and credentials contain secrets.
#[derive(Clone, Serialize, Deserialize)]
pub struct PairingInvite {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub code: String,
}

impl PairingInvite {
    pub fn encode(&self) -> String {
        format!(
            "xd1:{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).expect("invitation JSON"))
        )
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 4096 {
            return Err("Pairing code is too long.".into());
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(
                text.trim()
                    .strip_prefix("xd1:")
                    .ok_or("Paste the full pairing code copied from the host.")?,
            )
            .map_err(|_| "Invalid pairing code.")?;
        let invite: Self = serde_json::from_slice(&bytes).map_err(|_| "Invalid pairing code.")?;
        validate_endpoint(&invite.host, invite.port, &invite.fingerprint)?;
        if invite.code.is_empty() || invite.code.len() > 64 {
            return Err("Invalid pairing code.".into());
        }
        Ok(invite)
    }
}

pub fn validate_endpoint(host: &str, port: u16, pin: &str) -> Result<(), String> {
    if host.trim().is_empty()
        || host.len() > 253
        || host.chars().any(char::is_whitespace)
        || port == 0
    {
        return Err("Invalid host address or port.".into());
    }
    if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid host certificate fingerprint.".into());
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
        if !progress {
            thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invitations_validate_identity_and_endpoint() {
        let invite = PairingInvite {
            host: "100.64.0.1".into(),
            port: 4001,
            fingerprint: "ab".repeat(32),
            code: "ABCD-EFGH".into(),
        };
        let parsed = PairingInvite::parse(&invite.encode()).unwrap();
        assert_eq!(parsed.host, invite.host);
        assert_eq!(parsed.fingerprint, invite.fingerprint);
        assert!(PairingInvite::parse("ABCD-EFGH").is_err());
        assert!(validate_endpoint("host", 0, &invite.fingerprint).is_err());
        assert!(validate_endpoint("host", 4001, "").is_err());
    }
}
