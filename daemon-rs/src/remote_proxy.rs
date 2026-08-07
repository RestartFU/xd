use std::{
    env,
    io::{BufRead, BufReader, Read},
    net::{IpAddr, UdpSocket},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::Duration,
};

use serde_json::Value;
use xd_daemon::PeerEndpoint;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_LINE_LIMIT: u64 = 4 * 1024;

pub(crate) struct RemoteProxy {
    executable: PathBuf,
    upstream: PathBuf,
    certificate: PathBuf,
    private_key: PathBuf,
    running: Mutex<Option<RunningProxy>>,
}

struct RunningProxy {
    child: Child,
    bind: String,
    requested_port: u16,
    endpoint: PeerEndpoint,
}

impl RemoteProxy {
    pub(crate) fn new(upstream: PathBuf, data_directory: PathBuf) -> Self {
        let (certificate, private_key) = identity_paths(&data_directory);
        Self {
            executable: executable(),
            upstream,
            certificate,
            private_key,
            running: Mutex::new(None),
        }
    }

    pub(crate) fn listen(&self, bind: &str, port: u16) -> Result<PeerEndpoint, String> {
        let bind_ip: IpAddr = bind
            .parse()
            .map_err(|_| "the bind address must be an IPv4 or IPv6 address".to_owned())?;
        let mut running = self
            .running
            .lock()
            .map_err(|_| "remote TLS process state is unavailable".to_owned())?;
        if let Some(active) = running.as_mut() {
            match active.child.try_wait() {
                Ok(None)
                    if active.bind == bind
                        && (active.requested_port == port || active.endpoint.port == port) =>
                {
                    return Ok(active.endpoint.clone());
                }
                Ok(None) => return Err("a different remote listener is already running".into()),
                Ok(Some(_)) => *running = None,
                Err(error) => {
                    return Err(format!("cannot inspect the remote TLS process: {error}"));
                }
            }
        }

        let address = match bind_ip {
            IpAddr::V4(address) => format!("{address}:{port}"),
            IpAddr::V6(address) => format!("[{address}]:{port}"),
        };
        let mut child = Command::new(&self.executable)
            .arg("serve")
            .arg("--listen")
            .arg(&address)
            .arg("--upstream")
            .arg(&self.upstream)
            .arg("--certificate")
            .arg(&self.certificate)
            .arg("--private-key")
            .arg(&self.private_key)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                format!(
                    "cannot start {}: {error}",
                    self.executable.to_string_lossy()
                )
            })?;
        let Some(stdout) = child.stdout.take() else {
            stop(&mut child);
            return Err("cannot read the remote TLS startup result".into());
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("xd-tls-proxy-startup".into())
            .spawn(move || {
                let mut line = String::new();
                let result = BufReader::new(stdout)
                    .take(STARTUP_LINE_LIMIT + 1)
                    .read_line(&mut line)
                    .map(|count| (count, line));
                let _ = sender.send(result);
            })
            .map_err(|error| {
                stop(&mut child);
                format!("cannot monitor the remote TLS process: {error}")
            })?;
        let startup = match receiver.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok((count, line))) if count > 0 && count as u64 <= STARTUP_LINE_LIMIT => line,
            Ok(Ok(_)) => {
                stop(&mut child);
                return Err("remote TLS process returned an invalid startup result".into());
            }
            Ok(Err(error)) => {
                stop(&mut child);
                return Err(format!(
                    "cannot read the remote TLS startup result: {error}"
                ));
            }
            Err(_) => {
                stop(&mut child);
                return Err("remote TLS process did not start within 10 seconds".into());
            }
        };
        let response: Value = serde_json::from_str(&startup).map_err(|error| {
            stop(&mut child);
            format!("remote TLS process returned invalid JSON: {error}")
        })?;
        let actual_port = response
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .ok_or_else(|| {
                stop(&mut child);
                "remote TLS process returned an invalid port".to_owned()
            })?;
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!(
                    "remote TLS process exited during startup ({status})"
                ));
            }
            Ok(None) => {}
            Err(error) => {
                stop(&mut child);
                return Err(format!("cannot inspect the remote TLS process: {error}"));
            }
        }
        let endpoint = PeerEndpoint {
            host: advertised_host(bind_ip),
            port: actual_port,
        };
        *running = Some(RunningProxy {
            child,
            bind: bind.to_owned(),
            requested_port: port,
            endpoint: endpoint.clone(),
        });
        Ok(endpoint)
    }
}

fn identity_paths(data_directory: &Path) -> (PathBuf, PathBuf) {
    let legacy_certificate = data_directory.join("server-cert.pem");
    let legacy_private_key = data_directory.join("server-key.pem");
    if fs_entry_exists(&legacy_certificate) || fs_entry_exists(&legacy_private_key) {
        (legacy_certificate, legacy_private_key)
    } else {
        let identity_directory = data_directory.join("tls");
        (
            identity_directory.join("certificate.der"),
            identity_directory.join("private-key.der"),
        )
    }
}

fn fs_entry_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

impl Drop for RemoteProxy {
    fn drop(&mut self) {
        if let Ok(running) = self.running.get_mut()
            && let Some(mut running) = running.take()
        {
            stop(&mut running.child);
        }
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn advertised_host(bind: IpAddr) -> String {
    if !bind.is_unspecified() {
        return bind.to_string();
    }
    for target in ["1.1.1.1:53", "8.8.8.8:53", "192.0.2.1:53"] {
        let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };
        if socket.connect(target).is_ok()
            && let Ok(address) = socket.local_addr()
            && !address.ip().is_unspecified()
            && !address.ip().is_loopback()
        {
            return address.ip().to_string();
        }
    }
    "127.0.0.1".into()
}

fn executable() -> PathBuf {
    if let Some(path) = env::var_os("XD_TLS_PROXY_EXECUTABLE").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        return parent.join(if cfg!(windows) {
            "xd-tls-proxy.exe"
        } else {
            "xd-tls-proxy"
        });
    }
    Path::new(if cfg!(windows) {
        "xd-tls-proxy.exe"
    } else {
        "xd-tls-proxy"
    })
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn explicit_bind_addresses_are_advertised_unchanged() {
        assert_eq!(advertised_host("127.0.0.1".parse().unwrap()), "127.0.0.1");
        assert_eq!(advertised_host("::1".parse().unwrap()), "::1");
    }

    #[test]
    fn preserves_a_legacy_identity_when_adopting_an_existing_data_root() {
        let root = env::temp_dir().join(format!("xd-rust-legacy-identity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("server-cert.pem"), "legacy").unwrap();
        let (certificate, private_key) = identity_paths(&root);
        assert_eq!(certificate, root.join("server-cert.pem"));
        assert_eq!(private_key, root.join("server-key.pem"));
        fs::remove_dir_all(root).unwrap();
    }
}
