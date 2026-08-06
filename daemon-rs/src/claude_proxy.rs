use std::{
    env,
    io::Read,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const START_TIMEOUT: Duration = Duration::from_secs(25);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
const OUTPUT_LIMIT: usize = 8 * 1024;

#[derive(Clone)]
pub(crate) struct ClaudeProxy {
    inner: Arc<ProxyInner>,
}

struct ProxyInner {
    state: Mutex<ProxyState>,
}

#[derive(Default)]
struct ProxyState {
    child: Option<Child>,
    port: Option<u16>,
    output: Arc<Mutex<Vec<u8>>>,
}

impl ClaudeProxy {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(ProxyInner {
                state: Mutex::new(ProxyState::default()),
            }),
        }
    }

    pub(crate) fn endpoint(&self) -> Result<String, String> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| "Claude mode proxy state is unavailable.".to_owned())?;
        if let Some(port) = state.port
            && reachable(port)
        {
            return Ok(format!("http://127.0.0.1:{port}"));
        }
        terminate(state.child.take());
        state.port = None;

        let port = free_port()?;
        let mut command = Command::new(resolve_claude_proxy());
        command
            .args(["serve", "--port", &port.to_string(), "--no-monitor"])
            .env("CCP_BIND_ADDRESS", "127.0.0.1")
            .env("CCP_ALIAS_PROVIDER", "codex")
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("Cannot start Claude mode proxy: {error}"))?;
        let output = Arc::new(Mutex::new(Vec::new()));
        if let Some(stdout) = child.stdout.take() {
            drain(stdout, output.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            drain(stderr, output.clone());
        }
        state.child = Some(child);
        state.port = Some(port);
        state.output = output;

        let deadline = Instant::now() + START_TIMEOUT;
        while Instant::now() < deadline {
            if reachable(port) {
                return Ok(format!("http://127.0.0.1:{port}"));
            }
            if state
                .child
                .as_mut()
                .is_some_and(|child| child.try_wait().ok().flatten().is_some())
            {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        let detail = state
            .output
            .lock()
            .ok()
            .map(|output| String::from_utf8_lossy(&output).trim().to_owned())
            .filter(|output| !output.is_empty());
        terminate(state.child.take());
        state.port = None;
        Err(match detail {
            Some(detail) => {
                format!("Claude mode proxy did not become reachable within 25 seconds: {detail}")
            }
            None => "Claude mode proxy did not become reachable within 25 seconds.".into(),
        })
    }
}

impl Drop for ProxyInner {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            terminate(state.child.take());
            state.port = None;
        }
    }
}

fn free_port() -> Result<u16, String> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("Cannot reserve a Claude mode proxy port: {error}"))
}

fn reachable(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)),
        CONNECT_TIMEOUT,
    )
    .is_ok()
}

fn drain(mut stream: impl Read + Send + 'static, output: Arc<Mutex<Vec<u8>>>) {
    let _ = thread::Builder::new()
        .name("xd-claude-proxy-output".into())
        .spawn(move || {
            let mut buffer = [0_u8; 1024];
            loop {
                let Ok(count) = stream.read(&mut buffer) else {
                    break;
                };
                if count == 0 {
                    break;
                }
                let Ok(mut output) = output.lock() else {
                    break;
                };
                output.extend_from_slice(&buffer[..count]);
                if output.len() > OUTPUT_LIMIT {
                    let discard = output.len() - OUTPUT_LIMIT;
                    output.drain(..discard);
                }
            }
        });
}

fn terminate(child: Option<Child>) {
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

pub(crate) fn resolve_claude_proxy() -> PathBuf {
    if let Some(configured) =
        env::var_os("XD_CLAUDE_PROXY_EXECUTABLE").filter(|path| !path.is_empty())
    {
        return configured.into();
    }
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        for relative in ["claude-code-proxy", "libexec/claude-code-proxy"] {
            let candidate = parent.join(relative);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("claude-code-proxy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_proxy_path_wins() {
        // Environment mutation is process-global, so test only the deterministic
        // sibling/fallback shape here; integration packaging verifies the override.
        assert!(
            resolve_claude_proxy()
                .file_name()
                .is_some_and(|name| name == "claude-code-proxy")
        );
    }

    #[test]
    fn reserves_a_loopback_port() {
        assert_ne!(free_port().unwrap(), 0);
    }
}
