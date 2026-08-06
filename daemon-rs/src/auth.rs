use std::{
    collections::HashMap,
    io::{Read, Write},
    process::{ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    EventBus,
    agent::{resolve_claude, resolve_codex},
    claude_proxy::resolve_claude_proxy,
};

const OUTPUT_LIMIT: usize = 64 * 1024;
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct AuthManager {
    inner: Arc<AuthInner>,
}

struct AuthInner {
    states: Mutex<HashMap<String, AuthSnapshot>>,
    sessions: Mutex<HashMap<String, AuthSession>>,
    next_session: AtomicU64,
    events: Arc<EventBus>,
}

#[derive(Clone)]
struct AuthSnapshot {
    state: String,
    detail: Option<String>,
    login_url: Option<String>,
    device_code: Option<String>,
    needs_input: bool,
}

struct AuthSession {
    serial: u64,
    pid: Option<u32>,
    input: Option<Arc<Mutex<ChildStdin>>>,
    output: String,
    cancelled: bool,
}

impl AuthManager {
    pub(crate) fn new(events: Arc<EventBus>) -> Self {
        let states = ["codex", "claude", "claude-mode"]
            .into_iter()
            .map(|provider| {
                (
                    provider.to_owned(),
                    AuthSnapshot {
                        state: "unknown".into(),
                        detail: None,
                        login_url: None,
                        device_code: None,
                        needs_input: false,
                    },
                )
            })
            .collect();
        Self {
            inner: Arc::new(AuthInner {
                states: Mutex::new(states),
                sessions: Mutex::new(HashMap::new()),
                next_session: AtomicU64::new(1),
                events,
            }),
        }
    }

    pub(crate) fn refresh_all(&self) {
        self.refresh("codex");
        self.refresh("claude");
        self.refresh("claude-mode");
    }

    pub(crate) fn refresh(&self, provider: &str) -> bool {
        let provider = provider.to_owned();
        {
            let Ok(mut states) = self.inner.states.lock() else {
                return false;
            };
            let Some(snapshot) = states.get_mut(&provider) else {
                return false;
            };
            if snapshot.state == "checking" {
                return true;
            }
            snapshot.state = "checking".into();
            snapshot.detail = None;
            snapshot.login_url = None;
            snapshot.device_code = None;
            snapshot.needs_input = false;
        }
        self.publish(&provider);
        let manager = self.clone();
        let checked_provider = provider.clone();
        let spawned = thread::Builder::new()
            .name(format!("xd-auth-{provider}"))
            .spawn(move || {
                let snapshot = check_status(&checked_provider);
                if let Ok(mut states) = manager.inner.states.lock() {
                    states.insert(checked_provider.clone(), snapshot);
                }
                manager.publish(&checked_provider);
            });
        if let Err(error) = spawned {
            if let Ok(mut states) = self.inner.states.lock() {
                states.insert(
                    provider.clone(),
                    failed(format!("Cannot check sign-in status: {error}")),
                );
            }
            self.publish(&provider);
            return false;
        }
        true
    }

    pub(crate) fn state(&self, provider: &str) -> String {
        self.inner
            .states
            .lock()
            .ok()
            .and_then(|states| states.get(provider).map(|snapshot| snapshot.state.clone()))
            .unwrap_or_else(|| "unknown".into())
    }

    pub(crate) fn login(&self, provider: &str) -> Result<(), String> {
        self.begin_session(provider, true)
    }

    pub(crate) fn logout(&self, provider: &str) -> Result<(), String> {
        self.begin_session(provider, false)
    }

    pub(crate) fn input(&self, provider: &str, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() || text.len() > 4096 {
            return Err("Authentication input must contain 1 to 4096 bytes.".into());
        }
        let input = self
            .inner
            .sessions
            .lock()
            .map_err(|_| "Authentication service is unavailable.".to_owned())?
            .get(provider)
            .and_then(|session| session.input.clone())
            .ok_or_else(|| format!("{provider} is not waiting for authentication input."))?;
        let mut input = input
            .lock()
            .map_err(|_| "Authentication input is unavailable.".to_owned())?;
        writeln!(input, "{text}")
            .map_err(|error| format!("Cannot send authentication input: {error}"))
    }

    pub(crate) fn cancel(&self, provider: &str) -> Result<(), String> {
        let pid = {
            let mut sessions = self
                .inner
                .sessions
                .lock()
                .map_err(|_| "Authentication service is unavailable.".to_owned())?;
            let session = sessions
                .get_mut(provider)
                .ok_or_else(|| format!("{provider} is not signing in."))?;
            session.cancelled = true;
            session.pid
        };
        if let Some(pid) = pid {
            let _ = Command::new("kill")
                .args(["-INT", &pid.to_string()])
                .status();
        }
        Ok(())
    }

    fn begin_session(&self, provider: &str, login: bool) -> Result<(), String> {
        if !matches!(provider, "codex" | "claude" | "claude-mode") {
            return Err("No such assistant.".into());
        }
        let serial = self.inner.next_session.fetch_add(1, Ordering::Relaxed);
        {
            let mut sessions = self
                .inner
                .sessions
                .lock()
                .map_err(|_| "Authentication service is unavailable.".to_owned())?;
            if sessions.contains_key(provider) {
                return Err(format!("{provider} authentication is already busy."));
            }
            sessions.insert(
                provider.to_owned(),
                AuthSession {
                    serial,
                    pid: None,
                    input: None,
                    output: String::new(),
                    cancelled: false,
                },
            );
        }
        self.set_snapshot(
            provider,
            AuthSnapshot {
                state: if login { "signing-in" } else { "signing-out" }.into(),
                detail: Some(if login {
                    "Waiting for sign-in…".into()
                } else {
                    "Signing out…".into()
                }),
                login_url: None,
                device_code: None,
                needs_input: false,
            },
        );
        let manager = self.clone();
        let provider = provider.to_owned();
        let session_provider = provider.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("xd-auth-session-{provider}"))
            .spawn(move || manager.run_session(&session_provider, serial, login))
        {
            if let Ok(mut sessions) = self.inner.sessions.lock() {
                sessions.remove(&provider);
            }
            self.set_snapshot(
                &provider,
                failed(format!("Cannot start authentication: {error}")),
            );
            return Err(format!("Cannot start authentication: {error}"));
        }
        Ok(())
    }

    fn run_session(&self, provider: &str, serial: u64, login: bool) {
        let mut command = match provider {
            "claude" => {
                let mut command = Command::new(resolve_claude());
                command.args(if login {
                    ["auth", "login"].as_slice()
                } else {
                    ["auth", "logout"].as_slice()
                });
                command
            }
            "claude-mode" => {
                let mut command = Command::new(resolve_claude_proxy());
                command.args(if login {
                    ["codex", "auth", "device"].as_slice()
                } else {
                    ["codex", "auth", "logout"].as_slice()
                });
                command
            }
            _ => {
                let mut command = Command::new(resolve_codex());
                command.args(if login {
                    ["login", "--device-auth"].as_slice()
                } else {
                    ["logout"].as_slice()
                });
                command
            }
        };
        command
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .stdin(if login { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.finish_session(
                    provider,
                    serial,
                    Err(format!("Cannot start authentication: {error}")),
                );
                return;
            }
        };
        let input = child.stdin.take().map(|input| Arc::new(Mutex::new(input)));
        if let Ok(mut sessions) = self.inner.sessions.lock()
            && let Some(session) = sessions.get_mut(provider)
            && session.serial == serial
        {
            session.pid = Some(child.id());
            session.input = input;
        }
        let stdout = child
            .stdout
            .take()
            .map(|stream| self.drain_login_stream(provider, serial, stream, login));
        let stderr = child
            .stderr
            .take()
            .map(|stream| self.drain_login_stream(provider, serial, stream, login));
        let status = child.wait();
        let stdout = stdout
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        let stderr = stderr
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        let result = match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(command_detail(&stdout, &stderr, status.success())),
            Err(error) => Err(format!("Authentication process failed: {error}")),
        };
        self.finish_session(provider, serial, result);
    }

    fn drain_login_stream(
        &self,
        provider: &str,
        serial: u64,
        mut stream: impl Read + Send + 'static,
        publish: bool,
    ) -> thread::JoinHandle<Vec<u8>> {
        let manager = self.clone();
        let provider = provider.to_owned();
        thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let remaining = OUTPUT_LIMIT.saturating_sub(kept.len());
                        kept.extend_from_slice(&buffer[..count.min(remaining)]);
                        if publish {
                            manager.append_login_output(&provider, serial, &buffer[..count]);
                        }
                    }
                }
            }
            kept
        })
    }

    fn append_login_output(&self, provider: &str, serial: u64, bytes: &[u8]) {
        let output = {
            let Ok(mut sessions) = self.inner.sessions.lock() else {
                return;
            };
            let Some(session) = sessions
                .get_mut(provider)
                .filter(|session| session.serial == serial)
            else {
                return;
            };
            session.output.push_str(&String::from_utf8_lossy(bytes));
            if session.output.len() > OUTPUT_LIMIT {
                session.output.drain(..session.output.len() - OUTPUT_LIMIT);
            }
            session.output.clone()
        };
        let (login_url, device_code, needs_input) = login_instructions(provider, &output);
        if let Ok(mut states) = self.inner.states.lock()
            && let Some(snapshot) = states.get_mut(provider)
            && (
                snapshot.login_url.as_ref(),
                snapshot.device_code.as_ref(),
                snapshot.needs_input,
            ) != (login_url.as_ref(), device_code.as_ref(), needs_input)
        {
            snapshot.login_url = login_url;
            snapshot.device_code = device_code;
            snapshot.needs_input = needs_input;
            drop(states);
            self.publish(provider);
        }
    }

    fn finish_session(&self, provider: &str, serial: u64, result: Result<(), String>) {
        let cancelled = self
            .inner
            .sessions
            .lock()
            .ok()
            .and_then(|mut sessions| {
                sessions
                    .remove(provider)
                    .filter(|session| session.serial == serial)
                    .map(|session| session.cancelled)
            })
            .unwrap_or(false);
        match result {
            Ok(()) | Err(_) if cancelled => {
                self.set_snapshot(provider, failed("Sign-in canceled.".into()));
                self.refresh(provider);
            }
            Ok(()) => {
                self.refresh(provider);
            }
            Err(error) => self.set_snapshot(provider, failed(error)),
        }
    }

    fn set_snapshot(&self, provider: &str, snapshot: AuthSnapshot) {
        if let Ok(mut states) = self.inner.states.lock() {
            states.insert(provider.to_owned(), snapshot);
        }
        self.publish(provider);
    }

    pub(crate) fn snapshots(&self) -> Value {
        let providers = self
            .inner
            .states
            .lock()
            .map(|states| {
                ["codex", "claude", "claude-mode"]
                    .into_iter()
                    .filter_map(|provider| {
                        states
                            .get(provider)
                            .map(|snapshot| snapshot_value(provider, snapshot))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        json!({"ok": true, "providers": providers})
    }

    fn publish(&self, provider: &str) {
        let Some(snapshot) = self
            .inner
            .states
            .lock()
            .ok()
            .and_then(|states| states.get(provider).cloned())
        else {
            return;
        };
        let mut event = snapshot_value(provider, &snapshot);
        event["event"] = Value::String("agent-auth-changed".into());
        self.inner.events.publish(event);
    }
}

fn snapshot_value(provider: &str, snapshot: &AuthSnapshot) -> Value {
    let mut value = json!({
        "provider": provider,
        "display_name": match provider {
            "claude" => "Claude Code",
            "claude-mode" => "Claude mode",
            _ => "Codex",
        },
        "state": snapshot.state,
        "needs_input": snapshot.needs_input,
    });
    if let Some(detail) = &snapshot.detail {
        value["detail"] = Value::String(detail.clone());
    }
    if let Some(login_url) = &snapshot.login_url {
        value["login_url"] = Value::String(login_url.clone());
    }
    if let Some(device_code) = &snapshot.device_code {
        value["device_code"] = Value::String(device_code.clone());
    }
    value
}

fn check_status(provider: &str) -> AuthSnapshot {
    let mut command = match provider {
        "claude" => {
            let mut command = Command::new(resolve_claude());
            command.args(["auth", "status", "--json"]);
            command
        }
        "claude-mode" => {
            let mut command = Command::new(resolve_claude_proxy());
            command.args(["codex", "auth", "status"]);
            command
        }
        _ => {
            let mut command = Command::new(resolve_codex());
            command.args(["login", "status"]);
            command
        }
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failed(format!("Cannot check sign-in status: {error}")),
    };
    let stdout = child.stdout.take().map(drain_bounded);
    let stderr = child.stderr.take().map(drain_bounded);
    let deadline = Instant::now() + STATUS_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(_) => break None,
        }
    };
    let stdout = stdout
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let Some(status) = status else {
        return failed("Sign-in status check timed out.".into());
    };
    if provider == "claude" {
        return parse_claude_status(&stdout, &stderr, status.success());
    }
    if provider == "claude-mode" {
        return parse_proxy_status(&stdout, &stderr, status.success());
    }
    parse_codex_status(&stdout, &stderr, status.success())
}

fn parse_claude_status(stdout: &[u8], stderr: &[u8], success: bool) -> AuthSnapshot {
    let parsed = serde_json::from_slice::<Value>(stdout).ok();
    let logged_in = parsed
        .as_ref()
        .and_then(|value| value.get("loggedIn"))
        .and_then(Value::as_bool);
    match logged_in {
        Some(true) => AuthSnapshot {
            state: "signed-in".into(),
            detail: parsed
                .as_ref()
                .and_then(|value| value.get("authMethod"))
                .and_then(Value::as_str)
                .filter(|method| *method != "none")
                .map(|method| format!("Signed in with {method}."))
                .or_else(|| Some("Signed in.".into())),
            login_url: None,
            device_code: None,
            needs_input: false,
        },
        Some(false) => signed_out(),
        None => failed(command_detail(stdout, stderr, success)),
    }
}

fn parse_codex_status(stdout: &[u8], stderr: &[u8], success: bool) -> AuthSnapshot {
    let detail = command_detail(stdout, stderr, success);
    if detail.to_lowercase().contains("not logged in") {
        signed_out()
    } else if success {
        AuthSnapshot {
            state: "signed-in".into(),
            detail: Some(detail),
            login_url: None,
            device_code: None,
            needs_input: false,
        }
    } else {
        failed(detail)
    }
}

fn parse_proxy_status(stdout: &[u8], stderr: &[u8], success: bool) -> AuthSnapshot {
    let detail = command_detail(stdout, stderr, success);
    let normalized = detail.to_ascii_lowercase();
    if normalized.contains("not authenticated") || normalized.contains("not logged in") {
        signed_out()
    } else if success {
        AuthSnapshot {
            state: "signed-in".into(),
            detail: Some(detail),
            login_url: None,
            device_code: None,
            needs_input: false,
        }
    } else {
        failed(detail)
    }
}

fn drain_bounded(mut stream: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let remaining = OUTPUT_LIMIT.saturating_sub(kept.len());
                    kept.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            }
        }
        kept
    })
}

fn command_detail(stdout: &[u8], stderr: &[u8], success: bool) -> String {
    [stdout, stderr]
        .into_iter()
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .flat_map(str::lines)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("WARNING:"))
        .next_back()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            if success {
                "Signed in.".into()
            } else {
                "Could not read sign-in status.".into()
            }
        })
}

fn signed_out() -> AuthSnapshot {
    AuthSnapshot {
        state: "signed-out".into(),
        detail: Some("Not signed in.".into()),
        login_url: None,
        device_code: None,
        needs_input: false,
    }
}

fn failed(detail: String) -> AuthSnapshot {
    AuthSnapshot {
        state: "failed".into(),
        detail: Some(detail),
        login_url: None,
        device_code: None,
        needs_input: false,
    }
}

fn login_instructions(provider: &str, output: &str) -> (Option<String>, Option<String>, bool) {
    let clean = strip_ansi(output);
    let login_url = clean
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .map(|word| {
            word.trim_end_matches(['.', ',', ';', ':', ')', ']', '}'])
                .to_owned()
        });
    let device_code = matches!(provider, "codex" | "claude-mode")
        .then(|| {
            clean.split_whitespace().find_map(|word| {
                let word = word.trim_matches(|character: char| {
                    !character.is_ascii_alphanumeric() && character != '-'
                });
                (word.len() >= 9
                    && word.contains('-')
                    && word.chars().all(|character| {
                        character.is_ascii_uppercase()
                            || character.is_ascii_digit()
                            || character == '-'
                    }))
                .then(|| word.to_owned())
            })
        })
        .flatten();
    let needs_input = provider == "claude" && clean.to_ascii_lowercase().contains("paste code");
    (login_url, device_code, needs_input)
}

fn strip_ansi(text: &str) -> String {
    let mut clean = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == 0x1b {
            for next in bytes.by_ref() {
                if (0x40..=0x7e).contains(&next) {
                    break;
                }
            }
        } else {
            clean.push(byte);
        }
    }
    String::from_utf8_lossy(&clean).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_and_claude_authentication_status() {
        let codex = parse_codex_status(b"Logged in using ChatGPT\n", b"", true);
        assert_eq!(codex.state, "signed-in");
        assert_eq!(codex.detail.as_deref(), Some("Logged in using ChatGPT"));
        assert_eq!(
            parse_codex_status(b"Not logged in\n", b"", false).state,
            "signed-out"
        );
        assert_eq!(
            parse_proxy_status(b"Codex is not authenticated\n", b"", false).state,
            "signed-out"
        );
        assert_eq!(
            parse_proxy_status(b"Authenticated with Codex\n", b"", true).state,
            "signed-in"
        );

        let claude =
            parse_claude_status(br#"{"loggedIn":true,"authMethod":"claude.ai"}"#, b"", true);
        assert_eq!(claude.state, "signed-in");
        assert_eq!(claude.detail.as_deref(), Some("Signed in with claude.ai."));
        assert_eq!(
            parse_claude_status(br#"{"loggedIn":false}"#, b"", true).state,
            "signed-out"
        );
    }

    #[test]
    fn bounds_status_details_to_the_drained_output() {
        let detail = command_detail(b"WARNING: ignored\nfirst\nlast\n", b"", true);
        assert_eq!(detail, "last");
        assert_eq!(
            parse_claude_status(b"not json", b"bad status", false).state,
            "failed"
        );
    }

    #[test]
    fn extracts_only_structured_login_instructions() {
        assert_eq!(
            login_instructions(
                "codex",
                "Open https://auth.openai.com/device. Enter ABCD-EFGH."
            ),
            (
                Some("https://auth.openai.com/device".into()),
                Some("ABCD-EFGH".into()),
                false
            )
        );
        assert_eq!(
            login_instructions(
                "claude-mode",
                "Open https://auth.openai.com/device. Enter WXYZ-1234."
            ),
            (
                Some("https://auth.openai.com/device".into()),
                Some("WXYZ-1234".into()),
                false
            )
        );
        assert_eq!(
            login_instructions(
                "claude",
                "Visit https://claude.ai/login then paste code here"
            ),
            (Some("https://claude.ai/login".into()), None, true)
        );
    }
}
