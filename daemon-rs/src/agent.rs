use std::{
    collections::{HashMap, HashSet},
    env,
    path::PathBuf,
    process::Command,
};

use serde_json::{Value, json};

use crate::tool_diff;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    Session(String),
    Commands(Vec<String>),
    Text(String),
    TextDelta(String),
    Tool(String),
    Usage {
        input: u64,
        output: u64,
        window: u64,
    },
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
    pub system_prompt: Option<&'a str>,
    pub workdir: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
    pub access: &'a str,
    pub fast: bool,
    pub session_id: Option<&'a str>,
    pub environment: &'a [(String, String)],
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
                vec![
                    AgentEvent::Usage {
                        input,
                        output,
                        window: 0,
                    },
                    AgentEvent::Completed,
                ]
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
        let mut command = if self.backend == "claude" {
            self.build_claude()
        } else {
            self.build_codex()
        };
        command.envs(self.environment.iter().map(|(name, value)| (name, value)));
        command
    }

    fn build_codex(&self) -> Command {
        let mut command = Command::new(resolve_codex());
        command.arg("exec");
        append_system_prompt(&mut command, self.system_prompt);
        if self.fast {
            command.args(["-c", "service_tier=\"priority\""]);
        }
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

    /// Whether this backend takes its turns on stdin and stays up between them.
    ///
    /// Only claude. `codex exec` reads stdin as the initial prompt only, so
    /// there is no equivalent short of its experimental app-server.
    pub fn keeps_its_process(backend: &str) -> bool {
        backend == "claude"
    }

    /// One turn, as the line `--input-format stream-json` expects on stdin.
    ///
    /// The prompt is not in argv for a kept process: argv was fixed when the
    /// process started, and every turn after the first arrives this way.
    pub fn encode_turn(prompt: &str) -> String {
        json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": prompt}],
            },
        })
        .to_string()
    }

    fn build_claude(&self) -> Command {
        let mut command = Command::new(resolve_claude());
        if let Some(session_id) = self.session_id {
            command.args(["--resume", session_id]);
        }
        // The first turn goes in on stdin like every one after it, so the
        // process is identical whether it is new or resumed.
        command.args([
            "-p",
            "--input-format",
            "stream-json",
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
        if let Some(system_prompt) = self.system_prompt.filter(|prompt| !prompt.is_empty()) {
            command.args(["--append-system-prompt", system_prompt]);
        }
        command.current_dir(self.workdir);
        command
    }
}

fn append_system_prompt(command: &mut Command, prompt: Option<&str>) {
    if let Some(prompt) = prompt.filter(|prompt| !prompt.is_empty()) {
        let encoded = serde_json::to_string(prompt).expect("serialize developer instructions");
        command.args(["-c", &format!("developer_instructions={encoded}")]);
    }
}

#[derive(Default)]
pub struct ClaudeParser {
    saw_streamed_text: bool,
    saw_text: bool,
    /// Newlines the text so far already ends with, so a new block is separated
    /// by a blank line rather than running into the previous one.
    trailing_newlines: usize,
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

    fn record_text(&mut self, text: &str) {
        self.saw_text = true;
        self.trailing_newlines = if text.chars().all(|character| character == '\n') {
            self.trailing_newlines + text.len()
        } else {
            text.chars().rev().take_while(|c| *c == '\n').count()
        };
    }

    /// The blank line that keeps a new text block off the end of the last one.
    fn block_break(&self) -> Option<String> {
        if !self.saw_text || self.trailing_newlines >= 2 {
            return None;
        }
        Some("\n".repeat(2 - self.trailing_newlines))
    }

    pub fn feed(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match root.get("type").and_then(Value::as_str) {
            Some("system") if root.get("subtype").and_then(Value::as_str) == Some("init") => {
                let mut events = Vec::new();
                if let Some(session) = root.get("session_id").and_then(Value::as_str) {
                    events.push(AgentEvent::Session(session.to_owned()));
                }
                let commands = root
                    .get("slash_commands")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter_map(normalize_command)
                    .take(200)
                    .collect::<Vec<_>>();
                if !commands.is_empty() {
                    events.push(AgentEvent::Commands(commands));
                }
                events
            }
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
                    self.record_text(text);
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
                    if let Some(usage) = claude_usage(&root) {
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
                        events.push(AgentEvent::Usage {
                            input,
                            output,
                            window: claude_context_window(&root),
                        });
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
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    // Claude opens a fresh text block after each tool call. Left
                    // alone its first delta lands against the previous block's
                    // last word.
                    return self
                        .block_break()
                        .map(|separator| {
                            self.record_text(&separator);
                            vec![AgentEvent::TextDelta(separator)]
                        })
                        .unwrap_or_default();
                }
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
                            self.record_text(text);
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
        let blocks = content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut events = Vec::new();
        for text in blocks {
            // Without streaming, a whole assistant message arrives at once, so
            // each one is its own paragraph.
            if let Some(separator) = self.block_break() {
                self.record_text(&separator);
                events.push(AgentEvent::TextDelta(separator));
            }
            self.record_text(&text);
            events.push(AgentEvent::TextDelta(text));
        }
        events
    }
}

fn claude_usage(root: &Value) -> Option<&Value> {
    let usage = root.get("usage")?;
    usage
        .get("iterations")
        .and_then(Value::as_array)
        .and_then(|iterations| iterations.last())
        .or(Some(usage))
}

fn claude_context_window(root: &Value) -> u64 {
    root.get("modelUsage")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|models| models.values())
        .filter_map(|model| model.get("contextWindow").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
}

fn normalize_command(value: &str) -> Option<String> {
    let value = value.strip_prefix('/').unwrap_or(value).trim();
    if value.is_empty() || value.chars().count() > 256 || value.chars().any(char::is_whitespace) {
        return None;
    }
    Some(value.to_owned())
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
        for relative in [
            "codex-package/bin/codex.exe",
            "codex-package/bin/codex",
            "libexec/codex-package/bin/codex",
        ] {
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
            "claude.exe",
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
    // A file-editing call carries its own diff, so the row shows the change
    // rather than the path alone.
    if let Some(patch) = tool_diff::build(name, arguments) {
        return patch;
    }
    if let Some(workflow) = workflow_marker(arguments) {
        return workflow;
    }
    if matches!(name, "Task" | "Agent") {
        let mut identity = vec!["Claude".to_owned()];
        for key in ["subagent_type", "model"] {
            append_distinct(
                &mut identity,
                arguments
                    .and_then(|arguments| arguments.get(key))
                    .and_then(Value::as_str),
            );
        }
        let mut task = Vec::new();
        for key in ["description", "prompt"] {
            append_distinct(
                &mut task,
                arguments
                    .and_then(|arguments| arguments.get(key))
                    .and_then(Value::as_str),
            );
        }
        return subagent_marker(None, &identity.join(" · "), &task.join(" · "));
    }
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
    if let Some(workflow) = workflow_marker(Some(item)) {
        return workflow;
    }
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("tool");
    if let Some(patch) = tool_diff::build(kind, Some(item)) {
        return patch;
    }
    if matches!(kind, "collab_tool_call" | "collab_agent_tool_call")
        && matches!(
            item.get("tool").and_then(Value::as_str),
            Some("spawn_agent" | "spawnAgent")
        )
    {
        let receivers = item
            .get("receiverThreadIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let key = receivers.first().copied();
        let mut identity = vec!["Codex".to_owned()];
        for field in ["model", "reasoningEffort"] {
            append_distinct(&mut identity, item.get(field).and_then(Value::as_str));
        }
        let mut detail = vec![codex_agent_status(item, &receivers).to_owned()];
        append_distinct(&mut detail, item.get("prompt").and_then(Value::as_str));
        if let Some(receiver) = key {
            detail.push(format!("Agent {}", compact(receiver, 16)));
        }
        return subagent_marker(key, &identity.join(" · "), &detail.join(" · "));
    }
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

fn workflow_marker(arguments: Option<&Value>) -> Option<String> {
    let url = arguments?
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| url.starts_with("https://github.com/") && url.contains("/actions/runs/"))?;
    let id = url
        .split("/actions/runs/")
        .nth(1)?
        .split(['/', '?', '#'])
        .next()?;
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(format!("workflow_run\n{id}\n{}", compact(url, 500)))
}

fn subagent_marker(key: Option<&str>, identity: &str, task: &str) -> String {
    let key = key
        .map(|key| format!("{}\n", compact(key, 120)))
        .unwrap_or_default();
    let identity = (!identity.trim().is_empty())
        .then_some(identity)
        .unwrap_or("Agent");
    let task = (!task.trim().is_empty())
        .then_some(task)
        .unwrap_or("Delegated task");
    format!(
        "subagent\n{key}{}\n{}",
        compact(identity, 80),
        compact(task, 320)
    )
}

fn append_distinct(parts: &mut Vec<String>, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() || parts.iter().any(|part| part.eq_ignore_ascii_case(&value)) {
        return;
    }
    parts.push(value);
}

fn codex_agent_status<'a>(item: &'a Value, receivers: &[&str]) -> &'a str {
    let states = item.get("agentsStates").and_then(Value::as_object);
    let agent = states.and_then(|states| {
        receivers
            .iter()
            .find_map(|receiver| states.get(*receiver).and_then(Value::as_object))
            .or_else(|| states.values().find_map(Value::as_object))
    });
    let status = agent
        .and_then(|agent| agent.get("status"))
        .and_then(Value::as_str)
        .or_else(|| item.get("status").and_then(Value::as_str));
    match status {
        Some("pendingInit" | "inProgress") => "Starting",
        Some("running") => "Running",
        Some("interrupted") => "Interrupted",
        Some("completed") => "Completed",
        Some("errored" | "failed") => "Failed",
        Some("shutdown") => "Stopped",
        Some("notFound") => "Not found",
        _ => "Delegated",
    }
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
    fn only_claude_keeps_its_process_between_turns() {
        assert!(AgentCommand::keeps_its_process("claude"));
        // `codex exec` reads stdin as the initial prompt only, so there is
        // nothing to keep and nothing to send down it a second time.
        assert!(!AgentCommand::keeps_its_process("codex"));
    }

    #[test]
    fn a_turn_is_encoded_as_one_line_the_cli_accepts() {
        let line = AgentCommand::encode_turn("make it\nfaster");
        // One line, whatever the prompt contains: the stream is newline
        // delimited and a raw newline in the middle would end the turn early.
        assert_eq!(line.lines().count(), 1);
        let parsed: Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["message"]["role"], "user");
        assert_eq!(parsed["message"]["content"][0]["type"], "text");
        assert_eq!(parsed["message"]["content"][0]["text"], "make it\nfaster");
    }

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
                    output: 7,
                    window: 0,
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
    fn separates_claude_text_blocks_that_a_tool_call_interrupts() {
        let mut parser = ClaudeParser::default();
        let mut events = Vec::new();
        for line in [
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Now the renderer."}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","name":"Read"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":1}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"text"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Now the card."}}}"#,
        ] {
            events.extend(parser.feed(line));
        }

        let text = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, "Now the renderer.\n\nNow the card.");
    }

    #[test]
    fn separates_whole_assistant_messages_without_doubling_existing_breaks() {
        let mut parser = ClaudeParser::default();
        let text = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"First."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Second.\n\n"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Third."}]}}"#,
        ]
        .into_iter()
        .flat_map(|line| parser.feed(line))
        .filter_map(|event| match event {
            AgentEvent::TextDelta(text) => Some(text),
            _ => None,
        })
        .collect::<String>();

        assert_eq!(text, "First.\n\nSecond.\n\nThird.");
    }

    #[test]
    fn builds_new_and_resumed_noninteractive_commands() {
        let new = AgentCommand {
            backend: "codex",
            prompt: "hello",
            system_prompt: Some("Always test."),
            workdir: "/workspace",
            model: "gpt-5.6-sol",
            effort: "high",
            access: "edit",
            fast: true,
            session_id: None,
            environment: &[("API_TOKEN".into(), "private".into())],
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
        assert!(args.windows(2).any(|pair| {
            pair[0] == "-c" && pair[1] == "developer_instructions=\"Always test.\""
        }));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-c", "service_tier=\"priority\""])
        );
        assert_eq!(args.last().map(String::as_str), Some("hello"));
        assert!(new.get_envs().any(|(name, value)| {
            name == "API_TOKEN" && value.is_some_and(|value| value == "private")
        }));

        let resumed = AgentCommand {
            session_id: Some("thread-1"),
            access: "full",
            ..AgentCommand {
                backend: "codex",
                prompt: "continue",
                system_prompt: None,
                workdir: "/workspace",
                model: "gpt-5.6-sol",
                effort: "max",
                access: "full",
                fast: false,
                session_id: None,
                environment: &[],
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
                system_prompt: None,
                workdir: "/workspace",
                model: "gpt-5.6-sol",
                effort: "high",
                access: "read-only",
                fast: false,
                session_id: Some("thread-1"),
                environment: &[],
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
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Commands(commands)
                if commands.first().map(String::as_str) == Some("git-commit")
                    && commands.iter().any(|command| command == "code-review")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Usage {
                input: 21_328,
                output: 7,
                window: 1_000_000,
            }
        )));
        assert_eq!(normalize_command("/review"), Some("review".into()));
        assert_eq!(normalize_command("bad command"), None);

        let command = AgentCommand {
            backend: "claude",
            prompt: "continue",
            system_prompt: Some("Stay concise."),
            workdir: "/workspace",
            model: "claude-opus-5",
            effort: "xhigh",
            access: "edit",
            fast: false,
            session_id: Some("session-1"),
            environment: &[],
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
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--append-system-prompt", "Stay concise."])
        );
        assert_eq!(
            command.get_current_dir(),
            Some(std::path::Path::new("/workspace"))
        );
    }

    #[test]
    fn emits_stable_shared_card_records_for_subagents_and_workflows() {
        let claude = claude_tool_summary(
            "Task",
            Some(&serde_json::json!({
                "subagent_type": "Explore agent",
                "model": "haiku",
                "description": "Inspect the parser",
                "prompt": "Trace every path"
            })),
        );
        assert_eq!(
            claude,
            "subagent\nClaude · Explore agent · haiku\nInspect the parser · Trace every path"
        );

        let codex = tool_summary(&serde_json::json!({
            "type": "collab_agent_tool_call",
            "tool": "spawn_agent",
            "receiverThreadIds": ["thread-agent-123456789"],
            "model": "gpt-5.6-sol",
            "reasoningEffort": "high",
            "prompt": "Review the diff",
            "agentsStates": {
                "thread-agent-123456789": {"status": "running"}
            }
        }));
        assert_eq!(
            codex,
            "subagent\nthread-agent-123456789\nCodex · gpt-5.6-sol · high\nRunning · Review the diff · Agent thread-agent-12…"
        );

        let workflow = claude_tool_summary(
            "Workflow",
            Some(&serde_json::json!({
                "url": "https://github.com/RestartFU/xd/actions/runs/31028502744"
            })),
        );
        assert_eq!(
            workflow,
            "workflow_run\n31028502744\nhttps://github.com/RestartFU/xd/actions/runs/31028502744"
        );
    }
}
