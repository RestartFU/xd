use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{Image, ImageFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::markdown::{self, Document};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub data: String,
    pub preview: Arc<Image>,
}

impl Attachment {
    pub fn from_png(name: impl Into<String>, bytes: Vec<u8>) -> Result<Self, String> {
        if !bytes.starts_with(PNG_SIGNATURE) {
            return Err("Only PNG images can be attached.".into());
        }
        Ok(Self {
            name: name.into(),
            mime: "image/png".into(),
            data: STANDARD.encode(&bytes),
            preview: Arc::new(Image::from_bytes(ImageFormat::Png, bytes)),
        })
    }

    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        let name = value.get("name").and_then(Value::as_str)?;
        let mime = value.get("mime").and_then(Value::as_str)?;
        let data = value.get("data").and_then(Value::as_str)?;
        if mime != "image/png" {
            return None;
        }
        let bytes = STANDARD.decode(data).ok()?;
        let mut attachment = Self::from_png(name, bytes).ok()?;
        attachment.data = data.to_owned();
        Some(attachment)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSummary {
    pub id: String,
    pub folder: String,
    #[serde(default)]
    pub title: Option<String>,
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub working: bool,
    #[serde(default)]
    pub terminal_working: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalActivityStamp {
    pub revision: u64,
    pub working: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub id: Option<i64>,
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(skip)]
    markdown: Option<Arc<Document>>,
    #[serde(skip)]
    image_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl Message {
    pub fn new(
        id: Option<i64>,
        role: impl Into<String>,
        content: impl Into<String>,
        label: Option<String>,
    ) -> Self {
        let content = content.into();
        let role = role.into();
        let (shown, image_paths) = message_content(&role, &content);
        let markdown = Some(Arc::new(if role == "assistant" {
            markdown::parse_assistant(&shown)
        } else {
            markdown::parse(&shown)
        }));
        Self {
            id,
            role,
            content,
            label,
            markdown,
            image_paths,
        }
    }

    pub fn new_plain(
        id: Option<i64>,
        role: impl Into<String>,
        content: impl Into<String>,
        label: Option<String>,
    ) -> Self {
        let content = content.into();
        let role = role.into();
        let (shown, image_paths) = message_content(&role, &content);
        let markdown = Some(Arc::new(markdown::plain_document(&shown)));
        Self {
            id,
            role,
            content,
            label,
            markdown,
            image_paths,
        }
    }

    pub fn markdown(&self) -> Arc<Document> {
        self.markdown
            .clone()
            .unwrap_or_else(|| Arc::new(markdown::parse(&markdown::display_text(&self.content))))
    }

    pub fn image_paths(&self) -> &[String] {
        &self.image_paths
    }

    fn cache_markdown(&mut self) {
        let (shown, image_paths) = message_content(&self.role, &self.content);
        self.markdown = Some(Arc::new(if self.role == "assistant" {
            markdown::parse_assistant(&shown)
        } else {
            markdown::parse(&shown)
        }));
        self.image_paths = image_paths;
    }
}

fn message_content(role: &str, content: &str) -> (String, Vec<String>) {
    if role == "assistant" {
        return (content.to_owned(), Vec::new());
    }
    let mut shown = Vec::new();
    let mut images = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if let Some(path) = line
            .strip_prefix("[image: ")
            .and_then(|line| line.strip_suffix(']'))
            .filter(|path| !path.is_empty())
        {
            images.push(path.to_owned());
        } else {
            shown.push(line);
        }
    }
    if images.is_empty() {
        (content.to_owned(), images)
    } else {
        (shown.join("\n").trim().to_owned(), images)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
    pub detached: bool,
    pub main: bool,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModel {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBackend {
    pub id: String,
    pub name: String,
    pub default_model: String,
    pub models: Vec<AgentModel>,
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppModel {
    pub folders: Vec<Folder>,
    pub chats: Vec<ChatSummary>,
    pub terminal_activity_epoch: Option<String>,
    pub terminal_activity_by_chat: HashMap<String, TerminalActivityStamp>,
    pub selected_chat: Option<String>,
    pub unread_chats: HashSet<String>,
    pub messages: Vec<Message>,
    pub queue: Vec<String>,
    pub working: bool,
    pub working_started_at: Option<Instant>,
    pub new_worktree: bool,
    pub has_messages: bool,
    pub worktrees: Vec<Worktree>,
    pub agent_backends: Vec<AgentBackend>,
    pub backend: String,
    pub model: Option<String>,
    pub effort: String,
    pub access: String,
    pub plan: bool,
    pub fast: bool,
    pub context_used: u64,
    pub context_window: u64,
    pub auth_state: String,
    pub workdir: Option<String>,
    pub commands: Vec<String>,
    pub global_shortcuts: Vec<String>,
    pub workspace_shortcuts: Vec<String>,
    pub shortcuts: Vec<String>,
    pub connected: bool,
    pub connection_error: Option<String>,
    pub draft: String,
    pub draft_attachments: Vec<Attachment>,
    pub draft_revision: i64,
    pub live_text: String,
    pub live_items: Vec<Message>,
    pub todos: Vec<TodoItem>,
}

#[derive(Debug, Deserialize)]
struct TreeSnapshot {
    folders: Vec<Folder>,
    chats: Vec<ChatSummary>,
    #[serde(default)]
    terminal_activity_epoch: Option<String>,
    #[serde(default)]
    terminal_activity_revision: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct MessagesSnapshot {
    #[serde(default)]
    messages: Vec<Message>,
}

pub const MAX_RETAINED_MESSAGES: usize = 480;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePageDirection {
    Tail,
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePageChange {
    pub inserted_at_start: usize,
    pub inserted_at_end: usize,
    pub removed_from_start: usize,
    pub removed_from_end: usize,
    pub has_older: bool,
    pub has_newer: bool,
}

impl AppModel {
    pub fn apply_tree(&mut self, body: &Value) -> Result<(), serde_json::Error> {
        let mut snapshot: TreeSnapshot = serde_json::from_value(body.clone())?;
        let terminal_activity_revision = self.prepare_terminal_activity_version(
            snapshot.terminal_activity_epoch.as_deref(),
            snapshot.terminal_activity_revision,
        );
        if let Some(revision) = terminal_activity_revision {
            for chat in &mut snapshot.chats {
                if let Some(activity) = self
                    .terminal_activity_by_chat
                    .get(chat.id.as_str())
                    .filter(|activity| activity.revision > revision)
                    .copied()
                {
                    chat.terminal_working = activity.working;
                } else {
                    self.terminal_activity_by_chat.insert(
                        chat.id.clone(),
                        TerminalActivityStamp {
                            revision,
                            working: chat.terminal_working,
                        },
                    );
                }
            }
            let snapshot_chats = snapshot
                .chats
                .iter()
                .map(|chat| chat.id.as_str())
                .collect::<HashSet<_>>();
            self.terminal_activity_by_chat.retain(|chat_id, activity| {
                snapshot_chats.contains(chat_id.as_str()) || activity.revision > revision
            });
        }
        self.folders = snapshot.folders;
        self.chats = snapshot.chats;
        self.connected = true;
        self.unread_chats
            .retain(|chat_id| self.chats.iter().any(|chat| &chat.id == chat_id));

        if self
            .selected_chat
            .as_ref()
            .is_some_and(|selected| !self.chats.iter().any(|chat| &chat.id == selected))
        {
            self.selected_chat = None;
            self.messages.clear();
            self.queue.clear();
            self.working = false;
            self.working_started_at = None;
            self.new_worktree = false;
            self.has_messages = false;
            self.worktrees.clear();
            self.backend.clear();
            self.model = None;
            self.effort.clear();
            self.access.clear();
            self.plan = false;
            self.fast = false;
            self.context_used = 0;
            self.context_window = 0;
            self.auth_state.clear();
            self.workdir = None;
            self.commands.clear();
            self.global_shortcuts.clear();
            self.workspace_shortcuts.clear();
            self.shortcuts.clear();
            self.draft.clear();
            self.draft_attachments.clear();
            self.live_text.clear();
            self.live_items.clear();
            self.todos.clear();
        }
        Ok(())
    }

    pub fn select_chat(&mut self, chat_id: impl Into<String>) {
        let chat_id = chat_id.into();
        self.unread_chats.remove(&chat_id);
        self.selected_chat = Some(chat_id);
        self.messages.clear();
        self.queue.clear();
        self.working = false;
        self.working_started_at = None;
        self.new_worktree = false;
        self.has_messages = false;
        self.worktrees.clear();
        self.backend.clear();
        self.model = None;
        self.effort.clear();
        self.access.clear();
        self.plan = false;
        self.fast = false;
        self.context_used = 0;
        self.context_window = 0;
        self.auth_state.clear();
        self.workdir = None;
        self.commands.clear();
        self.global_shortcuts.clear();
        self.workspace_shortcuts.clear();
        self.shortcuts.clear();
        self.draft.clear();
        self.draft_attachments.clear();
        self.draft_revision = -1;
        self.live_text.clear();
        self.live_items.clear();
        self.todos.clear();
    }

    pub fn apply_chat(&mut self, body: &Value) {
        if let Some(backend) = body.get("backend").and_then(Value::as_str) {
            self.backend = backend.to_owned();
            if let Some(selected) = self.selected_chat.as_deref()
                && let Some(summary) = self.chats.iter_mut().find(|chat| chat.id == selected)
            {
                summary.backend = backend.to_owned();
            }
        }
        self.model = body
            .get("model")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.agent_backends
                    .iter()
                    .find(|backend| backend.id == self.backend)
                    .map(|backend| backend.default_model.clone())
            });
        self.effort = body
            .get("effort")
            .and_then(Value::as_str)
            .unwrap_or("high")
            .to_owned();
        self.access = body
            .get("access")
            .and_then(Value::as_str)
            .unwrap_or("read-only")
            .to_owned();
        self.plan = body.get("plan").and_then(Value::as_bool).unwrap_or(false);
        self.fast =
            self.backend == "codex" && body.get("fast").and_then(Value::as_bool).unwrap_or(false);
        self.context_used = body
            .get("context_used")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.context_window = body
            .get("context_window")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.auth_state = body
            .get("auth_state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        self.workdir = body
            .get("workdir")
            .and_then(Value::as_str)
            .filter(|workdir| !workdir.is_empty())
            .map(str::to_owned);
        if let Some(commands) = body.get("commands").and_then(Value::as_array) {
            self.commands = string_array(commands);
        }
        if let Some(shortcuts) = body.get("shortcuts").and_then(Value::as_array) {
            self.shortcuts = string_array(shortcuts);
        }
        self.queue = body
            .get("queue")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect();
        self.working = body
            .get("working")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // A chat opened while its turn is already running receives the text
        // streamed before this client selected it as a snapshot. Keep text
        // delivered by newer live events, but hydrate an otherwise empty live
        // transcript so switching away and back cannot make Claude's reply
        // disappear until the turn finishes.
        if self.working
            && self.live_text.is_empty()
            && self.live_items.is_empty()
            && let Some(segment) = body.get("segment").and_then(Value::as_str)
        {
            self.live_text = segment.to_owned();
        }
        if let Some(selected_chat) = self.selected_chat.as_deref()
            && let Some(summary) = self
                .chats
                .iter_mut()
                .find(|summary| summary.id == selected_chat)
        {
            summary.working = self.working;
        }
        let now = Instant::now();
        self.working_started_at = if self.working {
            body.get("working_for")
                .and_then(Value::as_u64)
                .and_then(|elapsed| now.checked_sub(Duration::from_secs(elapsed)))
                .or(self.working_started_at)
                .or(Some(now))
        } else {
            None
        };
        self.new_worktree = body
            .get("new_worktree")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.has_messages = body
            .get("has_messages")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.worktrees = body
            .get("worktrees")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        self.apply_draft(body);
    }

    pub fn apply_agent_catalog(&mut self, body: &Value) -> Result<(), serde_json::Error> {
        self.agent_backends = serde_json::from_value(
            body.get("backends")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        )?;
        if self.model.is_none() {
            self.model = self
                .agent_backends
                .iter()
                .find(|backend| backend.id == self.backend)
                .map(|backend| backend.default_model.clone());
        }
        Ok(())
    }

    pub fn selected_model_name(&self) -> Option<&str> {
        let model = self.model.as_deref()?;
        self.agent_backends
            .iter()
            .flat_map(|backend| &backend.models)
            .find(|candidate| candidate.id == model)
            .map(|candidate| candidate.name.as_str())
            .or(Some(model))
    }

    pub fn apply_shortcuts(&mut self, body: &Value) {
        if let Some(shortcuts) = body.get("global").and_then(Value::as_array) {
            self.global_shortcuts = string_array(shortcuts);
        }
        if let Some(shortcuts) = body.get("workspace").and_then(Value::as_array) {
            self.workspace_shortcuts = string_array(shortcuts);
        }
        if let Some(shortcuts) = body.get("effective").and_then(Value::as_array) {
            self.shortcuts = string_array(shortcuts);
        }
    }

    pub fn apply_message_page(
        &mut self,
        body: &Value,
        direction: MessagePageDirection,
    ) -> Result<MessagePageChange, serde_json::Error> {
        let mut snapshot: MessagesSnapshot = serde_json::from_value(body.clone())?;
        snapshot
            .messages
            .iter_mut()
            .for_each(Message::cache_markdown);
        if direction != MessagePageDirection::Before
            && let Some(todos) = snapshot
                .messages
                .iter()
                .rev()
                .find_map(|message| todo_snapshot(&message.content))
        {
            self.todos = todos;
        }
        snapshot
            .messages
            .retain(|message| todo_snapshot(&message.content).is_none());
        let has_older = body
            .get("has_older")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_newer = body
            .get("has_newer")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut change = MessagePageChange {
            inserted_at_start: 0,
            inserted_at_end: 0,
            removed_from_start: 0,
            removed_from_end: 0,
            has_older,
            has_newer,
        };
        if direction == MessagePageDirection::Tail {
            self.messages = snapshot.messages;
            if self.messages.len() > MAX_RETAINED_MESSAGES {
                change.removed_from_start = self.messages.len() - MAX_RETAINED_MESSAGES;
                self.messages.drain(0..change.removed_from_start);
            }
            return Ok(change);
        }

        snapshot.messages.retain(|candidate| {
            candidate.id.is_none()
                || !self
                    .messages
                    .iter()
                    .any(|existing| existing.id == candidate.id)
        });
        match direction {
            MessagePageDirection::Before => {
                change.inserted_at_start = snapshot.messages.len();
                self.messages.splice(0..0, snapshot.messages);
                change.removed_from_end = self.messages.len().saturating_sub(MAX_RETAINED_MESSAGES);
                self.messages.truncate(MAX_RETAINED_MESSAGES);
            }
            MessagePageDirection::After => {
                change.inserted_at_end = snapshot.messages.len();
                self.messages.extend(snapshot.messages);
                change.removed_from_start =
                    self.messages.len().saturating_sub(MAX_RETAINED_MESSAGES);
                if change.removed_from_start > 0 {
                    self.messages.drain(0..change.removed_from_start);
                }
            }
            MessagePageDirection::Tail => unreachable!(),
        }
        Ok(change)
    }

    pub fn apply_event(&mut self, name: &str, body: &Value) {
        if name == "terminal-activity"
            && let Some(chat_id) = body.get("chat").and_then(Value::as_str)
            && let Some(working) = body.get("terminal_working").and_then(Value::as_bool)
        {
            let revision = self.prepare_terminal_activity_version(
                body.get("terminal_activity_epoch").and_then(Value::as_str),
                body.get("terminal_activity_revision")
                    .and_then(Value::as_u64),
            );
            let accept = revision.is_none_or(|revision| {
                if self
                    .terminal_activity_by_chat
                    .get(chat_id)
                    .is_some_and(|activity| revision < activity.revision)
                {
                    return false;
                }
                self.terminal_activity_by_chat.insert(
                    chat_id.to_owned(),
                    TerminalActivityStamp { revision, working },
                );
                true
            });
            let completed = accept
                && self
                    .chats
                    .iter_mut()
                    .find(|chat| chat.id == chat_id)
                    .is_some_and(|chat| {
                        let completed = chat.terminal_working && !working;
                        chat.terminal_working = working;
                        completed
                    });
            if completed && self.selected_chat.as_deref() != Some(chat_id) {
                self.unread_chats.insert(chat_id.to_owned());
            }
        }
        let chat_working = match name {
            "turn-started" => Some(true),
            "turn-finished" => Some(false),
            _ => None,
        };
        if let Some(working) = chat_working
            && let Some(chat_id) = body.get("chat").and_then(Value::as_str)
            && let Some(chat) = self.chats.iter_mut().find(|chat| chat.id == chat_id)
        {
            chat.working = working;
        }
        if name == "turn-finished"
            && let Some(chat_id) = body.get("chat").and_then(Value::as_str)
            && self.selected_chat.as_deref() != Some(chat_id)
        {
            self.unread_chats.insert(chat_id.to_owned());
            return;
        }
        match name {
            "queued" if self.event_is_active(body) => {
                if let Some(queue) = body.get("queue").and_then(Value::as_array) {
                    self.queue = queue
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect();
                }
            }
            "draft" if self.event_is_active(body) => self.apply_draft(body),
            "commands" if self.event_is_active(body) => {
                if let Some(commands) = body.get("commands").and_then(Value::as_array) {
                    self.commands = string_array(commands);
                }
            }
            "turn-started" if self.event_is_active(body) => {
                self.start_working();
                self.live_text.clear();
                self.live_items.clear();
            }
            "text" if self.event_is_active(body) => {
                if let Some(text) = body.get("text").and_then(Value::as_str) {
                    self.live_text.push_str(text);
                }
            }
            "tool" if self.event_is_active(body) => {
                if let Some(todos) = body
                    .get("text")
                    .and_then(Value::as_str)
                    .and_then(todo_snapshot)
                {
                    self.todos = todos;
                    return;
                }
                if !self.live_text.is_empty() {
                    let text = std::mem::take(&mut self.live_text);
                    let label = self.selected_summary().map(|chat| chat.backend.clone());
                    self.live_items
                        .push(Message::new(None, "assistant", text, label));
                }
                self.live_items.push(Message::new(
                    None,
                    "tool",
                    body.get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("Used a tool")
                        .to_owned(),
                    None,
                ));
            }
            "turn-finished" if self.event_is_active(body) => self.stop_working(),
            _ => {}
        }
    }

    fn prepare_terminal_activity_version(
        &mut self,
        epoch: Option<&str>,
        revision: Option<u64>,
    ) -> Option<u64> {
        let (Some(epoch), Some(revision)) = (epoch, revision) else {
            if self.terminal_activity_epoch.take().is_some() {
                for chat in &mut self.chats {
                    chat.terminal_working = false;
                }
            }
            self.terminal_activity_by_chat.clear();
            return None;
        };
        if self.terminal_activity_epoch.as_deref() != Some(epoch) {
            self.terminal_activity_epoch = Some(epoch.to_owned());
            self.terminal_activity_by_chat.clear();
            for chat in &mut self.chats {
                chat.terminal_working = false;
            }
        }
        Some(revision)
    }

    pub fn selected_summary(&self) -> Option<&ChatSummary> {
        let selected = self.selected_chat.as_deref()?;
        self.chats.iter().find(|chat| chat.id == selected)
    }

    pub fn start_working(&mut self) {
        self.working = true;
        self.working_started_at.get_or_insert_with(Instant::now);
    }

    pub fn stop_working(&mut self) {
        self.working = false;
        self.working_started_at = None;
    }

    pub fn working_for(&self) -> Option<u64> {
        self.working.then(|| {
            self.working_started_at
                .map_or(0, |started_at| started_at.elapsed().as_secs())
        })
    }

    pub fn display_messages(&self) -> Vec<Message> {
        let mut messages = self.messages.clone();
        messages.extend(self.live_items.iter().cloned());
        if !self.live_text.is_empty() {
            messages.push(Message::new(
                None,
                "assistant",
                self.live_text.clone(),
                self.selected_summary().map(|chat| chat.backend.clone()),
            ));
        }
        messages
    }

    pub fn display_message_count(&self) -> usize {
        self.messages.len() + self.live_items.len() + usize::from(!self.live_text.is_empty())
    }

    pub fn apply_draft_snapshot(&mut self, body: &Value) {
        self.apply_draft(body);
    }

    fn apply_draft(&mut self, body: &Value) {
        let Some(revision) = body.get("draft_revision").and_then(Value::as_i64) else {
            return;
        };
        if revision <= self.draft_revision {
            return;
        }
        let Some(text) = body.get("draft").and_then(Value::as_str) else {
            return;
        };
        self.draft_revision = revision;
        self.draft = text.to_owned();
        if let Some(attachments) = body.get("draft_attachments").and_then(Value::as_array) {
            self.draft_attachments = attachments
                .iter()
                .filter_map(Attachment::from_value)
                .collect();
        }
    }

    fn event_is_active(&self, body: &Value) -> bool {
        body.get("chat").and_then(Value::as_str) == self.selected_chat.as_deref()
    }

    pub fn demo() -> Self {
        Self {
            folders: vec![Folder {
                id: "workspace-xd".into(),
                name: "xd".into(),
                parent: None,
            }],
            chats: vec![
                ChatSummary {
                    id: "chat-gpui".into(),
                    folder: "workspace-xd".into(),
                    title: Some("Rewrite desktop with GPUI".into()),
                    backend: "codex".into(),
                    branch: Some("main".into()),
                    working: true,
                    terminal_working: false,
                },
                ChatSummary {
                    id: "chat-scroll".into(),
                    folder: "workspace-xd".into(),
                    title: Some("Smooth transcript scrolling".into()),
                    backend: "codex".into(),
                    branch: Some("xd/smooth-transcript-scrolling".into()),
                    working: false,
                    terminal_working: false,
                },
            ],
            terminal_activity_epoch: None,
            terminal_activity_by_chat: HashMap::new(),
            selected_chat: Some("chat-gpui".into()),
            unread_chats: HashSet::new(),
            messages: vec![
                Message::new(
                    Some(1),
                    "user",
                    "Rewrite xd's desktop UI using GPUI, but keep the daemon.",
                    None,
                ),
                Message::new(
                    Some(2),
                    "assistant",
                    "The new frontend is connected through the existing JSON Lines protocol. Transcript rows are virtualized from the first milestone.",
                    Some("Codex".into()),
                ),
                Message::new(Some(3), "tool", "Building the Rust desktop shell", None),
            ],
            queue: vec!["Port workspace and chat shortcuts".into()],
            working: true,
            working_started_at: Some(Instant::now()),
            new_worktree: false,
            has_messages: true,
            worktrees: Vec::new(),
            agent_backends: Vec::new(),
            backend: "codex".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: "high".into(),
            access: "edit".into(),
            plan: false,
            fast: false,
            context_used: 16_948,
            context_window: 272_000,
            auth_state: "signed-in".into(),
            workdir: None,
            commands: vec!["review".into(), "compact".into()],
            global_shortcuts: Vec::new(),
            workspace_shortcuts: Vec::new(),
            shortcuts: vec!["Review the current diff".into()],
            connected: true,
            connection_error: None,
            draft: String::new(),
            draft_attachments: Vec::new(),
            draft_revision: 0,
            live_text: String::new(),
            live_items: Vec::new(),
            todos: Vec::new(),
        }
    }
}

fn todo_snapshot(content: &str) -> Option<Vec<TodoItem>> {
    serde_json::from_str(content.strip_prefix("todo_list\n")?).ok()
}

fn string_array(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chat_snapshots_restore_the_live_turn_age() {
        let mut model = AppModel::default();
        model.apply_chat(&json!({"working": true, "working_for": 41}));

        assert!(matches!(model.working_for(), Some(41..=42)));

        model.selected_chat = Some("chat-1".into());
        model.apply_event("turn-finished", &json!({"chat":"chat-1"}));
        assert_eq!(model.working_for(), None);
    }

    #[test]
    fn chat_snapshots_restore_in_progress_text_without_replacing_newer_events() {
        let mut model = AppModel::default();
        model.apply_chat(&json!({
            "working": true,
            "segment": "Claude already streamed this"
        }));
        assert_eq!(model.live_text, "Claude already streamed this");

        model.selected_chat = Some("chat-1".into());
        model.apply_event(
            "text",
            &json!({"chat":"chat-1", "text":" and this is newer"}),
        );
        model.apply_chat(&json!({
            "working": true,
            "segment": "stale snapshot"
        }));
        assert_eq!(
            model.live_text,
            "Claude already streamed this and this is newer"
        );
    }

    #[test]
    fn chat_snapshots_keep_the_selected_sidebar_summary_in_sync() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id": "folder-1", "name": "xd"}],
                "chats": [{
                    "id": "chat-1", "folder": "folder-1", "title": "GPUI",
                    "backend": "codex", "working": true
                }]
            }))
            .unwrap();
        model.select_chat("chat-1");

        model.apply_chat(&json!({"working": false}));

        assert!(!model.working);
        assert!(!model.selected_summary().unwrap().working);
    }

    #[test]
    fn applies_complete_tree_snapshots_and_retires_deleted_selection() {
        let mut model = AppModel {
            selected_chat: Some("deleted".into()),
            messages: vec![Message::new(Some(1), "user", "stale", None)],
            ..Default::default()
        };

        model
            .apply_tree(&json!({
                "folders": [{"id":"folder-1", "name":"xd"}],
                "chats": [{
                    "id":"chat-1", "folder":"folder-1", "title":"GPUI",
                    "backend":"codex", "working":false
                }]
            }))
            .unwrap();

        assert!(model.connected);
        assert_eq!(model.selected_chat, None);
        assert!(model.messages.is_empty());
    }

    #[test]
    fn tree_snapshots_preserve_each_chats_branch() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id":"folder-1", "name":"xd"}],
                "chats": [{
                    "id":"chat-1", "folder":"folder-1", "title":"GPUI",
                    "backend":"codex", "working":false,
                    "branch":"session/scheming-hawk-jhgk"
                }]
            }))
            .unwrap();

        let serialized = serde_json::to_value(&model.chats[0]).unwrap();
        assert_eq!(serialized["branch"], "session/scheming-hawk-jhgk");
    }

    #[test]
    fn queued_events_update_only_the_active_chat_queue() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            queue: vec!["old".into()],
            ..Default::default()
        };

        model.apply_event("queued", &json!({"chat":"chat-2", "queue":["wrong chat"]}));
        assert_eq!(model.queue, ["old"]);

        model.apply_event(
            "queued",
            &json!({"chat":"chat-1", "queue":["first", "second"]}),
        );
        assert_eq!(model.queue, ["first", "second"]);
    }

    #[test]
    fn command_snapshots_and_events_stay_chat_scoped() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_chat(&json!({"commands":["review", "compact"]}));
        assert_eq!(model.commands, ["review", "compact"]);
        model.apply_event("commands", &json!({"chat":"chat-2", "commands":["wrong"]}));
        assert_eq!(model.commands, ["review", "compact"]);
        model.apply_event("commands", &json!({"chat":"chat-1", "commands":["rename"]}));
        assert_eq!(model.commands, ["rename"]);
    }

    #[test]
    fn turn_lifecycle_is_reduced_without_refetching_transcript_state() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_event("turn-started", &json!({"chat":"chat-1"}));
        assert!(model.working);
        model.apply_event("turn-finished", &json!({"chat":"chat-1"}));
        assert!(!model.working);
    }

    #[test]
    fn turn_lifecycle_updates_the_background_chat_summary() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {
                        "id": "selected",
                        "folder": "folder",
                        "title": "Selected",
                        "backend": "codex"
                    },
                    {
                        "id": "background",
                        "folder": "folder",
                        "title": "Background",
                        "backend": "codex"
                    }
                ]
            }))
            .unwrap();
        model.select_chat("selected");

        model.apply_event("turn-started", &json!({"chat": "background"}));

        assert!(model.chats[1].working);
        assert!(!model.working);

        model.apply_event("turn-finished", &json!({"chat": "background"}));

        assert!(!model.chats[1].working);
        assert!(model.unread_chats.contains("background"));
    }

    #[test]
    fn terminal_activity_updates_only_the_matching_chat_summary() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {
                        "id": "selected",
                        "folder": "folder",
                        "title": "Selected",
                        "backend": "codex",
                        "working": true
                    },
                    {
                        "id": "background",
                        "folder": "folder",
                        "title": "Background",
                        "backend": "claude"
                    }
                ]
            }))
            .unwrap();
        model.select_chat("selected");
        model.working = true;

        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "background",
                "working": false,
                "terminal_working": true
            }),
        );

        assert!(model.chats[1].terminal_working);
        assert!(!model.chats[1].working);
        assert!(!model.chats[0].terminal_working);
        assert!(model.chats[0].working);
        assert!(model.working);

        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "background",
                "working": true,
                "terminal_working": false
            }),
        );

        assert!(!model.chats[1].terminal_working);
        assert!(model.working);
    }

    #[test]
    fn background_terminal_completion_stays_unread_until_selected() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {"id": "selected", "folder": "folder", "backend": "codex"},
                    {"id": "background", "folder": "folder", "backend": "claude"}
                ]
            }))
            .unwrap();
        model.select_chat("selected");

        model.apply_event(
            "terminal-activity",
            &json!({"chat": "background", "terminal_working": true}),
        );
        assert!(!model.unread_chats.contains("background"));

        model.apply_event(
            "terminal-activity",
            &json!({"chat": "background", "terminal_working": false}),
        );
        assert!(model.unread_chats.contains("background"));

        model.select_chat("background");
        assert!(!model.unread_chats.contains("background"));
    }

    #[test]
    fn stale_tree_preserves_newer_terminal_activity_in_the_same_epoch() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 1,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": false
                }]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "working": true,
                "terminal_working": true,
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 2
            }),
        );

        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 1,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": false
                }]
            }))
            .unwrap();

        assert!(model.chats[0].terminal_working);

        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 2,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": false
                }]
            }))
            .unwrap();

        assert!(!model.chats[0].terminal_working);
    }

    #[test]
    fn older_terminal_activity_events_are_ignored_in_the_same_epoch() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 0,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{"id": "chat", "folder": "folder", "backend": "codex"}]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": true,
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 2
            }),
        );
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": false,
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 1
            }),
        );

        assert!(model.chats[0].terminal_working);

        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": false,
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 2
            }),
        );
        assert!(!model.chats[0].terminal_working);
    }

    #[test]
    fn terminal_activity_event_revisions_are_scoped_per_chat() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 0,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {"id": "chat-a", "folder": "folder", "backend": "codex"},
                    {"id": "chat-b", "folder": "folder", "backend": "claude"}
                ]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat-a",
                "terminal_working": true,
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 2
            }),
        );
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat-b",
                "terminal_working": true,
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 1
            }),
        );

        assert!(model.chats[0].terminal_working);
        assert!(model.chats[1].terminal_working);
    }

    #[test]
    fn stale_tree_preserves_only_chats_with_newer_activity_events() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 0,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {"id": "chat-a", "folder": "folder", "backend": "codex"},
                    {"id": "chat-b", "folder": "folder", "backend": "claude"}
                ]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat-a",
                "terminal_working": true,
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 2
            }),
        );

        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 1,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {
                        "id": "chat-a",
                        "folder": "folder",
                        "backend": "codex",
                        "terminal_working": false
                    },
                    {
                        "id": "chat-b",
                        "folder": "folder",
                        "backend": "claude",
                        "terminal_working": true
                    }
                ]
            }))
            .unwrap();

        assert!(model.chats[0].terminal_working);
        assert!(model.chats[1].terminal_working);
    }

    #[test]
    fn terminal_activity_received_before_its_chat_row_survives_a_stale_tree() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 0,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": []
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": true,
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 2
            }),
        );

        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 1,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": false
                }]
            }))
            .unwrap();

        assert!(model.chats[0].terminal_working);
    }

    #[test]
    fn a_new_terminal_activity_epoch_accepts_a_lower_revision() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 10,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": true
                }]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": false,
                "terminal_activity_epoch": "daemon-b",
                "terminal_activity_revision": 1
            }),
        );
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": true,
                "terminal_activity_epoch": "daemon-b",
                "terminal_activity_revision": 0
            }),
        );

        assert!(!model.chats[0].terminal_working);
    }

    #[test]
    fn a_new_terminal_activity_epoch_clears_other_chats_before_its_tree() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 10,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {
                        "id": "chat-a",
                        "folder": "folder",
                        "backend": "codex",
                        "terminal_working": false
                    },
                    {
                        "id": "chat-b",
                        "folder": "folder",
                        "backend": "claude",
                        "terminal_working": true
                    }
                ]
            }))
            .unwrap();

        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat-a",
                "terminal_working": true,
                "terminal_activity_epoch": "daemon-b",
                "terminal_activity_revision": 1
            }),
        );

        assert!(model.chats[0].terminal_working);
        assert!(!model.chats[1].terminal_working);
    }

    #[test]
    fn unversioned_terminal_activity_resets_to_legacy_acceptance() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 10,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": true
                }]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({"chat": "chat", "terminal_working": false}),
        );
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": true,
                "terminal_activity_epoch": "daemon-a",
                "terminal_activity_revision": 1
            }),
        );

        assert!(model.chats[0].terminal_working);
    }

    #[test]
    fn background_turns_stay_unread_until_selected() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_event("turn-finished", &json!({"chat":"chat-2"}));
        assert!(model.unread_chats.contains("chat-2"));
        assert_eq!(model.selected_chat.as_deref(), Some("chat-1"));

        model.select_chat("chat-2");
        assert!(!model.unread_chats.contains("chat-2"));
    }

    #[test]
    fn chat_snapshots_control_first_message_worktree_selection() {
        let mut model = AppModel::default();
        model.apply_chat(&json!({
            "queue": [], "working": false, "new_worktree": true, "has_messages": false,
            "worktrees": [{
                "path": "/repo", "branch": "main", "detached": false,
                "main": true, "current": true
            }]
        }));
        assert!(model.new_worktree);
        assert!(!model.has_messages);
        assert_eq!(model.worktrees[0].branch.as_deref(), Some("main"));

        model.apply_chat(&json!({
            "queue": [], "working": false, "new_worktree": false, "has_messages": true
        }));
        assert!(!model.new_worktree);
        assert!(model.has_messages);
    }

    #[test]
    fn catalog_and_chat_snapshots_resolve_assistant_labels() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            chats: vec![ChatSummary {
                id: "chat-1".into(),
                folder: "folder-1".into(),
                title: None,
                backend: "codex".into(),
                branch: None,
                working: false,
                terminal_working: false,
            }],
            ..Default::default()
        };
        model
            .apply_agent_catalog(&json!({
                "backends": [{
                    "id": "claude", "name": "Claude Code",
                    "default_model": "claude-opus-5",
                    "models": [{"id": "claude-opus-5", "name": "Claude Opus 5"}],
                    "efforts": ["low", "high", "ultracode"]
                }]
            }))
            .unwrap();
        model.apply_chat(&json!({
            "backend": "claude", "model": "claude-opus-5", "effort": "high",
            "access": "edit", "plan": true, "fast": true,
            "queue": [], "working": false,
            "context_used": 21_335, "context_window": 1_000_000
        }));

        assert_eq!(model.backend, "claude");
        assert_eq!(model.selected_summary().unwrap().backend, "claude");
        assert_eq!(model.selected_model_name(), Some("Claude Opus 5"));
        assert_eq!(model.effort, "high");
        assert_eq!(model.access, "edit");
        assert_eq!(model.context_used, 21_335);
        assert_eq!(model.context_window, 1_000_000);
        assert!(model.plan);
        assert!(!model.fast);

        model.apply_chat(&json!({
            "backend": "codex", "model": "gpt-5.6-sol", "effort": "max",
            "access": "edit", "plan": false, "fast": true,
            "queue": [], "working": false
        }));
        assert!(model.fast);
        assert_eq!(model.context_used, 0);
        assert_eq!(model.context_window, 0);
    }

    #[test]
    fn shortcut_snapshots_keep_ownership_and_effective_order() {
        let mut model = AppModel::default();
        model.apply_shortcuts(&json!({
            "global": ["Review"],
            "workspace": ["Test"],
            "effective": ["Review", "Test"]
        }));
        assert_eq!(model.global_shortcuts, ["Review"]);
        assert_eq!(model.workspace_shortcuts, ["Test"]);
        assert_eq!(model.shortcuts, ["Review", "Test"]);

        model.apply_chat(&json!({
            "queue": [], "working": true, "shortcuts": ["Review", "Test", "Deploy"]
        }));
        assert_eq!(model.shortcuts, ["Review", "Test", "Deploy"]);
        assert_eq!(model.global_shortcuts, ["Review"]);
    }

    #[test]
    fn streaming_text_and_tools_preserve_wire_order_like_mobile() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_event("turn-started", &json!({"chat":"chat-1"}));
        model.apply_event("text", &json!({"chat":"chat-1", "text":"hello"}));
        model.apply_event("text", &json!({"chat":"chat-1", "text":" world"}));
        model.apply_event("tool", &json!({"chat":"chat-1", "text":"Read file"}));
        model.apply_event("text", &json!({"chat":"chat-1", "text":"done"}));

        let displayed = model.display_messages();
        assert_eq!(displayed.len(), 3);
        assert_eq!(
            displayed
                .iter()
                .map(|message| (message.role.as_str(), message.content.as_str()))
                .collect::<Vec<_>>(),
            [
                ("assistant", "hello world"),
                ("tool", "Read file"),
                ("assistant", "done"),
            ],
        );
    }

    #[test]
    fn draft_replies_do_not_replace_queue_or_turn_state() {
        let mut model = AppModel {
            queue: vec!["next".into()],
            working: true,
            draft_revision: 2,
            ..Default::default()
        };

        model.apply_draft_snapshot(&json!({"draft":"shared", "draft_revision":3}));

        assert_eq!(model.draft, "shared");
        assert_eq!(model.queue, ["next"]);
        assert!(model.working);
    }

    #[test]
    fn synchronized_attachment_previews_replace_only_when_present() {
        let mut model = AppModel::default();
        model.apply_draft_snapshot(&json!({
            "draft": "look",
            "draft_revision": 1,
            "draft_attachments": [{
                "name": "screen.png",
                "mime": "image/png",
                "data": "iVBORw0KGgo="
            }]
        }));
        assert_eq!(model.draft_attachments.len(), 1);
        assert_eq!(model.draft_attachments[0].name, "screen.png");
        assert_eq!(model.draft_attachments[0].preview.bytes, PNG_SIGNATURE);

        model.apply_draft_snapshot(&json!({"draft":"typing", "draft_revision":2}));
        assert_eq!(model.draft_attachments.len(), 1);

        model.apply_draft_snapshot(&json!({
            "draft": "",
            "draft_revision": 3,
            "draft_attachments": []
        }));
        assert!(model.draft_attachments.is_empty());
    }

    #[test]
    fn persisted_image_references_are_hidden_from_user_text() {
        let (shown, paths) = message_content(
            "user",
            "before\n[image: /private/paste-one.png]\nafter\n[image: /private/paste-two.png]",
        );
        assert_eq!(paths, ["/private/paste-one.png", "/private/paste-two.png"]);
        assert_eq!(shown, "before\nafter");

        let (shown, paths) =
            message_content("assistant", "Keep [image: /not/a/reference.png] literal");
        assert!(paths.is_empty());
        assert_eq!(shown, "Keep [image: /not/a/reference.png] literal");
    }

    #[test]
    fn cursor_pages_keep_a_bounded_bidirectional_message_window() {
        let mut model = AppModel {
            messages: (121..=600)
                .map(|id| Message::new(Some(id), "assistant", format!("message {id}"), None))
                .collect(),
            ..Default::default()
        };
        let older = (1..=120)
            .map(|id| json!({"id": id, "role": "assistant", "content": format!("message {id}")}))
            .collect::<Vec<_>>();
        let change = model
            .apply_message_page(
                &json!({"messages": older, "has_older": false, "has_newer": true}),
                MessagePageDirection::Before,
            )
            .unwrap();
        assert_eq!(change.inserted_at_start, 120);
        assert_eq!(change.removed_from_end, 120);
        assert_eq!(model.messages.len(), MAX_RETAINED_MESSAGES);
        assert_eq!(
            model.messages.first().and_then(|message| message.id),
            Some(1)
        );
        assert_eq!(
            model.messages.last().and_then(|message| message.id),
            Some(480)
        );

        let newer = (481..=600)
            .map(|id| json!({"id": id, "role": "assistant", "content": format!("message {id}")}))
            .collect::<Vec<_>>();
        let change = model
            .apply_message_page(
                &json!({"messages": newer, "has_older": true, "has_newer": false}),
                MessagePageDirection::After,
            )
            .unwrap();
        assert_eq!(change.inserted_at_end, 120);
        assert_eq!(change.removed_from_start, 120);
        assert_eq!(
            model.messages.first().and_then(|message| message.id),
            Some(121)
        );
        assert_eq!(
            model.messages.last().and_then(|message| message.id),
            Some(600)
        );
    }

    #[test]
    fn todo_snapshots_feed_the_pane_without_adding_transcript_rows() {
        let marker = "todo_list\n[{\"id\":\"1\",\"text\":\"Build pane\",\"status\":\"in_progress\"},{\"id\":\"2\",\"text\":\"Verify pane\",\"status\":\"pending\"}]";
        let mut model = AppModel::default();
        model.select_chat("chat-1");
        model
            .apply_message_page(
                &json!({"messages": [
                    {"id": 1, "role": "assistant", "content": "Starting"},
                    {"id": 2, "role": "tool", "content": marker}
                ]}),
                MessagePageDirection::Tail,
            )
            .unwrap();

        assert_eq!(model.messages.len(), 1);
        assert_eq!(model.todos.len(), 2);
        assert_eq!(model.todos[0].status, TodoStatus::InProgress);

        model.apply_event(
            "tool",
            &json!({
                "chat": "chat-1",
                "text": "todo_list\n[{\"id\":\"1\",\"text\":\"Build pane\",\"status\":\"completed\"}]"
            }),
        );
        assert!(model.live_items.is_empty());
        assert_eq!(model.todos[0].status, TodoStatus::Completed);
    }
}
