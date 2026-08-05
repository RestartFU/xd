use std::{
    collections::{HashMap, HashSet},
    env,
    path::PathBuf,
    process::Command,
};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    Session(String),
    Text(String),
    TextDelta(String),
    Tool(String),
    Usage { input: u64, output: u64 },
    Completed,
    Error(String),
}

#[derive(Default)]
pub struct CodexParser {
    started_commands: HashSet<String>,
}

pub struct AgentCommand<'a> {
    pub backend: &'a str,
    pub prompt: &'a str,
    pub workdir: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
    pub access: &'a str,
    pub session_id: Option<&'a str>,
}

pub enum AgentParser {
    Codex(CodexParser),
    Claude(ClaudeParser),
}

impl AgentParser {
    pub fn new(backend: &str) -> Result<Self, String> {
        match backend {
            "codex" => Ok(Self::Codex(CodexParser::default())),
            "claude" => Ok(Self::Claude(ClaudeParser::default())),
            _ => Err(format!("Unknown assistant backend: {backend}")),
        }
    }

    pub fn feed(&mut self, line: &str) -> Vec<AgentEvent> {
        match self {
            Self::Codex(parser) => parser.feed(line),
            Self::Claude(parser) => parser.feed(line),
        }
    }
}

impl CodexParser {
    pub fn feed(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match root.get("type").and_then(Value::as_str) {
            Some("thread.started") => root
                .get("thread_id")
                .and_then(Value::as_str)
                .map(|id| vec![AgentEvent::Session(id.to_owned())])
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
                vec![AgentEvent::Tool(tool_summary(item))]
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
                        .map(|text| vec![AgentEvent::Text(text.to_owned())])
                        .unwrap_or_default(),
                    Some(_) => vec![AgentEvent::Tool(tool_summary(item))],
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
                vec![AgentEvent::Usage { input, output }, AgentEvent::Completed]
            }
            Some("turn.failed") => vec![AgentEvent::Error(error_message(&root))],
            // Codex emits transient `error` frames while reconnecting. Keep the
            // latest message, but let a later turn.completed decide success.
            Some("error") => vec![AgentEvent::Error(error_message(&root))],
            _ => Vec::new(),
        }
    }
}

impl AgentCommand<'_> {
    pub fn build(&self) -> Command {
        if self.backend == "claude" {
            return self.build_claude();
        }
        let mut command = Command::new(resolve_codex());
        command.arg("exec");
        if let Some(session_id) = self.session_id {
            command.args(["resume", "--json", "--skip-git-repo-check"]);
            append_model_and_effort(&mut command, self.model, self.effort);
            if self.access == "full" {
                command.arg("--dangerously-bypass-approvals-and-sandbox");
            } else {
                let sandbox = if self.access == "edit" {
                    "workspace-write"
                } else {
                    "read-only"
                };
                command.args(["-c", &format!("sandbox_mode=\"{sandbox}\"")]);
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

    fn build_claude(&self) -> Command {
        let mut command = Command::new(resolve_claude());
        if let Some(session_id) = self.session_id {
            command.args(["--resume", session_id]);
        }
        command.args([
            "-p",
            self.prompt,
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--model",
            self.model,
            "--effort",
            self.effort,
            "--permission-mode",
            match self.access {
                "plan" => "plan",
                "edit" => "acceptEdits",
                "full" => "bypassPermissions",
                _ => "manual",
            },
        ]);
        command.current_dir(self.workdir);
        command
    }
}

#[derive(Default)]
pub struct ClaudeParser {
    saw_streamed_text: bool,
    saw_text: bool,
    pending_tools: HashMap<u64, PendingClaudeTool>,
}

struct PendingClaudeTool {
    name: String,
    arguments: String,
    overflowed: bool,
}

impl ClaudeParser {
    const MAX_PENDING_TOOLS: usize = 64;
    const ARGUMENT_LIMIT: usize = 2 * 1024 * 1024;

    pub fn feed(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match root.get("type").and_then(Value::as_str) {
            Some("system") if root.get("subtype").and_then(Value::as_str) == Some("init") => root
                .get("session_id")
                .and_then(Value::as_str)
                .map(|id| vec![AgentEvent::Session(id.to_owned())])
                .unwrap_or_default(),
            Some("stream_event") => self.stream_event(&root),
            Some("assistant") if !self.saw_streamed_text => self.assistant_event(&root),
            Some("result") => {
                let mut events = Vec::new();
                let failed = root
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !failed
                    && !self.saw_text
                    && let Some(text) = root.get("result").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    self.saw_text = true;
                    events.push(AgentEvent::TextDelta(text.to_owned()));
                }
                if failed {
                    events.push(AgentEvent::Error(
                        root.get("result")
                            .and_then(Value::as_str)
                            .filter(|text| !text.is_empty())
                            .unwrap_or("Claude turn failed")
                            .to_owned(),
                    ));
                } else {
                    if let Some(usage) = root.get("usage") {
                        let input = usage
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                            .saturating_add(
                                usage
                                    .get("cache_creation_input_tokens")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0),
                            )
                            .saturating_add(
                                usage
                                    .get("cache_read_input_tokens")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0),
                            );
                        let output = usage
                            .get("output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        events.push(AgentEvent::Usage { input, output });
                    }
                    events.push(AgentEvent::Completed);
                }
                events
            }
            _ => Vec::new(),
        }
    }

    fn stream_event(&mut self, root: &Value) -> Vec<AgentEvent> {
        let Some(event) = root.get("event") else {
            return Vec::new();
        };
        match event.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                let Some(index) = event.get("index").and_then(Value::as_u64) else {
                    return Vec::new();
                };
                let Some(block) = event.get("content_block") else {
                    return Vec::new();
                };
                if block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && (self.pending_tools.contains_key(&index)
                        || self.pending_tools.len() < Self::MAX_PENDING_TOOLS)
                {
                    self.pending_tools.insert(
                        index,
                        PendingClaudeTool {
                            name: block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_owned(),
                            arguments: String::new(),
                            overflowed: false,
                        },
                    );
                }
                Vec::new()
            }
            Some("content_block_delta") => {
                let Some(delta) = event.get("delta") else {
                    return Vec::new();
                };
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => delta
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(|text| {
                            self.saw_streamed_text = true;
                            self.saw_text = true;
                            vec![AgentEvent::TextDelta(text.to_owned())]
                        })
                        .unwrap_or_default(),
                    Some("input_json_delta") => {
                        if let (Some(index), Some(fragment)) = (
                            event.get("index").and_then(Value::as_u64),
                            delta.get("partial_json").and_then(Value::as_str),
                        ) && let Some(tool) = self.pending_tools.get_mut(&index)
                        {
                            let remaining =
                                Self::ARGUMENT_LIMIT.saturating_sub(tool.arguments.len());
                            if fragment.len() <= remaining {
                                tool.arguments.push_str(fragment);
                            } else {
                                tool.overflowed = true;
                            }
                        }
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            Some("content_block_stop") => {
                let Some(index) = event.get("index").and_then(Value::as_u64) else {
                    return Vec::new();
                };
                let Some(tool) = self.pending_tools.remove(&index) else {
                    return Vec::new();
                };
                let arguments = (!tool.overflowed)
                    .then(|| serde_json::from_str::<Value>(&tool.arguments).ok())
                    .flatten();
                vec![AgentEvent::Tool(claude_tool_summary(
                    &tool.name,
                    arguments.as_ref(),
                ))]
            }
            _ => Vec::new(),
        }
    }

    fn assistant_event(&mut self, root: &Value) -> Vec<AgentEvent> {
        let Some(content) = root
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .map(|text| {
                self.saw_text = true;
                AgentEvent::TextDelta(text.to_owned())
            })
            .collect()
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

pub(crate) fn resolve_codex() -> PathBuf {
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

pub(crate) fn resolve_claude() -> PathBuf {
    if let Some(configured) = env::var_os("XD_CLAUDE_EXECUTABLE").filter(|path| !path.is_empty()) {
        return configured.into();
    }
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        for relative in [
            "claude",
            "claude-bin",
            "libexec/claude",
            "libexec/claude-bin",
        ] {
            let candidate = parent.join(relative);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("claude")
}

fn claude_tool_summary(name: &str, arguments: Option<&Value>) -> String {
    for key in [
        "file_path",
        "path",
        "command",
        "pattern",
        "query",
        "subject",
        "description",
        "prompt",
        "url",
    ] {
        if let Some(detail) = arguments
            .and_then(|arguments| arguments.get(key))
            .and_then(Value::as_str)
        {
            return format!("{name}  {}", compact(detail, 110));
        }
    }
    name.to_owned()
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
                AgentEvent::Session("019f9b16-df5f-7182-bdc6-1cce26148979".into()),
                AgentEvent::Tool(
                    "$ gh run watch 30230367515 --repo RestartFU/xd --exit-status".into()
                ),
                AgentEvent::Text("hello from hy".into()),
                AgentEvent::Usage {
                    input: 16_941,
                    output: 7
                },
                AgentEvent::Completed,
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
        assert!(matches!(events[0], AgentEvent::Error(_)));
        assert_eq!(events[1], AgentEvent::Text("still working".into()));
        assert_eq!(events.last(), Some(&AgentEvent::Completed));
    }

    #[test]
    fn builds_new_and_resumed_noninteractive_commands() {
        let new = AgentCommand {
            backend: "codex",
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

        let resumed = AgentCommand {
            session_id: Some("thread-1"),
            access: "full",
            ..AgentCommand {
                backend: "codex",
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

        let resumed_read_only = AgentCommand {
            access: "read-only",
            ..AgentCommand {
                backend: "codex",
                prompt: "continue",
                workdir: "/workspace",
                model: "gpt-5.6-sol",
                effort: "high",
                access: "read-only",
                session_id: Some("thread-1"),
            }
        }
        .build();
        let args = resumed_read_only
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-c", "sandbox_mode=\"read-only\""])
        );
    }

    #[test]
    fn parses_captured_claude_stream_once_and_builds_resumed_commands() {
        let fixture = include_str!("../../tests/fixtures/claude-stream.jsonl");
        let mut parser = ClaudeParser::default();
        let events = fixture
            .lines()
            .flat_map(|line| parser.feed(line))
            .collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    AgentEvent::TextDelta(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            "hello from hy"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::Completed))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Session(session)
                if session == "653dbf2a-6521-4412-9ac9-81b4d94160e7"
        )));

        let command = AgentCommand {
            backend: "claude",
            prompt: "continue",
            workdir: "/workspace",
            model: "claude-opus-5",
            effort: "xhigh",
            access: "edit",
            session_id: Some("session-1"),
        }
        .build();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--resume", "session-1"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "claude-opus-5"])
        );
        assert!(args.windows(2).any(|pair| pair == ["--effort", "xhigh"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "acceptEdits"])
        );
        assert_eq!(
            command.get_current_dir(),
            Some(std::path::Path::new("/workspace"))
        );
    }
}
