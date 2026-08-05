use std::{env, path::PathBuf, process::Command};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexEvent {
    Session(String),
    Text(String),
    Tool(String),
    Usage { input: u64, output: u64 },
    Completed,
    Error(String),
}

#[derive(Default)]
pub struct CodexParser {
    started_commands: std::collections::HashSet<String>,
}

pub struct CodexCommand<'a> {
    pub prompt: &'a str,
    pub workdir: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
    pub access: &'a str,
    pub session_id: Option<&'a str>,
}

impl CodexParser {
    pub fn feed(&mut self, line: &str) -> Vec<CodexEvent> {
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match root.get("type").and_then(Value::as_str) {
            Some("thread.started") => root
                .get("thread_id")
                .and_then(Value::as_str)
                .map(|id| vec![CodexEvent::Session(id.to_owned())])
                .unwrap_or_default(),
            Some("item.started") => {
                let Some(item) = root.get("item") else {
                    return Vec::new();
                };
                if item.get("type").and_then(Value::as_str) != Some("command_execution") {
                    return Vec::new();
                }
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    self.started_commands.insert(id.to_owned());
                }
                vec![CodexEvent::Tool(tool_summary(item))]
            }
            Some("item.completed") => {
                let Some(item) = root.get("item") else {
                    return Vec::new();
                };
                if item
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| self.started_commands.remove(id))
                {
                    return Vec::new();
                }
                match item.get("type").and_then(Value::as_str) {
                    Some("agent_message") => item
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(|text| vec![CodexEvent::Text(text.to_owned())])
                        .unwrap_or_default(),
                    Some(_) => vec![CodexEvent::Tool(tool_summary(item))],
                    None => Vec::new(),
                }
            }
            Some("turn.completed") => {
                let usage = root.get("usage");
                let input = usage
                    .and_then(|usage| usage.get("input_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let output = usage
                    .and_then(|usage| usage.get("output_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                vec![CodexEvent::Usage { input, output }, CodexEvent::Completed]
            }
            Some("turn.failed") => vec![CodexEvent::Error(error_message(&root))],
            // Codex emits transient `error` frames while reconnecting. Keep the
            // latest message, but let a later turn.completed decide success.
            Some("error") => vec![CodexEvent::Error(error_message(&root))],
            _ => Vec::new(),
        }
    }
}

impl CodexCommand<'_> {
    pub fn build(&self) -> Command {
        let mut command = Command::new(resolve_codex());
        command.arg("exec");
        if let Some(session_id) = self.session_id {
            command.args(["resume", "--json", "--skip-git-repo-check"]);
            append_model_and_effort(&mut command, self.model, self.effort);
            if self.access == "full" {
                command.arg("--dangerously-bypass-approvals-and-sandbox");
            }
            command.args([session_id, self.prompt]);
        } else {
            command.args([
                "--json",
                "--skip-git-repo-check",
                "--color",
                "never",
                "-C",
                self.workdir,
            ]);
            append_model_and_effort(&mut command, self.model, self.effort);
            append_access(&mut command, self.access);
            command.arg(self.prompt);
        }
        command
    }
}

fn append_model_and_effort(command: &mut Command, model: &str, effort: &str) {
    command.args(["-m", model]);
    command.args(["-c", &format!("model_reasoning_effort=\"{effort}\"")]);
}

fn append_access(command: &mut Command, access: &str) {
    match access {
        "full" => {
            command.arg("--dangerously-bypass-approvals-and-sandbox");
        }
        "edit" => {
            command.args(["-s", "workspace-write"]);
        }
        _ => {
            command.args(["-s", "read-only"]);
        }
    }
}

fn resolve_codex() -> PathBuf {
    if let Some(configured) = env::var_os("XD_CODEX_EXECUTABLE").filter(|path| !path.is_empty()) {
        return configured.into();
    }
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        for relative in ["codex-package/bin/codex", "libexec/codex-package/bin/codex"] {
            let candidate = parent.join(relative);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("codex")
}

fn tool_summary(item: &Value) -> String {
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("tool");
    if kind == "command_execution"
        && let Some(command) = item.get("command").and_then(Value::as_str)
    {
        return format!("$ {}", compact(command, 110));
    }
    for key in [
        "file_path",
        "filePath",
        "path",
        "pattern",
        "url",
        "query",
        "description",
        "prompt",
    ] {
        if let Some(detail) = item.get(key).and_then(Value::as_str) {
            return format!("{kind}  {}", compact(detail, 110));
        }
    }
    kind.to_owned()
}

fn compact(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= limit {
        return compact;
    }
    let mut shortened = compact
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    shortened.push('…');
    shortened
}

fn error_message(root: &Value) -> String {
    root.get("message")
        .and_then(Value::as_str)
        .or_else(|| {
            root.get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
        })
        .unwrap_or("The turn failed")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_captured_codex_exec_stream_without_duplicate_commands() {
        let fixture = include_str!("../../tests/fixtures/codex-exec.jsonl");
        let mut parser = CodexParser::default();
        let events = fixture
            .lines()
            .flat_map(|line| parser.feed(line))
            .collect::<Vec<_>>();
        assert_eq!(
            events,
            vec![
                CodexEvent::Session("019f9b16-df5f-7182-bdc6-1cce26148979".into()),
                CodexEvent::Tool(
                    "$ gh run watch 30230367515 --repo RestartFU/xd --exit-status".into()
                ),
                CodexEvent::Text("hello from hy".into()),
                CodexEvent::Usage {
                    input: 16_941,
                    output: 7
                },
                CodexEvent::Completed,
            ]
        );
    }

    #[test]
    fn lets_completion_recover_after_a_transient_error_frame() {
        let fixture = include_str!("../../tests/fixtures/codex-recoverable-error.jsonl");
        let mut parser = CodexParser::default();
        let events = fixture
            .lines()
            .flat_map(|line| parser.feed(line))
            .collect::<Vec<_>>();
        assert!(matches!(events[0], CodexEvent::Error(_)));
        assert_eq!(events[1], CodexEvent::Text("still working".into()));
        assert_eq!(events.last(), Some(&CodexEvent::Completed));
    }

    #[test]
    fn builds_new_and_resumed_noninteractive_commands() {
        let new = CodexCommand {
            prompt: "hello",
            workdir: "/workspace",
            model: "gpt-5.6-sol",
            effort: "high",
            access: "edit",
            session_id: None,
        }
        .build();
        let args = new
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["-C", "/workspace"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-s", "workspace-write"])
        );
        assert_eq!(args.last().map(String::as_str), Some("hello"));

        let resumed = CodexCommand {
            session_id: Some("thread-1"),
            access: "full",
            ..CodexCommand {
                prompt: "continue",
                workdir: "/workspace",
                model: "gpt-5.6-sol",
                effort: "max",
                access: "full",
                session_id: None,
            }
        }
        .build();
        let args = resumed
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(&args[..3], ["exec", "resume", "--json"]);
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".into()));
        assert_eq!(&args[args.len() - 2..], ["thread-1", "continue"]);
    }
}
