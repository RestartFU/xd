//! Discover picker entries through the installed assistants' initialization protocols.
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Stdio},
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub(crate) struct Model {
    pub id: String,
    pub name: String,
    pub context_window: i64,
    pub is_default: bool,
}

#[derive(Default)]
struct Cache {
    checked: Option<Instant>,
    models: Vec<Model>,
}

pub(crate) fn models(backend: &str) -> Vec<Model> {
    static CODEX: OnceLock<Mutex<Cache>> = OnceLock::new();
    static CLAUDE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = match backend {
        "codex" => &CODEX,
        "claude" => &CLAUDE,
        _ => return Vec::new(),
    };
    // Unit tests use explicit protocol fixtures rather than installed accounts.
    if cfg!(test) {
        return Vec::new();
    }
    let Ok(mut cache) = cache.get_or_init(Default::default).lock() else {
        return Vec::new();
    };
    if cache
        .checked
        .is_none_or(|checked| checked.elapsed() >= Duration::from_secs(300))
    {
        if let Some(models) = discover(backend) {
            cache.models = models;
        }
        cache.checked = Some(Instant::now());
    }
    cache.models.clone()
}

struct Probe(Child);
impl Drop for Probe {
    fn drop(&mut self) {
        // Assistants can spawn helpers during initialization. Reap this probe's group.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn discover(backend: &str) -> Option<Vec<Model>> {
    let mut command = crate::background_process::command(match backend {
        "codex" => crate::agent::resolve_codex(),
        _ => crate::agent::resolve_claude(),
    });
    if backend == "codex" {
        command.arg("app-server");
    } else {
        command.args([
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
        ]);
    }
    probe(command, backend, Duration::from_secs(10))
}

fn probe(
    mut command: std::process::Command,
    backend: &str,
    timeout: Duration,
) -> Option<Vec<Model>> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut probe = Probe(command.spawn().ok()?);
    let mut stdin = probe.0.stdin.take()?;
    let stdout = probe.0.stdout.take()?;
    let (sender, receiver) = mpsc::sync_channel(16);
    thread::spawn(move || {
        // Bound both individual lines and the total initialization output.
        let reader = BufReader::new(stdout.take(8 * 1024 * 1024));
        for line in reader.split(b'\n') {
            let Ok(line) = line else { break };
            if line.len() > 2 * 1024 * 1024 {
                break;
            }
            if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                if sender.send(value).is_err() {
                    break;
                }
            }
        }
    });
    let initialize = if backend == "codex" {
        crate::codex_app::CodexAppParser::initialize()
    } else {
        json!({"type":"control_request", "request_id":"xd-models", "request":{"subtype":"initialize"}})
    };
    writeln!(stdin, "{initialize}").ok()?;
    let deadline = Instant::now() + timeout;
    let mut entries = Vec::new();
    let mut cursors = HashSet::new();
    loop {
        let value = receiver
            .recv_timeout(deadline.checked_duration_since(Instant::now())?)
            .ok()?;
        if backend == "claude" {
            if value["type"] == "control_response" && value["response"]["request_id"] == "xd-models"
            {
                return parse_models(backend, &value["response"]["response"]["models"]);
            }
        } else if value["id"] == 1 {
            if value.get("error").is_some() {
                return None;
            }
            writeln!(stdin, "{}", json!({"method":"initialized"})).ok()?;
            writeln!(
                stdin,
                "{}",
                json!({"id":2,"method":"model/list","params":{"limit":100}})
            )
            .ok()?;
        } else if value["id"] == 2 {
            entries.extend(value["result"]["data"].as_array()?.iter().cloned());
            if entries.len() > 2000 {
                return None;
            }
            match value["result"]["nextCursor"]
                .as_str()
                .filter(|c| !c.is_empty())
            {
                Some(cursor) if cursors.insert(cursor.to_owned()) => {
                    writeln!(
                        stdin,
                        "{}",
                        json!({"id":2,"method":"model/list","params":{"limit":100,"cursor":cursor}})
                    )
                    .ok()?;
                }
                Some(_) => return None,
                None => return parse_models(backend, &json!(entries)),
            }
        }
    }
}

fn parse_models(backend: &str, values: &Value) -> Option<Vec<Model>> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for value in values.as_array()? {
        if value["hidden"] == true {
            continue;
        }
        let id = if backend == "codex" {
            value["model"].as_str().or_else(|| value["id"].as_str())
        } else {
            // Resolve moving aliases so a saved chat retains its selected model.
            value["resolvedModel"]
                .as_str()
                .or_else(|| value["value"].as_str())
        }?;
        if id.trim().is_empty() || id.len() > 256 {
            continue;
        }
        let is_default =
            value["isDefault"] == true || (backend == "claude" && value["value"] == "default");
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let name = value["displayName"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(id);
        // The default Claude alias has a generic label; use its matching named entry.
        let name = if backend == "claude" && value["value"] == "default" {
            values
                .as_array()?
                .iter()
                .find(|other| other["resolvedModel"] == id && other["value"] != "default")
                .and_then(|other| other["displayName"].as_str())
                .unwrap_or(name)
        } else {
            name
        };
        models.push(Model {
            id: id.to_owned(),
            name: name.to_owned(),
            context_window: value["contextWindow"]
                .as_i64()
                .filter(|n| *n > 0)
                .unwrap_or(0),
            is_default,
        });
    }
    (!models.is_empty()).then_some(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_filters_hidden_models_and_preserves_provider_names() {
        let models = parse_models(
            "codex",
            &json!([
                {"id":"entry", "model":"new-model", "displayName":"New model", "isDefault":true},
                {"model":"internal", "hidden":true},
                {"model":"new-model", "displayName":"Duplicate"}
            ]),
        )
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "new-model");
        assert_eq!(models[0].name, "New model");
        assert!(models[0].is_default);
        assert_eq!(models[0].context_window, 0);
    }

    #[test]
    fn claude_resolves_aliases_and_deduplicates_default() {
        let models = parse_models("claude", &json!([
            {"value":"default", "resolvedModel":"claude-new", "displayName":"Default (recommended)"},
            {"value":"opus", "resolvedModel":"claude-new", "displayName":"Opus Next"},
            {"value":"sonnet", "resolvedModel":"claude-other", "displayName":"Sonnet Next"},
            {"value":"custom", "displayName":"Custom"}
        ])).unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "claude-new");
        assert_eq!(models[0].name, "Opus Next");
        assert!(models[0].is_default);
        assert_eq!(models[2].id, "custom");
    }

    #[cfg(unix)]
    #[test]
    fn probes_claude_without_sending_a_prompt() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", r#"read request
case "$request" in *control_request*initialize*) ;; *) exit 1;; esac
printf '%s\n' '{"type":"control_response","response":{"request_id":"xd-models","response":{"models":[{"value":"opus","resolvedModel":"next-opus","displayName":"Next Opus"}]}}}'
read unexpected
"#]);
        let models = probe(command, "claude", Duration::from_secs(1)).unwrap();
        assert_eq!(models[0].id, "next-opus");
    }

    #[cfg(unix)]
    #[test]
    fn probes_codex_with_pagination() {
        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            r#"read init
printf '%s\n' '{"id":1,"result":{}}'
read initialized
read list
printf '%s\n' '{"id":2,"result":{"data":[{"model":"first"}],"nextCursor":"page2"}}'
read next
case "$next" in *page2*) ;; *) exit 1;; esac
printf '%s\n' '{"id":2,"result":{"data":[{"model":"second"}],"nextCursor":null}}'
read unexpected
"#,
        ]);
        let models = probe(command, "codex", Duration::from_secs(1)).unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn stalled_probe_is_bounded() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "read request; sleep 30"]);
        let start = Instant::now();
        assert!(probe(command, "claude", Duration::from_millis(50)).is_none());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn empty_or_invalid_responses_do_not_replace_cached_models() {
        assert!(parse_models("codex", &json!([])).is_none());
        assert!(parse_models("claude", &json!({})).is_none());
    }
}
