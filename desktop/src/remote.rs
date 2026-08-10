use std::{
    env, fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::private_fs::{
    create_private_directory, create_private_file, secure_directory, secure_file,
    socket_is_private, socket_path_exists,
};
use crate::{
    channel,
    daemon::{DaemonHandle, DaemonUpdate, RequestKind},
};

const CREDENTIALS_VERSION: u32 = 1;
const STARTUP_LIMIT: usize = 4 * 1024;
const STDERR_LIMIT: usize = 8 * 1024;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
static TEMPORARY_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCredentials {
    pub version: u32,
    pub host: String,
    pub port: u16,
    pub token: String,
    pub fingerprint: String,
}

impl fmt::Debug for RemoteCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteCredentials")
            .field("version", &self.version)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("token", &"[redacted]")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

impl RemoteCredentials {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        token: impl Into<String>,
        fingerprint: impl Into<String>,
    ) -> Result<Self, RemoteError> {
        let credentials = Self {
            version: CREDENTIALS_VERSION,
            host: host.into().trim().to_owned(),
            port,
            token: token.into(),
            fingerprint: normalize_fingerprint(&fingerprint.into())?,
        };
        credentials.validate()?;
        Ok(credentials)
    }

    pub fn validate(&self) -> Result<(), RemoteError> {
        if self.version != CREDENTIALS_VERSION {
            return Err(RemoteError::Credentials(
                "Remote credentials version is unsupported.".into(),
            ));
        }
        if self.host.trim().is_empty() {
            return Err(RemoteError::Credentials(
                "Remote host cannot be empty.".into(),
            ));
        }
        if self.port == 0 {
            return Err(RemoteError::Credentials(
                "Remote port must be from 1 to 65535.".into(),
            ));
        }
        if self.token.is_empty() {
            return Err(RemoteError::Credentials(
                "Remote device token cannot be empty.".into(),
            ));
        }
        if self.fingerprint.len() != 64
            || !self
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(RemoteError::Credentials(
                "Remote certificate fingerprint is invalid.".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("{0}")]
    Credentials(String),
    #[error("Cannot read remote credentials: {0}")]
    ReadCredentials(String),
    #[error("Cannot save remote credentials: {0}")]
    SaveCredentials(String),
    #[error("Cannot remove remote credentials: {0}")]
    ClearCredentials(String),
    #[error("Cannot start the secure remote connection: {0}")]
    Bridge(String),
    #[error("Cannot authenticate with the remote machine: {0}")]
    Authentication(String),
}

#[derive(Clone)]
pub struct CredentialsFile {
    path: PathBuf,
}

impl CredentialsFile {
    pub fn default_path() -> Result<PathBuf, RemoteError> {
        if let Some(path) =
            env::var_os("XD_REMOTE_CREDENTIALS_FILE").filter(|path| !path.is_empty())
        {
            return Ok(PathBuf::from(path));
        }
        #[cfg(unix)]
        let data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .ok_or_else(|| {
                RemoteError::Credentials(
                    "Cannot locate the data directory for remote credentials.".into(),
                )
            })?;
        #[cfg(windows)]
        let data_home = env::var_os("LOCALAPPDATA")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                RemoteError::Credentials(
                    "Cannot locate the data directory for remote credentials.".into(),
                )
            })?;
        let data_name = channel::data_name();
        Ok(data_home.join(data_name).join("remote.json"))
    }

    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<RemoteCredentials>, RemoteError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(RemoteError::ReadCredentials(error.to_string())),
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(RemoteError::ReadCredentials(format!(
                "{} is not a regular file",
                self.path.display()
            )));
        }
        let bytes = fs::read(&self.path)
            .map_err(|error| RemoteError::ReadCredentials(error.to_string()))?;
        let mut credentials: RemoteCredentials = serde_json::from_slice(&bytes)
            .map_err(|error| RemoteError::ReadCredentials(error.to_string()))?;
        credentials.host = credentials.host.trim().to_owned();
        credentials.fingerprint = normalize_fingerprint(&credentials.fingerprint)
            .map_err(|error| RemoteError::ReadCredentials(error.to_string()))?;
        credentials
            .validate()
            .map_err(|error| RemoteError::ReadCredentials(error.to_string()))?;
        secure_file(&self.path).map_err(|error| RemoteError::ReadCredentials(error.to_string()))?;
        Ok(Some(credentials))
    }

    pub fn save(&self, credentials: &RemoteCredentials) -> Result<(), RemoteError> {
        credentials
            .validate()
            .map_err(|error| RemoteError::SaveCredentials(error.to_string()))?;
        let parent = self.path.parent().ok_or_else(|| {
            RemoteError::SaveCredentials("the credentials path has no parent directory".into())
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| RemoteError::SaveCredentials(error.to_string()))?;
        secure_directory(parent)
            .map_err(|error| RemoteError::SaveCredentials(error.to_string()))?;
        let temporary = parent.join(format!(
            ".xd-remote-{}-{}.tmp",
            std::process::id(),
            TEMPORARY_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut bytes = serde_json::to_vec_pretty(credentials)
                .map_err(|error| RemoteError::SaveCredentials(error.to_string()))?;
            bytes.push(b'\n');
            let mut file = create_private_file(&temporary)
                .map_err(|error| RemoteError::SaveCredentials(error.to_string()))?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| RemoteError::SaveCredentials(error.to_string()))?;
            fs::rename(&temporary, &self.path)
                .map_err(|error| RemoteError::SaveCredentials(error.to_string()))?;
            secure_file(&self.path).map_err(|error| RemoteError::SaveCredentials(error.to_string()))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn clear(&self) -> Result<(), RemoteError> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata)
                if metadata.file_type().is_file() && !metadata.file_type().is_symlink() =>
            {
                fs::remove_file(&self.path)
                    .map_err(|error| RemoteError::ClearCredentials(error.to_string()))
            }
            Ok(_) => Err(RemoteError::ClearCredentials(format!(
                "{} is not a regular file",
                self.path.display()
            ))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(RemoteError::ClearCredentials(error.to_string())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct BridgeStartup {
    pub fingerprint: String,
}

pub struct RemoteBridge {
    child: Child,
    socket: PathBuf,
    directory: PathBuf,
}

pub struct RemoteSession {
    daemon: DaemonHandle,
    updates: async_channel::Receiver<DaemonUpdate>,
    bridge: RemoteBridge,
}

impl RemoteSession {
    pub fn into_parts(
        self,
    ) -> (
        DaemonHandle,
        async_channel::Receiver<DaemonUpdate>,
        RemoteBridge,
    ) {
        (self.daemon, self.updates, self.bridge)
    }
}

pub fn connect(credentials: &RemoteCredentials) -> Result<RemoteSession, RemoteError> {
    credentials.validate()?;
    let (bridge, startup) = RemoteBridge::launch(
        &credentials.host,
        credentials.port,
        Some(&credentials.fingerprint),
    )?;
    if startup.fingerprint != credentials.fingerprint {
        return Err(RemoteError::Authentication(
            "the remote certificate changed".into(),
        ));
    }
    let (daemon, updates) = DaemonHandle::connect(bridge.socket().to_owned())
        .map_err(|error| RemoteError::Bridge(error.to_string()))?;
    wait_until(&updates, |update| {
        matches!(update, DaemonUpdate::Connected { .. })
    })?;
    daemon
        .hello_remote(&credentials.token)
        .map_err(RemoteError::Authentication)?;
    let reply = wait_for_reply(&updates, RequestKind::HelloRemote)?;
    require_success(&reply)?;
    Ok(RemoteSession {
        daemon,
        updates,
        bridge,
    })
}

pub fn pair(
    host: &str,
    port: u16,
    code: &str,
    name: &str,
) -> Result<(RemoteCredentials, RemoteSession), RemoteError> {
    let host = host.trim();
    let code = code
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_uppercase)
        .collect::<String>();
    let name = name.trim();
    if host.is_empty() {
        return Err(RemoteError::Credentials(
            "Remote host cannot be empty.".into(),
        ));
    }
    if port == 0 {
        return Err(RemoteError::Credentials(
            "Remote port must be from 1 to 65535.".into(),
        ));
    }
    if code.is_empty() {
        return Err(RemoteError::Authentication(
            "Pairing code cannot be empty.".into(),
        ));
    }
    if name.is_empty() {
        return Err(RemoteError::Authentication(
            "Device name cannot be empty.".into(),
        ));
    }
    let (bridge, startup) = RemoteBridge::launch(host, port, None)?;
    let (daemon, updates) = DaemonHandle::connect(bridge.socket().to_owned())
        .map_err(|error| RemoteError::Bridge(error.to_string()))?;
    wait_until(&updates, |update| {
        matches!(update, DaemonUpdate::Connected { .. })
    })?;
    daemon
        .pair_remote(&code, name)
        .map_err(RemoteError::Authentication)?;
    let reply = wait_for_reply(&updates, RequestKind::PairRemote)?;
    require_success(&reply)?;
    let token = reply
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| RemoteError::Authentication("Pairing returned no device token.".into()))?;
    let credentials = RemoteCredentials::new(host, port, token, startup.fingerprint)?;
    Ok((
        credentials,
        RemoteSession {
            daemon,
            updates,
            bridge,
        },
    ))
}

fn wait_for_reply(
    updates: &async_channel::Receiver<DaemonUpdate>,
    expected: RequestKind,
) -> Result<Map<String, Value>, RemoteError> {
    let update = wait_until(
        updates,
        move |update| matches!(update, DaemonUpdate::Reply { kind, .. } if *kind == expected),
    )?;
    match update {
        DaemonUpdate::Reply { body, .. } => Ok(body),
        _ => unreachable!("wait predicate accepted only a reply"),
    }
}

fn wait_until(
    updates: &async_channel::Receiver<DaemonUpdate>,
    predicate: impl Fn(&DaemonUpdate) -> bool + Send + 'static,
) -> Result<DaemonUpdate, RemoteError> {
    let updates = updates.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("xd-remote-authentication".into())
        .spawn(move || {
            loop {
                match updates.recv_blocking() {
                    Ok(update) if predicate(&update) => {
                        let _ = sender.send(Ok(update));
                        break;
                    }
                    Ok(DaemonUpdate::Disconnected { message }) => {
                        let _ = sender.send(Err(message));
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        let _ = sender.send(Err("the remote connection closed".into()));
                        break;
                    }
                }
            }
        })
        .map_err(|error| {
            RemoteError::Authentication(format!("cannot monitor authentication: {error}"))
        })?;
    match receiver.recv_timeout(STARTUP_TIMEOUT) {
        Ok(Ok(update)) => Ok(update),
        Ok(Err(error)) => Err(RemoteError::Authentication(error)),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(RemoteError::Authentication(
            "remote authentication timed out".into(),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(RemoteError::Authentication(
            "remote authentication ended unexpectedly".into(),
        )),
    }
}

fn require_success(reply: &Map<String, Value>) -> Result<(), RemoteError> {
    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    Err(RemoteError::Authentication(
        reply
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the remote daemon rejected authentication")
            .to_owned(),
    ))
}

impl RemoteBridge {
    pub fn launch(
        host: &str,
        port: u16,
        fingerprint: Option<&str>,
    ) -> Result<(Self, BridgeStartup), RemoteError> {
        let host = host.trim();
        if host.is_empty() {
            return Err(RemoteError::Bridge("remote host cannot be empty".into()));
        }
        if port == 0 {
            return Err(RemoteError::Bridge(
                "remote port must be from 1 to 65535".into(),
            ));
        }
        let fingerprint = fingerprint.map(normalize_fingerprint).transpose()?;
        let directory = private_bridge_directory()?;
        let socket = directory.join("daemon.sock");
        let mut failures = Vec::new();
        for executable in helper_candidates() {
            match launch_helper(&executable, host, port, fingerprint.as_deref(), &socket) {
                Ok((child, startup)) => {
                    return Ok((
                        Self {
                            child,
                            socket,
                            directory,
                        },
                        startup,
                    ));
                }
                Err(error) => failures.push(format!("{}: {error}", executable.display())),
            }
        }
        let _ = fs::remove_dir(&directory);
        Err(RemoteError::Bridge(failures.join("; ")))
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for RemoteBridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if socket_path_exists(&self.socket) {
            let _ = fs::remove_file(&self.socket);
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

fn launch_helper(
    executable: &Path,
    host: &str,
    port: u16,
    fingerprint: Option<&str>,
    socket: &Path,
) -> Result<(Child, BridgeStartup), String> {
    let mut command = Command::new(executable);
    command
        .arg("connect")
        .arg("--host")
        .arg(host)
        .arg("--port")
        .arg(port.to_string())
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(fingerprint) = fingerprint {
        command.arg("--fingerprint").arg(fingerprint);
    }
    channel::configure_background(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot launch the TLS bridge: {error}"))?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            stop_child(&mut child);
            return Err("cannot read TLS bridge startup".into());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            stop_child(&mut child);
            return Err("cannot read TLS bridge errors".into());
        }
    };
    let errors = Arc::new(Mutex::new(Vec::new()));
    drain_bounded(stderr, errors.clone());
    let (sender, receiver) = mpsc::sync_channel(1);
    if let Err(error) = thread::Builder::new()
        .name("xd-remote-bridge-startup".into())
        .spawn(move || {
            let _ = sender.send(read_startup(stdout));
        })
    {
        stop_child(&mut child);
        return Err(format!("cannot monitor TLS bridge startup: {error}"));
    }
    let startup = match receiver.recv_timeout(STARTUP_TIMEOUT) {
        Ok(Ok(startup)) => startup,
        Ok(Err(error)) => {
            stop_child(&mut child);
            return Err(with_stderr(error, &errors));
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            stop_child(&mut child);
            return Err(with_stderr("TLS bridge startup timed out".into(), &errors));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            stop_child(&mut child);
            return Err(with_stderr(
                "TLS bridge startup ended unexpectedly".into(),
                &errors,
            ));
        }
    };
    match child.try_wait() {
        Ok(Some(status)) => {
            return Err(with_stderr(
                format!("TLS bridge exited with {}", status_text(status)),
                &errors,
            ));
        }
        Ok(None) => {}
        Err(error) => {
            stop_child(&mut child);
            return Err(format!("cannot inspect the TLS bridge: {error}"));
        }
    }
    match fs::symlink_metadata(socket) {
        Ok(_) => {}
        Err(error) => {
            stop_child(&mut child);
            return Err(with_stderr(
                format!("TLS bridge did not create its socket: {error}"),
                &errors,
            ));
        }
    }
    if !socket_is_private(socket) {
        stop_child(&mut child);
        return Err("TLS bridge did not create a private Unix socket".into());
    }
    if fingerprint.is_some_and(|expected| expected != startup.fingerprint) {
        stop_child(&mut child);
        return Err("TLS bridge reported a different certificate fingerprint".into());
    }
    Ok((child, startup))
}

fn read_startup(mut reader: impl Read) -> Result<BridgeStartup, String> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    while line.len() <= STARTUP_LIMIT {
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => line.push(byte[0]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("cannot read TLS bridge startup: {error}")),
        }
    }
    if line.len() > STARTUP_LIMIT {
        return Err("TLS bridge startup response is too large".into());
    }
    if line.is_empty() {
        return Err("TLS bridge returned no startup response".into());
    }
    let mut startup: BridgeStartup = serde_json::from_slice(&line)
        .map_err(|error| format!("TLS bridge returned invalid startup data: {error}"))?;
    startup.fingerprint = normalize_fingerprint(&startup.fingerprint)
        .map_err(|error| format!("TLS bridge returned invalid startup data: {error}"))?;
    Ok(startup)
}

fn drain_bounded(mut stderr: impl Read + Send + 'static, destination: Arc<Mutex<Vec<u8>>>) {
    let _ = thread::Builder::new()
        .name("xd-remote-bridge-errors".into())
        .spawn(move || {
            let mut buffer = [0_u8; 1024];
            loop {
                let count = match stderr.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                if let Ok(mut destination) = destination.lock() {
                    destination.extend_from_slice(&buffer[..count]);
                    if destination.len() > STDERR_LIMIT {
                        let excess = destination.len() - STDERR_LIMIT;
                        destination.drain(..excess);
                    }
                }
            }
        });
}

fn with_stderr(message: String, stderr: &Arc<Mutex<Vec<u8>>>) -> String {
    let detail = stderr
        .lock()
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default();
    if detail.is_empty() {
        message
    } else {
        format!("{message}: {detail}")
    }
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn status_text(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return code.to_string();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    "an unknown status".into()
}

fn helper_candidates() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("XD_TLS_PROXY_EXECUTABLE").filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(path)];
    }
    let mut candidates = Vec::new();
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        let sibling = parent.join(if cfg!(windows) {
            "xd-tls-proxy.exe"
        } else {
            "xd-tls-proxy"
        });
        if sibling.is_file() {
            candidates.push(sibling);
        }
    }
    candidates.push(PathBuf::from(if cfg!(windows) {
        "xd-tls-proxy.exe"
    } else {
        "xd-tls-proxy"
    }));
    candidates
}

fn private_bridge_directory() -> Result<PathBuf, RemoteError> {
    let parent = env::var_os("XD_REMOTE_BRIDGE_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
        .unwrap_or_else(env::temp_dir)
        .join("xd");
    fs::create_dir_all(&parent).map_err(|error| RemoteError::Bridge(error.to_string()))?;
    secure_directory(&parent).map_err(|error| RemoteError::Bridge(error.to_string()))?;
    for _ in 0..32 {
        let directory = parent.join(format!(
            "remote-{}-{}",
            std::process::id(),
            TEMPORARY_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        match create_private_directory(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(RemoteError::Bridge(error.to_string())),
        }
    }
    Err(RemoteError::Bridge(
        "cannot allocate a private local bridge directory".into(),
    ))
}

fn normalize_fingerprint(value: &str) -> Result<String, RemoteError> {
    let fingerprint = value.trim().to_ascii_lowercase().replace(':', "");
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RemoteError::Credentials(
            "Remote certificate fingerprint is invalid.".into(),
        ));
    }
    Ok(fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "xd-remote-{name}-{}-{}",
            std::process::id(),
            TEMPORARY_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn credentials() -> RemoteCredentials {
        RemoteCredentials::new(
            " desktop.local ",
            4001,
            "secret-token",
            "00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF",
        )
        .unwrap()
    }

    #[test]
    fn credentials_match_the_existing_json_contract() {
        let credentials = credentials();
        assert_eq!(credentials.host, "desktop.local");
        assert_eq!(credentials.version, 1);
        assert_eq!(credentials.fingerprint.len(), 64);
        let encoded = serde_json::to_value(&credentials).unwrap();
        assert_eq!(encoded["port"], 4001);
        assert_eq!(encoded["token"], "secret-token");
        assert_eq!(encoded["fingerprint"], credentials.fingerprint);
    }

    #[test]
    fn credentials_file_is_atomic_private_and_clearable() {
        let root = fixture("credentials");
        let path = root.join("private").join("remote.json");
        let file = CredentialsFile::new(path.clone());
        assert!(file.load().unwrap().is_none());
        file.save(&credentials()).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(file.load().unwrap(), Some(credentials()));
        file.clear().unwrap();
        assert!(file.load().unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_or_unsafe_credentials_are_rejected() {
        assert!(RemoteCredentials::new("", 4001, "token", "0".repeat(64)).is_err());
        assert!(RemoteCredentials::new("host", 0, "token", "0".repeat(64)).is_err());
        assert!(RemoteCredentials::new("host", 4001, "", "0".repeat(64)).is_err());
        assert!(RemoteCredentials::new("host", 4001, "token", "not-a-pin").is_err());

        let root = fixture("symlink");
        let target = root.join("target");
        fs::write(&target, b"keep").unwrap();
        let path = root.join("remote.json");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let file = CredentialsFile::new(path);
        assert!(file.load().is_err());
        assert!(file.clear().is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bridge_startup_is_bounded_and_normalizes_the_fingerprint() {
        let startup = read_startup(
            br#"{"fingerprint":"00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF"}
"#
            .as_slice(),
        )
        .unwrap();
        assert_eq!(startup.fingerprint.len(), 64);
        assert_eq!(
            startup.fingerprint,
            startup.fingerprint.to_ascii_lowercase()
        );
        assert!(read_startup(b"{}\n".as_slice()).is_err());
        assert!(read_startup(vec![b'x'; STARTUP_LIMIT + 1].as_slice()).is_err());
    }

    #[test]
    fn authentication_replies_never_accept_missing_tokens_or_errors() {
        let success = serde_json::from_value::<Map<String, Value>>(serde_json::json!({
            "ok": true,
            "token": "private"
        }))
        .unwrap();
        assert!(require_success(&success).is_ok());
        assert_eq!(
            success.get("token").and_then(Value::as_str),
            Some("private")
        );

        let rejected = serde_json::from_value::<Map<String, Value>>(serde_json::json!({
            "ok": false,
            "error": "No such pairing code."
        }))
        .unwrap();
        assert_eq!(
            require_success(&rejected).unwrap_err().to_string(),
            "Cannot authenticate with the remote machine: No such pairing code."
        );
    }
}
