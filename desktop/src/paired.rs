//! Direct remote daemon connections authenticated with a pinned pairing invite.
use crate::host::{HostHandle, HostUpdate};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
};
use thiserror::Error;
use xd_transport::{PairingInvite, RemoteCredentials, TlsSession};

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("{0}")]
    Connection(String),
    #[error("{0}")]
    Authentication(String),
}
impl RemoteError {
    pub fn fatal(&self) -> bool {
        matches!(self, Self::Authentication(_))
    }
}
pub struct RemoteBridge {
    _session: TlsSession,
}
pub struct RemoteSession {
    host: HostHandle,
    updates: async_channel::Receiver<HostUpdate>,
    bridge: RemoteBridge,
    pub credentials: RemoteCredentials,
}
impl RemoteSession {
    pub fn into_parts(
        self,
    ) -> (
        HostHandle,
        async_channel::Receiver<HostUpdate>,
        RemoteBridge,
    ) {
        (self.host, self.updates, self.bridge)
    }
}
pub fn pair(invite: PairingInvite) -> Result<RemoteSession, RemoteError> {
    let name = std::env::var("HOSTNAME").unwrap_or_else(|_| "Desktop".into());
    connect_authenticated(
        &invite.host,
        invite.port,
        &invite.fingerprint,
        json!({"op":"pair", "code":invite.code, "name":name}),
        None,
    )
}
pub fn resume(credentials: RemoteCredentials) -> Result<RemoteSession, RemoteError> {
    connect_authenticated(
        &credentials.host,
        credentials.port,
        &credentials.fingerprint,
        json!({"op":"hello", "token":credentials.token}),
        Some(credentials.token.clone()),
    )
}
fn connect_authenticated(
    host: &str,
    port: u16,
    pin: &str,
    mut auth: Value,
    saved_token: Option<String>,
) -> Result<RemoteSession, RemoteError> {
    let (mut reader, mut writer, session) =
        xd_transport::connect(host, port, pin).map_err(|error| {
            let message = error.to_string();
            if message.contains("certificate") {
                RemoteError::Authentication(message)
            } else {
                RemoteError::Connection(message)
            }
        })?;
    reader.set_timeout(Some(std::time::Duration::from_secs(10)));
    auth["_xd_request"] = json!(0);
    writeln!(writer, "{auth}").map_err(|e| RemoteError::Connection(e.to_string()))?;
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    // Authentication is the first application frame; no events are sent beforehand.
    reader
        .by_ref()
        .take(64 * 1024 + 1)
        .read_until(b'\n', &mut line)
        .map_err(|e| RemoteError::Connection(e.to_string()))?;
    if line.len() > 64 * 1024 {
        return Err(RemoteError::Authentication(
            "Invalid authentication response.".into(),
        ));
    }
    let reply: Value = serde_json::from_slice(&line)
        .map_err(|_| RemoteError::Authentication("Invalid authentication response.".into()))?;
    if reply["ok"] != true {
        return Err(RemoteError::Authentication(
            reply["error"]
                .as_str()
                .unwrap_or("Pairing was refused.")
                .into(),
        ));
    }
    let token = saved_token
        .or_else(|| reply["token"].as_str().map(str::to_owned))
        .ok_or_else(|| {
            RemoteError::Authentication("Host did not return device credentials.".into())
        })?;
    reader.get_mut().set_timeout(None);
    let credentials = RemoteCredentials {
        host: host.to_owned(),
        port,
        fingerprint: pin.to_owned(),
        token,
    };
    let (handle, updates) =
        HostHandle::connect_io(reader, writer, PathBuf::from(format!("{host}:{port}")));
    Ok(RemoteSession {
        host: handle,
        updates,
        bridge: RemoteBridge { _session: session },
        credentials,
    })
}
