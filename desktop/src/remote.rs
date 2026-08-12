use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    channel,
    daemon::{DaemonHandle, DaemonUpdate},
    private_fs::{
        create_private_directory, secure_directory, socket_is_private, socket_path_exists,
    },
    session_host::SshCommand,
};

const STDERR_LIMIT: usize = 8 * 1024;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
static TEMPORARY_DIRECTORY: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("Cannot start the SSH connection: {0}")]
    Bridge(String),
}

/// The SSH process only forwards a private remote Unix socket. Authentication,
/// host verification, proxy jumps, and keys all remain OpenSSH's job.
pub struct SshRemoteBridge {
    child: Child,
    socket: PathBuf,
    directory: PathBuf,
}

pub struct SshRemoteSession {
    daemon: DaemonHandle,
    updates: async_channel::Receiver<DaemonUpdate>,
    bridge: SshRemoteBridge,
}

impl SshRemoteSession {
    pub fn into_parts(
        self,
    ) -> (
        DaemonHandle,
        async_channel::Receiver<DaemonUpdate>,
        SshRemoteBridge,
    ) {
        (self.daemon, self.updates, self.bridge)
    }
}

pub fn connect_ssh(command: &SshCommand) -> Result<SshRemoteSession, RemoteError> {
    let home = probe_remote_home(command)?;
    let remote_socket = PathBuf::from(home)
        .join(".local/share")
        .join(channel::data_name())
        .join("daemon.sock");
    let bridge = SshRemoteBridge::launch(command, &remote_socket)?;
    let (daemon, updates) = DaemonHandle::connect(bridge.socket().to_owned())
        .map_err(|error| RemoteError::Bridge(error.to_string()))?;
    Ok(SshRemoteSession {
        daemon,
        updates,
        bridge,
    })
}

fn ssh_base_arguments(command: &SshCommand) -> Vec<String> {
    let mut arguments = command.options().to_vec();
    arguments.extend([
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
    ]);
    arguments
}

fn home_probe_arguments(command: &SshCommand) -> Vec<String> {
    let mut arguments = ssh_base_arguments(command);
    arguments.extend([
        "--".into(),
        command.destination().into(),
        "printf '%s' \"$HOME\"".into(),
    ]);
    arguments
}

fn forward_arguments(command: &SshCommand, local: &Path, remote: &Path) -> Vec<String> {
    let mut arguments = ssh_base_arguments(command);
    arguments.extend([
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-N".into(),
        "-L".into(),
        format!("{}:{}", local.display(), remote.display()),
        "--".into(),
        command.destination().into(),
    ]);
    arguments
}

fn probe_remote_home(command: &SshCommand) -> Result<PathBuf, RemoteError> {
    let output = Command::new(command.program())
        .args(home_probe_arguments(command))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| RemoteError::Bridge(format!("cannot launch SSH: {error}")))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(RemoteError::Bridge(if error.is_empty() {
            format!("SSH exited with {}", output.status)
        } else {
            error
        }));
    }
    let home = String::from_utf8(output.stdout)
        .map_err(|_| RemoteError::Bridge("SSH returned a non-UTF-8 home directory".into()))?;
    let home = home.trim();
    if !home.starts_with('/') || home.contains('\n') || home.contains('\r') {
        return Err(RemoteError::Bridge(
            "SSH returned an invalid remote home directory".into(),
        ));
    }
    Ok(PathBuf::from(home))
}

impl SshRemoteBridge {
    fn launch(command: &SshCommand, remote_socket: &Path) -> Result<Self, RemoteError> {
        let directory = private_bridge_directory()?;
        let socket = directory.join("daemon.sock");
        let mut process = Command::new(command.program());
        process
            .args(forward_arguments(command, &socket, remote_socket))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        channel::configure_background(&mut process);
        let mut child = process
            .spawn()
            .map_err(|error| RemoteError::Bridge(format!("cannot launch SSH: {error}")))?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        if let Some(stderr) = child.stderr.take() {
            drain_bounded(stderr, errors.clone());
        }
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if socket_path_exists(&socket) {
                break;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    let message = format!("SSH exited with {}", status_text(status));
                    stop_child(&mut child);
                    let _ = fs::remove_dir(&directory);
                    return Err(RemoteError::Bridge(with_stderr(message, &errors)));
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => {
                    stop_child(&mut child);
                    let _ = fs::remove_dir(&directory);
                    return Err(RemoteError::Bridge(with_stderr(
                        "SSH forwarding timed out".into(),
                        &errors,
                    )));
                }
                Err(error) => {
                    stop_child(&mut child);
                    let _ = fs::remove_dir(&directory);
                    return Err(RemoteError::Bridge(format!("cannot inspect SSH: {error}")));
                }
            }
        }
        if !socket_is_private(&socket) {
            stop_child(&mut child);
            let _ = fs::remove_file(&socket);
            let _ = fs::remove_dir(&directory);
            return Err(RemoteError::Bridge(
                "SSH did not create a private local socket".into(),
            ));
        }
        Ok(Self {
            child,
            socket,
            directory,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for SshRemoteBridge {
    fn drop(&mut self) {
        stop_child(&mut self.child);
        if socket_path_exists(&self.socket) {
            let _ = fs::remove_file(&self.socket);
        }
        let _ = fs::remove_dir(&self.directory);
    }
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
            TEMPORARY_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        match create_private_directory(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(RemoteError::Bridge(error.to_string())),
        }
    }
    Err(RemoteError::Bridge(
        "cannot allocate a private local SSH directory".into(),
    ))
}

fn drain_bounded(mut stderr: impl Read + Send + 'static, destination: Arc<Mutex<Vec<u8>>>) {
    let _ = thread::Builder::new()
        .name("xd-ssh-errors".into())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_transport_uses_the_entered_connection_and_a_private_unix_forward() {
        let command = SshCommand::parse(
            r#"ssh zenomc.org -p 22 -i "keys/zeno mc" -o ServerAliveInterval=15"#,
        )
        .unwrap();
        assert_eq!(
            home_probe_arguments(&command),
            vec![
                "-p",
                "22",
                "-i",
                "keys/zeno mc",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "--",
                "zenomc.org",
                "printf '%s' \"$HOME\"",
            ]
        );
        assert_eq!(
            forward_arguments(
                &command,
                Path::new("/tmp/xd/daemon.sock"),
                Path::new("/home/zeno/.local/share/xd-nightly/daemon.sock"),
            ),
            vec![
                "-p",
                "22",
                "-i",
                "keys/zeno mc",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ExitOnForwardFailure=yes",
                "-N",
                "-L",
                "/tmp/xd/daemon.sock:/home/zeno/.local/share/xd-nightly/daemon.sock",
                "--",
                "zenomc.org",
            ]
        );
    }
}
