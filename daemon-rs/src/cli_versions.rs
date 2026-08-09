use std::{
    collections::HashMap,
    io::Read,
    path::Path,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    EventBus,
    agent::{resolve_claude, resolve_codex},
    background_process::command as background_command,
    claude_proxy::resolve_claude_proxy,
};

const OUTPUT_LIMIT: usize = 4 * 1024;
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(crate) struct CliVersions {
    inner: Arc<CliVersionsInner>,
}

struct CliVersionsInner {
    states: Mutex<HashMap<String, CliSnapshot>>,
    events: Arc<EventBus>,
}

#[derive(Clone)]
struct CliSnapshot {
    display_name: String,
    state: String,
    version: Option<String>,
    detail: Option<String>,
}

impl CliVersions {
    pub(crate) fn new(events: Arc<EventBus>) -> Self {
        let states = [
            ("codex", "Codex"),
            ("claude", "Claude Code"),
            ("claude-mode", "Claude mode proxy"),
        ]
        .into_iter()
        .map(|(provider, display_name)| {
            (
                provider.to_owned(),
                CliSnapshot {
                    display_name: display_name.into(),
                    state: "idle".into(),
                    version: None,
                    detail: None,
                },
            )
        })
        .collect();
        Self {
            inner: Arc::new(CliVersionsInner {
                states: Mutex::new(states),
                events,
            }),
        }
    }

    pub(crate) fn snapshots(&self) -> Value {
        self.refresh_all();
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

    fn refresh_all(&self) {
        for provider in ["codex", "claude", "claude-mode"] {
            self.refresh(provider);
        }
    }

    fn refresh(&self, provider: &str) {
        {
            let Ok(mut states) = self.inner.states.lock() else {
                return;
            };
            let Some(snapshot) = states.get_mut(provider) else {
                return;
            };
            if snapshot.state == "checking" {
                return;
            }
            snapshot.state = "checking".into();
            snapshot.detail = None;
        }
        self.publish(provider);
        let versions = self.clone();
        let provider = provider.to_owned();
        let checked_provider = provider.clone();
        let spawned = thread::Builder::new()
            .name(format!("xd-cli-version-{provider}"))
            .spawn(move || {
                let result = match checked_provider.as_str() {
                    "claude" => read_version(&resolve_claude(), CHECK_TIMEOUT),
                    "claude-mode" => read_version(&resolve_claude_proxy(), CHECK_TIMEOUT),
                    _ => read_version(&resolve_codex(), CHECK_TIMEOUT),
                };
                if let Ok(mut states) = versions.inner.states.lock()
                    && let Some(snapshot) = states.get_mut(&checked_provider)
                {
                    snapshot.state = if result.is_ok() { "idle" } else { "failed" }.into();
                    match result {
                        Ok(version) => {
                            snapshot.version = Some(version);
                            snapshot.detail = None;
                        }
                        Err(error) => snapshot.detail = Some(error),
                    }
                }
                versions.publish(&checked_provider);
            });
        if let Err(error) = spawned
            && let Ok(mut states) = self.inner.states.lock()
            && let Some(snapshot) = states.get_mut(&provider)
        {
            snapshot.state = "failed".into();
            snapshot.detail = Some(format!("Cannot start the version check: {error}"));
            drop(states);
            self.publish(&provider);
        }
    }

    fn publish(&self, provider: &str) {
        let snapshot = self
            .inner
            .states
            .lock()
            .ok()
            .and_then(|states| states.get(provider).cloned());
        if let Some(snapshot) = snapshot {
            let mut event = snapshot_value(provider, &snapshot);
            event["event"] = Value::String("agent-cli-changed".into());
            self.inner.events.publish(event);
        }
    }
}

fn snapshot_value(provider: &str, snapshot: &CliSnapshot) -> Value {
    let mut value = json!({
        "provider": provider,
        "display_name": snapshot.display_name,
        "state": snapshot.state,
    });
    if let Some(version) = &snapshot.version {
        value["version"] = Value::String(version.clone());
    }
    if let Some(detail) = &snapshot.detail {
        value["detail"] = Value::String(detail.clone());
    }
    value
}

fn read_version(executable: &Path, timeout: Duration) -> Result<String, String> {
    let mut child = background_command(executable)
        .arg("--version")
        .env("DISABLE_AUTOUPDATER", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot run {} --version: {error}", executable.display()))?;
    let stdout = child
        .stdout
        .take()
        .map(|stream| thread::spawn(move || read_limited(stream)))
        .ok_or_else(|| "Cannot capture assistant version output.".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .map(|stream| thread::spawn(move || read_limited(stream)))
        .ok_or_else(|| "Cannot capture assistant version errors.".to_owned())?;
    let deadline = Instant::now() + timeout;
    let mut status = None;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(result)) => {
                status = Some(result);
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(format!("Cannot wait for the version check: {error}")),
        }
    }
    let timed_out = status.is_none();
    if timed_out {
        let _ = child.kill();
        status = child.wait().ok();
    }
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if timed_out {
        return Err(format!(
            "Assistant version check timed out after {} seconds.",
            timeout.as_secs()
        ));
    }
    let status = status.ok_or_else(|| "Cannot read assistant version status.".to_owned())?;
    parse_version_output(status, &stdout, &stderr)
}

fn read_limited(mut stream: impl Read) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let Ok(count) = stream.read(&mut buffer) else {
            break;
        };
        if count == 0 {
            break;
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    output
}

fn parse_version_output(
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<String, String> {
    let stdout = clean_output(stdout);
    let stderr = clean_output(stderr);
    if status.success() {
        return stdout
            .lines()
            .chain(stderr.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "The assistant returned no version.".to_owned());
    }
    let detail = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if detail.is_empty() {
        Err(format!("Version check exited with status {status}."))
    } else {
        Err(detail)
    }
}

fn clean_output(output: &[u8]) -> String {
    let mut without_ansi = Vec::with_capacity(output.len());
    let mut bytes = output.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        if byte == 0x1b && bytes.peek() == Some(&b'[') {
            bytes.next();
            for code in bytes.by_ref() {
                if (0x40..=0x7e).contains(&code) {
                    break;
                }
            }
            continue;
        }
        without_ansi.push(byte);
    }
    String::from_utf8_lossy(&without_ansi)
        .chars()
        .filter(|character| matches!(character, '\n' | '\t') || !character.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn reads_only_bounded_version_output_without_running_an_updater() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let directory =
            std::env::temp_dir().join(format!("xd-rust-cli-version-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("assistant");
        fs::write(
            &executable,
            "#!/bin/sh\nset -eu\ntest \"$*\" = --version\ntest \"$DISABLE_AUTOUPDATER\" = 1\nprintf '\\033[32massistant 1.2.3\\033[0m\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            read_version(&executable, Duration::from_secs(2)).unwrap(),
            "assistant 1.2.3"
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn cleans_control_sequences_and_reports_failures() {
        assert_eq!(clean_output(b"\x1b[31mCodex 1.0\x1b[0m\n"), "Codex 1.0\n");
        let failed = if cfg!(unix) {
            std::os::unix::process::ExitStatusExt::from_raw(2 << 8)
        } else {
            return;
        };
        assert_eq!(
            parse_version_output(failed, b"", b"not installed").unwrap_err(),
            "not installed"
        );
    }
}
