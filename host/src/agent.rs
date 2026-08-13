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
    Todos(TodoUpdate),
    Usage {
        input: u64,
        output: u64,
        window: u64,
    },
    Completed,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub status: TodoStatus,
}

impl TodoItem {
    pub fn new(id: impl Into<String>, text: impl Into<String>, status: TodoStatus) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoUpdate {
    Replace(Vec<TodoItem>),
    Upsert(TodoItem),
    Patch {
        id: String,
        text: Option<String>,
        status: Option<TodoStatus>,
    },
    Remove(String),
}

#[derive(Default)]
pub struct CodexParser {
    started_commands: HashSet<String>,
    started_file_changes: HashMap<String, Vec<tool_diff::CodexSnapshot>>,
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
    Jcode(JcodeParser),
}

impl AgentParser {
    pub fn new(backend: &str) -> Result<Self, String> {
        match backend {
            "codex" => Ok(Self::Codex(CodexParser::default())),
            "claude" => Ok(Self::Claude(ClaudeParser::default())),
            "jcode" => Ok(Self::Jcode(JcodeParser::default())),
            _ => Err(format!("Unknown assistant backend: {backend}")),
        }
    }

    pub fn feed(&mut self, line: &str) -> Vec<AgentEvent> {
        match self {
            Self::Codex(parser) => parser.feed(line),
            Self::Claude(parser) => parser.feed(line),
            Self::Jcode(parser) => parser.feed(line),
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
                match item.get("type").and_then(Value::as_str) {
                    Some("todo_list") => codex_todo_update(item),
                    Some("command_execution") => {
                        if let Some(id) = item.get("id").and_then(Value::as_str) {
                            self.started_commands.insert(id.to_owned());
                        }
                        vec![AgentEvent::Tool(tool_summary(item))]
                    }
                    Some("file_change") => {
                        if let Some(id) = item.get("id").and_then(Value::as_str) {
                            self.started_file_changes
                                .insert(id.to_owned(), tool_diff::capture_codex(item));
                        }
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            Some("item.updated") => root
                .get("item")
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("todo_list"))
                .map(codex_todo_update)
                .unwrap_or_default(),
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
                if item.get("type").and_then(Value::as_str) == Some("file_change")
                    && let Some(snapshots) = item
                        .get("id")
                        .and_then(Value::as_str)
                        .and_then(|id| self.started_file_changes.remove(id))
                    && let Some(diff) = tool_diff::build_captured_codex(snapshots)
                {
                    return vec![AgentEvent::Tool(diff)];
                }
                match item.get("type").and_then(Value::as_str) {
                    Some("todo_list") => codex_todo_update(item),
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
        let mut command = match self.backend {
            "claude" => self.build_claude(),
            "jcode" => self.build_jcode(),
            _ => self.build_codex(),
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

    fn build_jcode(&self) -> Command {
        let mut command = Command::new(resolve_jcode());
        command.args(["--quiet", "--no-update", "-C", self.workdir]);
        if self.model != "default" {
            command.args(["--model", self.model]);
        }
        if let Some(session_id) = self.session_id {
            command.args(["--resume", session_id]);
        }
        // JCode does not currently expose a filesystem sandbox in `run` mode.
        // Keep read and plan chats genuinely non-mutating by exposing only its
        // read-only built-ins. Edit/full chats use the user's normal JCode tool
        // configuration.
        if !matches!(self.access, "edit" | "full") {
            command.args([
                "--tools",
                "read,agentgrep,ls,webfetch,websearch,jcode_docs,todo",
            ]);
        }
        command.args(["run", "--ndjson"]);
        if let Some(system_prompt) = self.system_prompt.filter(|prompt| !prompt.is_empty()) {
            command.arg(format!(
                "<system-instructions>\n{system_prompt}\n</system-instructions>\n\n{}",
                self.prompt
            ));
        } else {
            command.arg(self.prompt);
        }
        command.current_dir(self.workdir);
        command
    }
}

#[derive(Default)]
pub struct JcodeParser {
    streamed_text: String,
    pending_tools: HashMap<String, PendingJcodeTool>,
    active_tool_id: Option<String>,
}

struct PendingJcodeTool {
    name: String,
    input: String,
}

impl JcodeParser {
    pub fn feed(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match root.get("type").and_then(Value::as_str) {
            Some("start") => root
                .get("session_id")
                .and_then(Value::as_str)
                .map(|id| vec![AgentEvent::Session(id.to_owned())])
                .unwrap_or_default(),
            Some("session") => root
                .get("session_id")
                .and_then(Value::as_str)
                .map(|id| vec![AgentEvent::Session(id.to_owned())])
                .unwrap_or_default(),
            Some("text_delta") => root
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(|text| {
                    self.streamed_text.push_str(text);
                    vec![AgentEvent::TextDelta(text.to_owned())]
                })
                .unwrap_or_default(),
            Some("text_replace") => {
                let Some(text) = root.get("text").and_then(Value::as_str) else {
                    return Vec::new();
                };
                // A replacement commonly extends a provider-retried prefix. We
                // can stream that suffix without duplicating text already shown.
                // If it rewrites an earlier prefix, retain the final replacement
                // for the `done` event instead of publishing corrupt deltas.
                if let Some(suffix) = text.strip_prefix(&self.streamed_text) {
                    self.streamed_text = text.to_owned();
                    (!suffix.is_empty())
                        .then(|| AgentEvent::TextDelta(suffix.to_owned()))
                        .into_iter()
                        .collect()
                } else {
                    self.streamed_text = text.to_owned();
                    Vec::new()
                }
            }
            Some("tool_start") => {
                if let (Some(id), Some(name)) = (
                    root.get("id").and_then(Value::as_str),
                    root.get("name").and_then(Value::as_str),
                ) {
                    self.pending_tools.insert(
                        id.to_owned(),
                        PendingJcodeTool {
                            name: name.to_owned(),
                            input: String::new(),
                        },
                    );
                    self.active_tool_id = Some(id.to_owned());
                }
                Vec::new()
            }
            Some("tool_input") => {
                if let Some(delta) = root.get("delta").and_then(Value::as_str)
                    && let Some(tool) = self
                        .active_tool_id
                        .as_ref()
                        .and_then(|id| self.pending_tools.get_mut(id))
                    && tool.input.len() < tool_diff::LIMIT
                {
                    let kept = (tool_diff::LIMIT - tool.input.len()).min(delta.len());
                    let kept = floor_char_boundary(delta, kept);
                    tool.input.push_str(&delta[..kept]);
                }
                Vec::new()
            }
            Some("tool_done") => {
                let id = root.get("id").and_then(Value::as_str).unwrap_or_default();
                if self.active_tool_id.as_deref() == Some(id) {
                    self.active_tool_id = None;
                }
                let tool = self.pending_tools.remove(id).or_else(|| {
                    root.get("name")
                        .and_then(Value::as_str)
                        .map(|name| PendingJcodeTool {
                            name: name.to_owned(),
                            input: String::new(),
                        })
                });
                tool.map(|tool| {
                    let input = serde_json::from_str::<Value>(&tool.input).ok();
                    let summary = tool_diff::build(&tool.name, input.as_ref())
                        .unwrap_or_else(|| tool.name.clone());
                    vec![AgentEvent::Tool(summary)]
                })
                .unwrap_or_default()
            }
            Some("tokens") => vec![AgentEvent::Usage {
                input: root.get("input").and_then(Value::as_u64).unwrap_or(0),
                output: root.get("output").and_then(Value::as_u64).unwrap_or(0),
                window: 0,
            }],
            Some("done") => {
                let mut events = Vec::new();
                if self.streamed_text.is_empty()
                    && let Some(text) = root.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    events.push(AgentEvent::TextDelta(text.to_owned()));
                }
                events.push(AgentEvent::Completed);
                events
            }
            Some("error") => vec![AgentEvent::Error(
                root.get("message")
                    .and_then(Value::as_str)
                    .filter(|message| !message.is_empty())
                    .unwrap_or("JCode turn failed")
                    .to_owned(),
            )],
            _ => Vec::new(),
        }
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
    completed_task_tools: HashMap<String, PendingClaudeTool>,
}

struct PendingClaudeTool {
    id: Option<String>,
    name: String,
    arguments: String,
    overflowed: bool,
}

impl ClaudeParser {
    const MAX_PENDING_TOOLS: usize = 64;
    const ARGUMENT_LIMIT: usize = 2 * 1024 * 1024;
    const MAX_COMPLETED_TASK_TOOLS: usize = 64;

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
            Some("user") => self.user_event(&root),
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
                            id: block.get("id").and_then(Value::as_str).map(str::to_owned),
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
                if let Some(event) = claude_task_update(&tool.name, arguments.as_ref()) {
                    return vec![AgentEvent::Todos(event)];
                }
                if matches!(tool.name.as_str(), "TaskCreate" | "TaskList" | "TaskGet")
                    && let Some(id) = tool.id.clone()
                {
                    if self.completed_task_tools.len() < Self::MAX_COMPLETED_TASK_TOOLS {
                        self.completed_task_tools.insert(id, tool);
                        return Vec::new();
                    }
                }
                vec![AgentEvent::Tool(claude_tool_summary(
                    &tool.name,
                    arguments.as_ref(),
                ))]
            }
            _ => Vec::new(),
        }
    }

    fn user_event(&mut self, root: &Value) -> Vec<AgentEvent> {
        root.get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            .filter_map(|block| {
                let id = block.get("tool_use_id").and_then(Value::as_str)?;
                let tool = self.completed_task_tools.remove(id)?;
                let content = tool_result_text(block.get("content")?)?;
                let arguments = (!tool.overflowed)
                    .then(|| serde_json::from_str::<Value>(&tool.arguments).ok())
                    .flatten();
                claude_task_result(&tool.name, arguments.as_ref(), &content).map(AgentEvent::Todos)
            })
            .collect()
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

fn codex_todo_update(item: &Value) -> Vec<AgentEvent> {
    let todos = item
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, item)| {
            let text = item.get("text").and_then(Value::as_str)?.trim();
            if text.is_empty() {
                return None;
            }
            let status = if item
                .get("completed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                TodoStatus::Completed
            } else {
                TodoStatus::Pending
            };
            Some(TodoItem::new(
                (index + 1).to_string(),
                compact(text, 240),
                status,
            ))
        })
        .take(100)
        .collect();
    vec![AgentEvent::Todos(TodoUpdate::Replace(todos))]
}

fn claude_task_update(name: &str, arguments: Option<&Value>) -> Option<TodoUpdate> {
    if name != "TaskUpdate" {
        return None;
    }
    let arguments = arguments?;
    let id = arguments
        .get("taskId")
        .or_else(|| arguments.get("task_id"))
        .and_then(value_id)?
        .to_owned();
    let status = arguments
        .get("status")
        .and_then(Value::as_str)
        .and_then(todo_status);
    if arguments.get("status").and_then(Value::as_str) == Some("deleted") {
        return Some(TodoUpdate::Remove(id));
    }
    let text = arguments
        .get("subject")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| compact(text, 240));
    Some(TodoUpdate::Patch { id, text, status })
}

fn claude_task_result(name: &str, arguments: Option<&Value>, content: &str) -> Option<TodoUpdate> {
    match name {
        "TaskCreate" => {
            let id = content
                .strip_prefix("Task #")?
                .split_once(' ')?
                .0
                .trim_end_matches(':');
            let text = arguments?.get("subject").and_then(Value::as_str)?.trim();
            (!id.is_empty() && !text.is_empty()).then(|| {
                TodoUpdate::Upsert(TodoItem::new(id, compact(text, 240), TodoStatus::Pending))
            })
        }
        "TaskList" => Some(TodoUpdate::Replace(parse_claude_task_list(content))),
        "TaskGet" => parse_claude_task_get(content).map(TodoUpdate::Upsert),
        _ => None,
    }
}

fn parse_claude_task_list(content: &str) -> Vec<TodoItem> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (id, rest) = line.strip_prefix('#')?.split_once(' ')?;
            let (status, text) = rest.strip_prefix('[')?.split_once("] ")?;
            let text = text.trim();
            (!id.is_empty() && !text.is_empty()).then(|| {
                TodoItem::new(
                    id,
                    compact(text, 240),
                    todo_status(status).unwrap_or(TodoStatus::Pending),
                )
            })
        })
        .take(100)
        .collect()
}

fn parse_claude_task_get(content: &str) -> Option<TodoItem> {
    let mut lines = content.lines();
    let heading = lines.next()?.trim().strip_prefix("Task #")?;
    let (id, text) = heading.split_once(':')?;
    let status = lines.find_map(|line| {
        line.trim()
            .strip_prefix("Status:")
            .map(str::trim)
            .and_then(todo_status)
    });
    let text = text.trim();
    (!id.trim().is_empty() && !text.is_empty()).then(|| {
        TodoItem::new(
            id.trim(),
            compact(text, 240),
            status.unwrap_or(TodoStatus::Pending),
        )
    })
}

fn todo_status(value: &str) -> Option<TodoStatus> {
    match value {
        "pending" | "not_started" | "not-started" => Some(TodoStatus::Pending),
        "in_progress" | "in-progress" => Some(TodoStatus::InProgress),
        "completed" => Some(TodoStatus::Completed),
        _ => None,
    }
}

fn value_id(value: &Value) -> Option<&str> {
    value.as_str()
}

fn tool_result_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    value.as_array().map(|blocks| {
        blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    })
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
    env::var_os("XD_CODEX_EXECUTABLE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("codex"))
}

pub(crate) fn resolve_claude() -> PathBuf {
    env::var_os("XD_CLAUDE_EXECUTABLE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("claude"))
}

pub(crate) fn resolve_jcode() -> PathBuf {
    env::var_os("XD_JCODE_EXECUTABLE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("jcode"))
}

fn floor_char_boundary(value: &str, index: usize) -> usize {
    let mut index = index.min(value.len());
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    index
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
        assert!(!AgentCommand::keeps_its_process("jcode"));
    }

    #[test]
    fn builds_a_resumable_read_only_jcode_run() {
        let command = AgentCommand {
            backend: "jcode",
            prompt: "inspect this",
            system_prompt: Some("Keep the answer concise."),
            workdir: "/tmp/project",
            model: "default",
            effort: "high",
            access: "read",
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
        assert!(args.windows(2).any(|pair| pair == ["run", "--ndjson"]));
        assert!(args.contains(&"--tools".to_owned()));
        assert!(!args.contains(&"--model".to_owned()));
        assert!(args.last().is_some_and(|prompt| {
            prompt.contains("Keep the answer concise.") && prompt.ends_with("inspect this")
        }));
    }

    #[test]
    fn parses_jcode_ndjson_streams_and_tool_diffs() {
        let mut parser = JcodeParser::default();
        let events = [
            r#"{"type":"start","session_id":"session-1","provider":"openai","model":"gpt"}"#,
            r#"{"type":"text_delta","text":"Working"}"#,
            r#"{"type":"tool_start","id":"tool-1","name":"write"}"#,
            r#"{"type":"tool_input","delta":"{\"path\":\"note.txt\",\"content\":\"hello\\n\"}"}"#,
            r#"{"type":"tool_done","id":"tool-1","name":"write","output":"ok","error":null}"#,
            r#"{"type":"tokens","input":12,"output":4,"cache_read_input":0,"cache_creation_input":0}"#,
            r#"{"type":"done","session_id":"session-1","text":"Working"}"#,
        ]
        .into_iter()
        .flat_map(|line| parser.feed(line))
        .collect::<Vec<_>>();

        assert_eq!(events[0], AgentEvent::Session("session-1".into()));
        assert_eq!(events[1], AgentEvent::TextDelta("Working".into()));
        assert!(matches!(&events[2], AgentEvent::Tool(text) if text.starts_with("file_change\n")));
        assert_eq!(
            events[3],
            AgentEvent::Usage {
                input: 12,
                output: 4,
                window: 0,
            }
        );
        assert_eq!(events[4], AgentEvent::Completed);
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
    fn captures_codex_file_changes_when_exec_omits_the_diff() {
        let path =
            std::env::temp_dir().join(format!("xd-codex-file-change-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, "before\nkeep\n").expect("write original fixture");
        let path = path.to_string_lossy().into_owned();
        let started = serde_json::json!({
            "type": "item.started",
            "item": {
                "id": "edit-1",
                "type": "file_change",
                "changes": [{"path": path, "kind": "update"}],
                "status": "in_progress"
            }
        });
        let completed = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "edit-1",
                "type": "file_change",
                "changes": [{"path": path, "kind": "update"}],
                "status": "completed"
            }
        });

        let mut parser = CodexParser::default();
        assert!(parser.feed(&started.to_string()).is_empty());
        std::fs::write(&path, "after\nkeep\n").expect("write changed fixture");
        let events = parser.feed(&completed.to_string());
        std::fs::remove_file(&path).expect("remove fixture");

        let AgentEvent::Tool(diff) = &events[0] else {
            panic!("expected a file-change tool event, got {events:?}");
        };
        assert!(diff.starts_with("file_change\ndiff --git "));
        assert!(diff.contains("-before"));
        assert!(diff.contains("+after"));
        assert!(!diff.contains("Diff content wasn’t captured"));

        let started = serde_json::json!({
            "type": "item.started",
            "item": {
                "id": "add-1",
                "type": "file_change",
                "changes": [{"path": path, "kind": "add"}],
                "status": "in_progress"
            }
        });
        let completed = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "add-1",
                "type": "file_change",
                "changes": [{"path": path, "kind": "add"}],
                "status": "completed"
            }
        });
        assert!(parser.feed(&started.to_string()).is_empty());
        std::fs::write(&path, "created\n").expect("write added fixture");
        let events = parser.feed(&completed.to_string());
        std::fs::remove_file(&path).expect("remove added fixture");
        let AgentEvent::Tool(diff) = &events[0] else {
            panic!("expected an added-file tool event, got {events:?}");
        };
        assert!(diff.contains("new file mode 100644"));
        assert!(diff.contains("+created"));
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

    #[test]
    fn parses_codex_todo_snapshots() {
        let mut parser = CodexParser::default();
        let events = parser.feed(
            &serde_json::json!({
                "type": "item.updated",
                "item": {
                    "id": "item_1",
                    "type": "todo_list",
                    "items": [
                        {"text": "Trace the payload", "completed": true},
                        {"text": "Render the pane", "completed": false}
                    ]
                }
            })
            .to_string(),
        );

        assert_eq!(
            events,
            vec![AgentEvent::Todos(TodoUpdate::Replace(vec![
                TodoItem::new("1", "Trace the payload", TodoStatus::Completed),
                TodoItem::new("2", "Render the pane", TodoStatus::Pending),
            ]))]
        );
    }

    #[test]
    fn parses_claude_task_create_update_and_list_results() {
        let mut parser = ClaudeParser::default();
        let mut events = Vec::new();
        for line in [
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_create","name":"TaskCreate"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"subject\":\"Build the pane\",\"description\":\"Show tasks\",\"activeForm\":\"Building the pane\"}"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call_create","content":"Task #7 created successfully: Build the pane"}]}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_update","name":"TaskUpdate"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"taskId\":\"7\",\"status\":\"in_progress\"}"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":1}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call_list","name":"TaskList"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":2}}"#,
            r##"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call_list","content":"#7 [in_progress] Build the pane\n#8 [pending] Verify it"}]}}"##,
        ] {
            events.extend(parser.feed(line));
        }

        assert_eq!(
            events,
            vec![
                AgentEvent::Todos(TodoUpdate::Upsert(TodoItem::new(
                    "7",
                    "Build the pane",
                    TodoStatus::Pending,
                ))),
                AgentEvent::Todos(TodoUpdate::Patch {
                    id: "7".into(),
                    text: None,
                    status: Some(TodoStatus::InProgress),
                }),
                AgentEvent::Todos(TodoUpdate::Replace(vec![
                    TodoItem::new("7", "Build the pane", TodoStatus::InProgress),
                    TodoItem::new("8", "Verify it", TodoStatus::Pending),
                ])),
            ]
        );
    }
}
