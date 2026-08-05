use std::{
    collections::HashMap,
    io::Read,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    EventBus,
    agent::{resolve_claude, resolve_codex},
};

const OUTPUT_LIMIT: usize = 64 * 1024;
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct AuthManager {
    inner: Arc<AuthInner>,
}

struct AuthInner {
    states: Mutex<HashMap<String, AuthSnapshot>>,
    events: Arc<EventBus>,
}

#[derive(Clone)]
struct AuthSnapshot {
    state: String,
    detail: Option<String>,
}

impl AuthManager {
    pub(crate) fn new(events: Arc<EventBus>) -> Self {
        let states = ["codex", "claude"]
            .into_iter()
            .map(|provider| {
                (
                    provider.to_owned(),
                    AuthSnapshot {
                        state: "unknown".into(),
                        detail: None,
                    },
                )
            })
            .collect();
        Self {
            inner: Arc::new(AuthInner {
                states: Mutex::new(states),
                events,
            }),
        }
    }

    pub(crate) fn refresh_all(&self) {
        self.refresh("codex");
        self.refresh("claude");
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

    pub(crate) fn snapshots(&self) -> Value {
        let providers = self
            .inner
            .states
            .lock()
            .map(|states| {
                ["codex", "claude"]
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
        "display_name": if provider == "claude" { "Claude Code" } else { "Codex" },
        "state": snapshot.state,
        "needs_input": false,
    });
    if let Some(detail) = &snapshot.detail {
        value["detail"] = Value::String(detail.clone());
    }
    value
}

fn check_status(provider: &str) -> AuthSnapshot {
    let mut command = if provider == "claude" {
        let mut command = Command::new(resolve_claude());
        command.args(["auth", "status", "--json"]);
        command
    } else {
        let mut command = Command::new(resolve_codex());
        command.args(["login", "status"]);
        command
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
    }
}

fn failed(detail: String) -> AuthSnapshot {
    AuthSnapshot {
        state: "failed".into(),
        detail: Some(detail),
    }
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
}
