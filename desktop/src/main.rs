#![deny(dead_code, unused_imports)]

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque, hash_map::DefaultHasher},
    env,
    hash::{Hash, Hasher},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::{
    process::{Command, Stdio},
    thread,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{
    Animation, AnimationExt, App, Application, AssetSource, Bounds, ClipboardItem, Context,
    CursorStyle, Decorations, Entity, FocusHandle, Focusable, FontWeight, HighlightStyle,
    KeyBinding, ListAlignment, ListState, MouseButton, MouseDownEvent, Pixels, Point, Render,
    ResizeEdge, ScrollHandle, ScrollWheelEvent, SharedString, StyledText, TextRun, Timer,
    TitlebarOptions, WeakFocusHandle, Window, WindowBackgroundAppearance, WindowBounds,
    WindowControlArea, WindowDecorations, WindowOptions, canvas, div, point, prelude::*, px, rgb,
    rgba, size, svg,
};
use serde::Deserialize;
use serde_json::Value;
use xd_desktop::{
    activity,
    host::{HostHandle, HostUpdate, MessageCursor, NewSessionWorktree, RequestKind, StartedHost},
    markdown,
    model::{AppModel, Attachment, Message, MessagePageDirection, Worktree},
    remote::{self, RemoteError, SshRemoteBridge, SshRemoteSession},
    session_host::{AgentCommand, SessionHost, SshCommand, TMUX_CONFIGURATION},
    session_runtime::{SessionEvent, SessionRuntime},
    theme::ThemeColors,
};

mod editor;
mod files;
mod input;
mod minimal;
mod selection;
mod settings;
mod source_build;
mod speech;
mod terminal;

use editor::{
    Backspace as EditorBackspace, Copy as EditorCopy, Cut as EditorCut, Delete as EditorDelete,
    DeleteWord as EditorDeleteWord, DeleteWordForward as EditorDeleteWordForward,
    Down as EditorDown, EditorEvent, End as EditorEnd, FileEditor, Home as EditorHome,
    Left as EditorLeft, Newline as EditorNewline, Paste as EditorPaste, Right as EditorRight,
    Save as EditorSave, SelectAll as EditorSelectAll, SelectLeft as EditorSelectLeft,
    SelectRight as EditorSelectRight, SelectWordLeft as EditorSelectWordLeft,
    SelectWordRight as EditorSelectWordRight, Submit as EditorSubmit, Tab as EditorTab,
    Up as EditorUp, WordLeft as EditorWordLeft, WordRight as EditorWordRight,
};
use input::{
    Backspace, ClearScreen as TerminalClearScreen, ComposerEvent, ComposerInput,
    ControlA as TerminalControlA, ControlB as TerminalControlB, ControlE as TerminalControlE,
    ControlF as TerminalControlF, ControlG as TerminalControlG, ControlH as TerminalControlH,
    ControlI as TerminalControlI, ControlJ as TerminalControlJ, ControlK as TerminalControlK,
    ControlM as TerminalControlM, ControlN as TerminalControlN, ControlO as TerminalControlO,
    ControlP as TerminalControlP, ControlQ as TerminalControlQ, ControlS as TerminalControlS,
    ControlT as TerminalControlT, ControlU as TerminalControlU, ControlW as TerminalControlW,
    ControlX as TerminalControlX, ControlY as TerminalControlY, Copy, Cut, Delete, DeleteWord,
    DeleteWordForward, Down, End, EndOfFile as TerminalEndOfFile, Escape, Home, Interrupt, Left,
    PageDown as TerminalPageDown, PageUp as TerminalPageUp, Paste,
    ReverseSearch as TerminalReverseSearch, Right, SelectAll, SelectLeft, SelectRight,
    SelectWordLeft, SelectWordRight, ShiftTab as TerminalShiftTab, ShowCharacterPalette, Submit,
    Suspend as TerminalSuspend, Tab, Up, WordLeft, WordRight, terminal_paste_bytes,
};
use minimal::{
    AgentCli, MinimalRoute, project_cards, project_sessions, reconcile_route, resumable_session,
};
use selection::{TextSelection, selectable_in_document, selectable_links_in_document};
use settings::{AppSettings, ThemePreset};
use source_build::{SourceBuildEvent, SourceBuildRun, SourceTarget};
use speech::SpeechOutput;
use terminal::TerminalScreen;

const UI_FONT: &str = "DM Sans";
const EMBEDDED_UI_FONT: &[u8] = include_bytes!("../../data/fonts/DMSans-Variable.ttf");
// The bundle ships this face and points fontconfig at itself. The generic
// `monospace` alias resolves through the host's fontconfig instead, which lands
// on DejaVu Sans Mono and reads nothing like the rest of the shell.
pub(crate) const MONO: &str = "JetBrains Mono";
const CLAUDE_ICON: &str = "icons/claude.svg";
const CODEX_ICON: &str = "icons/codex.svg";
const JCODE_ICON: &str = "icons/jcode.svg";
const COPILOT_ICON: &str = "icons/copilot.svg";
const XD_MARK_ICON: &str = "icons/xd-mark.svg";
const SEND_ICON: &str = "icons/send.svg";
const STOP_ICON: &str = "icons/stop.svg";
const FOLDER_ICON: &str = "icons/folder.svg";
const FILE_ICON: &str = "icons/file.svg";
const GIT_BRANCH_ICON: &str = "icons/git-branch.svg";
const TRASH_ICON: &str = "icons/trash.svg";
const DONE_STATUS_COLOR: u32 = 0x43a047;
/// What a chat is called when nobody chose a name for it.
const DEFAULT_CHAT_TITLE: &str = "New Chat";
const MAX_ATTACHMENTS: usize = 4;
const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
const MAX_SOURCE_BUILD_OUTPUT_BYTES: usize = 8 * 1024;
const ACTION_ERROR_LIFETIME: Duration = Duration::from_secs(8);
const WORKING_DOT_CYCLE: Duration = Duration::from_millis(1_600);

fn working_dot_alphas(frame: usize) -> [u8; 3] {
    let lit = frame % 4;
    std::array::from_fn(|index| if index < lit { 0xff } else { 0x4d })
}

fn working_dots(animation_instance: usize, color: u32) -> gpui::AnyElement {
    div()
        .w(px(13.0))
        .flex_none()
        .text_sm()
        .with_animation(
            ("working-dots", animation_instance),
            Animation::new(WORKING_DOT_CYCLE).repeat(),
            move |dots, progress| {
                let frame = ((progress * 4.0).floor() as usize).min(3);
                let highlights =
                    working_dot_alphas(frame)
                        .into_iter()
                        .enumerate()
                        .map(|(index, alpha)| {
                            (
                                index..index + 1,
                                HighlightStyle {
                                    color: Some(rgba((color << 8) | u32::from(alpha)).into()),
                                    ..Default::default()
                                },
                            )
                        });
                dots.child(StyledText::new("...").with_highlights(highlights))
            },
        )
        .into_any_element()
}

fn plus_icon(color: u32) -> gpui::AnyElement {
    div()
        .relative()
        .size(px(16.0))
        .child(
            div()
                .absolute()
                .left(px(3.0))
                .top(px(7.0))
                .w(px(10.0))
                .h(px(2.0))
                .rounded_full()
                .bg(rgb(color)),
        )
        .child(
            div()
                .absolute()
                .left(px(7.0))
                .top(px(3.0))
                .w(px(2.0))
                .h(px(10.0))
                .rounded_full()
                .bg(rgb(color)),
        )
        .into_any_element()
}

fn xd_mark(color: u32) -> gpui::AnyElement {
    svg()
        .path(XD_MARK_ICON)
        .size(px(20.0))
        .flex_none()
        .text_color(rgb(color))
        .into_any_element()
}

fn session_status_icon(color: u32) -> gpui::AnyElement {
    div()
        .relative()
        .size(px(14.0))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(rgb(color))
        .child(
            div()
                .absolute()
                .left(px(3.0))
                .top(px(3.0))
                .size(px(6.0))
                .rounded_full()
                .bg(rgb(color)),
        )
        .into_any_element()
}

gpui::actions!(xd, [CloseSearch, CopyRenderedSelection]);

#[derive(Clone, Debug, Deserialize)]
struct AuthProvider {
    provider: String,
    state: String,
    #[serde(default)]
    needs_input: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct CliVersion {
    provider: String,
    state: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SelfUpdateStatus {
    #[serde(default)]
    state: String,
}

#[derive(Clone, Default)]
struct SelfUpdatePanel {
    status: Option<SelfUpdateStatus>,
    busy: bool,
    error: Option<String>,
}

#[derive(Clone, Default)]
struct SourceBuildPanel {
    text: String,
    target: Option<SourceTarget>,
    running: bool,
    stopping: bool,
    installed: bool,
    message: Option<String>,
    output: VecDeque<String>,
    output_bytes: usize,
}

impl SourceBuildPanel {
    fn set_text(&mut self, text: String) {
        self.target = source_build::parse_target(&text);
        self.text = text;
        if !self.running {
            self.message = None;
            self.installed = false;
        }
    }

    fn append_output(&mut self, chunk: String) {
        let chunk = strip_source_build_controls(&chunk);
        if chunk.is_empty() {
            return;
        }
        self.output_bytes = self.output_bytes.saturating_add(chunk.len());
        self.output.push_back(chunk);
        while self.output_bytes > MAX_SOURCE_BUILD_OUTPUT_BYTES && self.output.len() > 1 {
            if let Some(removed) = self.output.pop_front() {
                self.output_bytes = self.output_bytes.saturating_sub(removed.len());
            }
        }
    }
}

#[derive(Clone, Debug)]
struct DiffLine;

#[derive(Clone, Debug)]
struct DiffFile {
    path: String,
    additions: usize,
    deletions: usize,
    lines: Vec<DiffLine>,
    lazy_read: Option<String>,
    loaded: bool,
    loading: bool,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct FilePreview {
    path: String,
    content: String,
    original: String,
    truncated: bool,
    saving: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct BrowseEntry {
    name: String,
    directory: bool,
}

#[derive(Clone, Default)]
struct DiffPanel {
    branch: bool,
    base: Option<String>,
    files_mode: bool,
    loading: bool,
    files: Vec<DiffFile>,
    error: Option<String>,
    truncated: bool,
    status: Option<GitStatus>,
    status_loading: bool,
    action: Option<String>,
    action_error: Option<String>,
    browse_path: String,
    browse_entries: Vec<BrowseEntry>,
    file_preview: Option<FilePreview>,
    file_loading: bool,
    pr_url: Option<String>,
    pr_loading: bool,
    pr_title: Option<String>,
    pr_body: String,
}

struct TerminalTab {
    id: String,
    title: String,
    agent: Option<AgentCli>,
    sequence: Option<u64>,
    screen: TerminalScreen,
}

#[derive(Clone)]
enum PendingTerminalEvent {
    Opened(Value),
    Output(Value),
    Resized(Value),
    Closed(Value),
}

struct TerminalPanel {
    chat_id: String,
    agent: Option<AgentCli>,
    allow_agent_tabs: bool,
    sessions: Vec<TerminalTab>,
    selected: Option<String>,
    viewport: Option<(usize, usize)>,
    auto_open: bool,
    opening: bool,
    opening_agent: Option<AgentCli>,
    loading: bool,
    pending_events: Vec<PendingTerminalEvent>,
    error: Option<String>,
}

impl TerminalPanel {
    fn accepts_agent(&self, agent: Option<AgentCli>) -> bool {
        self.allow_agent_tabs || self.agent == agent
    }

    fn opening_matches_protocol_agent(&self, agent: Option<&str>) -> bool {
        if !self.opening {
            return false;
        }
        match (agent, self.opening_agent) {
            (None, None) => true,
            (Some(protocol), Some(agent)) => protocol == agent.protocol_name(),
            _ => false,
        }
    }

    fn finish_opening(&mut self) {
        self.opening = false;
        self.opening_agent = None;
    }

    fn selected(&self) -> Option<&TerminalTab> {
        let selected = self.selected.as_deref()?;
        self.sessions.iter().find(|session| session.id == selected)
    }

    fn has_requested_session(&self) -> bool {
        self.sessions
            .iter()
            .any(|session| session.agent == self.agent)
    }

    fn should_auto_open(&self) -> bool {
        !self.loading && !self.has_requested_session() && self.auto_open && !self.opening
    }

    fn selection_after_refresh(&self, previous: Option<String>) -> Option<String> {
        previous
            .filter(|selected| self.sessions.iter().any(|session| &session.id == selected))
            .or_else(|| {
                self.sessions
                    .iter()
                    .find(|session| session.agent == self.agent)
                    .map(|session| session.id.clone())
            })
            .or_else(|| self.sessions.first().map(|session| session.id.clone()))
    }

    fn selected_mut(&mut self) -> Option<&mut TerminalTab> {
        let selected = self.selected.as_deref()?;
        self.sessions
            .iter_mut()
            .find(|session| session.id == selected)
    }

    fn remove(&mut self, terminal_id: &str) {
        self.sessions
            .retain(|session| session.id.as_str() != terminal_id);
        if self.selected.as_deref() == Some(terminal_id) {
            self.selected = self.sessions.first().map(|session| session.id.clone());
        }
    }
}

fn terminal_tab_title(agent: Option<AgentCli>) -> String {
    agent.map(AgentCli::label).unwrap_or("Terminal").to_owned()
}

fn stable_agent_session_id(terminal_id: &str) -> String {
    fn hash(seed: u64, value: &str) -> u64 {
        value.as_bytes().iter().fold(seed, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
    }

    let high = hash(0xcbf29ce484222325, terminal_id);
    let low = hash(0x84222325cbf29ce4, terminal_id);
    let mut bytes = ((u128::from(high) << 64) | u128::from(low)).to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn terminal_session_id(
    chat_id: &str,
    agent: Option<AgentCli>,
    reuse: bool,
    unique: u128,
) -> String {
    if agent.is_some() || reuse {
        format!(
            "{chat_id}:{}",
            agent.map(AgentCli::protocol_name).unwrap_or("terminal")
        )
    } else {
        format!("{chat_id}:terminal:{unique}")
    }
}

fn jcode_terminal_screen_working(screen: &str) -> Option<bool> {
    let prompt = screen
        .lines()
        .rev()
        .map(str::trim_start)
        .find(|line| !line.is_empty())?;
    let marker = prompt.trim_start_matches(|character: char| character.is_ascii_digit());
    match marker.chars().next()? {
        '…' => Some(true),
        '>' => Some(false),
        _ => None,
    }
}

fn terminal_runtime_event(event: SessionEvent) -> (&'static str, Value) {
    match event {
        SessionEvent::Opened {
            chat_id,
            terminal_id,
            title,
            agent,
            columns,
            rows,
        } => (
            "terminal-opened",
            serde_json::json!({
                "chat": chat_id,
                "terminal": terminal_id,
                "title": title,
                "agent": agent,
                "columns": columns,
                "rows": rows,
            }),
        ),
        SessionEvent::Output {
            chat_id,
            terminal_id,
            data,
        } => (
            "terminal-output",
            serde_json::json!({
                "chat": chat_id,
                "terminal": terminal_id,
                "data": STANDARD.encode(data),
            }),
        ),
        SessionEvent::Resized {
            chat_id,
            terminal_id,
            columns,
            rows,
        } => (
            "terminal-resized",
            serde_json::json!({
                "chat": chat_id,
                "terminal": terminal_id,
                "columns": columns,
                "rows": rows,
            }),
        ),
        SessionEvent::Activity {
            chat_id,
            terminal_id,
            working,
        } => (
            "terminal-activity",
            serde_json::json!({
                "chat": chat_id,
                "terminal": terminal_id,
                "working": working,
                "terminal_working": working,
            }),
        ),
        SessionEvent::Closed {
            chat_id,
            terminal_id,
        } => (
            "terminal-closed",
            serde_json::json!({"chat": chat_id, "terminal": terminal_id}),
        ),
    }
}

fn insert_terminal_cursor_highlight(
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    cursor: Option<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some((cursor_range, cursor_style)) = cursor else {
        return highlights;
    };
    if cursor_range.is_empty() {
        return highlights;
    }

    let mut result = Vec::with_capacity(highlights.len() + 2);
    let mut inserted = false;
    for (range, style) in highlights {
        if range.end <= cursor_range.start {
            result.push((range, style));
        } else if cursor_range.end <= range.start {
            if !inserted {
                result.push((cursor_range.clone(), cursor_style));
                inserted = true;
            }
            result.push((range, style));
        } else {
            if range.start < cursor_range.start {
                result.push((range.start..cursor_range.start, style));
            }
            if !inserted {
                result.push((cursor_range.clone(), cursor_style));
                inserted = true;
            }
            if cursor_range.end < range.end {
                result.push((cursor_range.end..range.end, style));
            }
        }
    }
    if !inserted {
        result.push((cursor_range, cursor_style));
    }
    result
}

const TERMINAL_OUTPUT_PADDING: f32 = 16.0;

fn terminal_geometry(width: f32, height: f32, cell_width: f32, line_height: f32) -> (usize, usize) {
    let total_padding = TERMINAL_OUTPUT_PADDING * 2.0;
    let content_width = (width - total_padding).max(cell_width);
    let content_height = (height - total_padding).max(line_height);
    let columns = (content_width / cell_width.max(1.0)).floor() as usize;
    let rows = (content_height / line_height.max(1.0)).floor() as usize;
    (columns.clamp(20, 500), rows.clamp(4, 200))
}

fn terminal_scroll_is_at_bottom(scroll: &ScrollHandle) -> bool {
    let remaining = f32::from(scroll.max_offset().height) + f32::from(scroll.offset().y);
    remaining <= 2.0
}

fn terminal_mouse_scroll_bytes(delta_y: f32) -> Vec<u8> {
    let button = if delta_y > 0.0 {
        64
    } else if delta_y < 0.0 {
        65
    } else {
        return Vec::new();
    };
    format!("\x1b[<{button};1;1M").into_bytes()
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GitStatus {
    branch: String,
    #[serde(default)]
    base: String,
    upstream: String,
    ahead: u64,
    conflicted: u64,
    clean: bool,
}

impl GitStatus {
    fn can_open_pull_request(&self) -> bool {
        self.clean
            && self.conflicted == 0
            && !self.branch.is_empty()
            && !matches!(self.branch.as_str(), "(detached)" | "(initial)")
            && !self.base.is_empty()
            && self.branch != self.base
            && !self.upstream.is_empty()
            && self.ahead == 0
    }
}

struct PendingSend {
    chat_id: String,
    text: String,
    attachments: Vec<Attachment>,
    restore: bool,
    optimistic: OptimisticSend,
}

#[derive(Clone, Copy)]
enum OptimisticSend {
    Started { message_index: Option<usize> },
    Queued { index: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenQuestion {
    chat_id: String,
    question: String,
    options: Vec<String>,
    accepts_input: bool,
}

fn question_from_event(body: &Value) -> Option<OpenQuestion> {
    if body.get("waiting").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let chat_id = body.get("chat")?.as_str()?.to_owned();
    let options = body
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|option| {
            let option = option.trim();
            (!option.is_empty()).then(|| option.chars().take(1_000).collect())
        })
        .take(6)
        .collect::<Vec<String>>();
    let accepts_input = body
        .get("accepts_input")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if options.len() < 2 && !accepts_input {
        return None;
    }
    let question = body
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|question| !question.is_empty())
        .unwrap_or("Which one?")
        .chars()
        .take(2_000)
        .collect();
    Some(OpenQuestion {
        chat_id,
        question,
        options,
        accepts_input,
    })
}

fn connection_state_key(endpoint: ChatEndpoint, remote: Option<&str>) -> String {
    match endpoint {
        ChatEndpoint::Local => "local".into(),
        ChatEndpoint::Remote => {
            format!("remote/{}", remote.unwrap_or("ssh"))
        }
    }
}

struct PendingSpeech {
    chat_id: String,
    previous_assistant_id: Option<i64>,
}

/// Where a row sits inside a run of consecutive plain activity, so the run reads
/// as one card instead of one card per command.
#[derive(Clone, Copy, Default)]
struct ActivityRun {
    position: usize,
}

#[derive(Clone, Default)]
struct TranscriptSnapshot {
    messages: Arc<Vec<Message>>,
    live_text: Option<Arc<Message>>,
    live_items: Arc<Vec<Message>>,
}

impl TranscriptSnapshot {
    fn sync_messages(&mut self, model: &AppModel) {
        self.messages = Arc::new(model.messages.clone());
    }

    fn sync_live_text(&mut self, model: &AppModel) {
        self.live_text = (!model.live_text.is_empty()).then(|| {
            Arc::new(Message::new_plain(
                None,
                "assistant",
                model.live_text.clone(),
                model.selected_summary().map(|chat| chat.backend.clone()),
            ))
        });
    }

    fn sync_live_items(&mut self, model: &AppModel) {
        self.live_items = Arc::new(model.live_items.clone());
    }

    fn stacks_activity(&self, index: usize) -> bool {
        self.get(index).is_some_and(|message| {
            message.role == "tool" && activity::is_plain_activity(&message.content)
        })
    }

    fn activity_run(&self, index: usize) -> ActivityRun {
        if !self.stacks_activity(index) {
            return ActivityRun::default();
        }
        let mut start = index;
        while start > 0 && self.stacks_activity(start - 1) {
            start -= 1;
        }
        ActivityRun {
            position: index - start,
        }
    }

    /// One summary card for the head of a plain activity run. The remaining
    /// transcript rows render empty; expanding this card reveals every command
    /// without adding a second disclosure level to each one.

    fn get(&self, index: usize) -> Option<&Message> {
        if let Some(message) = self.messages.get(index) {
            return Some(message);
        }
        let mut live_index = index.saturating_sub(self.messages.len());
        if let Some(item) = self.live_items.get(live_index) {
            return Some(item);
        }
        live_index = live_index.saturating_sub(self.live_items.len());
        if let Some(live_text) = self.live_text.as_deref() {
            if live_index == 0 {
                return Some(live_text);
            }
        }
        None
    }
}

#[derive(Clone)]
struct QueueEdit {
    chat_id: String,
    index: usize,
    original: String,
    text: String,
    submitting: Option<String>,
}

#[derive(Clone)]
struct ShortcutRow;

#[derive(Clone)]
struct ShortcutPanel {
    folder_id: Option<String>,
    rows: Vec<ShortcutRow>,
    loading: bool,
    submitting: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SidebarTarget {
    Folder(String),
    Chat(String),
}

#[derive(Clone)]
struct SidebarEdit {
    target: SidebarTarget,
    original: String,
    text: String,
    submitting: bool,
}

#[derive(Clone)]
struct SessionContextMenu {
    chat_id: String,
    title: String,
    position: Point<Pixels>,
}

#[derive(Clone)]
struct WorkspaceDefaults {
    folder_id: String,
    workdir: String,
    repo: String,
    loading: bool,
    submitting: bool,
}

#[derive(Clone)]
struct SecretsPanel {
    folder_id: Option<String>,
    names: Vec<String>,
    name: String,
    value: String,
    loading: bool,
    submitting: bool,
    error: Option<String>,
}

#[derive(Clone, Default)]
struct DevicesPanel {
    devices: Vec<Value>,
    loading: bool,
    mutating: Option<String>,
    editing_id: Option<String>,
    edit_name: String,
    revoke_confirmation: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Default)]
struct SharePanel {
    loading: bool,
    host: String,
    port: Option<u16>,
    code: String,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RemoteState {
    #[default]
    Unconfigured,
    Connecting,
    Connected,
    Offline,
}

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
enum ChatEndpoint {
    #[default]
    Local,
    Remote,
}

impl ChatEndpoint {}

fn persisted_runtime(saved: Option<&str>, remote_key: Option<&str>) -> ChatEndpoint {
    match (saved, remote_key) {
        (Some(saved), Some(remote)) if saved == remote => ChatEndpoint::Remote,
        _ => ChatEndpoint::Local,
    }
}

#[derive(Clone, Default)]
struct RemotePanel {
    command: String,
    submitting: bool,
    error: Option<String>,
}

struct XdDesktop {
    model: AppModel,
    inactive_model: AppModel,
    active_endpoint: ChatEndpoint,
    minimal_route: MinimalRoute,
    minimal_theme_open: bool,
    minimal_new_tab_open: bool,
    minimal_popup_focus: FocusHandle,
    minimal_popup_previous_focus: Option<WeakFocusHandle>,
    minimal_popup_focus_captured: bool,
    minimal_new_session_agent: AgentCli,
    pending_minimal_session: Option<(String, String)>,
    settings: AppSettings,
    settings_open: bool,
    auth_open: bool,
    secrets_panel: Option<SecretsPanel>,
    devices_panel: Option<DevicesPanel>,
    share_panel: Option<SharePanel>,
    shortcut_panel: Option<ShortcutPanel>,
    remote_panel: Option<RemotePanel>,
    self_update_panel: Option<SelfUpdatePanel>,
    auth_providers: Vec<AuthProvider>,
    cli_versions: Vec<CliVersion>,
    cli_versions_loading: bool,
    cli_versions_error: Option<String>,
    auth_input_text: String,
    speech_output: SpeechOutput,
    pending_speech: Option<PendingSpeech>,
    host: Option<HostHandle>,
    _started_host: Option<StartedHost>,
    connection_generation: u64,
    reconnect_attempt: u32,
    connecting: bool,
    connection_in_flight: bool,
    remote_host: Option<HostHandle>,
    remote_bridge: Option<SshRemoteBridge>,
    remote_state: RemoteState,
    remote_error: Option<String>,
    remote_generation: u64,
    remote_reconnect_attempt: u32,
    transcript: ListState,
    transcript_snapshot: TranscriptSnapshot,
    /// Whether the selected chat has returned at least one transcript page.
    /// This is deliberately separate from `messages.is_empty()`: a new chat is
    /// successfully loaded and empty, while a request lost during reconnect is
    /// still waiting to be hydrated.
    transcript_loaded: bool,
    transcript_loading: bool,
    transcript_page_loading: bool,
    transcript_refresh_pending: bool,
    transcript_has_older: bool,
    transcript_has_newer: bool,
    composer_input: Entity<FileEditor>,
    queue_edit_input: Entity<FileEditor>,
    sidebar_edit_input: Entity<ComposerInput>,
    workspace_create_input: Entity<ComposerInput>,
    workspace_repo_input: Entity<ComposerInput>,
    workspace_clone_input: Entity<ComposerInput>,
    chat_create_input: Entity<ComposerInput>,
    workspace_context_input: Entity<ComposerInput>,
    workspace_workdir_input: Entity<ComposerInput>,
    workspace_repo_default_input: Entity<ComposerInput>,
    git_commit_input: Entity<ComposerInput>,
    file_editor: Entity<FileEditor>,
    /// The editor behind whichever file tab is in front.
    tab_editor: Entity<FileEditor>,
    terminal_input: Entity<ComposerInput>,
    terminal_scroll: ScrollHandle,
    auth_input: Entity<ComposerInput>,
    secret_name_input: Entity<ComposerInput>,
    secret_value_input: Entity<ComposerInput>,
    device_name_input: Entity<ComposerInput>,
    remote_ssh_input: Entity<ComposerInput>,
    question_input: Entity<ComposerInput>,
    source_build_input: Entity<ComposerInput>,
    composer: String,
    queue_edit: Option<QueueEdit>,
    sidebar_edit: Option<SidebarEdit>,
    session_context_menu: Option<SessionContextMenu>,
    pending_sidebar_delete: Option<SidebarTarget>,
    sidebar_delete_submitting: bool,
    sidebar_move: Option<SidebarTarget>,
    sidebar_move_submitting: bool,
    sidebar_move_destination: Option<Option<String>>,
    collapsed_folders: HashSet<String>,
    creating_workspace: bool,
    workspace_create_name: String,
    workspace_create_repo: String,
    workspace_create_clone: String,
    workspace_create_submitting: bool,
    workspace_clone_status: Option<String>,
    pending_clone_requests: HashMap<ChatEndpoint, String>,
    workspace_clone_outcomes: HashMap<(ChatEndpoint, String), Option<String>>,
    creating_chat_folder: Option<String>,
    chat_create_title: String,
    chat_create_submitting: bool,
    chat_create_worktree: Option<NewSessionWorktree>,
    chat_create_worktrees: Vec<Worktree>,
    chat_create_worktrees_loading: bool,
    chat_create_can_new_worktree: bool,
    workspace_context_folder: Option<String>,
    workspace_context_text: String,
    workspace_context_loading: bool,
    workspace_context_submitting: bool,
    workspace_defaults: Option<WorkspaceDefaults>,
    diff_panel: Option<DiffPanel>,
    terminal_panel: Option<TerminalPanel>,
    terminal_runtime: SessionRuntime,
    terminal_panel_cache: HashMap<(ChatEndpoint, String), TerminalPanel>,
    terminal_cache_refresh: HashSet<ChatEndpoint>,
    terminal_cursor_visible: bool,
    diff_generation: u64,
    /// The working directory as a folding tree, in the sidebar.
    file_tree: files::FileTree,
    /// The files opened out of it, as tabs beside the chat.
    open_files: files::OpenFiles,
    /// Bumped when the chat changes, so listings for the old one are dropped.
    tree_generation: u64,
    collapsed_diff_files: HashSet<String>,
    git_commit_message: String,
    draft_generation: u64,
    draft_dirty: bool,
    attachments_dirty: bool,
    attachment_generation: u64,
    sending: bool,
    pending_send: Option<PendingSend>,
    open_question: Option<OpenQuestion>,
    question_answer: String,
    source_build_panel: SourceBuildPanel,
    source_build_run: Option<SourceBuildRun>,
    source_build_generation: u64,
    workflow_statuses: Arc<HashMap<String, Value>>,
    workflow_pending: Arc<HashSet<String>>,
    workflow_ticking: HashSet<String>,
    live_render_generation: u64,
    live_render_scheduled: Option<u64>,
    window_settings_generation: u64,
    /// The banner text a dismissal timer is counting down, so a newer error
    /// restarts the clock instead of inheriting the old one's.
    expiring_error: Option<String>,
    error_generation: u64,
}

impl XdDesktop {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer_input = cx.new(FileEditor::composer);
        cx.subscribe(&composer_input, |this, _, event, cx| match event {
            EditorEvent::Changed(text) => this.composer_changed(text.clone(), cx),
            EditorEvent::PasteImage { format, bytes } => {
                this.attach_clipboard_image(*format, bytes.clone(), cx)
            }
            EditorEvent::Submit => this.send_composer(cx),
            EditorEvent::Save => {}
        })
        .detach();
        let queue_edit_input = cx.new(|cx| FileEditor::message(cx, "Edit queued message…"));
        cx.subscribe(&queue_edit_input, |this, _, event, cx| match event {
            EditorEvent::Changed(text) => this.queue_edit_changed(text.clone(), cx),
            EditorEvent::PasteImage { .. } => {}
            EditorEvent::Submit => this.save_queue_edit(cx),
            EditorEvent::Save => {}
        })
        .detach();
        let sidebar_edit_input = cx.new(|cx| ComposerInput::new(cx, "Name…"));
        cx.subscribe(&sidebar_edit_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.sidebar_edit_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_sidebar_edit(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let workspace_create_input = cx.new(|cx| ComposerInput::new(cx, "Workspace name…"));
        cx.subscribe(&workspace_create_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.workspace_create_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_workspace_create(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let workspace_repo_input =
            cx.new(|cx| ComposerInput::new(cx, "Existing repository path (optional)…"));
        cx.subscribe(&workspace_repo_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.workspace_repo_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_workspace_create(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let workspace_clone_input =
            cx.new(|cx| ComposerInput::new(cx, "Git clone URL (optional)…"));
        cx.subscribe(&workspace_clone_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.workspace_clone_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_workspace_create(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let chat_create_input = cx.new(|cx| ComposerInput::new(cx, "Chat title…"));
        cx.subscribe(&chat_create_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.chat_create_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_minimal_chat_create(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let workspace_context_input =
            cx.new(|cx| ComposerInput::new(cx, "Instructions inherited by this workspace…"));
        cx.subscribe(&workspace_context_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.workspace_context_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_workspace_context(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let workspace_workdir_input =
            cx.new(|cx| ComposerInput::new(cx, "Working directory (inherit when empty)…"));
        cx.subscribe(&workspace_workdir_input, |this, _, event, cx| {
            if let ComposerEvent::Changed(text) = event {
                this.workspace_workdir_changed(text.clone(), cx);
            }
        })
        .detach();
        let workspace_repo_default_input =
            cx.new(|cx| ComposerInput::new(cx, "Repository path (inherit when empty)…"));
        cx.subscribe(&workspace_repo_default_input, |this, _, event, cx| {
            if let ComposerEvent::Changed(text) = event {
                this.workspace_repo_default_changed(text.clone(), cx);
            }
        })
        .detach();
        let git_commit_input = cx.new(|cx| ComposerInput::new(cx, "Commit message…"));
        cx.subscribe(&git_commit_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.git_commit_changed(text.clone(), cx),
            ComposerEvent::Submit => this.commit_changes(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let file_editor = cx.new(FileEditor::new);
        let tab_editor = cx.new(FileEditor::new);
        cx.subscribe(&tab_editor, |this, _, event, cx| match event {
            EditorEvent::Changed(text) => {
                if let files::FileTab::File(path) = this.open_files.active.clone() {
                    this.open_files.edit(&path, text.clone());
                    cx.notify();
                }
            }
            EditorEvent::Save => this.save_file_tab(cx),
            EditorEvent::PasteImage { .. } | EditorEvent::Submit => {}
        })
        .detach();
        cx.subscribe(&file_editor, |this, _, event, cx| match event {
            EditorEvent::Changed(text) => {
                if let Some(preview) = this
                    .diff_panel
                    .as_mut()
                    .and_then(|panel| panel.file_preview.as_mut())
                    && !preview.truncated
                {
                    preview.content = text.clone();
                    cx.notify();
                }
            }
            EditorEvent::PasteImage { .. } => {}
            EditorEvent::Submit => {}
            EditorEvent::Save => this.save_browse_file(cx),
        })
        .detach();
        let terminal_input = cx.new(ComposerInput::terminal);
        cx.subscribe(&terminal_input, |this, _, event, cx| match event {
            ComposerEvent::Bytes(bytes) => this.send_terminal_input(bytes, cx),
            ComposerEvent::PasteImage { format, bytes } => {
                this.paste_terminal_image(*format, bytes, cx)
            }
            ComposerEvent::Changed(_) | ComposerEvent::Submit => {}
        })
        .detach();
        let auth_input = cx.new(|cx| ComposerInput::new(cx, "Paste authorization code…"));
        cx.subscribe(&auth_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                this.auth_input_text = text.clone();
                cx.notify();
            }
            ComposerEvent::Submit => this.submit_auth_input(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let secret_name_input = cx.new(|cx| ComposerInput::new(cx, "ENVIRONMENT_VARIABLE"));
        cx.subscribe(&secret_name_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                if let Some(panel) = &mut this.secrets_panel {
                    panel.name = text.clone();
                    panel.error = None;
                }
                cx.notify();
            }
            ComposerEvent::Submit => this.save_secret(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let secret_value_input =
            cx.new(|cx| ComposerInput::password(cx, "Secret value (never displayed)"));
        cx.subscribe(&secret_value_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                if let Some(panel) = &mut this.secrets_panel {
                    panel.value = text.clone();
                    panel.error = None;
                }
                cx.notify();
            }
            ComposerEvent::Submit => this.save_secret(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let device_name_input = cx.new(|cx| ComposerInput::new(cx, "Device name…"));
        cx.subscribe(&device_name_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                if let Some(panel) = &mut this.devices_panel {
                    panel.edit_name = text.clone();
                    panel.error = None;
                }
                cx.notify();
            }
            ComposerEvent::Submit => this.save_device_name(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let remote_ssh_input = cx.new(|cx| ComposerInput::new(cx, "ssh user@host -p 22"));
        cx.subscribe(&remote_ssh_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                if let Some(panel) = &mut this.remote_panel {
                    panel.command = text.clone();
                    panel.error = None;
                }
                cx.notify();
            }
            ComposerEvent::Submit => this.connect_remote_machine(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let question_input = cx.new(|cx| ComposerInput::new(cx, "Type your answer…"));
        cx.subscribe(&question_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                this.question_answer = text.clone();
                cx.notify();
            }
            ComposerEvent::Submit => this.send_question_input(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let mut settings = AppSettings::load();
        let source_build_input =
            cx.new(|cx| ComposerInput::new(cx, "main, #128, GitHub URL, or commit SHA…"));
        cx.subscribe(&source_build_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                if this.source_build_panel.running {
                    let value = this.source_build_panel.text.clone();
                    this.source_build_input
                        .update(cx, |input, cx| input.set_text(value, cx));
                } else {
                    this.source_build_panel.set_text(text.clone());
                }
                cx.notify();
            }
            ComposerEvent::Submit => this.start_source_build(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        source_build_input.update(cx, |input, cx| {
            input.set_text(settings.build_source.clone(), cx)
        });
        let source_build_panel = SourceBuildPanel {
            text: settings.build_source.clone(),
            target: source_build::parse_target(&settings.build_source),
            ..Default::default()
        };
        let parsed_remote = settings
            .remote_ssh_command
            .as_deref()
            .map(SshCommand::parse)
            .transpose();
        let remote_error = parsed_remote.as_ref().err().cloned();
        let remote_key = parsed_remote
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(|command| connection_state_key(ChatEndpoint::Remote, Some(command.destination())));
        let active_endpoint =
            persisted_runtime(settings.active_connection.as_deref(), remote_key.as_deref());
        let active_connection = match active_endpoint {
            ChatEndpoint::Local => "local".to_owned(),
            ChatEndpoint::Remote => remote_key.expect("a cached remote connection has credentials"),
        };
        let collapsed_folders = settings
            .collapsed_folder_sets
            .get(&active_connection)
            .or_else(|| {
                (active_endpoint == ChatEndpoint::Local).then_some(&settings.collapsed_folders)
            })
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        let corrected_active_connection =
            settings.active_connection.as_deref() != Some(active_connection.as_str());
        settings.active_connection = Some(active_connection);
        let remote_configured = settings.remote_ssh_command.is_some();
        let minimal_popup_focus = cx.focus_handle();
        let (terminal_runtime, terminal_updates) = SessionRuntime::new();
        let mut desktop = Self {
            model: AppModel {
                draft_revision: -1,
                ..Default::default()
            },
            inactive_model: AppModel {
                draft_revision: -1,
                ..Default::default()
            },
            active_endpoint,
            minimal_route: MinimalRoute::default(),
            minimal_theme_open: false,
            minimal_new_tab_open: false,
            minimal_popup_focus,
            minimal_popup_previous_focus: None,
            minimal_popup_focus_captured: false,
            minimal_new_session_agent: AgentCli::Codex,
            pending_minimal_session: None,
            settings,
            settings_open: false,
            auth_open: false,
            secrets_panel: None,
            devices_panel: None,
            share_panel: None,
            shortcut_panel: None,
            remote_panel: None,
            self_update_panel: None,
            auth_providers: Vec::new(),
            cli_versions: Vec::new(),
            cli_versions_loading: false,
            cli_versions_error: None,
            auth_input_text: String::new(),
            speech_output: SpeechOutput::default(),
            pending_speech: None,
            host: None,
            _started_host: None,
            connection_generation: 0,
            reconnect_attempt: 0,
            connecting: false,
            connection_in_flight: false,
            remote_host: None,
            remote_bridge: None,
            remote_state: if remote_configured {
                RemoteState::Offline
            } else {
                RemoteState::Unconfigured
            },
            remote_error,
            remote_generation: 0,
            remote_reconnect_attempt: 0,
            transcript: ListState::new(0, ListAlignment::Bottom, px(700.0)),
            transcript_snapshot: TranscriptSnapshot::default(),
            transcript_loaded: false,
            transcript_loading: false,
            transcript_page_loading: false,
            transcript_refresh_pending: false,
            transcript_has_older: false,
            transcript_has_newer: false,
            composer_input,
            queue_edit_input,
            sidebar_edit_input,
            workspace_create_input,
            workspace_repo_input,
            workspace_clone_input,
            chat_create_input,
            workspace_context_input,
            workspace_workdir_input,
            workspace_repo_default_input,
            git_commit_input,
            file_editor,
            tab_editor,
            terminal_input,
            terminal_scroll: ScrollHandle::new(),
            auth_input,
            secret_name_input,
            secret_value_input,
            device_name_input,
            remote_ssh_input,
            question_input,
            source_build_input,
            composer: String::new(),
            queue_edit: None,
            sidebar_edit: None,
            session_context_menu: None,
            pending_sidebar_delete: None,
            sidebar_delete_submitting: false,
            sidebar_move: None,
            sidebar_move_submitting: false,
            sidebar_move_destination: None,
            collapsed_folders,
            creating_workspace: false,
            workspace_create_name: String::new(),
            workspace_create_repo: String::new(),
            workspace_create_clone: String::new(),
            workspace_create_submitting: false,
            workspace_clone_status: None,
            pending_clone_requests: HashMap::new(),
            workspace_clone_outcomes: HashMap::new(),
            creating_chat_folder: None,
            chat_create_title: String::new(),
            chat_create_submitting: false,
            chat_create_worktree: None,
            chat_create_worktrees: Vec::new(),
            chat_create_worktrees_loading: false,
            chat_create_can_new_worktree: false,
            workspace_context_folder: None,
            workspace_context_text: String::new(),
            workspace_context_loading: false,
            workspace_context_submitting: false,
            workspace_defaults: None,
            diff_panel: None,
            terminal_panel: None,
            terminal_runtime,
            terminal_panel_cache: HashMap::new(),
            terminal_cache_refresh: HashSet::new(),
            terminal_cursor_visible: true,
            diff_generation: 0,
            file_tree: files::FileTree::default(),
            open_files: files::OpenFiles::default(),
            tree_generation: 0,
            collapsed_diff_files: HashSet::new(),
            git_commit_message: String::new(),
            draft_generation: 0,
            draft_dirty: false,
            attachments_dirty: false,
            attachment_generation: 0,
            sending: false,
            pending_send: None,
            open_question: None,
            question_answer: String::new(),
            source_build_panel,
            source_build_run: None,
            source_build_generation: 0,
            workflow_statuses: Arc::new(HashMap::new()),
            workflow_pending: Arc::new(HashSet::new()),
            workflow_ticking: HashSet::new(),
            live_render_generation: 0,
            live_render_scheduled: None,
            window_settings_generation: 0,
            expiring_error: None,
            error_generation: 0,
        };
        if corrected_active_connection {
            let _ = desktop.settings.save();
        }
        cx.observe_window_bounds(window, |this, window, cx| {
            this.window_bounds_changed(window, cx);
        })
        .detach();
        match desktop.active_endpoint {
            ChatEndpoint::Local => desktop.schedule_connect(Duration::ZERO, cx),
            ChatEndpoint::Remote => desktop.schedule_remote_connect(Duration::ZERO, cx),
        }
        desktop.listen_for_terminal_runtime(terminal_updates, cx);
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(500)).await;
                let alive = this
                    .update(cx, |this, cx| {
                        this.terminal_cursor_visible = !this.terminal_cursor_visible;
                        if this.terminal_panel.is_some() {
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
        desktop
    }

    fn window_bounds_changed(&mut self, window: &Window, cx: &mut Context<Self>) {
        let maximized = window.is_maximized();
        let bounds = window.bounds();
        let mut changed = self.settings.window_maximized != maximized;
        self.settings.window_maximized = maximized;
        if !maximized {
            let width = f32::from(bounds.size.width)
                .round()
                .clamp(760.0, u16::MAX as f32) as u16;
            let height = f32::from(bounds.size.height)
                .round()
                .clamp(560.0, u16::MAX as f32) as u16;
            changed |= self.settings.window_width != width || self.settings.window_height != height;
            self.settings.window_width = width;
            self.settings.window_height = height;
        }
        if !changed {
            return;
        }
        self.window_settings_generation = self.window_settings_generation.saturating_add(1);
        let generation = self.window_settings_generation;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(300)).await;
            let _ = this.update(cx, |this, _| {
                if this.window_settings_generation == generation
                    && let Err(error) = this.settings.save()
                {
                    this.model.connection_error = Some(error);
                }
            });
        })
        .detach();
    }

    /// An error about one action says a keystroke did not land, not that xd is
    /// broken, so it clears itself. A connection error stays put: what it
    /// reports is still true, and its banner carries the retry control.
    fn expire_action_error(&mut self, cx: &mut Context<Self>) {
        if self.model.connection_error == self.expiring_error {
            return;
        }
        self.expiring_error = self.model.connection_error.clone();
        let Some(error) = self.expiring_error.clone() else {
            return;
        };
        if !self.model.connected {
            return;
        }
        self.error_generation = self.error_generation.saturating_add(1);
        let generation = self.error_generation;
        cx.spawn(async move |this, cx| {
            Timer::after(ACTION_ERROR_LIFETIME).await;
            let _ = this.update(cx, |this, cx| {
                if this.error_generation == generation
                    && this.model.connection_error.as_deref() == Some(error.as_str())
                {
                    this.dismiss_error(cx);
                }
            });
        })
        .detach();
    }

    fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        self.model.connection_error = None;
        self.expiring_error = None;
        cx.notify();
    }

    fn active_host(&self) -> Option<&HostHandle> {
        self.endpoint_host(self.active_endpoint)
    }

    fn endpoint_host(&self, endpoint: ChatEndpoint) -> Option<&HostHandle> {
        match endpoint {
            ChatEndpoint::Local => self.host.as_ref(),
            ChatEndpoint::Remote => self.remote_host.as_ref(),
        }
    }

    fn secrets_host(&self) -> Option<&HostHandle> {
        self.active_host()
    }

    fn endpoint_model(&self, endpoint: ChatEndpoint) -> &AppModel {
        if endpoint == self.active_endpoint {
            &self.model
        } else {
            &self.inactive_model
        }
    }

    fn endpoint_model_mut(&mut self, endpoint: ChatEndpoint) -> &mut AppModel {
        if endpoint == self.active_endpoint {
            &mut self.model
        } else {
            &mut self.inactive_model
        }
    }

    fn apply_passive_event(model: &mut AppModel, name: &str, body: &Value) {
        if name == "tree" {
            let _ = model.apply_tree(body);
            return;
        }
        model.apply_event(name, body);
    }

    fn apply_passive_reply(model: &mut AppModel, kind: &RequestKind, body: Value) {
        match kind {
            RequestKind::Tree => {
                let _ = model.apply_tree(&body);
            }
            RequestKind::AgentCatalog => {
                let _ = model.apply_agent_catalog(&body);
            }
            _ => {}
        }
    }

    fn remote_chat_reply(kind: &RequestKind) -> bool {
        matches!(
            kind,
            RequestKind::Tree
                | RequestKind::AgentCatalog
                | RequestKind::AgentAuth
                | RequestKind::AgentAuthMutation
                | RequestKind::AgentClis
                | RequestKind::Chat { .. }
                | RequestKind::Messages { .. }
                | RequestKind::Shortcuts { .. }
                | RequestKind::WorkflowStatus { .. }
                | RequestKind::ImageRead { .. }
                | RequestKind::Send { .. }
                | RequestKind::QueueMutation { .. }
                | RequestKind::EditQueue { .. }
                | RequestKind::Cancel { .. }
                | RequestKind::SetOption { .. }
                | RequestKind::SetShortcuts { .. }
                | RequestKind::RemoveWorktree { .. }
                | RequestKind::SetDraft { .. }
                | RequestKind::DiffRead { .. }
                | RequestKind::GitStatus { .. }
                | RequestKind::GitState { .. }
                | RequestKind::GitDraft { .. }
                | RequestKind::GitPullRequestStatus { .. }
                | RequestKind::GitPullRequestCreate { .. }
                | RequestKind::FileBrowseList { .. }
                | RequestKind::FileBrowseRead { .. }
                | RequestKind::FileBrowseWrite { .. }
                | RequestKind::FileTreeList { .. }
                | RequestKind::FileTabRead { .. }
                | RequestKind::FileTabWrite { .. }
                | RequestKind::GitCommit { .. }
                | RequestKind::GitPush { .. }
                | RequestKind::TerminalOpen { .. }
                | RequestKind::TerminalList { .. }
                | RequestKind::TerminalInput { .. }
                | RequestKind::TerminalPasteImage { .. }
                | RequestKind::TerminalResize { .. }
                | RequestKind::TerminalKill { .. }
                | RequestKind::AgentSecrets { .. }
                | RequestKind::SetAgentSecrets { .. }
                | RequestKind::FolderContext { .. }
                | RequestKind::SetFolderContext { .. }
                | RequestKind::FolderSettings { .. }
                | RequestKind::SetFolderSettings { .. }
                | RequestKind::NewFolder { .. }
                | RequestKind::NewChat { .. }
                | RequestKind::RenameFolder { .. }
                | RequestKind::MoveFolder { .. }
                | RequestKind::TrashFolder { .. }
                | RequestKind::RenameChat { .. }
                | RequestKind::MoveChat { .. }
                | RequestKind::DeleteChat { .. }
        )
    }

    fn remote_read_event(name: &str) -> bool {
        matches!(
            name,
            "tree"
                | "changed"
                | "agent-auth-changed"
                | "agent-cli-changed"
                | "draft"
                | "commands"
                | "turn-started"
                | "text"
                | "tool"
                | "turn-finished"
                | "queued"
                | "shortcuts-changed"
                | "workflow-status"
                | "terminal-opened"
                | "terminal-activity"
                | "terminal-output"
                | "terminal-resized"
                | "terminal-closed"
                | "git-draft-finished"
                | "folder-clone"
        )
    }

    fn local_admin_reply(kind: &RequestKind) -> bool {
        matches!(
            kind,
            RequestKind::HostUpdate { .. }
                | RequestKind::Devices
                | RequestKind::PeerPairing
                | RequestKind::PairRemote
                | RequestKind::RenameDevice { .. }
                | RequestKind::RevokeDevice { .. }
        )
    }

    fn local_admin_event(name: &str) -> bool {
        name == "host-update"
    }

    fn switch_active_endpoint(&mut self, endpoint: ChatEndpoint, cx: &mut Context<Self>) {
        if endpoint == self.active_endpoint {
            return;
        }
        self.stash_terminal_panel();
        self.sync_draft();
        self.draft_generation = self.draft_generation.saturating_add(1);
        std::mem::swap(&mut self.model, &mut self.inactive_model);
        self.active_endpoint = endpoint;
        self.remember_active_connection();
        self.restore_collapsed_folders();
        self.model.selected_chat = None;
        self.invalidate_live_render();
        self.transcript_snapshot = TranscriptSnapshot::default();
        self.transcript_loaded = false;
        self.transcript.reset(0);
        self.pending_speech = None;
        self.speech_output.stop();
        self.diff_panel = None;
        self.sync_terminal_input_mode(cx);
        self.sidebar_edit = None;
        self.session_context_menu = None;
        self.pending_sidebar_delete = None;
        self.sidebar_move = None;
        self.creating_workspace = false;
        self.creating_chat_folder = None;
        self.workspace_context_folder = None;
        self.workspace_defaults = None;
        self.workspace_clone_status = None;
        self.secrets_panel = None;
        self.auth_open = false;
        self.auth_providers.clear();
        self.cli_versions.clear();
        self.cli_versions_loading = false;
        self.cli_versions_error = None;
        self.auth_input_text.clear();
        self.auth_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.pending_send = None;
        self.sending = false;
        self.clear_question(cx);
        self.cancel_queue_edit(cx);
        Arc::make_mut(&mut self.workflow_statuses).clear();
        Arc::make_mut(&mut self.workflow_pending).clear();
        self.minimal_route = MinimalRoute::default();
        self.pending_minimal_session = None;
        self.request_agent_catalog();
    }

    fn schedule_connect(&mut self, delay: Duration, cx: &mut Context<Self>) {
        if self.active_endpoint != ChatEndpoint::Local
            || self.connecting
            || self.endpoint_model(ChatEndpoint::Local).connected
        {
            return;
        }
        self.connecting = true;
        self.connection_in_flight = false;
        self.connection_generation = self.connection_generation.saturating_add(1);
        let generation = self.connection_generation;
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                Timer::after(delay).await;
            }
            let _ = this.update(cx, |this, cx| this.begin_connect(generation, cx));
        })
        .detach();
    }

    fn begin_connect(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.connection_generation != generation || !self.connecting || self.connection_in_flight
        {
            return;
        }
        self.connection_in_flight = true;
        let connection = cx
            .background_executor()
            .spawn(async { HostHandle::start_local() });
        cx.spawn(async move |this, cx| {
            let result = connection.await;
            let _ = this.update(cx, |this, cx| {
                if this.connection_generation != generation {
                    return;
                }
                this.connecting = false;
                this.connection_in_flight = false;
                match result {
                    Ok((host, updates, started_host)) => {
                        this.host = Some(host);
                        this._started_host = Some(started_host);
                        this.reconnect_attempt = 0;
                        this.endpoint_model_mut(ChatEndpoint::Local)
                            .connection_error = None;
                        this.listen_for_host(updates, generation, cx);
                    }
                    Err(error) => {
                        this.endpoint_model_mut(ChatEndpoint::Local).connected = false;
                        this.endpoint_model_mut(ChatEndpoint::Local)
                            .connection_error = Some(format!("{error}. Retrying automatically…"));
                        this.schedule_reconnect(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn listen_for_host(
        &mut self,
        updates: async_channel::Receiver<HostUpdate>,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.connection_generation == generation {
                            this.handle_host_update(update, generation, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn schedule_reconnect(&mut self, cx: &mut Context<Self>) {
        self.reconnect_attempt = self.reconnect_attempt.saturating_add(1);
        self.schedule_connect(reconnect_delay(self.reconnect_attempt), cx);
    }

    fn schedule_remote_connect(&mut self, delay: Duration, cx: &mut Context<Self>) {
        if self.active_endpoint != ChatEndpoint::Remote
            || self.settings.remote_ssh_command.is_none()
            || matches!(
                self.remote_state,
                RemoteState::Connecting | RemoteState::Connected
            )
        {
            return;
        }
        self.remote_state = RemoteState::Connecting;
        self.remote_generation = self.remote_generation.saturating_add(1);
        let generation = self.remote_generation;
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                Timer::after(delay).await;
            }
            let _ = this.update(cx, |this, cx| {
                this.begin_remote_connect(generation, cx);
            });
        })
        .detach();
    }

    fn begin_remote_connect(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.remote_generation != generation || self.remote_state != RemoteState::Connecting {
            return;
        }
        let Some(command) = self.settings.remote_ssh_command.clone() else {
            self.remote_state = RemoteState::Unconfigured;
            return;
        };
        let command = match SshCommand::parse(&command) {
            Ok(command) => command,
            Err(error) => {
                self.remote_state = RemoteState::Unconfigured;
                self.remote_error = Some(error);
                return;
            }
        };
        let connection = cx
            .background_executor()
            .spawn(async move { remote::connect_ssh(&command) });
        cx.spawn(async move |this, cx| {
            let result = connection.await;
            let _ = this.update(cx, |this, cx| {
                if this.remote_generation != generation {
                    return;
                }
                match result {
                    Ok(session) => this.install_remote_session(session, generation, cx),
                    Err(error) => this.remote_connection_failed(error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install_remote_session(
        &mut self,
        session: SshRemoteSession,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let (host, updates, bridge) = session.into_parts();
        self.remote_host = Some(host);
        self.remote_bridge = Some(bridge);
        self.remote_state = RemoteState::Connected;
        self.remote_error = None;
        self.remote_reconnect_attempt = 0;
        let remote_model = self.endpoint_model_mut(ChatEndpoint::Remote);
        remote_model.connected = true;
        remote_model.connection_error = None;
        if let Some(panel) = &mut self.remote_panel {
            panel.submitting = false;
            panel.error = None;
        }
        self.listen_for_remote(updates, generation, cx);
        if let Some(host) = &self.remote_host
            && let Err(error) = host.tree()
        {
            self.remote_error = Some(error);
        }
        if let Some(host) = &self.remote_host {
            let _ = host.agent_catalog();
        }
        if self.active_endpoint == ChatEndpoint::Remote {
            self.refresh_selected_chat_after_connect(cx);
        }
    }

    fn listen_for_remote(
        &mut self,
        updates: async_channel::Receiver<HostUpdate>,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.remote_generation == generation {
                            this.handle_remote_update(update, generation, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn handle_remote_update(
        &mut self,
        update: HostUpdate,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        match update {
            HostUpdate::Connected { .. } => {}
            HostUpdate::Disconnected { message } => {
                if self.remote_generation != generation {
                    return;
                }
                self.terminal_cache_refresh.insert(ChatEndpoint::Remote);
                self.remote_host = None;
                self.remote_bridge = None;
                let remote_model = self.endpoint_model_mut(ChatEndpoint::Remote);
                remote_model.connected = false;
                remote_model.connection_error = Some(format!("{message} Reconnecting…"));
                if self.active_endpoint == ChatEndpoint::Remote {
                    self.sending = false;
                    self.transcript_loading = false;
                    self.transcript_page_loading = false;
                    self.transcript_refresh_pending = false;
                    self.pending_speech = None;
                    Arc::make_mut(&mut self.workflow_pending).clear();
                    self.speech_output.stop();
                    self.restore_pending_send(cx);
                    if let Some(diff) = &mut self.diff_panel {
                        diff.loading = false;
                        diff.status_loading = false;
                        diff.action = None;
                        diff.action_error = Some(message.clone());
                    }
                    if let Some(panel) = &mut self.terminal_panel {
                        panel.loading = false;
                        panel.finish_opening();
                        panel.error = Some(message.clone());
                    }
                    if let Some(panel) = &mut self.secrets_panel {
                        panel.loading = false;
                        panel.submitting = false;
                        panel.error = Some("Agent secrets disconnected.".into());
                    }
                    if self.auth_open {
                        self.cli_versions_loading = false;
                        self.cli_versions_error = Some("Assistant versions disconnected.".into());
                    }
                }
                self.remote_state = RemoteState::Offline;
                self.remote_error = Some(format!("{message} Reconnecting…"));
                if let Some(panel) = &mut self.remote_panel {
                    panel.submitting = false;
                    panel.error = self.remote_error.clone();
                }
                self.remote_reconnect_attempt = self.remote_reconnect_attempt.saturating_add(1);
                self.schedule_remote_connect(reconnect_delay(self.remote_reconnect_attempt), cx);
            }
            HostUpdate::Reply {
                kind,
                body,
                attachments,
            } => {
                if self.active_endpoint == ChatEndpoint::Remote {
                    if Self::remote_chat_reply(&kind) {
                        self.handle_reply(kind, body, attachments, cx);
                    }
                } else {
                    let value = Value::Object(body);
                    let tree = matches!(&kind, RequestKind::Tree);
                    if !self.handle_workspace_create_reply(ChatEndpoint::Remote, &kind, &value, cx)
                        && !self.handle_cached_terminal_reply(ChatEndpoint::Remote, &kind, &value)
                    {
                        Self::apply_passive_reply(&mut self.inactive_model, &kind, value);
                        if tree {
                            self.prime_terminal_cache(ChatEndpoint::Remote);
                        }
                    }
                }
            }
            HostUpdate::Event {
                name,
                body,
                attachments,
            } => {
                let body = Value::Object(body);
                if self.active_endpoint == ChatEndpoint::Remote {
                    if Self::remote_read_event(&name) {
                        self.handle_event(&name, body, attachments, cx);
                    }
                } else {
                    if name == "folder-clone" {
                        self.handle_folder_clone_event(ChatEndpoint::Remote, &body);
                    } else {
                        if Self::is_terminal_screen_event(&name) {
                            self.handle_terminal_screen_event(
                                ChatEndpoint::Remote,
                                &name,
                                &body,
                                cx,
                            );
                        }
                        Self::apply_passive_event(&mut self.inactive_model, &name, &body);
                    }
                    if name == "turn-finished"
                        && let Some(host) = &self.remote_host
                    {
                        let _ = host.tree();
                    }
                }
            }
        }
        cx.notify();
    }

    fn remote_connection_failed(&mut self, error: RemoteError, cx: &mut Context<Self>) {
        let message = error.to_string();
        self.remote_host = None;
        self.remote_bridge = None;
        self.endpoint_model_mut(ChatEndpoint::Remote).connected = false;
        self.remote_state = RemoteState::Offline;
        self.remote_error = Some(format!("{message} Retrying automatically…"));
        self.remote_reconnect_attempt = self.remote_reconnect_attempt.saturating_add(1);
        self.schedule_remote_connect(reconnect_delay(self.remote_reconnect_attempt), cx);
        if let Some(panel) = &mut self.remote_panel {
            panel.submitting = false;
            panel.error = self.remote_error.clone();
        }
        let remote_error = self.remote_error.clone();
        self.endpoint_model_mut(ChatEndpoint::Remote)
            .connection_error = remote_error;
    }

    fn activate_remote_runtime(&mut self, cx: &mut Context<Self>) {
        if self.settings.remote_ssh_command.is_none() {
            self.remote_error = Some("Enter an SSH command first.".into());
            cx.notify();
            return;
        }
        if self.active_endpoint == ChatEndpoint::Local {
            self.connection_generation = self.connection_generation.saturating_add(1);
            self.host = None;
            self.connecting = false;
            self.connection_in_flight = false;
            self.endpoint_model_mut(ChatEndpoint::Local).connected = false;
            self.switch_active_endpoint(ChatEndpoint::Remote, cx);
        }
        if self.remote_state == RemoteState::Connected {
            if let Some(host) = &self.remote_host {
                let _ = host.tree();
                let _ = host.agent_catalog();
            }
        } else {
            self.schedule_remote_connect(Duration::ZERO, cx);
        }
        cx.notify();
    }

    fn disconnect_remote_runtime(&mut self, cx: &mut Context<Self>) {
        if self.active_endpoint != ChatEndpoint::Remote {
            return;
        }
        self.remote_generation = self.remote_generation.saturating_add(1);
        self.remote_host = None;
        self.remote_bridge = None;
        self.remote_state = if self.settings.remote_ssh_command.is_some() {
            RemoteState::Offline
        } else {
            RemoteState::Unconfigured
        };
        self.remote_reconnect_attempt = 0;
        self.endpoint_model_mut(ChatEndpoint::Remote).connected = false;
        self.switch_active_endpoint(ChatEndpoint::Local, cx);
        self.schedule_connect(Duration::ZERO, cx);
        cx.notify();
    }

    fn handle_host_update(&mut self, update: HostUpdate, generation: u64, cx: &mut Context<Self>) {
        if generation != self.connection_generation {
            return;
        }
        if self.active_endpoint == ChatEndpoint::Remote {
            match update {
                HostUpdate::Connected { .. } => {
                    self.inactive_model.connected = true;
                    self.inactive_model.connection_error = None;
                    self.connecting = false;
                    self.connection_in_flight = false;
                    self.reconnect_attempt = 0;
                    if let Some(host) = &self.host {
                        let _ = host.tree();
                        let _ = host.agent_catalog();
                        if let Some(chat_id) = self.inactive_model.selected_chat.as_deref() {
                            let _ = host.git_state(chat_id);
                        }
                    }
                }
                HostUpdate::Disconnected { message } => {
                    if self.connection_generation != generation {
                        return;
                    }
                    self.terminal_cache_refresh.insert(ChatEndpoint::Local);
                    self.host = None;
                    self.inactive_model.connected = false;
                    self.inactive_model.connection_error = Some(format!("{message} Reconnecting…"));
                    self.connecting = false;
                    self.connection_in_flight = false;
                    self.schedule_reconnect(cx);
                }
                HostUpdate::Reply {
                    kind,
                    body,
                    attachments,
                } => {
                    if Self::local_admin_reply(&kind) {
                        self.handle_reply(kind, body, attachments, cx);
                    } else {
                        let value = Value::Object(body);
                        let tree = matches!(&kind, RequestKind::Tree);
                        if !self.handle_workspace_create_reply(
                            ChatEndpoint::Local,
                            &kind,
                            &value,
                            cx,
                        ) && !self.handle_cached_terminal_reply(
                            ChatEndpoint::Local,
                            &kind,
                            &value,
                        ) {
                            Self::apply_passive_reply(&mut self.inactive_model, &kind, value);
                            if tree {
                                self.prime_terminal_cache(ChatEndpoint::Local);
                            }
                        }
                    }
                }
                HostUpdate::Event {
                    name,
                    body,
                    attachments,
                } => {
                    let body = Value::Object(body);
                    if Self::local_admin_event(&name) {
                        self.handle_event(&name, body, attachments, cx);
                    } else {
                        if name == "folder-clone" {
                            self.handle_folder_clone_event(ChatEndpoint::Local, &body);
                        } else {
                            if Self::is_terminal_screen_event(&name) {
                                self.handle_terminal_screen_event(
                                    ChatEndpoint::Local,
                                    &name,
                                    &body,
                                    cx,
                                );
                            }
                            Self::apply_passive_event(&mut self.inactive_model, &name, &body);
                        }
                        if name == "turn-finished"
                            && let Some(host) = &self.host
                        {
                            let _ = host.tree();
                        }
                    }
                }
            }
            cx.notify();
            return;
        }
        match update {
            HostUpdate::Connected { .. } => {
                self.model.connected = true;
                self.connecting = false;
                self.connection_in_flight = false;
                self.reconnect_attempt = 0;
                self.model.connection_error = None;
                self.request_tree();
                self.request_agent_catalog();
                self.refresh_selected_chat_after_connect(cx);
            }
            HostUpdate::Disconnected { message } => {
                if self.connection_generation != generation {
                    return;
                }
                self.terminal_cache_refresh.insert(ChatEndpoint::Local);
                self.host = None;
                self.model.connected = false;
                self.model.connection_error = Some(format!("{message} Reconnecting…"));
                self.sending = false;
                self.transcript_loading = false;
                self.transcript_page_loading = false;
                self.transcript_refresh_pending = false;
                self.workspace_create_submitting = false;
                self.chat_create_submitting = false;
                self.workspace_context_loading = false;
                self.workspace_context_submitting = false;
                self.workspace_clone_status = None;
                self.pending_clone_requests.remove(&ChatEndpoint::Local);
                self.workspace_clone_outcomes
                    .retain(|(endpoint, _), _| *endpoint != ChatEndpoint::Local);
                self.pending_speech = None;
                Arc::make_mut(&mut self.workflow_pending).clear();
                self.speech_output.stop();
                if let Some(defaults) = &mut self.workspace_defaults {
                    defaults.loading = false;
                    defaults.submitting = false;
                }
                if let Some(diff) = &mut self.diff_panel {
                    diff.loading = false;
                    diff.status_loading = false;
                    diff.action = None;
                    diff.file_loading = false;
                }
                if let Some(panel) = &mut self.terminal_panel {
                    panel.loading = false;
                    panel.finish_opening();
                    panel.error = Some("Terminal disconnected.".into());
                }
                if let Some(panel) = &mut self.secrets_panel {
                    panel.loading = false;
                    panel.submitting = false;
                    panel.error = Some("Agent secrets disconnected.".into());
                }
                if let Some(panel) = &mut self.devices_panel {
                    panel.loading = false;
                    panel.mutating = None;
                    panel.error = Some("Paired devices disconnected.".into());
                }
                if let Some(panel) = &mut self.share_panel {
                    panel.loading = false;
                    panel.error = Some("Device pairing disconnected.".into());
                }
                if let Some(panel) = &mut self.shortcut_panel {
                    panel.loading = false;
                    panel.submitting = false;
                    panel.error = Some("Shortcut management disconnected.".into());
                }
                if let Some(panel) = &mut self.self_update_panel {
                    panel.busy = false;
                    panel.error = Some("The state connection closed. Reconnecting…".into());
                }
                if self.auth_open {
                    self.cli_versions_loading = false;
                    self.cli_versions_error = Some("Assistant versions disconnected.".into());
                }
                self.restore_pending_send(cx);
                self.schedule_reconnect(cx);
            }
            HostUpdate::Reply {
                kind,
                body,
                attachments,
            } => self.handle_reply(kind, body, attachments, cx),
            HostUpdate::Event {
                name,
                body,
                attachments,
            } => self.handle_event(&name, Value::Object(body), attachments, cx),
        }
        cx.notify();
    }

    fn handle_reply(
        &mut self,
        kind: RequestKind,
        body: serde_json::Map<String, Value>,
        attachments: Option<Vec<Attachment>>,
        cx: &mut Context<Self>,
    ) {
        let value = Value::Object(body);
        if self.handle_workspace_create_reply(self.active_endpoint, &kind, &value, cx) {
            return;
        }
        if self.handle_cached_terminal_reply(self.active_endpoint, &kind, &value) {
            return;
        }
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            if !matches!(
                &kind,
                RequestKind::DiffRead { .. }
                    | RequestKind::GitStatus { .. }
                    | RequestKind::GitState { .. }
                    | RequestKind::GitDraft { .. }
                    | RequestKind::GitPullRequestStatus { .. }
                    | RequestKind::GitPullRequestCreate { .. }
                    | RequestKind::FileBrowseList { .. }
                    | RequestKind::FileBrowseRead { .. }
                    | RequestKind::FileBrowseWrite { .. }
                    | RequestKind::FileTreeList { .. }
                    | RequestKind::GitCommit { .. }
                    | RequestKind::GitPush { .. }
                    | RequestKind::TerminalOpen { .. }
                    | RequestKind::TerminalList { .. }
                    | RequestKind::TerminalInput { .. }
                    | RequestKind::TerminalPasteImage { .. }
                    | RequestKind::TerminalResize { .. }
                    | RequestKind::TerminalKill { .. }
                    | RequestKind::AgentSecrets { .. }
                    | RequestKind::SetAgentSecrets { .. }
                    | RequestKind::AgentClis
                    | RequestKind::HostUpdate { .. }
                    | RequestKind::Devices
                    | RequestKind::PeerPairing
                    | RequestKind::RenameDevice { .. }
                    | RequestKind::RevokeDevice { .. }
                    | RequestKind::WorkflowStatus { .. }
                    | RequestKind::ImageRead { .. }
                    | RequestKind::Shortcuts { .. }
                    | RequestKind::SetShortcuts { .. }
            ) {
                self.model.connection_error = Some(
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("The xd host rejected the request.")
                        .to_owned(),
                );
            }
            match &kind {
                RequestKind::Send { .. } => {
                    self.sending = false;
                    self.restore_pending_send(cx);
                }
                RequestKind::QueueMutation { chat_id }
                | RequestKind::Cancel { chat_id }
                | RequestKind::SetOption { chat_id }
                    if self.chat_is_active(chat_id) =>
                {
                    self.request_chat(chat_id);
                }
                RequestKind::Messages { chat_id, .. } if self.chat_is_active(chat_id) => {
                    self.transcript_loading = false;
                    self.transcript_page_loading = false;
                    self.transcript_refresh_pending = false;
                    if self
                        .pending_speech
                        .as_ref()
                        .is_some_and(|pending| pending.chat_id == *chat_id)
                    {
                        self.pending_speech = None;
                    }
                }
                RequestKind::NewChat {
                    folder_id, title, ..
                } if self.creating_chat_folder.as_deref() == Some(folder_id)
                    && self.chat_create_title.trim() == title =>
                {
                    self.chat_create_submitting = false;
                }
                RequestKind::FolderContext { folder_id }
                    if self.workspace_context_folder.as_deref() == Some(folder_id) =>
                {
                    self.workspace_context_loading = false;
                }
                RequestKind::SetFolderContext { folder_id, .. }
                    if self.workspace_context_folder.as_deref() == Some(folder_id) =>
                {
                    self.workspace_context_submitting = false;
                }
                RequestKind::FolderSettings { folder_id }
                    if self
                        .workspace_defaults
                        .as_ref()
                        .is_some_and(|defaults| &defaults.folder_id == folder_id) =>
                {
                    if let Some(defaults) = &mut self.workspace_defaults {
                        defaults.loading = false;
                    }
                }
                RequestKind::FolderSettings { folder_id }
                    if self.creating_chat_folder.as_deref() == Some(folder_id.as_str()) =>
                {
                    self.chat_create_worktrees_loading = false;
                }
                RequestKind::SetFolderSettings { folder_id }
                    if self
                        .workspace_defaults
                        .as_ref()
                        .is_some_and(|defaults| &defaults.folder_id == folder_id) =>
                {
                    if let Some(defaults) = &mut self.workspace_defaults {
                        defaults.submitting = false;
                    }
                }
                RequestKind::Shortcuts { folder_id }
                    if self
                        .shortcut_panel
                        .as_ref()
                        .is_some_and(|panel| panel.folder_id == *folder_id) =>
                {
                    if let Some(panel) = &mut self.shortcut_panel {
                        panel.loading = false;
                        panel.error = Some(
                            value
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("Cannot load shortcuts.")
                                .to_owned(),
                        );
                    }
                }
                RequestKind::SetShortcuts { folder_id }
                    if self
                        .shortcut_panel
                        .as_ref()
                        .is_some_and(|panel| panel.folder_id == *folder_id) =>
                {
                    if let Some(panel) = &mut self.shortcut_panel {
                        panel.submitting = false;
                        panel.error = Some(
                            value
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("Cannot save shortcuts.")
                                .to_owned(),
                        );
                    }
                }
                RequestKind::WorkflowStatus { marker } => {
                    Arc::make_mut(&mut self.workflow_pending).remove(marker);
                    Arc::make_mut(&mut self.workflow_statuses)
                        .insert(marker.clone(), value.clone());
                    self.invalidate_workflow_rows(marker);
                    self.schedule_workflow_refresh(marker.clone(), cx);
                }
                RequestKind::DiffRead {
                    path, generation, ..
                } if *generation == self.diff_generation => {
                    let message = value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("Cannot read repository changes.")
                        .to_owned();
                    if let Some(path) = path {
                        if let Some(file) = self
                            .diff_panel
                            .as_mut()
                            .and_then(|diff| diff.files.iter_mut().find(|file| &file.path == path))
                        {
                            file.loading = false;
                            file.error = Some(message);
                        }
                    } else if let Some(diff) = &mut self.diff_panel {
                        diff.loading = false;
                        diff.error = Some(message);
                    }
                }
                RequestKind::GitStatus { generation, .. }
                    if *generation == self.diff_generation =>
                {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.status_loading = false;
                        diff.action_error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::FileBrowseList { generation, .. }
                    if *generation == self.diff_generation =>
                {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.loading = false;
                        diff.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::FileBrowseRead { generation, .. }
                    if *generation == self.diff_generation =>
                {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.file_loading = false;
                        diff.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::FileBrowseWrite { generation, .. }
                    if *generation == self.diff_generation =>
                {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.file_loading = false;
                        diff.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        if let Some(preview) = &mut diff.file_preview {
                            preview.saving = false;
                        }
                    }
                }
                RequestKind::FileTreeList {
                    chat_id,
                    path,
                    generation,
                } if *generation == self.tree_generation
                    && self.model.selected_chat.as_deref() == Some(chat_id.as_str()) =>
                {
                    self.file_tree.set_failed(path);
                }
                RequestKind::GitCommit { generation, .. }
                | RequestKind::GitPush { generation, .. }
                    if *generation == self.diff_generation =>
                {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.action = None;
                        diff.action_error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::GitDraft { generation, .. } if *generation == self.diff_generation => {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.action = None;
                        diff.action_error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::GitPullRequestStatus { generation, .. }
                    if *generation == self.diff_generation =>
                {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.pr_loading = false;
                        diff.pr_url = None;
                    }
                }
                RequestKind::GitPullRequestCreate { generation, .. }
                    if *generation == self.diff_generation =>
                {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.action = None;
                        diff.action_error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::TerminalOpen { chat_id, agent, .. }
                    if self.terminal_panel.as_ref().is_some_and(|panel| {
                        &panel.chat_id == chat_id
                            && panel.opening_matches_protocol_agent(agent.as_deref())
                    }) =>
                {
                    if let Some(panel) = &mut self.terminal_panel {
                        panel.loading = false;
                        panel.finish_opening();
                        panel.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::TerminalList { chat_id }
                    if self
                        .terminal_panel
                        .as_ref()
                        .is_some_and(|panel| &panel.chat_id == chat_id) =>
                {
                    if let Some(panel) = &mut self.terminal_panel {
                        panel.loading = false;
                        panel.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::TerminalInput { terminal_id }
                | RequestKind::TerminalPasteImage { terminal_id }
                | RequestKind::TerminalResize { terminal_id }
                | RequestKind::TerminalKill { terminal_id }
                    if self.terminal_panel.as_ref().is_some_and(|panel| {
                        panel
                            .sessions
                            .iter()
                            .any(|session| &session.id == terminal_id)
                    }) =>
                {
                    if let Some(panel) = &mut self.terminal_panel {
                        panel.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::AgentSecrets { folder_id }
                    if self
                        .secrets_panel
                        .as_ref()
                        .is_some_and(|panel| panel.folder_id == *folder_id) =>
                {
                    if let Some(panel) = &mut self.secrets_panel {
                        panel.loading = false;
                        panel.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::SetAgentSecrets { folder_id }
                    if self
                        .secrets_panel
                        .as_ref()
                        .is_some_and(|panel| panel.folder_id == *folder_id) =>
                {
                    if let Some(panel) = &mut self.secrets_panel {
                        panel.submitting = false;
                        panel.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::Devices => {
                    if let Some(panel) = &mut self.devices_panel {
                        panel.loading = false;
                        panel.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::PeerPairing => {
                    if let Some(panel) = &mut self.share_panel {
                        panel.loading = false;
                        panel.host.clear();
                        panel.port = None;
                        panel.code.clear();
                        panel.error = Some(
                            value
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("Could not create a pairing code.")
                                .to_owned(),
                        );
                    }
                }
                RequestKind::AgentClis => {
                    self.cli_versions_loading = false;
                    self.cli_versions_error = value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                RequestKind::HostUpdate { action: _ } => {
                    if let Some(panel) = &mut self.self_update_panel {
                        panel.busy = false;
                        panel.error = Some(
                            value
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("The host update request failed.")
                                .to_owned(),
                        );
                    }
                }
                RequestKind::RenameDevice { device_id }
                | RequestKind::RevokeDevice { device_id } => {
                    if let Some(panel) = &mut self.devices_panel
                        && panel.mutating.as_ref() == Some(device_id)
                    {
                        panel.mutating = None;
                        panel.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::EditQueue {
                    chat_id,
                    index,
                    old_text,
                    new_text,
                } => {
                    if let Some(edit) = &mut self.queue_edit
                        && edit.chat_id == *chat_id
                        && edit.index == *index
                        && edit.original == *old_text
                        && edit.submitting.as_deref() == Some(new_text.as_str())
                    {
                        edit.submitting = None;
                    }
                    if self.chat_is_active(chat_id) {
                        self.request_chat(chat_id);
                    }
                }
                RequestKind::RenameFolder { folder_id, .. } => {
                    if let Some(edit) = &mut self.sidebar_edit
                        && edit.target == SidebarTarget::Folder(folder_id.clone())
                    {
                        edit.submitting = false;
                    }
                    self.request_tree();
                }
                RequestKind::MoveFolder { folder_id, .. } => {
                    if self.sidebar_move.as_ref() == Some(&SidebarTarget::Folder(folder_id.clone()))
                    {
                        self.sidebar_move_submitting = false;
                        self.sidebar_move_destination = None;
                    }
                    self.request_tree();
                }
                RequestKind::RenameChat { chat_id, .. } => {
                    if let Some(edit) = &mut self.sidebar_edit
                        && edit.target == SidebarTarget::Chat(chat_id.clone())
                    {
                        edit.submitting = false;
                    }
                    self.request_tree();
                }
                RequestKind::MoveChat { chat_id, .. } => {
                    if self.sidebar_move.as_ref() == Some(&SidebarTarget::Chat(chat_id.clone())) {
                        self.sidebar_move_submitting = false;
                        self.sidebar_move_destination = None;
                    }
                    self.request_tree();
                }
                RequestKind::TrashFolder { folder_id } => {
                    if self.pending_sidebar_delete.as_ref()
                        == Some(&SidebarTarget::Folder(folder_id.clone()))
                    {
                        self.sidebar_delete_submitting = false;
                    }
                    self.request_tree();
                }
                RequestKind::DeleteChat { chat_id } => {
                    if self.pending_sidebar_delete.as_ref()
                        == Some(&SidebarTarget::Chat(chat_id.clone()))
                    {
                        self.sidebar_delete_submitting = false;
                    }
                    self.request_tree();
                }
                _ => {}
            }
            return;
        }

        match kind {
            RequestKind::Tree => {
                let selected_before = self.model.selected_chat.clone();
                if let Err(error) = self.model.apply_tree(&value) {
                    self.model.connection_error = Some(format!("Invalid tree response: {error}"));
                    return;
                }
                if selected_before.is_some() && self.model.selected_chat.is_none() {
                    self.invalidate_live_render();
                    self.transcript_snapshot = TranscriptSnapshot::default();
                    self.transcript_loaded = false;
                    self.transcript.reset(0);
                }
                if self
                    .sidebar_edit
                    .as_ref()
                    .is_some_and(|edit| sidebar_edit_applied(&self.model, edit))
                {
                    self.cancel_sidebar_edit(cx);
                }
                if self
                    .sidebar_edit
                    .as_ref()
                    .is_some_and(|edit| !self.sidebar_target_exists(&edit.target))
                {
                    self.cancel_sidebar_edit(cx);
                }
                if self
                    .pending_sidebar_delete
                    .as_ref()
                    .is_some_and(|target| !self.sidebar_target_exists(target))
                {
                    self.pending_sidebar_delete = None;
                    self.sidebar_delete_submitting = false;
                }
                if self
                    .sidebar_move
                    .as_ref()
                    .zip(self.sidebar_move_destination.as_ref())
                    .is_some_and(|(target, destination)| {
                        sidebar_move_applied(&self.model, target, destination.as_deref())
                    })
                {
                    self.sidebar_move = None;
                    self.sidebar_move_submitting = false;
                    self.sidebar_move_destination = None;
                }
                if self
                    .sidebar_move
                    .as_ref()
                    .is_some_and(|target| !self.sidebar_target_exists(target))
                {
                    self.sidebar_move = None;
                    self.sidebar_move_submitting = false;
                    self.sidebar_move_destination = None;
                }
                self.collapsed_folders.retain(|folder_id| {
                    self.model
                        .folders
                        .iter()
                        .any(|folder| &folder.id == folder_id)
                });
                self.persist_collapsed_folders();
                if self
                    .workspace_context_folder
                    .as_ref()
                    .is_some_and(|folder_id| {
                        !self
                            .model
                            .folders
                            .iter()
                            .any(|folder| &folder.id == folder_id)
                    })
                {
                    self.cancel_workspace_context(cx);
                }
                if self.workspace_defaults.as_ref().is_some_and(|defaults| {
                    !self
                        .model
                        .folders
                        .iter()
                        .any(|folder| folder.id == defaults.folder_id)
                }) {
                    self.workspace_defaults = None;
                }
                if self.secrets_panel.as_ref().is_some_and(|panel| {
                    panel.folder_id.as_ref().is_some_and(|folder_id| {
                        !self
                            .model
                            .folders
                            .iter()
                            .any(|folder| &folder.id == folder_id)
                    })
                }) {
                    self.close_secrets(cx);
                }
                self.reconcile_minimal_navigation(cx);
                self.prime_terminal_cache(self.active_endpoint);
            }
            RequestKind::AgentCatalog => {
                if let Err(error) = self.model.apply_agent_catalog(&value) {
                    self.model.connection_error =
                        Some(format!("Invalid assistant catalog response: {error}"));
                }
            }
            RequestKind::AgentAuth => self.apply_auth_providers(&value),
            RequestKind::AgentAuthMutation => {}
            RequestKind::AgentClis => self.apply_cli_versions(&value),
            RequestKind::HostUpdate { action: _ } => {
                self.apply_self_update(&value);
            }
            RequestKind::AgentSecrets { folder_id }
                if self
                    .secrets_panel
                    .as_ref()
                    .is_some_and(|panel| panel.folder_id == folder_id) =>
            {
                match serde_json::from_value::<Vec<String>>(
                    value.get("names").cloned().unwrap_or_default(),
                ) {
                    Ok(mut names) => {
                        names.sort();
                        names.dedup();
                        if let Some(panel) = &mut self.secrets_panel {
                            panel.names = names;
                            panel.loading = false;
                            panel.submitting = false;
                            panel.error = None;
                            panel.name.clear();
                            panel.value.clear();
                        }
                        self.secret_name_input
                            .update(cx, |input, cx| input.set_text("", cx));
                        self.secret_value_input
                            .update(cx, |input, cx| input.set_text("", cx));
                    }
                    Err(error) => {
                        if let Some(panel) = &mut self.secrets_panel {
                            panel.loading = false;
                            panel.error = Some(format!("Invalid agent secrets response: {error}"));
                        }
                    }
                }
            }
            RequestKind::SetAgentSecrets { folder_id }
                if self
                    .secrets_panel
                    .as_ref()
                    .is_some_and(|panel| panel.folder_id == folder_id) =>
            {
                if let Some(panel) = &mut self.secrets_panel {
                    panel.submitting = false;
                    panel.loading = true;
                }
                if let Some(host) = self.secrets_host().cloned()
                    && let Err(error) = host.agent_secrets(folder_id.as_deref())
                    && let Some(panel) = &mut self.secrets_panel
                {
                    panel.loading = false;
                    panel.error = Some(error);
                }
            }
            RequestKind::Devices => {
                match serde_json::from_value::<Vec<Value>>(
                    value.get("devices").cloned().unwrap_or_default(),
                ) {
                    Ok(devices) => {
                        if let Some(panel) = &mut self.devices_panel {
                            panel.devices = devices;
                            panel.loading = false;
                            panel.mutating = None;
                            panel.error = None;
                            panel.revoke_confirmation = None;
                        }
                    }
                    Err(error) => {
                        if let Some(panel) = &mut self.devices_panel {
                            panel.loading = false;
                            panel.error = Some(format!("Invalid paired devices response: {error}"));
                        }
                    }
                }
            }
            RequestKind::PeerPairing => {
                if let Some(panel) = &mut self.share_panel {
                    panel.loading = false;
                    match pairing_details(&value) {
                        Ok((host, port, code)) => {
                            panel.host = host;
                            panel.port = Some(port);
                            panel.code = code;
                            panel.error = None;
                        }
                        Err(error) => {
                            panel.host.clear();
                            panel.port = None;
                            panel.code.clear();
                            panel.error = Some(error);
                        }
                    }
                }
            }
            RequestKind::RenameDevice { device_id } | RequestKind::RevokeDevice { device_id } => {
                if let Some(panel) = &mut self.devices_panel
                    && panel.mutating.as_ref() == Some(&device_id)
                {
                    panel.loading = true;
                    panel.mutating = None;
                    panel.editing_id = None;
                    panel.edit_name.clear();
                    panel.revoke_confirmation = None;
                    panel.error = None;
                }
                self.device_name_input
                    .update(cx, |input, cx| input.set_text("", cx));
                self.request_devices();
            }
            RequestKind::WorkflowStatus { marker } => {
                if value.get("pending").and_then(Value::as_bool) != Some(true) {
                    Arc::make_mut(&mut self.workflow_pending).remove(&marker);
                    Arc::make_mut(&mut self.workflow_statuses)
                        .insert(marker.clone(), value.clone());
                    self.invalidate_workflow_rows(&marker);
                    self.schedule_workflow_refresh(marker, cx);
                }
            }
            RequestKind::DiffRead {
                chat_id,
                read,
                path,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                    || self.diff_panel.is_none()
                {
                    return;
                }
                let output = value
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if read == "base" {
                    let base = output.trim();
                    if base.is_empty() {
                        if let Some(diff) = &mut self.diff_panel {
                            diff.loading = false;
                            diff.error = Some("No branch to compare against.".into());
                        }
                    } else {
                        if let Some(diff) = &mut self.diff_panel {
                            diff.base = Some(base.to_owned());
                        }
                        if let Some(host) = self.active_host().cloned()
                            && let Err(error) = host.diff_read(
                                &chat_id,
                                "branch-status",
                                Some(base),
                                None,
                                generation,
                            )
                            && let Some(diff) = &mut self.diff_panel
                        {
                            diff.loading = false;
                            diff.error = Some(error);
                        }
                    }
                } else if matches!(read.as_str(), "working-status" | "branch-status") {
                    self.prepare_diff_listing(output, read == "branch-status", generation, cx);
                } else if matches!(
                    read.as_str(),
                    "working-file" | "untracked-file" | "branch-file"
                ) {
                    if let Some(path) = path {
                        self.prepare_diff_file(path, output, generation, cx);
                    }
                } else {
                    self.prepare_diff(output, generation, cx);
                }
            }
            RequestKind::GitStatus {
                chat_id,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                {
                    return;
                }
                match serde_json::from_value::<GitStatus>(value.clone()) {
                    Ok(status) => {
                        let check_pull_request = status.can_open_pull_request();
                        if let Some(diff) = &mut self.diff_panel {
                            diff.status = Some(status);
                            diff.status_loading = false;
                            diff.action_error = None;
                            diff.pr_loading = check_pull_request;
                            if !check_pull_request {
                                diff.pr_url = None;
                            }
                        }
                        if check_pull_request
                            && let Some(host) = self.active_host().cloned()
                            && let Err(error) = host.git_pull_request_status(&chat_id, generation)
                            && let Some(diff) = &mut self.diff_panel
                        {
                            diff.pr_loading = false;
                            diff.action_error = Some(error);
                        }
                    }
                    Err(error) => {
                        if let Some(diff) = &mut self.diff_panel {
                            diff.status_loading = false;
                            diff.action_error =
                                Some(format!("Invalid Git status response: {error}"));
                        }
                    }
                }
            }
            RequestKind::GitState { .. } => {}
            RequestKind::FileBrowseList {
                chat_id,
                path,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                    || !self.diff_panel.as_ref().is_some_and(|diff| diff.files_mode)
                {
                    return;
                }
                match serde_json::from_value::<Vec<BrowseEntry>>(
                    value.get("entries").cloned().unwrap_or_default(),
                ) {
                    Ok(entries) => {
                        if let Some(diff) = &mut self.diff_panel {
                            diff.browse_path = path;
                            diff.browse_entries = entries;
                            diff.file_preview = None;
                            diff.loading = false;
                            diff.error = None;
                        }
                    }
                    Err(error) => {
                        if let Some(diff) = &mut self.diff_panel {
                            diff.loading = false;
                            diff.error = Some(format!("Invalid directory listing: {error}"));
                        }
                    }
                }
            }
            RequestKind::FileBrowseRead {
                chat_id,
                path,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                    || !self.diff_panel.as_ref().is_some_and(|diff| diff.files_mode)
                {
                    return;
                }
                let content = value.get("content").and_then(Value::as_str);
                if content.is_none() {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.file_loading = false;
                        diff.error = Some("Invalid file response.".into());
                    }
                    return;
                }
                if let Some(diff) = &mut self.diff_panel {
                    let content = content.unwrap_or_default().to_owned();
                    let editor_path = path.clone();
                    diff.file_preview = Some(FilePreview {
                        path,
                        original: content.clone(),
                        content: content.clone(),
                        truncated: false,
                        saving: false,
                    });
                    diff.file_loading = false;
                    diff.error = None;
                    self.file_editor.update(cx, |editor, cx| {
                        editor.set_file(&editor_path, content, cx);
                    });
                }
            }
            RequestKind::FileTreeList {
                chat_id,
                path,
                generation,
            } => {
                if generation != self.tree_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                {
                    return;
                }
                match serde_json::from_value::<Vec<BrowseEntry>>(
                    value.get("entries").cloned().unwrap_or_default(),
                ) {
                    Ok(entries) => {
                        self.file_tree.set_children(&path, entries);
                        for child in self.file_tree.expanded_unloaded_children(&path) {
                            self.list_tree_directory(child, cx);
                        }
                    }
                    Err(error) => {
                        self.file_tree.set_failed(&path);
                        self.model.connection_error =
                            Some(format!("Invalid directory listing: {error}"));
                    }
                }
            }
            RequestKind::FileTabRead {
                chat_id,
                path,
                generation,
            } => {
                if generation != self.tree_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                {
                    return;
                }
                let Some(content) = value.get("content").and_then(Value::as_str) else {
                    self.model.connection_error = Some("Invalid file response.".into());
                    return;
                };
                self.open_files.open(&path, content.to_owned());
                self.tab_editor.update(cx, |editor, cx| {
                    editor.set_file(&path, content.to_owned(), cx);
                });
            }
            RequestKind::FileTabWrite {
                chat_id,
                path,
                content,
                generation,
            } => {
                if generation != self.tree_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                {
                    return;
                }
                // What was sent, not what is on screen: typing carries on
                // during the round trip and is not part of what landed.
                self.open_files.set_saved(&path, content);
            }
            RequestKind::FileBrowseWrite {
                chat_id,
                path,
                content,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                {
                    return;
                }
                if let Some(preview) = self
                    .diff_panel
                    .as_mut()
                    .and_then(|panel| panel.file_preview.as_mut())
                    && preview.path == path
                    && preview.content == content
                {
                    preview.original = content;
                    preview.saving = false;
                    if let Some(diff) = &mut self.diff_panel {
                        diff.error = None;
                    }
                }
                if self
                    .diff_panel
                    .as_ref()
                    .is_some_and(|diff| diff.status.is_some())
                {
                    self.refresh_git_status();
                }
            }
            RequestKind::GitCommit {
                chat_id,
                message,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                {
                    return;
                }
                if self.git_commit_message.trim() == message {
                    self.git_commit_message.clear();
                    self.git_commit_input
                        .update(cx, |input, cx| input.set_text("", cx));
                }
                if let Some(diff) = &mut self.diff_panel {
                    diff.action = None;
                    diff.action_error = None;
                }
                self.refresh_diff(cx);
            }
            RequestKind::GitPush {
                chat_id,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                {
                    return;
                }
                if let Some(diff) = &mut self.diff_panel {
                    diff.action = None;
                    diff.action_error = None;
                }
                self.refresh_diff(cx);
            }
            RequestKind::GitPullRequestStatus {
                chat_id,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                {
                    return;
                }
                if let Some(diff) = &mut self.diff_panel {
                    diff.pr_loading = false;
                    diff.pr_url = value
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|url| !url.is_empty())
                        .map(str::to_owned);
                }
            }
            RequestKind::GitPullRequestCreate {
                chat_id,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                {
                    return;
                }
                if let Some(diff) = &mut self.diff_panel {
                    diff.action = None;
                    diff.action_error = None;
                    diff.pr_url = value
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|url| !url.is_empty())
                        .map(str::to_owned);
                    diff.pr_title = None;
                    diff.pr_body.clear();
                }
            }
            RequestKind::Shortcuts { folder_id } => {
                if folder_id.as_ref().is_some_and(|folder_id| {
                    self.model
                        .selected_summary()
                        .is_some_and(|chat| chat.folder == *folder_id)
                }) {
                    self.model.apply_shortcuts(&value);
                }
                if self
                    .shortcut_panel
                    .as_ref()
                    .is_some_and(|panel| panel.folder_id == folder_id)
                {
                    let key = if folder_id.is_some() {
                        "workspace"
                    } else {
                        "global"
                    };
                    let prompts = value
                        .get(key)
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    self.replace_shortcut_rows(prompts, cx);
                    if let Some(panel) = &mut self.shortcut_panel {
                        panel.loading = false;
                        panel.error = None;
                    }
                }
            }
            RequestKind::FolderContext { folder_id } => {
                if self.workspace_context_folder.as_deref() == Some(folder_id.as_str()) {
                    self.workspace_context_loading = false;
                    self.workspace_context_text = value
                        .get("context")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let text = self.workspace_context_text.clone();
                    self.workspace_context_input
                        .update(cx, |input, cx| input.set_text(text, cx));
                }
            }
            RequestKind::SetFolderContext { folder_id, context } => {
                if self.workspace_context_folder.as_deref() == Some(folder_id.as_str())
                    && optional_trimmed(&self.workspace_context_text) == context.as_deref()
                {
                    self.cancel_workspace_context(cx);
                }
                self.request_tree();
            }
            RequestKind::FolderSettings { folder_id } => {
                if self.creating_chat_folder.as_deref() == Some(folder_id.as_str()) {
                    let (worktrees, can_create, selected) = new_session_worktree_state(&value);
                    self.chat_create_worktrees = worktrees;
                    self.chat_create_can_new_worktree = can_create;
                    self.chat_create_worktree = selected;
                    self.chat_create_worktrees_loading = false;
                }
                if self
                    .workspace_defaults
                    .as_ref()
                    .is_some_and(|defaults| defaults.folder_id == folder_id)
                {
                    let workdir = value
                        .get("workdir")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let repo = value
                        .get("repo")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    self.workspace_defaults = Some(WorkspaceDefaults {
                        folder_id: folder_id.clone(),
                        workdir: workdir.clone(),
                        repo: repo.clone(),
                        loading: false,
                        submitting: false,
                    });
                    self.workspace_workdir_input
                        .update(cx, |input, cx| input.set_text(workdir, cx));
                    self.workspace_repo_default_input
                        .update(cx, |input, cx| input.set_text(repo, cx));
                }
            }
            RequestKind::SetFolderSettings { folder_id } => {
                if self
                    .workspace_defaults
                    .as_ref()
                    .is_some_and(|defaults| defaults.folder_id == folder_id)
                {
                    self.workspace_defaults = None;
                }
                self.request_tree();
            }
            RequestKind::NewChat {
                folder_id, title, ..
            } => {
                let Some(chat_id) = value.get("id").and_then(Value::as_str) else {
                    self.chat_create_submitting = false;
                    self.model.connection_error = Some("The host returned no chat id.".into());
                    return;
                };
                if self.creating_chat_folder.as_deref() == Some(folder_id.as_str())
                    && self.chat_create_title.trim() == title
                {
                    self.cancel_chat_create(cx);
                }
                self.request_tree();
                let chat_id = chat_id.to_owned();
                if let Some(agent) = value
                    .get("backend")
                    .and_then(Value::as_str)
                    .and_then(AgentCli::from_backend)
                {
                    self.select_minimal_session(folder_id, chat_id, agent, cx);
                } else {
                    self.pending_minimal_session = Some((folder_id, chat_id));
                }
            }
            RequestKind::RenameFolder { .. } => self.request_tree(),
            RequestKind::MoveFolder { .. } => self.request_tree(),
            RequestKind::TrashFolder { .. } => self.request_tree(),
            RequestKind::RenameChat { .. } => self.request_tree(),
            RequestKind::MoveChat { .. } => self.request_tree(),
            RequestKind::DeleteChat { .. } => self.request_tree(),
            RequestKind::Chat { chat_id } if self.chat_is_active(&chat_id) => {
                let local_attachments = self
                    .attachments_dirty
                    .then(|| self.model.draft_attachments.clone());
                self.model.apply_chat(&value);
                self.sync_active_auth_state();
                self.transcript_snapshot.sync_live_items(&self.model);
                self.transcript_snapshot.sync_live_text(&self.model);
                self.sync_transcript_count(false);
                if let Some(local_attachments) = local_attachments {
                    self.model.draft_attachments = local_attachments;
                } else if let Some(attachments) = attachments {
                    self.model.draft_attachments = attachments;
                }
                if !self.draft_dirty {
                    let draft = self.model.draft.clone();
                    self.set_composer_text(draft, cx);
                }
                if self.model.working || !self.model.queue.is_empty() {
                    self.clear_question(cx);
                }
                if self
                    .terminal_panel
                    .as_ref()
                    .is_some_and(|panel| panel.chat_id == chat_id && panel.sessions.is_empty())
                {
                    self.refresh_terminal_sessions(cx);
                }
            }
            RequestKind::Messages { chat_id, cursor } if self.chat_is_active(&chat_id) => {
                let direction = match cursor {
                    MessageCursor::Tail => MessagePageDirection::Tail,
                    MessageCursor::Before(_) => MessagePageDirection::Before,
                    MessageCursor::After(_) => MessagePageDirection::After,
                };
                let old_message_count = self.model.messages.len();
                let change = match self.model.apply_message_page(&value, direction) {
                    Ok(change) => change,
                    Err(error) => {
                        self.model.connection_error =
                            Some(format!("Invalid transcript response: {error}"));
                        self.transcript_loading = false;
                        self.transcript_page_loading = false;
                        self.transcript_refresh_pending = false;
                        return;
                    }
                };
                self.transcript_has_older = change.has_older;
                self.transcript_has_newer = change.has_newer;
                if !self.model.working {
                    self.model.live_text.clear();
                    self.model.live_items.clear();
                }
                self.transcript_snapshot.sync_messages(&self.model);
                self.transcript_snapshot.sync_live_items(&self.model);
                if !self.model.working {
                    self.transcript_snapshot.sync_live_text(&self.model);
                }
                if !self.transcript_has_newer {
                    self.sync_question_from_history(&chat_id, cx);
                }
                if self
                    .pending_speech
                    .as_ref()
                    .is_some_and(|pending| pending.chat_id == chat_id)
                    && let Some(pending) = self.pending_speech.take()
                {
                    if self.settings.speech
                        && let Some(message) = self.model.messages.iter().rev().find(|message| {
                            message.role == "assistant"
                                && message.id.is_some()
                                && message.id != pending.previous_assistant_id
                        })
                        && let Some(text) = markdown::spoken_text(&message.content)
                    {
                        self.speech_output.speak(&text);
                    }
                }
                match direction {
                    MessagePageDirection::Tail => {
                        self.transcript.reset(self.model.display_message_count());
                    }
                    MessagePageDirection::Before => {
                        if change.inserted_at_start > 0 {
                            self.transcript.splice(0..0, change.inserted_at_start);
                        }
                        if change.removed_from_end > 0 {
                            let end = old_message_count + change.inserted_at_start;
                            self.transcript
                                .splice(end - change.removed_from_end..end, 0);
                        }
                        self.sync_transcript_count(false);
                    }
                    MessagePageDirection::After => {
                        if change.inserted_at_end > 0 {
                            self.transcript.splice(
                                old_message_count..old_message_count,
                                change.inserted_at_end,
                            );
                        }
                        if change.removed_from_start > 0 {
                            self.transcript.splice(0..change.removed_from_start, 0);
                        }
                        self.sync_transcript_count(false);
                    }
                }
                self.transcript_loading = false;
                self.transcript_page_loading = false;
                self.transcript_loaded = true;
                self.request_workflow_statuses();
                if std::mem::take(&mut self.transcript_refresh_pending) {
                    self.request_messages(&chat_id);
                }
            }
            RequestKind::Send { chat_id, text } if self.chat_is_active(&chat_id) => {
                self.sending = false;
                let queued = value
                    .get("queued")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(pending) = self.pending_send.take() {
                    let predicted_queued =
                        matches!(pending.optimistic, OptimisticSend::Queued { .. });
                    if predicted_queued != queued {
                        self.rollback_optimistic_send(&pending);
                        if !queued {
                            let _ = self.apply_optimistic_send(&pending.text);
                        }
                    }
                }
                if !queued {
                    self.model.start_working();
                    self.request_messages(&chat_id);
                }
                if self.composer.is_empty() {
                    self.draft_dirty = false;
                }
                let _ = text;
            }
            // Queue events carry the authoritative complete queue. Mutation
            // replies are acknowledgements only, so they never refetch chat.
            RequestKind::QueueMutation { chat_id } if self.chat_is_active(&chat_id) => {}
            RequestKind::EditQueue {
                chat_id,
                index,
                old_text,
                new_text,
            } if self.chat_is_active(&chat_id) => {
                if self.queue_edit.as_ref().is_some_and(|edit| {
                    edit.chat_id == chat_id
                        && edit.index == index
                        && edit.original == old_text
                        && edit.submitting.as_deref() == Some(new_text.as_str())
                }) {
                    self.cancel_queue_edit(cx);
                }
            }
            RequestKind::Cancel { chat_id } if self.chat_is_active(&chat_id) => {}
            RequestKind::SetOption { chat_id } if self.chat_is_active(&chat_id) => {
                self.request_chat(&chat_id);
            }
            // The authoritative shortcuts-changed event follows this reply
            // and refetches the active workspace's merged shortcut state.
            RequestKind::SetShortcuts { folder_id } => {
                if self
                    .shortcut_panel
                    .as_ref()
                    .is_some_and(|panel| panel.folder_id == folder_id && panel.submitting)
                {
                    self.shortcut_panel = None;
                }
            }
            RequestKind::RemoveWorktree { chat_id } if self.chat_is_active(&chat_id) => {
                self.request_chat(&chat_id);
            }
            RequestKind::SetDraft {
                chat_id,
                text,
                attachment_generation,
            } if self.chat_is_active(&chat_id) => {
                let attachment_reply_is_current = attachment_generation
                    .is_some_and(|generation| generation == self.attachment_generation);
                let local_attachments = (!attachment_reply_is_current && self.attachments_dirty)
                    .then(|| self.model.draft_attachments.clone());
                self.model.apply_draft_snapshot(&value);
                if let Some(local_attachments) = local_attachments {
                    self.model.draft_attachments = local_attachments;
                } else if attachment_reply_is_current && let Some(attachments) = attachments {
                    self.model.draft_attachments = attachments;
                }
                if self.composer == text {
                    self.draft_dirty = false;
                }
                if attachment_reply_is_current {
                    self.attachments_dirty = false;
                }
            }
            RequestKind::TerminalOpen { chat_id, agent, .. }
                if self.terminal_panel.as_ref().is_some_and(|panel| {
                    panel.chat_id == chat_id
                        && panel.opening_matches_protocol_agent(agent.as_deref())
                }) =>
            {
                if let Some(panel) = &mut self.terminal_panel {
                    panel.selected = value.get("id").and_then(Value::as_str).map(str::to_owned);
                    panel.finish_opening();
                    // The terminal is live as soon as open returns. Keep it
                    // interactive while terminal-list hydrates scrollback;
                    // loading here both lied with "Starting CLI" and disabled
                    // input for the duration of a remote replay transfer.
                    panel.loading = false;
                }
            }
            RequestKind::TerminalList { chat_id }
                if self
                    .terminal_panel
                    .as_ref()
                    .is_some_and(|panel| panel.chat_id == chat_id) =>
            {
                self.apply_terminal_list(&value, cx);
            }
            RequestKind::TerminalPasteImage { terminal_id } => {
                let result = value
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "The host did not return the pasted image path.".to_owned())
                    .and_then(|path| {
                        let bracketed = self
                            .terminal_panel
                            .as_ref()
                            .and_then(|panel| {
                                panel
                                    .sessions
                                    .iter()
                                    .find(|session| session.id == terminal_id)
                            })
                            .is_some_and(|session| session.screen.bracketed_paste());
                        self.terminal_runtime
                            .input(&terminal_id, &terminal_paste_bytes(path, bracketed))
                    });
                if let Err(error) = result
                    && let Some(panel) = &mut self.terminal_panel
                {
                    panel.error = Some(error);
                }
            }
            RequestKind::TerminalInput { .. } | RequestKind::TerminalResize { .. } => {}
            RequestKind::TerminalKill { .. } => {}
            _ => {}
        }
    }

    fn handle_cached_terminal_reply(
        &mut self,
        endpoint: ChatEndpoint,
        kind: &RequestKind,
        value: &Value,
    ) -> bool {
        match kind {
            RequestKind::TerminalList { chat_id } => {
                if endpoint == self.active_endpoint
                    && self
                        .terminal_panel
                        .as_ref()
                        .is_some_and(|panel| &panel.chat_id == chat_id)
                {
                    return false;
                }
                let Some(panel) = self
                    .terminal_panel_cache
                    .get_mut(&(endpoint, chat_id.clone()))
                else {
                    return false;
                };
                if value.get("ok").and_then(Value::as_bool) == Some(true) {
                    Self::merge_terminal_list(panel, value);
                } else {
                    panel.loading = false;
                    panel.error = value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                true
            }
            RequestKind::TerminalOpen { chat_id, agent, .. } => {
                if endpoint == self.active_endpoint
                    && self.terminal_panel.as_ref().is_some_and(|panel| {
                        &panel.chat_id == chat_id
                            && panel.opening_matches_protocol_agent(agent.as_deref())
                    })
                {
                    return false;
                }
                let Some(panel) = self
                    .terminal_panel_cache
                    .get_mut(&(endpoint, chat_id.clone()))
                    .filter(|panel| panel.opening_matches_protocol_agent(agent.as_deref()))
                else {
                    return false;
                };
                panel.selected = value.get("id").and_then(Value::as_str).map(str::to_owned);
                panel.finish_opening();
                panel.loading = false;
                panel.error = value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                true
            }
            _ => false,
        }
    }

    fn handle_workspace_create_reply(
        &mut self,
        endpoint: ChatEndpoint,
        kind: &RequestKind,
        value: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let RequestKind::NewFolder {
            name,
            repo,
            repo_url,
        } = kind
        else {
            return false;
        };
        let form_matches = endpoint == self.active_endpoint
            && self.creating_workspace
            && self.workspace_create_name.trim() == name
            && optional_trimmed(&self.workspace_create_repo) == repo.as_deref()
            && optional_trimmed(&self.workspace_create_clone) == repo_url.as_deref();
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            self.pending_clone_requests.remove(&endpoint);
            if form_matches {
                self.workspace_create_submitting = false;
            }
            self.endpoint_model_mut(endpoint).connection_error = Some(
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("The xd host rejected the workspace.")
                    .to_owned(),
            );
            return true;
        }
        let Some(folder_id) = value.get("id").and_then(Value::as_str) else {
            self.pending_clone_requests.remove(&endpoint);
            self.endpoint_model_mut(endpoint).connection_error =
                Some("The host returned no workspace id.".into());
            return true;
        };
        if form_matches {
            self.cancel_workspace_create(cx);
        }
        self.pending_clone_requests.remove(&endpoint);
        let key = (endpoint, folder_id.to_owned());
        if value.get("cloning").and_then(Value::as_str).is_some() {
            match self.workspace_clone_outcomes.remove(&key) {
                Some(None) => {
                    if endpoint == self.active_endpoint {
                        self.workspace_clone_status = Some("Repository cloned".into());
                    }
                }
                Some(Some(error)) => {
                    if endpoint == self.active_endpoint {
                        self.workspace_clone_status = None;
                    }
                    self.endpoint_model_mut(endpoint).connection_error = Some(error);
                }
                None => {
                    if endpoint == self.active_endpoint {
                        self.workspace_clone_status = Some("Cloning repository…".into());
                    }
                }
            }
        }
        true
    }

    fn handle_folder_clone_event(&mut self, endpoint: ChatEndpoint, body: &Value) {
        let folder_id = body.get("folder").and_then(Value::as_str);
        let key = folder_id.map(|folder_id| (endpoint, folder_id.to_owned()));
        let event_url = body.get("url").and_then(Value::as_str);
        let preserve_outcome = self
            .pending_clone_requests
            .get(&endpoint)
            .is_some_and(|url| Some(url.as_str()) == event_url);
        match body.get("state").and_then(Value::as_str) {
            Some("cloning") => {
                if endpoint == self.active_endpoint {
                    self.workspace_clone_status = Some("Cloning repository…".into());
                }
            }
            Some("ready") => {
                if endpoint == self.active_endpoint {
                    self.workspace_clone_status = Some("Repository cloned".into());
                }
                if preserve_outcome && let Some(key) = &key {
                    self.workspace_clone_outcomes.insert(key.clone(), None);
                }
            }
            Some("failed") => {
                let error = body
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Git could not clone that repository.")
                    .to_owned();
                if preserve_outcome && let Some(key) = &key {
                    self.workspace_clone_outcomes
                        .insert(key.clone(), Some(error.clone()));
                }
                if endpoint == self.active_endpoint {
                    self.workspace_clone_status = None;
                }
                self.endpoint_model_mut(endpoint).connection_error = Some(error);
            }
            _ => {}
        }
    }

    fn is_terminal_screen_event(name: &str) -> bool {
        matches!(
            name,
            "terminal-opened" | "terminal-output" | "terminal-resized" | "terminal-closed"
        )
    }

    fn handle_terminal_screen_event(
        &mut self,
        endpoint: ChatEndpoint,
        name: &str,
        body: &Value,
        cx: &mut Context<Self>,
    ) {
        let Some(chat_id) = body.get("chat").and_then(Value::as_str).map(str::to_owned) else {
            return;
        };
        let active = endpoint == self.active_endpoint
            && self
                .terminal_panel
                .as_ref()
                .is_some_and(|panel| panel.chat_id == chat_id);
        let terminal_id = body.get("terminal").and_then(Value::as_str);
        let follow_output = active
            && self
                .terminal_panel
                .as_ref()
                .is_some_and(|panel| panel.selected.as_deref() == terminal_id)
            && terminal_scroll_is_at_bottom(&self.terminal_scroll);
        let agent = self
            .endpoint_model(endpoint)
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .and_then(|chat| AgentCli::from_backend(&chat.backend));
        let key = (endpoint, chat_id.clone());
        let needs_hydration = !active && !self.terminal_panel_cache.contains_key(&key);
        let panel = if active {
            self.terminal_panel.as_mut().expect("active panel checked")
        } else {
            self.terminal_panel_cache.entry(key).or_insert_with(|| {
                let mut panel = agent
                    .map(|agent| Self::new_agent_terminal_panel(chat_id.clone(), agent))
                    .unwrap_or_else(|| Self::new_terminal_panel(chat_id.clone()));
                panel.auto_open = false;
                panel
            })
        };
        if panel.loading {
            panel.pending_events.push(match name {
                "terminal-opened" => PendingTerminalEvent::Opened(body.clone()),
                "terminal-output" => PendingTerminalEvent::Output(body.clone()),
                "terminal-resized" => PendingTerminalEvent::Resized(body.clone()),
                "terminal-closed" => PendingTerminalEvent::Closed(body.clone()),
                _ => return,
            });
        }
        let changed = match name {
            "terminal-opened" => Self::apply_terminal_opened_event(panel, body),
            "terminal-output" => Self::apply_terminal_output_event(panel, body),
            "terminal-resized" => Self::apply_terminal_resized_event(panel, body),
            "terminal-closed" => Self::apply_terminal_closed_event(panel, body),
            _ => false,
        };
        let terminal_working = (name == "terminal-output" && changed)
            .then(|| {
                let terminal_id = terminal_id?;
                let session = panel
                    .sessions
                    .iter()
                    .find(|session| session.id == terminal_id)?;
                (session.agent == Some(AgentCli::Jcode)).then(|| {
                    jcode_terminal_screen_working(&session.screen.rendered_with_cursor().text)
                })?
            })
            .flatten();
        if needs_hydration {
            panel.loading = false;
        }
        if active {
            if name == "terminal-output" && changed {
                self.terminal_cursor_visible = true;
            }
            if changed && (name == "terminal-opened" || follow_output) {
                self.terminal_scroll.scroll_to_bottom();
            }
            if matches!(
                name,
                "terminal-opened" | "terminal-output" | "terminal-closed"
            ) {
                self.sync_terminal_input_mode(cx);
            }
        }
        if let Some(working) = terminal_working {
            let unchanged = self
                .endpoint_model(endpoint)
                .chats
                .iter()
                .find(|chat| chat.id == chat_id)
                .is_some_and(|chat| chat.terminal_working == working);
            if !unchanged {
                self.endpoint_model_mut(endpoint)
                    .apply_desktop_terminal_activity(&chat_id, working);
            }
        }
    }

    fn handle_event(
        &mut self,
        name: &str,
        body: Value,
        attachments: Option<Vec<Attachment>>,
        cx: &mut Context<Self>,
    ) {
        if Self::is_terminal_screen_event(name) {
            self.handle_terminal_screen_event(self.active_endpoint, name, &body, cx);
        }
        if name == "turn-started" && !self.event_is_active(&body) {
            self.model.apply_event(name, &body);
        }
        if name == "turn-finished" && !self.event_is_active(&body) {
            let title = body
                .get("chat")
                .and_then(Value::as_str)
                .and_then(|chat_id| self.model.chats.iter().find(|chat| chat.id == chat_id))
                .and_then(|chat| chat.title.as_deref())
                .unwrap_or("Background chat")
                .to_owned();
            self.model.apply_event(name, &body);
            if self.settings.notifications {
                notify_turn_finished(&title);
            }
            self.request_tree();
        }
        match name {
            "terminal-activity" => self.model.apply_event(name, &body),
            "workflow-status" => {
                if let Some(marker) = body.get("text").and_then(Value::as_str).map(str::to_owned)
                    && self
                        .model
                        .messages
                        .iter()
                        .chain(self.model.live_items.iter())
                        .any(|message| message.role == "tool" && message.content == marker)
                {
                    Arc::make_mut(&mut self.workflow_pending).remove(&marker);
                    Arc::make_mut(&mut self.workflow_statuses).insert(marker.clone(), body.clone());
                    self.invalidate_workflow_rows(&marker);
                    self.schedule_workflow_refresh(marker, cx);
                }
            }
            "terminal-opened" | "terminal-output" | "terminal-resized" | "terminal-closed" => {}
            "git-draft-finished" if self.event_is_active(&body) => {
                let kind = body.get("kind").and_then(Value::as_str).unwrap_or_default();
                let expected = match kind {
                    "commit" => format!("gpui-{}", self.diff_generation),
                    "pull-request" => format!("gpui-pr-{}", self.diff_generation),
                    _ => String::new(),
                };
                if !expected.is_empty()
                    && body.get("request").and_then(Value::as_str) == Some(expected.as_str())
                {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.action = None;
                        if body.get("success").and_then(Value::as_bool) == Some(true) {
                            diff.action_error = None;
                            let title = body
                                .get("title")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned();
                            if kind == "commit" {
                                self.git_commit_message = title.clone();
                                self.git_commit_input
                                    .update(cx, |input, cx| input.set_text(title, cx));
                            } else {
                                diff.pr_title = Some(title);
                                diff.pr_body = body
                                    .get("body")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned();
                            }
                        } else {
                            diff.action_error = Some(
                                body.get("error")
                                    .and_then(Value::as_str)
                                    .unwrap_or("The assistant could not write a commit message.")
                                    .to_owned(),
                            );
                        }
                    }
                }
            }
            "tree" => self.request_tree(),
            "changed" if self.event_is_active(&body) => {
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_chat(&chat_id);
                }
            }
            "repository-changed" if self.event_is_active(&body) => {
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_chat(&chat_id);
                    if self.diff_panel.is_some() {
                        self.refresh_diff(cx);
                    }
                }
            }
            "draft" if self.event_is_active(&body) => {
                let local_attachments = self
                    .attachments_dirty
                    .then(|| self.model.draft_attachments.clone());
                self.model.apply_event(name, &body);
                if let Some(local_attachments) = local_attachments {
                    self.model.draft_attachments = local_attachments;
                } else if let Some(attachments) = attachments {
                    self.model.draft_attachments = attachments;
                }
                if !self.draft_dirty {
                    let draft = self.model.draft.clone();
                    self.set_composer_text(draft, cx);
                }
            }
            "commands" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
            }
            "turn-started" if self.event_is_active(&body) => {
                self.clear_question(cx);
                self.model.apply_event(name, &body);
                self.invalidate_live_render();
                self.transcript_snapshot.sync_live_text(&self.model);
                self.transcript_snapshot.sync_live_items(&self.model);
                self.sync_transcript_count(false);
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_messages(&chat_id);
                    self.request_chat(&chat_id);
                }
            }
            "text" | "tool" if self.event_is_active(&body) => {
                let old_count = self.model.display_message_count();
                let had_live_text = name == "tool" && !self.model.live_text.is_empty();
                self.model.apply_event(name, &body);
                if name == "text" {
                    self.schedule_live_text_render(cx);
                } else {
                    self.transcript_snapshot.sync_live_items(&self.model);
                    self.transcript_snapshot.sync_live_text(&self.model);
                    let new_count = self.model.display_message_count();
                    let closed_live_text = had_live_text && self.model.live_text.is_empty();
                    if closed_live_text {
                        self.transcript.splice(old_count - 1..old_count, 2);
                    } else if new_count > old_count {
                        self.transcript
                            .splice(old_count..old_count, new_count - old_count);
                    } else if new_count > 0 {
                        self.transcript.splice(new_count - 1..new_count, 1);
                    }
                    // A new tail row can change the count and status displayed
                    // by the run's head, which is a different virtualized row.
                    if new_count > 0 {
                        let run = self.transcript_snapshot.activity_run(new_count - 1);
                        if run.position > 0 {
                            let head = new_count - 1 - run.position;
                            self.transcript.splice(head..head + 1, 1);
                        }
                    }
                    self.request_workflow_statuses();
                }
            }
            "turn-finished" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
                self.open_question = question_from_event(&body);
                self.question_answer.clear();
                self.question_input
                    .update(cx, |input, cx| input.set_text("", cx));
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.pending_speech = self.settings.speech.then(|| PendingSpeech {
                        chat_id: chat_id.clone(),
                        previous_assistant_id: self
                            .model
                            .messages
                            .iter()
                            .rev()
                            .find(|message| message.role == "assistant")
                            .and_then(|message| message.id),
                    });
                    self.request_messages(&chat_id);
                    self.request_chat(&chat_id);
                }
            }
            "queued" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
                let edit_is_stale = self.queue_edit.as_ref().is_some_and(|edit| {
                    edit.submitting.is_none()
                        && self.model.queue.get(edit.index) != Some(&edit.original)
                });
                if edit_is_stale {
                    self.cancel_queue_edit(cx);
                    self.model.connection_error =
                        Some("That queued message changed on another client.".into());
                }
            }
            "shortcuts-changed" => self.request_shortcuts(),
            "folder-clone" => self.handle_folder_clone_event(self.active_endpoint, &body),
            "agent-auth-changed" => self.apply_auth_provider(&body),
            "agent-cli-changed" => self.apply_cli_version(&body),
            "host-update" => self.apply_self_update(&body),
            _ => {}
        }
        cx.notify();
    }

    fn request_tree(&mut self) {
        if let Some(host) = self.active_host() {
            if let Err(error) = host.tree() {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn apply_auth_providers(&mut self, value: &Value) {
        match serde_json::from_value::<Vec<AuthProvider>>(
            value.get("providers").cloned().unwrap_or_default(),
        ) {
            Ok(providers) => {
                self.auth_providers = providers;
                self.sync_active_auth_state();
            }
            Err(error) => {
                self.model.connection_error = Some(format!("Invalid account response: {error}"));
            }
        }
    }

    fn apply_auth_provider(&mut self, value: &Value) {
        let Ok(provider) = serde_json::from_value::<AuthProvider>(value.clone()) else {
            return;
        };
        if let Some(existing) = self
            .auth_providers
            .iter_mut()
            .find(|existing| existing.provider == provider.provider)
        {
            *existing = provider;
        } else {
            self.auth_providers.push(provider);
        }
        self.sync_active_auth_state();
    }

    fn apply_cli_versions(&mut self, value: &Value) {
        match serde_json::from_value::<Vec<CliVersion>>(
            value.get("providers").cloned().unwrap_or_default(),
        ) {
            Ok(versions) => {
                self.cli_versions = versions;
                self.cli_versions_loading = self
                    .cli_versions
                    .iter()
                    .any(|version| version.state == "checking");
                self.cli_versions_error = None;
            }
            Err(error) => {
                self.cli_versions_loading = false;
                self.cli_versions_error = Some(format!("Invalid assistant versions: {error}"));
            }
        }
    }

    fn apply_cli_version(&mut self, value: &Value) {
        let Ok(version) = serde_json::from_value::<CliVersion>(value.clone()) else {
            return;
        };
        if let Some(existing) = self
            .cli_versions
            .iter_mut()
            .find(|existing| existing.provider == version.provider)
        {
            *existing = version;
        } else {
            self.cli_versions.push(version);
        }
        self.cli_versions_loading = self
            .cli_versions
            .iter()
            .any(|version| version.state == "checking");
        self.cli_versions_error = None;
    }

    fn apply_self_update(&mut self, value: &Value) {
        let Some(panel) = &mut self.self_update_panel else {
            return;
        };
        match serde_json::from_value::<SelfUpdateStatus>(value.clone()) {
            Ok(status) => {
                panel.busy = matches!(status.state.as_str(), "checking" | "installing");
                panel.error = None;
                panel.status = Some(status);
            }
            Err(error) => {
                panel.busy = false;
                panel.error = Some(format!("Invalid host update response: {error}"));
            }
        }
    }

    fn start_source_build(&mut self, cx: &mut Context<Self>) {
        if self.source_build_panel.running {
            return;
        }
        let Some(target) = self.source_build_panel.target.clone() else {
            self.source_build_panel.message = Some("Source is not valid.".into());
            cx.notify();
            return;
        };
        self.settings.build_source = self.source_build_panel.text.clone();
        let _ = self.settings.save();
        self.source_build_panel.output.clear();
        self.source_build_panel.output_bytes = 0;
        self.source_build_panel.message = None;
        self.source_build_panel.installed = false;
        self.source_build_panel.stopping = false;
        match SourceBuildRun::start(target) {
            Ok(run) => {
                self.source_build_run = Some(run);
                self.source_build_panel.running = true;
                self.source_build_generation = self.source_build_generation.saturating_add(1);
                self.schedule_source_build_poll(self.source_build_generation, cx);
            }
            Err(error) => {
                self.source_build_panel.message = Some(error);
            }
        }
        cx.notify();
    }

    fn schedule_source_build_poll(&mut self, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(100)).await;
            let _ = this.update(cx, |this, cx| {
                if this.source_build_generation != generation {
                    return;
                }
                let mut events = Vec::new();
                if let Some(run) = &this.source_build_run {
                    while let Some(event) = run.try_recv() {
                        events.push(event);
                    }
                }
                let mut finished = false;
                for event in events {
                    match event {
                        SourceBuildEvent::Output(output) => {
                            this.source_build_panel.append_output(output)
                        }
                        SourceBuildEvent::Finished(result) => {
                            finished = true;
                            this.source_build_panel.running = false;
                            this.source_build_panel.stopping = false;
                            match result {
                                Ok(()) => {
                                    this.source_build_panel.installed = true;
                                    this.source_build_panel.message = Some(
                                        "Installed. Quit and reopen xd to run this build.".into(),
                                    );
                                }
                                Err(error) => {
                                    this.source_build_panel.message = Some(error);
                                }
                            }
                        }
                    }
                }
                if finished {
                    this.source_build_run = None;
                } else if this.source_build_panel.running {
                    this.schedule_source_build_poll(generation, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_active_auth_state(&mut self) {
        let active_provider = self.model.backend.as_str();
        if let Some(provider) = self
            .auth_providers
            .iter()
            .find(|provider| provider.provider == active_provider)
        {
            self.model.auth_state = provider.state.clone();
        }
    }

    fn request_agent_catalog(&mut self) {
        if let Some(host) = self.active_host()
            && let Err(error) = host.agent_catalog()
        {
            self.model.connection_error = Some(error);
        }
    }

    fn request_shortcuts(&mut self) {
        let Some(folder_id) = self
            .model
            .selected_summary()
            .map(|chat| chat.folder.clone())
        else {
            return;
        };
        if let Some(host) = self.active_host()
            && let Err(error) = host.shortcuts(Some(&folder_id))
        {
            self.model.connection_error = Some(error);
        }
    }

    fn minimal_popup_is_open(&self) -> bool {
        self.minimal_theme_open
            || self.minimal_new_tab_open
            || self.remote_panel.is_some()
            || self.creating_workspace
            || self.sidebar_edit.is_some()
            || self.session_context_menu.is_some()
            || self.pending_sidebar_delete.is_some()
            || self.creating_chat_folder.is_some()
    }

    fn focus_minimal_popup(
        &mut self,
        focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.minimal_popup_focus_captured {
            self.minimal_popup_previous_focus = window.focused(cx).map(|focus| focus.downgrade());
            self.minimal_popup_focus_captured = true;
        }
        window.focus(&focus);
    }

    fn restore_minimal_popup_focus(&mut self, window: &mut Window) {
        if self.minimal_popup_is_open() {
            return;
        }
        if !self.minimal_popup_focus_captured {
            return;
        }
        self.minimal_popup_focus_captured = false;
        if let Some(focus) = self
            .minimal_popup_previous_focus
            .take()
            .and_then(|focus| focus.upgrade())
        {
            window.focus(&focus);
        } else {
            window.blur();
        }
    }

    fn begin_workspace_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creating_workspace = true;
        self.workspace_create_submitting = false;
        self.workspace_create_name.clear();
        self.workspace_create_repo.clear();
        self.workspace_create_clone.clear();
        self.workspace_create_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.workspace_repo_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.workspace_clone_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        let focus = self.workspace_create_input.read(cx).focus_handle(cx);
        self.focus_minimal_popup(focus, window, cx);
        cx.notify();
    }

    fn workspace_create_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if self.creating_workspace && !self.workspace_create_submitting {
            self.workspace_create_name = text;
            cx.notify();
        }
    }

    fn workspace_repo_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if self.creating_workspace && !self.workspace_create_submitting {
            self.workspace_create_repo = text;
            cx.notify();
        }
    }

    fn workspace_clone_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if self.creating_workspace && !self.workspace_create_submitting {
            self.workspace_create_clone = text;
            cx.notify();
        }
    }

    // --- the file tree and its tabs -----------------------------------------

    /// Starts the tree over for the chat now selected.
    ///
    /// Open files are dropped with it: their paths are relative to a working
    /// directory that has just changed, so keeping them would leave tabs
    /// pointing at files in a repository nobody is looking at.

    fn list_tree_directory(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        self.file_tree.set_loading(&path);
        let generation = self.tree_generation;
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.file_tree_list(&chat_id, &path, generation));
        if let Err(error) = result {
            self.file_tree.set_failed(&path);
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    /// Opens or closes a folder, fetching its entries the first time.

    /// Brings a file to the front, reading it only if it is not open yet.

    fn save_file_tab(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self.open_files.current() else {
            return;
        };
        if file.saving || !file.dirty() {
            return;
        }
        let (path, original, sending) = (file.path.clone(), file.saved.clone(), file.text.clone());
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        self.open_files.set_saving(&path);
        let generation = self.tree_generation;
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.file_tab_write(&chat_id, &path, &original, &sending, generation));
        if let Err(error) = result {
            self.open_files.set_failed(&path, error);
        }
        cx.notify();
    }

    fn save_workspace_create(&mut self, cx: &mut Context<Self>) {
        if !self.creating_workspace || self.workspace_create_submitting {
            return;
        }
        let name = self.workspace_create_name.trim();
        if name.is_empty() {
            self.model.connection_error = Some("A workspace name cannot be empty.".into());
            cx.notify();
            return;
        }
        let repo = optional_trimmed(&self.workspace_create_repo).map(str::to_owned);
        let repo_url = optional_trimmed(&self.workspace_create_clone).map(str::to_owned);
        if repo.is_some() && repo_url.is_some() {
            self.model.connection_error =
                Some("Choose either an existing repository path or a clone URL.".into());
            cx.notify();
            return;
        }
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.new_folder(name, repo.as_deref(), repo_url.as_deref()));
        match result {
            Ok(()) => {
                if let Some(repo_url) = &repo_url {
                    self.pending_clone_requests
                        .insert(self.active_endpoint, repo_url.clone());
                } else {
                    self.pending_clone_requests.remove(&self.active_endpoint);
                }
                self.workspace_create_name = name.to_owned();
                self.workspace_create_repo = repo.unwrap_or_default();
                self.workspace_create_clone = repo_url.unwrap_or_default();
                self.workspace_create_submitting = true;
            }
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    fn cancel_workspace_create(&mut self, cx: &mut Context<Self>) {
        self.creating_workspace = false;
        self.workspace_create_submitting = false;
        self.workspace_create_name.clear();
        self.workspace_create_repo.clear();
        self.workspace_create_clone.clear();
        self.workspace_create_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.workspace_repo_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.workspace_clone_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn begin_chat_create(
        &mut self,
        folder_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.creating_chat_folder = Some(folder_id.clone());
        // Naming a chat up front is busywork: the title is renamed later or
        // never. Open on the name the chat would have had anyway, selected, so
        // Enter takes it and typing replaces it.
        self.chat_create_title = DEFAULT_CHAT_TITLE.into();
        self.chat_create_submitting = false;
        self.chat_create_worktree = None;
        self.chat_create_worktrees.clear();
        self.chat_create_worktrees_loading = true;
        self.chat_create_can_new_worktree = false;
        self.chat_create_input.update(cx, |input, cx| {
            input.set_text_selected(DEFAULT_CHAT_TITLE, cx)
        });
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.folder_settings(&folder_id));
        if let Err(error) = result {
            self.chat_create_worktrees_loading = false;
            self.model.connection_error = Some(error);
        }
        let focus = self.chat_create_input.read(cx).focus_handle(cx);
        self.focus_minimal_popup(focus, window, cx);
        cx.notify();
    }

    fn chat_create_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if self.creating_chat_folder.is_some() && !self.chat_create_submitting {
            self.chat_create_title = text;
            cx.notify();
        }
    }

    fn cancel_chat_create(&mut self, cx: &mut Context<Self>) {
        self.creating_chat_folder = None;
        self.chat_create_title.clear();
        self.chat_create_submitting = false;
        self.chat_create_worktree = None;
        self.chat_create_worktrees.clear();
        self.chat_create_worktrees_loading = false;
        self.chat_create_can_new_worktree = false;
        self.chat_create_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn workspace_context_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if self.workspace_context_folder.is_some()
            && !self.workspace_context_loading
            && !self.workspace_context_submitting
        {
            self.workspace_context_text = text;
            cx.notify();
        }
    }

    fn save_workspace_context(&mut self, cx: &mut Context<Self>) {
        let Some(folder_id) = self.workspace_context_folder.clone() else {
            return;
        };
        if self.workspace_context_loading || self.workspace_context_submitting {
            return;
        }
        let context = optional_trimmed(&self.workspace_context_text).map(str::to_owned);
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.set_folder_context(&folder_id, context.as_deref()));
        match result {
            Ok(()) => {
                self.workspace_context_text = context.unwrap_or_default();
                self.workspace_context_submitting = true;
            }
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    fn cancel_workspace_context(&mut self, cx: &mut Context<Self>) {
        self.workspace_context_folder = None;
        self.workspace_context_text.clear();
        self.workspace_context_loading = false;
        self.workspace_context_submitting = false;
        self.workspace_context_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn workspace_workdir_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(defaults) = &mut self.workspace_defaults
            && !defaults.loading
            && !defaults.submitting
        {
            defaults.workdir = text;
            cx.notify();
        }
    }

    fn workspace_repo_default_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(defaults) = &mut self.workspace_defaults
            && !defaults.loading
            && !defaults.submitting
        {
            defaults.repo = text;
            cx.notify();
        }
    }

    fn close_secrets(&mut self, cx: &mut Context<Self>) {
        self.secrets_panel = None;
        self.secret_name_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.secret_value_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    fn open_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = false;
        let command = self.settings.remote_ssh_command.clone().unwrap_or_default();
        self.remote_panel = Some(RemotePanel {
            command: command.clone(),
            submitting: false,
            error: self.remote_error.clone(),
        });
        self.remote_ssh_input
            .update(cx, |input, cx| input.set_text(command, cx));
        let focus = self.remote_ssh_input.read(cx).focus_handle(cx);
        self.focus_minimal_popup(focus, window, cx);
        cx.notify();
    }

    fn close_remote(&mut self, cx: &mut Context<Self>) {
        let canceled_connection = self
            .remote_panel
            .as_ref()
            .is_some_and(|panel| panel.submitting);
        if canceled_connection {
            self.remote_generation = self.remote_generation.saturating_add(1);
            self.remote_state = if self.settings.remote_ssh_command.is_some() {
                RemoteState::Offline
            } else {
                RemoteState::Unconfigured
            };
        }
        self.remote_panel = None;
        if canceled_connection && self.settings.remote_ssh_command.is_some() {
            self.schedule_remote_connect(Duration::ZERO, cx);
        }
        cx.notify();
    }

    fn connect_remote_machine(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.remote_panel.clone() else {
            return;
        };
        if panel.submitting {
            return;
        }
        let command = match SshCommand::parse(&panel.command) {
            Ok(command) => command,
            Err(error) => {
                if let Some(panel) = &mut self.remote_panel {
                    panel.error = Some(error);
                }
                cx.notify();
                return;
            }
        };
        if panel.command.trim().is_empty() {
            if let Some(panel) = &mut self.remote_panel {
                panel.error = Some("Enter an SSH command.".into());
            }
            cx.notify();
            return;
        }
        self.remote_generation = self.remote_generation.saturating_add(1);
        let generation = self.remote_generation;
        self.remote_host = None;
        self.remote_bridge = None;
        self.remote_state = RemoteState::Connecting;
        self.remote_error = None;
        if let Some(panel) = &mut self.remote_panel {
            panel.submitting = true;
            panel.error = None;
        }
        self.settings.remote_ssh_command = Some(panel.command.trim().to_owned());
        if let Err(error) = self.settings.save() {
            self.remote_state = RemoteState::Unconfigured;
            if let Some(panel) = &mut self.remote_panel {
                panel.submitting = false;
                panel.error = Some(error);
            }
            cx.notify();
            return;
        }
        let connection = cx
            .background_executor()
            .spawn(async move { remote::connect_ssh(&command) });
        cx.spawn(async move |this, cx| {
            let result = connection.await;
            let _ = this.update(cx, |this, cx| {
                if this.remote_generation != generation || this.remote_panel.is_none() {
                    return;
                }
                match result {
                    Ok(session) => {
                        this.install_remote_session(session, generation, cx);
                        this.activate_remote_runtime(cx);
                        this.remote_panel = None;
                    }
                    Err(error) => {
                        this.remote_state = if this.settings.remote_ssh_command.is_some() {
                            RemoteState::Offline
                        } else {
                            RemoteState::Unconfigured
                        };
                        this.remote_error = Some(error.to_string());
                        if let Some(panel) = &mut this.remote_panel {
                            panel.submitting = false;
                            panel.error = this.remote_error.clone();
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn request_devices(&mut self) {
        match self.host.as_ref() {
            Some(host) => {
                if let Err(error) = host.devices()
                    && let Some(panel) = &mut self.devices_panel
                {
                    panel.loading = false;
                    panel.error = Some(error);
                }
            }
            None => {
                if let Some(panel) = &mut self.devices_panel {
                    panel.loading = false;
                    panel.error = Some("xd is not connected to a host.".into());
                }
            }
        }
    }

    fn save_device_name(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.devices_panel.clone() else {
            return;
        };
        let Some(device_id) = panel.editing_id else {
            return;
        };
        if panel.loading || panel.mutating.is_some() {
            return;
        }
        let name = panel.edit_name.trim();
        if name.is_empty() {
            if let Some(panel) = &mut self.devices_panel {
                panel.error = Some("Enter a device name.".into());
            }
            cx.notify();
            return;
        }
        match self.host.as_ref() {
            Some(host) => match host.rename_device(&device_id, name) {
                Ok(()) => {
                    if let Some(panel) = &mut self.devices_panel {
                        panel.mutating = Some(device_id);
                        panel.error = None;
                    }
                }
                Err(error) => {
                    if let Some(panel) = &mut self.devices_panel {
                        panel.error = Some(error);
                    }
                }
            },
            None => {
                if let Some(panel) = &mut self.devices_panel {
                    panel.error = Some("xd is not connected to a host.".into());
                }
            }
        }
        cx.notify();
    }

    fn save_secret(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.secrets_panel.clone() else {
            return;
        };
        if panel.loading || panel.submitting {
            return;
        }
        let name = panel.name.trim();
        if name.is_empty() || panel.value.is_empty() {
            if let Some(panel) = &mut self.secrets_panel {
                panel.error = Some("Enter an environment name and secret value.".into());
            }
            cx.notify();
            return;
        }
        let mut entries = panel
            .names
            .iter()
            .filter(|existing| existing.as_str() != name)
            .map(|existing| (existing.clone(), None))
            .collect::<Vec<_>>();
        entries.push((name.to_owned(), Some(panel.value)));
        match self.secrets_host().cloned() {
            Some(host) => match host.set_agent_secrets(panel.folder_id.as_deref(), &entries) {
                Ok(()) => {
                    if let Some(current) = &mut self.secrets_panel {
                        current.submitting = true;
                        current.error = None;
                    }
                }
                Err(error) => {
                    if let Some(current) = &mut self.secrets_panel {
                        current.error = Some(error);
                    }
                }
            },
            None => {
                if let Some(current) = &mut self.secrets_panel {
                    current.error = Some("xd is not connected to a host.".into());
                }
            }
        }
        cx.notify();
    }

    fn submit_auth_input(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self
            .auth_providers
            .iter()
            .find(|provider| provider.state == "signing-in" && provider.needs_input)
            .map(|provider| provider.provider.clone())
        else {
            return;
        };
        let input = self.auth_input_text.trim().to_owned();
        if input.is_empty() {
            return;
        }
        if let Some(host) = self.active_host().cloned()
            && let Err(error) = host.agent_auth_action("agent-auth-input", &provider, Some(&input))
        {
            self.model.connection_error = Some(error);
            return;
        }
        self.auth_input_text.clear();
        self.auth_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    fn new_terminal_panel(chat_id: String) -> TerminalPanel {
        TerminalPanel {
            chat_id,
            agent: None,
            allow_agent_tabs: false,
            sessions: Vec::new(),
            selected: None,
            viewport: None,
            auto_open: true,
            opening: false,
            opening_agent: None,
            loading: true,
            pending_events: Vec::new(),
            error: None,
        }
    }

    fn new_agent_terminal_panel(chat_id: String, agent: AgentCli) -> TerminalPanel {
        TerminalPanel {
            agent: Some(agent),
            allow_agent_tabs: true,
            ..Self::new_terminal_panel(chat_id)
        }
    }

    fn stash_terminal_panel(&mut self) {
        let Some(panel) = self.terminal_panel.take() else {
            return;
        };
        self.terminal_panel_cache
            .insert((self.active_endpoint, panel.chat_id.clone()), panel);
    }

    fn restore_terminal_panel(&mut self, chat_id: &str, agent: AgentCli) -> bool {
        let Some(mut panel) = self
            .terminal_panel_cache
            .remove(&(self.active_endpoint, chat_id.to_owned()))
        else {
            return false;
        };
        panel.agent = Some(agent);
        panel.allow_agent_tabs = true;
        panel.auto_open = !panel.has_requested_session();
        self.terminal_panel = Some(panel);
        true
    }

    fn prime_terminal_cache(&mut self, endpoint: ChatEndpoint) {
        let refresh_all = self.terminal_cache_refresh.remove(&endpoint);
        let chats = self
            .endpoint_model(endpoint)
            .chats
            .iter()
            .filter_map(|chat| {
                AgentCli::from_backend(&chat.backend).map(|agent| (chat.id.clone(), agent))
            })
            .collect::<Vec<_>>();
        let live_chats = chats
            .iter()
            .map(|(chat_id, _)| chat_id.clone())
            .collect::<HashSet<_>>();
        self.terminal_panel_cache
            .retain(|(cached_endpoint, chat_id), _| {
                *cached_endpoint != endpoint || live_chats.contains(chat_id)
            });

        for (chat_id, agent) in chats {
            let active = endpoint == self.active_endpoint
                && self
                    .terminal_panel
                    .as_ref()
                    .is_some_and(|panel| panel.chat_id == chat_id);
            if active {
                if refresh_all && let Some(panel) = &mut self.terminal_panel {
                    panel.loading = false;
                    panel.error = None;
                }
                continue;
            }
            let key = (endpoint, chat_id.clone());
            let missing = !self.terminal_panel_cache.contains_key(&key);
            if missing {
                let mut panel = Self::new_agent_terminal_panel(chat_id.clone(), agent);
                panel.auto_open = false;
                panel.loading = false;
                self.terminal_panel_cache.insert(key.clone(), panel);
            } else if refresh_all && let Some(panel) = self.terminal_panel_cache.get_mut(&key) {
                panel.loading = false;
                panel.error = None;
            }
        }
    }

    fn current_connection_key(&self) -> String {
        let remote = self
            .settings
            .remote_ssh_command
            .as_deref()
            .and_then(|command| SshCommand::parse(command).ok())
            .map(|command| command.destination().to_owned());
        connection_state_key(self.active_endpoint, remote.as_deref())
    }

    fn cached_last_chat(&self) -> Option<String> {
        let key = self.current_connection_key();
        self.settings.last_chats.get(&key).cloned().or_else(|| {
            (self.active_endpoint == ChatEndpoint::Local)
                .then(|| self.settings.last_chat.clone())
                .flatten()
        })
    }

    fn remember_active_connection(&mut self) {
        let key = self.current_connection_key();
        if self.settings.active_connection.as_deref() == Some(key.as_str()) {
            return;
        }
        self.settings.active_connection = Some(key);
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
    }

    fn remember_last_chat(&mut self, chat_id: &str) {
        let key = self.current_connection_key();
        let mut changed = self.settings.last_chats.get(&key).map(String::as_str) != Some(chat_id);
        self.settings.last_chats.insert(key, chat_id.to_owned());
        if self.active_endpoint == ChatEndpoint::Local
            && self.settings.last_chat.as_deref() != Some(chat_id)
        {
            self.settings.last_chat = Some(chat_id.to_owned());
            changed = true;
        }
        if changed && let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
    }

    fn restore_collapsed_folders(&mut self) {
        let key = self.current_connection_key();
        self.collapsed_folders = self
            .settings
            .collapsed_folder_sets
            .get(&key)
            .or_else(|| {
                (self.active_endpoint == ChatEndpoint::Local)
                    .then_some(&self.settings.collapsed_folders)
            })
            .into_iter()
            .flatten()
            .cloned()
            .collect();
    }

    fn listen_for_terminal_runtime(
        &mut self,
        updates: async_channel::Receiver<SessionEvent>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Ok(event) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        let (name, body) = terminal_runtime_event(event);
                        if name == "terminal-activity" {
                            if let (Some(chat_id), Some(working)) = (
                                body.get("chat").and_then(Value::as_str),
                                body.get("terminal_working").and_then(Value::as_bool),
                            ) {
                                this.model.apply_desktop_terminal_activity(chat_id, working);
                            }
                            cx.notify();
                        } else {
                            this.handle_terminal_screen_event(
                                this.active_endpoint,
                                name,
                                &body,
                                cx,
                            );
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn active_session_host(&self) -> Result<SessionHost, String> {
        match self.active_endpoint {
            ChatEndpoint::Local => {
                let tmux = env::var_os("XD_TMUX_EXECUTABLE")
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("tmux"));
                let runtime = env::var_os("XD_SESSION_RUNTIME")
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        env::temp_dir()
                            .join(xd_desktop::channel::data_name())
                            .join("sessions")
                    });
                std::fs::create_dir_all(&runtime)
                    .map_err(|error| format!("Cannot prepare terminal sessions: {error}."))?;
                let configuration = runtime.join("tmux.conf");
                std::fs::write(&configuration, TMUX_CONFIGURATION)
                    .map_err(|error| format!("Cannot configure terminal sessions: {error}."))?;
                Ok(SessionHost::local(tmux, runtime))
            }
            ChatEndpoint::Remote => {
                let command = self
                    .settings
                    .remote_ssh_command
                    .as_deref()
                    .ok_or_else(|| "Configure an SSH connection first.".to_owned())
                    .and_then(SshCommand::parse)?;
                Ok(SessionHost::ssh(command, ".local/share/xd/runtime/v1"))
            }
        }
    }

    fn terminal_agent_command(&self, agent: Option<AgentCli>, terminal_id: &str) -> AgentCommand {
        let remote = self.active_endpoint == ChatEndpoint::Remote;
        match agent {
            Some(AgentCli::Codex) => {
                let program = if remote {
                    "codex".into()
                } else {
                    env::var("XD_CODEX_EXECUTABLE").unwrap_or_else(|_| "codex".into())
                };
                let mut arguments = vec![
                    "--no-alt-screen".to_owned(),
                    "-c".to_owned(),
                    "tui.terminal_title=[\"run-state\"]".to_owned(),
                    "-c".to_owned(),
                    "tui.terminal_resize_reflow_max_rows=5000".to_owned(),
                ];
                if self.settings.allow_all_permissions {
                    arguments.push("--dangerously-bypass-approvals-and-sandbox".into());
                }
                let mut resume_arguments = vec!["resume".to_owned(), "--last".to_owned()];
                resume_arguments.extend(arguments.clone());
                AgentCommand::new(program, arguments)
                    .resume_with(resume_arguments)
                    .record_codex_session()
                    .discover_in_user_shell("Codex")
            }
            Some(AgentCli::Claude) => {
                let executable = if remote {
                    "claude".into()
                } else {
                    env::var("XD_CLAUDE_EXECUTABLE").unwrap_or_else(|_| "claude".into())
                };
                let session_id = stable_agent_session_id(terminal_id);
                let mut common_arguments = Vec::new();
                if self.settings.allow_all_permissions {
                    common_arguments.push("--dangerously-skip-permissions".into());
                }
                let mut arguments = vec!["--session-id".to_owned(), session_id.clone()];
                arguments.extend(common_arguments.clone());
                let mut resume_arguments = vec!["--resume".to_owned(), session_id];
                resume_arguments.extend(common_arguments);
                AgentCommand::new(executable, arguments)
                    .resume_with(resume_arguments)
                    .unset_environment("TMUX")
                    .discover_in_user_shell("Claude Code")
            }
            Some(AgentCli::Jcode) => AgentCommand::new(
                if remote {
                    "jcode".into()
                } else {
                    env::var("XD_JCODE_EXECUTABLE").unwrap_or_else(|_| "jcode".into())
                },
                ["--no-update"],
            )
            .resume_with(["--no-update", "--resume"])
            .discover_in_user_shell("JCode"),
            Some(AgentCli::Copilot) => {
                let executable = if remote {
                    "copilot".into()
                } else {
                    env::var("XD_COPILOT_EXECUTABLE").unwrap_or_else(|_| "copilot".into())
                };
                let session_id = stable_agent_session_id(terminal_id);
                let mut arguments = vec![
                    "--no-auto-update".to_owned(),
                    "--no-banner".to_owned(),
                    "--no-mouse".to_owned(),
                    "--session-id".to_owned(),
                    session_id,
                ];
                if self.settings.allow_all_permissions {
                    arguments.push("--allow-all".into());
                }
                AgentCommand::new(executable, arguments.clone())
                    .resume_with(arguments)
                    .discover_in_user_shell("GitHub Copilot CLI")
            }
            None => AgentCommand::user_shell(),
        }
    }

    fn resize_terminal_viewport(&mut self, columns: usize, rows: usize, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        let geometry = (columns, rows);
        let geometry_changed = panel.viewport != Some(geometry);
        if geometry_changed {
            panel.viewport = Some(geometry);
            if let Some(session) = panel.selected_mut()
                && session.screen.geometry() != geometry
            {
                session.screen.resize(columns, rows);
            }
        }
        let terminal_id = panel.selected.clone();
        let should_auto_open = panel.should_auto_open();
        if geometry_changed && let Some(terminal_id) = terminal_id {
            let result = self.terminal_runtime.resize(&terminal_id, columns, rows);
            if let Err(error) = result
                && let Some(panel) = &mut self.terminal_panel
            {
                panel.error = Some(error);
            }
        }
        if should_auto_open {
            self.start_terminal_session(true, cx);
        }
        cx.notify();
    }

    fn send_terminal_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.terminal_cursor_visible = true;
        self.terminal_scroll.scroll_to_bottom();
        let Some(terminal_id) = self
            .terminal_panel
            .as_ref()
            .and_then(|panel| panel.selected.clone())
        else {
            return;
        };
        if let Err(error) = self.terminal_runtime.input(&terminal_id, bytes)
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn paste_terminal_image(
        &mut self,
        format: gpui::ImageFormat,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) {
        let Some(terminal_id) = self
            .terminal_panel
            .as_ref()
            .and_then(|panel| panel.selected.clone())
        else {
            return;
        };
        if format != gpui::ImageFormat::Png {
            self.model.connection_error = Some(
                "That clipboard image is not available as PNG. Save it as PNG and paste it again."
                    .into(),
            );
            cx.notify();
            return;
        }
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            self.model.connection_error = Some("That image is larger than 10 MiB.".into());
            cx.notify();
            return;
        }
        let name = format!(
            "paste-{}.png",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let attachment = match Attachment::from_png(name, bytes.to_vec()) {
            Ok(attachment) => attachment,
            Err(error) => {
                self.model.connection_error = Some(error);
                cx.notify();
                return;
            }
        };
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.terminal_paste_image(&terminal_id, &attachment));
        if let Err(error) = result
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn sync_terminal_input_mode(&mut self, cx: &mut Context<Self>) {
        let bracketed_paste = self
            .terminal_panel
            .as_ref()
            .and_then(TerminalPanel::selected)
            .is_some_and(|session| session.screen.bracketed_paste());
        self.terminal_input.update(cx, |input, _| {
            input.set_terminal_bracketed_paste(bracketed_paste);
        });
    }

    fn kill_terminal_id(&mut self, terminal_id: String, cx: &mut Context<Self>) {
        if let Err(error) = self.terminal_runtime.kill(&terminal_id)
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn start_terminal_session(&mut self, reuse: bool, cx: &mut Context<Self>) {
        let agent = self.terminal_panel.as_ref().and_then(|panel| panel.agent);
        self.start_terminal_session_as(reuse, agent, cx);
    }

    fn toggle_minimal_new_tab_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.minimal_new_tab_open = !self.minimal_new_tab_open;
        if self.minimal_new_tab_open {
            self.minimal_theme_open = false;
            self.focus_minimal_popup(self.minimal_popup_focus.clone(), window, cx);
        } else {
            self.restore_minimal_popup_focus(window);
        }
        cx.notify();
    }

    fn open_minimal_terminal_tab(
        &mut self,
        agent: Option<AgentCli>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.minimal_new_tab_open = false;
        self.restore_minimal_popup_focus(window);
        self.start_terminal_session_as(false, agent, cx);
        let focus = self.terminal_input.read(cx).focus_handle(cx);
        window.focus(&focus);
    }

    fn start_terminal_session_as(
        &mut self,
        reuse: bool,
        agent: Option<AgentCli>,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        if panel.opening || !panel.accepts_agent(agent) {
            return;
        }
        let (columns, rows) = panel.viewport.unwrap_or((120, 32));
        let chat_id = panel.chat_id.clone();
        panel.auto_open = false;
        panel.opening = true;
        panel.opening_agent = agent;
        panel.error = None;
        let terminal_id = terminal_session_id(
            &chat_id,
            agent,
            reuse,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        let title = terminal_tab_title(agent);
        let workdir = self.model.workdir.clone();
        let result = workdir
            .ok_or_else(|| "The session working directory is still loading.".to_owned())
            .and_then(|workdir| {
                let host = self.active_session_host()?;
                let command = self.terminal_agent_command(agent, &terminal_id);
                let process = host.attach(&terminal_id, Path::new(&workdir), &command);
                let cleanup = host.kill_process(&terminal_id);
                self.terminal_runtime.open(
                    &chat_id,
                    &terminal_id,
                    &title,
                    agent.map(AgentCli::protocol_name),
                    process,
                    Some(cleanup),
                    columns,
                    rows,
                )
            });
        if let Err(error) = result
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.finish_opening();
            panel.error = Some(error);
        }
        cx.notify();
    }

    /// Reattaches the selected card to its persistent tmux session. The PTY is
    /// window-owned, while the command remains alive in tmux between windows.
    fn refresh_terminal_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self
            .terminal_panel
            .as_ref()
            .filter(|panel| self.model.selected_chat.as_deref() == Some(panel.chat_id.as_str()))
            .map(|panel| panel.chat_id.clone())
        else {
            return;
        };
        if self.model.workdir.is_none() {
            // The tree primes an empty panel for every session before its full
            // chat snapshot (including workdir) has been fetched. Restoring
            // one of those panels must remain in the loading state so the
            // viewport cannot turn this normal dependency into a terminal
            // startup error while the chat request is in flight. A panel that
            // already has a live terminal can remain usable during hydration.
            if let Some(panel) = &mut self.terminal_panel
                && panel.sessions.is_empty()
            {
                panel.loading = true;
                panel.error = None;
            }
            self.request_chat(&chat_id);
            return;
        }
        if let Some(panel) = &mut self.terminal_panel {
            panel.loading = false;
            panel.error = None;
        }
        self.start_terminal_session(true, cx);
    }

    fn select_terminal(&mut self, terminal_id: String, cx: &mut Context<Self>) {
        self.minimal_new_tab_open = false;
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        if panel.selected.as_deref() == Some(terminal_id.as_str())
            || !panel
                .sessions
                .iter()
                .any(|session| session.id == terminal_id)
        {
            return;
        }
        panel.selected = Some(terminal_id.clone());
        self.terminal_scroll.scroll_to_bottom();
        let viewport = panel.viewport;
        if let Some((columns, rows)) = viewport
            && let Some(session) = panel.selected_mut()
        {
            session.screen.resize(columns, rows);
            if let Err(error) = self.terminal_runtime.resize(&terminal_id, columns, rows)
                && let Some(panel) = &mut self.terminal_panel
            {
                panel.error = Some(error);
            }
        }
        self.sync_terminal_input_mode(cx);
        cx.notify();
    }

    fn terminal_event_is_new(session: &TerminalTab, body: &Value) -> bool {
        match body.get("sequence").and_then(Value::as_u64) {
            Some(sequence) => session.sequence.is_none_or(|current| sequence > current),
            None => true,
        }
    }

    fn advance_terminal_sequence(session: &mut TerminalTab, body: &Value) {
        if let Some(sequence) = body.get("sequence").and_then(Value::as_u64) {
            session.sequence = Some(sequence);
        }
    }

    fn apply_terminal_opened_event(panel: &mut TerminalPanel, body: &Value) -> bool {
        let protocol_agent = body.get("agent").and_then(Value::as_str);
        let event_agent = protocol_agent.and_then(AgentCli::from_protocol_name);
        let completes_opening = panel.opening_matches_protocol_agent(protocol_agent);
        if !panel.accepts_agent(event_agent) {
            return false;
        }
        let Some(terminal_id) = body.get("terminal").and_then(Value::as_str) else {
            return false;
        };
        if !panel
            .sessions
            .iter()
            .any(|session| session.id == terminal_id)
        {
            let title = terminal_tab_title(event_agent);
            let columns = body.get("columns").and_then(Value::as_u64).unwrap_or(120) as usize;
            let rows = body.get("rows").and_then(Value::as_u64).unwrap_or(32) as usize;
            panel.sessions.push(TerminalTab {
                id: terminal_id.to_owned(),
                title,
                agent: event_agent,
                sequence: body.get("sequence").and_then(Value::as_u64),
                screen: TerminalScreen::new(columns, rows),
            });
        }
        panel.selected = Some(terminal_id.to_owned());
        if completes_opening {
            panel.finish_opening();
        }
        true
    }

    fn apply_terminal_output_event(panel: &mut TerminalPanel, body: &Value) -> bool {
        let Some(terminal_id) = body.get("terminal").and_then(Value::as_str) else {
            return false;
        };
        let Some(session) = panel
            .sessions
            .iter_mut()
            .find(|session| session.id == terminal_id)
        else {
            return false;
        };
        if !Self::terminal_event_is_new(session, body) {
            return false;
        }
        let Some(data) = body
            .get("data")
            .and_then(Value::as_str)
            .and_then(|data| STANDARD.decode(data).ok())
        else {
            return false;
        };
        session.screen.feed(&data);
        Self::advance_terminal_sequence(session, body);
        true
    }

    fn apply_terminal_resized_event(panel: &mut TerminalPanel, body: &Value) -> bool {
        let Some(terminal_id) = body.get("terminal").and_then(Value::as_str) else {
            return false;
        };
        let Some(session) = panel
            .sessions
            .iter_mut()
            .find(|session| session.id == terminal_id)
        else {
            return false;
        };
        if !Self::terminal_event_is_new(session, body) {
            return false;
        }
        let columns = body.get("columns").and_then(Value::as_u64).unwrap_or(120) as usize;
        let rows = body.get("rows").and_then(Value::as_u64).unwrap_or(32) as usize;
        session.screen.resize(columns, rows);
        Self::advance_terminal_sequence(session, body);
        true
    }

    fn apply_terminal_closed_event(panel: &mut TerminalPanel, body: &Value) -> bool {
        let Some(terminal_id) = body.get("terminal").and_then(Value::as_str) else {
            return false;
        };
        let Some(session) = panel
            .sessions
            .iter()
            .find(|session| session.id == terminal_id)
        else {
            return false;
        };
        if !Self::terminal_event_is_new(session, body) {
            return false;
        }
        panel.remove(terminal_id);
        true
    }

    fn apply_pending_terminal_event(panel: &mut TerminalPanel, event: PendingTerminalEvent) {
        match event {
            PendingTerminalEvent::Opened(body) => {
                Self::apply_terminal_opened_event(panel, &body);
            }
            PendingTerminalEvent::Output(body) => {
                Self::apply_terminal_output_event(panel, &body);
            }
            PendingTerminalEvent::Resized(body) => {
                Self::apply_terminal_resized_event(panel, &body);
            }
            PendingTerminalEvent::Closed(body) => {
                Self::apply_terminal_closed_event(panel, &body);
            }
        }
    }

    fn apply_terminal_list(&mut self, value: &Value, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        let should_auto_open = Self::merge_terminal_list(panel, value);
        self.terminal_scroll.scroll_to_bottom();
        if should_auto_open {
            self.start_terminal_session(true, cx);
        }
        self.sync_terminal_input_mode(cx);
    }

    fn merge_terminal_list(panel: &mut TerminalPanel, value: &Value) -> bool {
        panel.loading = false;
        panel.error = None;
        let pending_events = std::mem::take(&mut panel.pending_events);
        let previous = panel.selected.clone();
        let mut existing = std::mem::take(&mut panel.sessions);
        let sessions = value
            .get("terminals")
            .and_then(Value::as_array)
            .map(|items| {
                let mut sessions = Vec::new();
                for item in items {
                    let item_id = item.get("id").and_then(Value::as_str);
                    let Some(snapshot) = Self::terminal_tab_from_snapshot(item) else {
                        // Never adopt a sequence watermark from a partial or
                        // malformed replay. Preserve a known-good live screen
                        // until a later terminal-list can repair the snapshot.
                        if let Some(index) = item_id
                            .and_then(|id| existing.iter().position(|session| session.id == id))
                        {
                            sessions.push(existing.swap_remove(index));
                        }
                        continue;
                    };
                    if !panel.accepts_agent(snapshot.agent) {
                        continue;
                    }
                    let newer = existing
                        .iter()
                        .position(|session| session.id == snapshot.id)
                        .filter(|index| {
                            matches!(
                                (existing[*index].sequence, snapshot.sequence),
                                (Some(current), Some(boundary)) if current > boundary
                            )
                        });
                    sessions.push(
                        newer
                            .map(|index| existing.swap_remove(index))
                            .unwrap_or(snapshot),
                    );
                }
                sessions
            })
            .unwrap_or_default();
        panel.sessions = sessions;
        panel.selected = panel.selection_after_refresh(previous);
        for event in pending_events {
            Self::apply_pending_terminal_event(panel, event);
        }
        if panel.has_requested_session() {
            panel.auto_open = false;
        }
        panel.should_auto_open()
    }

    fn terminal_tab_from_snapshot(terminal: &Value) -> Option<TerminalTab> {
        let id = terminal.get("id")?.as_str()?.to_owned();
        let agent = terminal
            .get("agent")
            .and_then(Value::as_str)
            .and_then(AgentCli::from_protocol_name);
        let title = terminal_tab_title(agent);
        let dimension = |key: &str, default: usize| match terminal.get(key) {
            Some(value) => usize::try_from(value.as_u64()?)
                .ok()
                .filter(|value| (1..=1_000).contains(value)),
            None => Some(default),
        };
        let columns = dimension("columns", 120)?;
        let rows = dimension("rows", 32)?;
        let mut screen = TerminalScreen::new(columns, rows);
        if let Some(replay) = terminal.get("replay") {
            let replay = replay.as_array()?;
            for frame in replay {
                if let Some(checkpoint) = frame
                    .get("checkpoint")
                    .and_then(Value::as_str)
                    .and_then(|checkpoint| STANDARD.decode(checkpoint).ok())
                    .and_then(|checkpoint| TerminalScreen::from_checkpoint_bytes(&checkpoint))
                {
                    screen = checkpoint;
                    continue;
                }
                if let Some(data) = frame.get("data") {
                    let data = data.as_str().and_then(|data| STANDARD.decode(data).ok())?;
                    screen.feed(&data);
                    continue;
                }
                if frame.get("checkpoint").is_some() {
                    return None;
                }
                let columns = usize::try_from(frame.get("columns")?.as_u64()?).ok()?;
                let rows = usize::try_from(frame.get("rows")?.as_u64()?).ok()?;
                if !(1..=1_000).contains(&columns) || !(1..=1_000).contains(&rows) {
                    return None;
                }
                screen.resize(columns, rows);
            }
        }
        let sequence = match terminal.get("sequence") {
            Some(sequence) => Some(sequence.as_u64()?),
            None => None,
        };
        Some(TerminalTab {
            id,
            title,
            agent,
            sequence,
            screen,
        })
    }

    fn save_browse_file(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some((path, original, content)) = self.diff_panel.as_ref().and_then(|panel| {
            panel.file_preview.as_ref().and_then(|preview| {
                (!preview.truncated && !preview.saving && preview.content != preview.original).then(
                    || {
                        (
                            preview.path.clone(),
                            preview.original.clone(),
                            preview.content.clone(),
                        )
                    },
                )
            })
        }) else {
            return;
        };
        if let Some(preview) = self
            .diff_panel
            .as_mut()
            .and_then(|panel| panel.file_preview.as_mut())
        {
            preview.saving = true;
        }
        let generation = self.diff_generation;
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| {
                host.file_browse_write(&chat_id, &path, &original, &content, generation)
            });
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.error = Some(error);
            if let Some(preview) = &mut diff.file_preview {
                preview.saving = false;
            }
        }
        cx.notify();
    }

    fn refresh_git_status(&mut self) {
        let (Some(chat_id), Some(host)) = (
            self.model.selected_chat.clone(),
            self.active_host().cloned(),
        ) else {
            return;
        };
        if let Some(diff) = &mut self.diff_panel {
            diff.status_loading = true;
        }
        if let Err(error) = host.git_status(&chat_id, self.diff_generation)
            && let Some(diff) = &mut self.diff_panel
        {
            diff.status_loading = false;
            diff.action_error = Some(error);
        }
    }

    fn git_commit_changed(&mut self, text: String, cx: &mut Context<Self>) {
        self.git_commit_message = text;
        cx.notify();
    }

    fn commit_changes(&mut self, cx: &mut Context<Self>) {
        let message = self.git_commit_message.trim().to_owned();
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let can_commit = self.diff_panel.as_ref().is_some_and(|diff| {
            diff.action.is_none()
                && diff.status.as_ref().is_some_and(|status| {
                    !status.clean && status.conflicted == 0 && !message.is_empty()
                })
        });
        if !can_commit {
            return;
        }
        let generation = self.diff_generation;
        if let Some(diff) = &mut self.diff_panel {
            diff.action = Some("Committing all changes…".into());
            diff.action_error = None;
        }
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.git_commit(&chat_id, &message, generation));
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.action = None;
            diff.action_error = Some(error);
        }
        cx.notify();
    }

    fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        if self
            .diff_panel
            .as_ref()
            .is_some_and(|diff| diff.action.is_some())
        {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            if let Some(diff) = &mut self.diff_panel {
                diff.loading = false;
                diff.status_loading = false;
                diff.error = Some("Select a chat to inspect its changes.".into());
            }
            cx.notify();
            return;
        };
        let branch = self.diff_panel.as_ref().is_some_and(|diff| diff.branch);
        let files_mode = self.diff_panel.as_ref().is_some_and(|diff| diff.files_mode);
        self.diff_generation = self.diff_generation.saturating_add(1);
        let generation = self.diff_generation;
        if let Some(diff) = &mut self.diff_panel {
            diff.loading = true;
            diff.status_loading = !files_mode;
            diff.base = None;
            diff.files.clear();
            diff.file_preview = None;
            diff.file_loading = false;
            diff.error = None;
            diff.truncated = false;
            diff.pr_loading = false;
        } else {
            return;
        }
        match self.active_host().cloned() {
            Some(host) => {
                let content = if files_mode {
                    let path = self
                        .diff_panel
                        .as_ref()
                        .map(|diff| diff.browse_path.as_str())
                        .unwrap_or_default();
                    host.file_browse_list(&chat_id, path, generation)
                } else {
                    host.diff_read(
                        &chat_id,
                        if branch { "base" } else { "working-status" },
                        None,
                        None,
                        generation,
                    )
                };
                if let Err(error) = content
                    && let Some(diff) = &mut self.diff_panel
                {
                    diff.loading = false;
                    diff.error = Some(error);
                }
                if !files_mode {
                    if let Err(error) = host.git_status(&chat_id, generation)
                        && let Some(diff) = &mut self.diff_panel
                    {
                        diff.status_loading = false;
                        diff.action_error = Some(error);
                    }
                }
            }
            None => {
                if let Some(diff) = &mut self.diff_panel {
                    diff.loading = false;
                    diff.status_loading = false;
                    diff.error = Some("xd is not connected to a host.".into());
                }
            }
        }
        cx.notify();
    }

    fn prepare_diff(&mut self, output: String, generation: u64, cx: &mut Context<Self>) {
        let prepare = cx
            .background_executor()
            .spawn(async move { parse_unified_diff(&output) });
        cx.spawn(async move |this, cx| {
            let result = prepare.await;
            let _ = this.update(cx, |this, cx| {
                if this.diff_generation != generation {
                    return;
                }
                if let Some(diff) = &mut this.diff_panel {
                    diff.loading = false;
                    match result {
                        Ok((files, truncated)) => {
                            diff.files = files;
                            diff.truncated = truncated;
                            diff.error = None;
                        }
                        Err(error) => diff.error = Some(error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn prepare_diff_listing(
        &mut self,
        output: String,
        branch: bool,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let prepare = cx
            .background_executor()
            .spawn(async move { parse_diff_file_list(&output, branch) });
        cx.spawn(async move |this, cx| {
            let result = prepare.await;
            let _ = this.update(cx, |this, cx| {
                if this.diff_generation != generation {
                    return;
                }
                let mut load_paths = Vec::new();
                if let Some(diff) = &mut this.diff_panel {
                    diff.loading = false;
                    match result {
                        Ok((files, truncated)) => {
                            load_paths.extend(
                                files
                                    .iter()
                                    .filter(|file| !this.collapsed_diff_files.contains(&file.path))
                                    .map(|file| file.path.clone()),
                            );
                            diff.files = files;
                            diff.truncated = truncated;
                            diff.error = None;
                        }
                        Err(error) => diff.error = Some(error),
                    }
                }
                for path in load_paths {
                    this.load_diff_file(path, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn prepare_diff_file(
        &mut self,
        path: String,
        output: String,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let empty = output.trim().is_empty();
        let prepare = cx
            .background_executor()
            .spawn(async move { parse_unified_diff(&output) });
        cx.spawn(async move |this, cx| {
            let result = prepare.await;
            let _ = this.update(cx, |this, cx| {
                if this.diff_generation != generation {
                    return;
                }
                let Some(diff) = &mut this.diff_panel else {
                    return;
                };
                if this.collapsed_diff_files.contains(&path) {
                    if let Some(file) = diff.files.iter_mut().find(|file| file.path == path) {
                        file.loading = false;
                    }
                    return;
                }
                let Some(file) = diff.files.iter_mut().find(|file| file.path == path) else {
                    return;
                };
                file.loading = false;
                match result {
                    Ok((mut parsed, truncated)) => {
                        if let Some(prepared) = parsed
                            .iter()
                            .position(|prepared| prepared.path == path)
                            .map(|index| parsed.swap_remove(index))
                            .or_else(|| parsed.pop())
                        {
                            file.additions = prepared.additions;
                            file.deletions = prepared.deletions;
                            file.lines = prepared.lines;
                        } else if !empty {
                            file.error =
                                Some("Git returned a diff that xd could not parse.".into());
                            cx.notify();
                            return;
                        }
                        file.loaded = true;
                        file.error = None;
                        diff.truncated |= truncated;
                    }
                    Err(error) => file.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_diff_file(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            cx.notify();
            return;
        };
        let request = self.diff_panel.as_mut().and_then(|diff| {
            let file = diff.files.iter_mut().find(|file| file.path == path)?;
            if file.loading {
                return None;
            }
            if file.loaded {
                return None;
            }
            let read = file.lazy_read.clone()?;
            file.loading = true;
            file.error = None;
            Some((read, diff.base.clone()))
        });
        if let Some((read, base)) = request {
            let result = self
                .active_host()
                .ok_or_else(|| "xd is not connected to a host.".to_owned())
                .and_then(|host| {
                    host.diff_read(
                        &chat_id,
                        &read,
                        base.as_deref(),
                        Some(&path),
                        self.diff_generation,
                    )
                });
            if let Err(error) = result
                && let Some(file) = self
                    .diff_panel
                    .as_mut()
                    .and_then(|diff| diff.files.iter_mut().find(|file| file.path == path))
            {
                file.loading = false;
                file.error = Some(error);
            }
        }
        cx.notify();
    }

    fn begin_sidebar_edit(
        &mut self,
        target: SidebarTarget,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_context_menu = None;
        self.pending_sidebar_delete = None;
        self.sidebar_delete_submitting = false;
        self.sidebar_move = None;
        self.sidebar_move_submitting = false;
        self.sidebar_move_destination = None;
        self.sidebar_edit = Some(SidebarEdit {
            target,
            original: current.clone(),
            text: current.clone(),
            submitting: false,
        });
        self.sidebar_edit_input
            .update(cx, |input, cx| input.set_text(current, cx));
        let focus = self.sidebar_edit_input.read(cx).focus_handle(cx);
        self.focus_minimal_popup(focus, window, cx);
        cx.notify();
    }

    fn begin_sidebar_delete(
        &mut self,
        target: SidebarTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_context_menu = None;
        self.sidebar_edit = None;
        self.sidebar_move = None;
        self.sidebar_move_submitting = false;
        self.sidebar_move_destination = None;
        self.pending_sidebar_delete = Some(target);
        self.sidebar_delete_submitting = false;
        self.focus_minimal_popup(self.minimal_popup_focus.clone(), window, cx);
        cx.notify();
    }

    fn open_session_context_menu(
        &mut self,
        chat_id: String,
        title: String,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = window.bounds().size;
        let x = f32::from(position.x).clamp(8.0, (f32::from(viewport.width) - 196.0).max(8.0));
        let y = f32::from(position.y).clamp(8.0, (f32::from(viewport.height) - 112.0).max(8.0));
        self.session_context_menu = Some(SessionContextMenu {
            chat_id,
            title,
            position: point(px(x), px(y)),
        });
        self.focus_minimal_popup(self.minimal_popup_focus.clone(), window, cx);
        cx.notify();
    }

    fn close_session_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session_context_menu = None;
        self.restore_minimal_popup_focus(window);
        cx.notify();
    }

    fn cancel_sidebar_delete(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_delete_submitting {
            return;
        }
        self.pending_sidebar_delete = None;
        cx.notify();
    }

    fn confirm_sidebar_delete(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.pending_sidebar_delete.clone() else {
            return;
        };
        if self.sidebar_delete_submitting {
            return;
        }
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| match &target {
                SidebarTarget::Folder(folder_id) => host.trash_folder(folder_id),
                SidebarTarget::Chat(chat_id) => host.delete_chat(chat_id),
            });
        match result {
            Ok(()) => self.sidebar_delete_submitting = true,
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    fn sidebar_edit_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(edit) = &mut self.sidebar_edit
            && !edit.submitting
        {
            edit.text = text;
            cx.notify();
        }
    }

    fn save_sidebar_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.sidebar_edit.clone() else {
            return;
        };
        if edit.submitting {
            return;
        }
        let text = edit.text.trim();
        if text.is_empty() {
            self.model.connection_error = Some("A name cannot be empty.".into());
            cx.notify();
            return;
        }
        if text == edit.original {
            self.cancel_sidebar_edit(cx);
            return;
        }
        let result = self.active_host().map(|host| match &edit.target {
            SidebarTarget::Folder(folder_id) => host.rename_folder(folder_id, text),
            SidebarTarget::Chat(chat_id) => host.rename_chat(chat_id, text),
        });
        match result {
            Some(Ok(())) => {
                match &edit.target {
                    SidebarTarget::Folder(folder_id) => {
                        if let Some(folder) = self
                            .model
                            .folders
                            .iter_mut()
                            .find(|folder| &folder.id == folder_id)
                        {
                            folder.name = text.to_owned();
                        }
                    }
                    SidebarTarget::Chat(chat_id) => {
                        if let Some(chat) =
                            self.model.chats.iter_mut().find(|chat| &chat.id == chat_id)
                        {
                            chat.title = Some(text.to_owned());
                        }
                    }
                }
                self.cancel_sidebar_edit(cx);
            }
            Some(Err(error)) => self.model.connection_error = Some(error),
            None => self.model.connection_error = Some("xd is not connected to a host.".into()),
        }
        cx.notify();
    }

    fn cancel_sidebar_edit(&mut self, cx: &mut Context<Self>) {
        self.sidebar_edit = None;
        self.sidebar_edit_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn request_chat(&mut self, chat_id: &str) {
        if let Some(host) = self.active_host() {
            if let Err(error) = host.chat(chat_id) {
                self.model.connection_error = Some(error);
            }
        }
    }

    /// Rehydrates the selected chat after its host connection comes back.
    ///
    /// Keep the transcript already on screen and ask only for rows after its
    /// last message. If the first request was lost before any page arrived,
    /// this naturally falls back to the tail page. The file root is retried as
    /// well because its previous request may have died with the same socket.
    fn refresh_selected_chat_after_connect(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        self.transcript_page_loading = false;
        self.transcript_refresh_pending = false;
        self.transcript_loading = !self.transcript_loaded;
        self.request_chat(&chat_id);
        self.request_messages(&chat_id);
        let mut file_requests = self.file_tree.loading_paths();
        if !self.file_tree.is_loaded("") && !file_requests.iter().any(String::is_empty) {
            file_requests.insert(0, String::new());
        }
        for path in file_requests {
            self.list_tree_directory(path, cx);
        }
        self.refresh_terminal_sessions(cx);
        if let Some(host) = self.active_host()
            && let Err(error) = host.git_state(&chat_id)
        {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn request_messages(&mut self, chat_id: &str) {
        if self.transcript_page_loading {
            self.transcript_refresh_pending = true;
            return;
        }
        let cursor = self
            .model
            .messages
            .last()
            .and_then(|message| message.id)
            .map(MessageCursor::After)
            .unwrap_or(MessageCursor::Tail);
        self.request_message_page(chat_id, cursor);
    }

    fn request_message_page(&mut self, chat_id: &str, cursor: MessageCursor) {
        if self.transcript_page_loading {
            return;
        }
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| host.messages(chat_id, cursor));
        if let Err(error) = result {
            self.model.connection_error = Some(error);
            self.transcript_loading = false;
            self.transcript_page_loading = false;
        } else {
            self.transcript_page_loading = true;
        }
    }

    fn request_workflow_statuses(&mut self) {
        let markers = self
            .model
            .messages
            .iter()
            .chain(self.model.live_items.iter())
            .filter(|message| message.role == "tool")
            .map(|message| message.content.clone())
            .filter(|content| content.starts_with("workflow_run\n"))
            .collect::<HashSet<_>>();
        Arc::make_mut(&mut self.workflow_statuses).retain(|marker, _| markers.contains(marker));
        Arc::make_mut(&mut self.workflow_pending).retain(|marker| markers.contains(marker));
        for marker in markers {
            if !self.workflow_statuses.contains_key(&marker) {
                self.request_workflow_status(marker);
            }
        }
    }

    fn request_workflow_status(&mut self, marker: String) {
        if !Arc::make_mut(&mut self.workflow_pending).insert(marker.clone()) {
            return;
        }
        let result = self
            .active_host()
            .ok_or_else(|| "The xd host is offline.".to_owned())
            .and_then(|host| host.workflow_status(&marker));
        if let Err(error) = result {
            Arc::make_mut(&mut self.workflow_pending).remove(&marker);
            Arc::make_mut(&mut self.workflow_statuses)
                .insert(marker, serde_json::json!({"ok": false, "error": error}));
        }
    }

    fn schedule_workflow_refresh(&mut self, marker: String, cx: &mut Context<Self>) {
        self.schedule_workflow_clock(marker.clone(), cx);
        let terminal = self
            .workflow_statuses
            .get(&marker)
            .is_some_and(workflow_status_terminal);
        let selected_chat = self.model.selected_chat.clone();
        if terminal || selected_chat.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(10)).await;
            let _ = this.update(cx, |this, _cx| {
                let still_visible = this.model.selected_chat == selected_chat
                    && this
                        .model
                        .messages
                        .iter()
                        .chain(this.model.live_items.iter())
                        .any(|message| message.role == "tool" && message.content == marker);
                if still_visible {
                    this.request_workflow_status(marker);
                }
            });
        })
        .detach();
    }

    fn schedule_workflow_clock(&mut self, marker: String, cx: &mut Context<Self>) {
        let active = self
            .workflow_statuses
            .get(&marker)
            .is_some_and(workflow_clock_active);
        if !active || !self.workflow_ticking.insert(marker.clone()) {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                let keep_ticking = this
                    .update(cx, |this, cx| {
                        let visible = this
                            .model
                            .messages
                            .iter()
                            .chain(this.model.live_items.iter())
                            .any(|message| message.role == "tool" && message.content == marker);
                        let active = visible
                            && this
                                .workflow_statuses
                                .get(&marker)
                                .is_some_and(workflow_clock_active);
                        if !active {
                            this.workflow_ticking.remove(&marker);
                            return false;
                        }
                        this.invalidate_workflow_rows(&marker);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_ticking {
                    break;
                }
            }
        })
        .detach();
    }

    fn send_composer(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let text = self.composer.trim().to_owned();
        let attachments = self.model.draft_attachments.clone();
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if text.is_empty() && attachments.is_empty() {
            return;
        }
        let Some(host) = self.active_host().cloned() else {
            self.model.connection_error = Some("xd is not connected to a host.".into());
            return;
        };
        if let Err(error) = host.send_message(
            &chat_id,
            &text,
            &attachments,
            self.settings.git_writer.backend(),
            self.settings.git_writer_model.as_deref(),
        ) {
            self.model.connection_error = Some(error);
            return;
        }

        let optimistic = self.apply_optimistic_send(&text);
        self.sending = true;
        self.pending_send = Some(PendingSend {
            chat_id: chat_id.clone(),
            text,
            attachments,
            restore: true,
            optimistic,
        });
        self.set_composer_text(String::new(), cx);
        self.model.draft_attachments.clear();
        self.draft_dirty = true;
        self.attachments_dirty = true;
        self.attachment_generation = self.attachment_generation.saturating_add(1);
        self.draft_generation = self.draft_generation.saturating_add(1);
        let _ = host.set_draft(&chat_id, "", Some(&[]), Some(self.attachment_generation));
        cx.notify();
    }

    fn apply_optimistic_send(&mut self, text: &str) -> OptimisticSend {
        if self.model.working {
            let index = self.model.queue.len();
            self.model.queue.push(text.to_owned());
            return OptimisticSend::Queued { index };
        }

        let message_index = (!text.is_empty()).then(|| {
            let index = self.model.messages.len();
            self.model
                .messages
                .push(Message::new(None, "user", text, None));
            self.transcript_snapshot.sync_messages(&self.model);
            self.sync_transcript_count(false);
            index
        });
        self.model.start_working();
        self.model.has_messages = true;
        if let Some(summary) = self
            .model
            .chats
            .iter_mut()
            .find(|summary| Some(summary.id.as_str()) == self.model.selected_chat.as_deref())
        {
            summary.working = true;
        }
        OptimisticSend::Started { message_index }
    }

    fn rollback_optimistic_send(&mut self, pending: &PendingSend) {
        if self.model.selected_chat.as_deref() != Some(pending.chat_id.as_str()) {
            return;
        }
        match pending.optimistic {
            OptimisticSend::Started { message_index } => {
                if let Some(index) = message_index
                    && self.model.messages.get(index).is_some_and(|message| {
                        message.id.is_none()
                            && message.role == "user"
                            && message.content == pending.text
                    })
                {
                    self.model.messages.remove(index);
                    self.transcript_snapshot.sync_messages(&self.model);
                    self.sync_transcript_count(false);
                }
                self.model.stop_working();
                self.model.has_messages = self
                    .model
                    .messages
                    .iter()
                    .any(|message| message.role == "user");
                if let Some(summary) = self.model.chats.iter_mut().find(|summary| {
                    Some(summary.id.as_str()) == self.model.selected_chat.as_deref()
                }) {
                    summary.working = false;
                }
            }
            OptimisticSend::Queued { index } => {
                if self.model.queue.get(index) == Some(&pending.text) {
                    self.model.queue.remove(index);
                }
            }
        }
    }

    fn restore_pending_send(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_send.take() else {
            return;
        };
        self.rollback_optimistic_send(&pending);
        if !pending.restore {
            return;
        }
        let restored = match (pending.text.is_empty(), self.composer.is_empty()) {
            (false, false) => format!("{}\n{}", pending.text, self.composer),
            (false, true) => pending.text,
            (true, false) => self.composer.clone(),
            (true, true) => String::new(),
        };
        self.set_composer_text(restored, cx);
        self.model.draft_attachments = pending.attachments;
        self.draft_dirty = true;
        self.attachments_dirty = true;
        self.attachment_generation = self.attachment_generation.saturating_add(1);
    }

    fn send_shortcut(&mut self, prompt: String) -> bool {
        if self.sending || prompt.trim().is_empty() {
            return false;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return false;
        };
        let Some(host) = self.active_host().cloned() else {
            self.model.connection_error = Some("xd is not connected to a host.".into());
            return false;
        };
        if let Err(error) = host.send_message(
            &chat_id,
            &prompt,
            &[],
            self.settings.git_writer.backend(),
            self.settings.git_writer_model.as_deref(),
        ) {
            self.model.connection_error = Some(error);
            return false;
        }
        self.sending = true;
        let optimistic = self.apply_optimistic_send(&prompt);
        self.pending_send = Some(PendingSend {
            chat_id,
            text: prompt,
            attachments: Vec::new(),
            restore: false,
            optimistic,
        });
        true
    }

    fn clear_question(&mut self, cx: &mut Context<Self>) {
        self.open_question = None;
        self.question_answer.clear();
        self.question_input
            .update(cx, |input, cx| input.set_text("", cx));
    }

    fn sync_question_from_history(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let pending = (!self.model.working && self.model.queue.is_empty())
            .then(|| {
                self.model
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.role != "duration")
            })
            .flatten()
            .filter(|message| message.role == "assistant")
            .and_then(|message| markdown::ask(&message.content))
            .map(|ask| OpenQuestion {
                chat_id: chat_id.to_owned(),
                question: ask.question,
                options: ask.options,
                accepts_input: ask.accepts_input,
            });
        if pending != self.open_question {
            self.open_question = pending;
            self.question_answer.clear();
            self.question_input
                .update(cx, |input, cx| input.set_text("", cx));
        }
    }

    fn answer_question(&mut self, answer: String, cx: &mut Context<Self>) {
        if self.send_shortcut(answer) {
            self.clear_question(cx);
            cx.notify();
        }
    }

    fn send_question_input(&mut self, cx: &mut Context<Self>) {
        self.answer_question(self.question_answer.trim().to_owned(), cx);
    }

    fn queue_edit_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(edit) = &mut self.queue_edit
            && edit.submitting.is_none()
        {
            edit.text = text;
            cx.notify();
        }
    }

    fn save_queue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.queue_edit.clone() else {
            return;
        };
        let text = edit.text.trim();
        if text.is_empty() {
            self.model.connection_error = Some("A queued message cannot be empty.".into());
            cx.notify();
            return;
        }
        if text == edit.original {
            self.cancel_queue_edit(cx);
            return;
        }
        if let Some(host) = self.active_host().cloned() {
            if let Err(error) = host.edit_queue(&edit.chat_id, edit.index, &edit.original, text) {
                self.model.connection_error = Some(error);
            } else {
                if self.model.selected_chat.as_deref() == Some(edit.chat_id.as_str())
                    && self.model.queue.get(edit.index) == Some(&edit.original)
                {
                    self.model.queue[edit.index] = text.to_owned();
                }
                self.cancel_queue_edit(cx);
            }
        }
        cx.notify();
    }

    fn cancel_queue_edit(&mut self, cx: &mut Context<Self>) {
        self.queue_edit = None;
        self.queue_edit_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn make_shortcut_row(&mut self, _prompt: String, _cx: &mut Context<Self>) -> ShortcutRow {
        ShortcutRow
    }

    fn replace_shortcut_rows(&mut self, prompts: Vec<String>, cx: &mut Context<Self>) {
        let rows = prompts
            .into_iter()
            .map(|prompt| self.make_shortcut_row(prompt, cx))
            .collect();
        if let Some(panel) = &mut self.shortcut_panel {
            panel.rows = rows;
        }
    }

    fn set_composer_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.composer.clone_from(&text);
        self.composer_input
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    fn attach_clipboard_image(
        &mut self,
        format: gpui::ImageFormat,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        if self.model.selected_chat.is_none() {
            return;
        }
        if format != gpui::ImageFormat::Png {
            self.model.connection_error = Some(
                "That clipboard image is not available as PNG. Save it as PNG and attach it."
                    .into(),
            );
            cx.notify();
            return;
        }
        if self.model.draft_attachments.len() >= MAX_ATTACHMENTS {
            self.model.connection_error = Some(format!(
                "A message can contain at most {MAX_ATTACHMENTS} images."
            ));
            cx.notify();
            return;
        }
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            self.model.connection_error = Some("That image is larger than 10 MiB.".into());
            cx.notify();
            return;
        }
        let total = self
            .model
            .draft_attachments
            .iter()
            .map(|attachment| attachment.preview.bytes.len())
            .sum::<usize>();
        if total.saturating_add(bytes.len()) > MAX_TOTAL_ATTACHMENT_BYTES {
            self.model.connection_error = Some("Attached images exceed the 20 MiB limit.".into());
            cx.notify();
            return;
        }
        let name = format!(
            "paste-{}.png",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        match Attachment::from_png(name, bytes) {
            Ok(attachment) => {
                self.model.draft_attachments.push(attachment);
                self.attachments_dirty = true;
                self.attachment_generation = self.attachment_generation.saturating_add(1);
                self.schedule_draft_sync(cx);
            }
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    fn composer_changed(&mut self, text: String, cx: &mut Context<Self>) {
        self.composer = text;
        self.draft_dirty = true;
        self.schedule_draft_sync(cx);
        cx.notify();
    }

    fn schedule_draft_sync(&mut self, cx: &mut Context<Self>) {
        self.draft_generation = self.draft_generation.saturating_add(1);
        let generation = self.draft_generation;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(250)).await;
            let _ = this.update(cx, |this, _cx| {
                if this.draft_generation == generation {
                    this.sync_draft();
                }
            });
        })
        .detach();
    }

    fn sync_draft(&mut self) {
        if !self.draft_dirty && !self.attachments_dirty {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if let Some(host) = self.active_host().cloned() {
            let attachments = self
                .attachments_dirty
                .then_some(self.model.draft_attachments.as_slice());
            let attachment_generation = attachments.map(|_| self.attachment_generation);
            if let Err(error) =
                host.set_draft(&chat_id, &self.composer, attachments, attachment_generation)
            {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn event_is_active(&self, body: &Value) -> bool {
        body.get("chat").and_then(Value::as_str) == self.model.selected_chat.as_deref()
    }

    fn chat_is_active(&self, chat_id: &str) -> bool {
        self.model.selected_chat.as_deref() == Some(chat_id)
    }

    fn sidebar_target_exists(&self, target: &SidebarTarget) -> bool {
        match target {
            SidebarTarget::Folder(folder_id) => self
                .model
                .folders
                .iter()
                .any(|folder| &folder.id == folder_id),
            SidebarTarget::Chat(chat_id) => self.model.chats.iter().any(|chat| &chat.id == chat_id),
        }
    }

    fn toggle_folder_collapsed(&mut self, folder_id: String, cx: &mut Context<Self>) {
        if !self.collapsed_folders.remove(&folder_id) {
            self.collapsed_folders.insert(folder_id);
        }
        self.persist_collapsed_folders();
        cx.notify();
    }

    fn persist_collapsed_folders(&mut self) {
        let key = self.current_connection_key();
        let mut collapsed = self.collapsed_folders.iter().cloned().collect::<Vec<_>>();
        collapsed.sort();
        let mut changed = self.settings.collapsed_folder_sets.get(&key) != Some(&collapsed);
        self.settings
            .collapsed_folder_sets
            .insert(key, collapsed.clone());
        if self.active_endpoint == ChatEndpoint::Local
            && self.settings.collapsed_folders != collapsed
        {
            self.settings.collapsed_folders = collapsed;
            changed = true;
        }
        if !changed {
            return;
        }
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
    }

    fn sync_transcript_count(&self, reset: bool) {
        let count = self.model.display_message_count();
        if reset {
            self.transcript.reset(count);
            return;
        }
        let old_count = self.transcript.item_count();
        if count > old_count {
            self.transcript
                .splice(old_count..old_count, count - old_count);
        } else if count < old_count {
            self.transcript.splice(count..old_count, 0);
        }
    }

    fn invalidate_live_render(&mut self) {
        self.live_render_generation = self.live_render_generation.saturating_add(1);
        self.live_render_scheduled = None;
    }

    fn schedule_live_text_render(&mut self, cx: &mut Context<Self>) {
        self.live_render_generation = self.live_render_generation.saturating_add(1);
        if self.live_render_scheduled.is_some() {
            return;
        }
        let token = self.live_render_generation;
        let chat_id = self.model.selected_chat.clone();
        self.live_render_scheduled = Some(token);
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(16)).await;
            let _ = this.update(cx, |this, cx| {
                if this.live_render_scheduled != Some(token) || this.model.selected_chat != chat_id
                {
                    return;
                }
                this.live_render_scheduled = None;
                if this.model.live_text.is_empty() {
                    return;
                }
                let had_live_text = this.transcript_snapshot.live_text.is_some();
                this.transcript_snapshot.sync_live_text(&this.model);
                let index = this.model.messages.len() + this.model.live_items.len();
                let anchor = this.transcript.logical_scroll_top();
                if had_live_text {
                    this.transcript.splice(index..index + 1, 1);
                } else {
                    this.transcript.splice(index..index, 1);
                }
                this.transcript.scroll_to(anchor);
                cx.notify();
            });
        })
        .detach();
    }

    fn invalidate_workflow_rows(&self, marker: &str) {
        let indices = workflow_row_indices(&self.model, marker);
        if indices.is_empty() {
            return;
        }
        let anchor = self.transcript.logical_scroll_top();
        for index in indices {
            self.transcript.splice(index..index + 1, 1);
        }
        self.transcript.scroll_to(anchor);
    }
}

impl XdDesktop {
    fn show_minimal_projects(&mut self, cx: &mut Context<Self>) {
        let project_id = match &self.minimal_route {
            MinimalRoute::Projects { project_id } => project_id.clone(),
            MinimalRoute::Sessions { project_id } => project_id.clone(),
            MinimalRoute::Cli { project_id, .. } => Some(project_id.clone()),
        };
        self.minimal_route = MinimalRoute::Projects { project_id };
        self.minimal_new_tab_open = false;
        self.stash_terminal_panel();
        self.model.selected_chat = None;
        self.sync_terminal_input_mode(cx);
        cx.notify();
    }

    fn show_minimal_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let requested_project_id = match &self.minimal_route {
            MinimalRoute::Projects { project_id } | MinimalRoute::Sessions { project_id } => {
                project_id.clone()
            }
            MinimalRoute::Cli { project_id, .. } => Some(project_id.clone()),
        };
        let last_chat = self.cached_last_chat();
        if let Some((project_id, chat_id, agent)) =
            resumable_session(last_chat.as_deref(), &self.model.chats)
        {
            self.open_minimal_session(project_id, chat_id, agent, window, cx);
            return;
        }
        self.minimal_route = MinimalRoute::Sessions {
            project_id: requested_project_id
                .or_else(|| self.model.folders.first().map(|folder| folder.id.clone())),
        };
        self.stash_terminal_panel();
        self.model.selected_chat = None;
        self.sync_terminal_input_mode(cx);
        cx.notify();
    }

    fn open_minimal_session(
        &mut self,
        project_id: String,
        chat_id: String,
        agent: AgentCli,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_minimal_session(project_id, chat_id, agent, cx);
        let focus = self.terminal_input.read(cx).focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    fn select_minimal_session(
        &mut self,
        project_id: String,
        chat_id: String,
        agent: AgentCli,
        cx: &mut Context<Self>,
    ) {
        self.minimal_new_tab_open = false;
        let same_panel = self
            .terminal_panel
            .as_ref()
            .is_some_and(|panel| panel.chat_id == chat_id && panel.agent == Some(agent));
        if self.model.selected_chat.as_deref() != Some(chat_id.as_str()) {
            self.model.select_chat(chat_id.clone());
            self.remember_last_chat(&chat_id);
            self.invalidate_live_render();
            self.transcript_snapshot = TranscriptSnapshot::default();
            self.transcript_loaded = false;
            self.transcript_loading = false;
            self.transcript_page_loading = false;
            self.transcript.reset(0);
        }
        self.minimal_route = MinimalRoute::Cli {
            project_id,
            chat_id: chat_id.clone(),
            agent,
        };
        if !same_panel {
            self.stash_terminal_panel();
            let restored = self.restore_terminal_panel(&chat_id, agent);
            if !restored {
                self.terminal_panel = Some(Self::new_agent_terminal_panel(chat_id, agent));
            }
            self.sync_terminal_input_mode(cx);
            self.terminal_scroll.scroll_to_bottom();
        }
        // Tree hydration pre-creates empty cached panels, so both a new panel
        // and a restored panel need the selected chat snapshot before their
        // terminal can use its working directory. Re-running this for an
        // already selected panel also makes clicking a loading card retry the
        // request instead of leaving it stuck forever.
        self.refresh_terminal_sessions(cx);
        cx.notify();
    }

    fn reconcile_minimal_navigation(&mut self, cx: &mut Context<Self>) {
        if let Some((project_id, chat_id)) = self.pending_minimal_session.clone()
            && let Some(agent) = self
                .model
                .chats
                .iter()
                .find(|chat| chat.id == chat_id && chat.folder == project_id)
                .and_then(|chat| AgentCli::from_backend(&chat.backend))
        {
            self.pending_minimal_session = None;
            self.select_minimal_session(project_id, chat_id, agent, cx);
            return;
        }

        let next = reconcile_route(&self.minimal_route, &self.model.folders, &self.model.chats);
        if next == self.minimal_route {
            return;
        }
        match next.clone() {
            MinimalRoute::Cli {
                project_id,
                chat_id,
                agent,
            } => self.select_minimal_session(project_id, chat_id, agent, cx),
            MinimalRoute::Sessions { .. } => {
                self.minimal_route = next;
                self.minimal_new_tab_open = false;
                self.stash_terminal_panel();
                self.model.selected_chat = None;
                self.sync_terminal_input_mode(cx);
                cx.notify();
            }
            MinimalRoute::Projects { .. } => {
                self.minimal_route = next;
                self.minimal_new_tab_open = false;
                self.stash_terminal_panel();
                self.model.selected_chat = None;
                cx.notify();
            }
        }
    }

    fn save_minimal_chat_create(&mut self, cx: &mut Context<Self>) {
        let Some(folder_id) = self.creating_chat_folder.clone() else {
            return;
        };
        if self.chat_create_submitting || self.chat_create_worktrees_loading {
            return;
        }
        let Some(worktree) = self.chat_create_worktree.clone() else {
            return;
        };
        let title = chat_create_title(&self.chat_create_title).to_owned();
        let agent = self.minimal_new_session_agent.protocol_name();
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| {
                host.new_chat_with_backend_in_worktree(&folder_id, &title, agent, &worktree)
            });
        match result {
            Ok(()) => {
                self.chat_create_title = title;
                self.chat_create_submitting = true;
            }
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    fn choose_minimal_theme(&mut self, theme: ThemePreset, cx: &mut Context<Self>) {
        self.settings.theme = theme;
        self.minimal_theme_open = false;
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn toggle_minimal_theme_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.minimal_theme_open = !self.minimal_theme_open;
        if self.minimal_theme_open {
            self.minimal_new_tab_open = false;
            self.focus_minimal_popup(self.minimal_popup_focus.clone(), window, cx);
        } else {
            self.restore_minimal_popup_focus(window);
        }
        cx.notify();
    }

    fn close_minimal_theme_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.minimal_theme_open = false;
        self.restore_minimal_popup_focus(window);
        cx.notify();
    }

    fn toggle_minimal_all_permissions(&mut self, cx: &mut Context<Self>) {
        self.settings.allow_all_permissions = !self.settings.allow_all_permissions;
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn render_minimal_terminal(
        &mut self,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let terminal_input = self.terminal_input.clone();
        let terminal_focus = self.terminal_input.read(cx).focus_handle(cx);
        let minimal_popup_focus = self.minimal_popup_focus.clone();
        let desktop = cx.entity();
        let Some(panel) = self.terminal_panel.as_ref() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(rgb(colors.muted))
                .child("Choose a session to start its CLI.")
                .into_any_element();
        };

        let selected_id = panel.selected.clone();
        let output = panel
            .selected()
            .map(|session| session.screen.rendered_with_cursor());
        let (output_text, output_spans, output_cursor, output_links) = output
            .map(|output| (output.text, output.spans, output.cursor, output.links))
            .unwrap_or_else(|| (String::new(), Vec::new(), None, Vec::new()));
        let mut terminal_links = output_links
            .into_iter()
            .map(|link| (link.range, link.url))
            .collect::<Vec<_>>();
        let detected_links = markdown::web_links(&output_text)
            .into_iter()
            .filter(|(range, _)| {
                !terminal_links
                    .iter()
                    .any(|(native, _)| native.start < range.end && range.start < native.end)
            })
            .collect::<Vec<_>>();
        terminal_links.extend(detected_links);
        terminal_links.sort_by_key(|(range, _)| range.start);
        let highlights = output_spans
            .into_iter()
            .map(|span| {
                (
                    span.range,
                    HighlightStyle {
                        color: span.style.foreground.map(|color| rgb(color).into()),
                        background_color: span.style.background.map(|color| rgb(color).into()),
                        font_weight: span.style.bold.then_some(FontWeight::BOLD),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        let cursor_highlight = if self.terminal_cursor_visible && terminal_focus.is_focused(window)
        {
            output_cursor.map(|cursor| {
                (
                    cursor,
                    HighlightStyle {
                        color: Some(rgb(colors.background).into()),
                        background_color: Some(rgb(colors.text).into()),
                        ..Default::default()
                    },
                )
            })
        } else {
            None
        };
        let highlights = insert_terminal_cursor_highlight(highlights, cursor_highlight);
        let output_text: SharedString = output_text.into();
        let output_range = 0..output_text.len();
        let output = StyledText::new(output_text.clone()).with_highlights(highlights);
        let output_layout = output.layout().clone();
        let output = if let Some(terminal_id) = selected_id.as_deref() {
            let scope = format!("minimal-terminal-output:{terminal_id}");
            let document = scoped_element_id(&scope, 0);
            if terminal_links.is_empty() {
                selectable_in_document(
                    document,
                    document,
                    output_text,
                    output_range,
                    output_layout,
                    output,
                )
                .with_selection_scroll(self.terminal_scroll.clone())
                .into_any_element()
            } else {
                let (ranges, urls) = terminal_links.into_iter().unzip::<_, _, Vec<_>, Vec<_>>();
                selectable_links_in_document(
                    document,
                    document,
                    output_text,
                    output_range,
                    output_layout,
                    output,
                    ranges,
                    move |index, _, cx| {
                        if let Some(url) = urls.get(index) {
                            cx.open_url(url);
                        }
                    },
                )
                .with_selection_scroll(self.terminal_scroll.clone())
                .into_any_element()
            }
        } else {
            output.into_any_element()
        };
        let active = selected_id.is_some();
        let tabs = panel
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let terminal_id = session.id.clone();
                let close_id = terminal_id.clone();
                let selected = selected_id.as_deref() == Some(terminal_id.as_str());
                div()
                    .id(("minimal-terminal-tab", index))
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_2()
                    .border_color(rgb(if selected {
                        colors.accent
                    } else {
                        colors.surface
                    }))
                    .text_xs()
                    .text_color(rgb(if selected { colors.text } else { colors.muted }))
                    .cursor_pointer()
                    .hover(|style| style.text_color(rgb(colors.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_terminal(terminal_id.clone(), cx);
                    }))
                    .child(session.title.clone())
                    .child(
                        div()
                            .id(("minimal-close-terminal", index))
                            .px_1()
                            .rounded_sm()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.kill_terminal_id(close_id.clone(), cx);
                            }))
                            .child("×"),
                    )
            })
            .collect::<Vec<_>>();

        let output_scroller = div()
            .id("minimal-terminal-output")
            .size_full()
            .whitespace_nowrap()
            .overflow_y_scroll()
            .track_scroll(&self.terminal_scroll)
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(19.0)).y;
                let bytes = terminal_mouse_scroll_bytes(f32::from(delta));
                if !bytes.is_empty() {
                    this.send_terminal_input(&bytes, cx);
                    cx.stop_propagation();
                }
            }))
            .p(px(TERMINAL_OUTPUT_PADDING))
            .track_focus(&terminal_focus)
            .when(active, |output| {
                output
                    .cursor(CursorStyle::IBeam)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let focus = this.terminal_input.read(cx).focus_handle(cx);
                        window.focus(&focus);
                    }))
            })
            .child(output);
        let measurement_canvas = canvas(
            {
                let desktop = desktop.clone();
                move |bounds, window, cx| {
                    const SAMPLE: &str = "MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM";
                    let style = window.text_style();
                    let run = TextRun {
                        len: SAMPLE.len(),
                        font: style.font(),
                        color: style.color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window.text_system().shape_line(
                        SharedString::from(SAMPLE),
                        style.font_size.to_pixels(window.rem_size()),
                        &[run],
                        None,
                    );
                    let cell_width = f32::from(line.width) / SAMPLE.len() as f32;
                    let geometry = terminal_geometry(
                        f32::from(bounds.size.width),
                        f32::from(bounds.size.height),
                        cell_width,
                        19.0,
                    );
                    window.defer(cx, move |_, cx| {
                        desktop.update(cx, |this, cx| {
                            this.resize_terminal_viewport(geometry.0, geometry.1, cx);
                        });
                    });
                }
            },
            |_, _, _, _| {},
        );

        div()
            .size_full()
            .min_h_0()
            .relative()
            .flex()
            .flex_col()
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .child(
                div()
                    .h(px(42.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .children(tabs)
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("minimal-new-terminal")
                            .size(px(30.0))
                            .mr_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_minimal_new_tab_menu(window, cx)
                            }))
                            .child(plus_icon(colors.text)),
                    ),
            )
            .when(panel.loading && panel.sessions.is_empty(), |pane| {
                pane.child(
                    div()
                        .absolute()
                        .top(px(54.0))
                        .left(px(16.0))
                        .text_xs()
                        .text_color(rgb(colors.muted))
                        .child("Starting CLI…"),
                )
            })
            .when_some(panel.error.clone(), |pane, error| {
                pane.child(
                    div()
                        .absolute()
                        .top(px(54.0))
                        .left(px(16.0))
                        .right(px(16.0))
                        .text_xs()
                        .text_color(rgb(0xd86f7c))
                        .child(error),
                )
            })
            .child(
                div()
                    .id("minimal-terminal-viewport")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .font_family(MONO)
                    .text_size(px(13.0))
                    .line_height(px(19.0))
                    .text_color(rgb(colors.text))
                    .child(measurement_canvas.absolute().inset_0())
                    .child(output_scroller)
                    .child(terminal_input),
            )
            .when(self.minimal_new_tab_open, |pane| {
                pane.child(
                    div()
                        .id("minimal-new-tab-shield")
                        .occlude()
                        .absolute()
                        .inset_0()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.minimal_new_tab_open = false;
                            this.restore_minimal_popup_focus(window);
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("minimal-new-tab-menu")
                        .occlude()
                        .absolute()
                        .top(px(38.0))
                        .right(px(8.0))
                        .w(px(168.0))
                        .p_1()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.background))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .track_focus(&minimal_popup_focus)
                        .child(
                            div()
                                .id("minimal-new-shell-tab")
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(colors.text))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_minimal_terminal_tab(None, window, cx)
                                }))
                                .child(div().w(px(18.0)).font_family(MONO).text_xs().child(">_"))
                                .child("Terminal"),
                        )
                        .child(
                            div()
                                .id("minimal-new-codex-tab")
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(colors.text))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_minimal_terminal_tab(
                                        Some(AgentCli::Codex),
                                        window,
                                        cx,
                                    )
                                }))
                                .child(
                                    svg()
                                        .path(CODEX_ICON)
                                        .size(px(18.0))
                                        .text_color(rgb(colors.text)),
                                )
                                .child("Codex"),
                        )
                        .child(
                            div()
                                .id("minimal-new-claude-tab")
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(colors.text))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_minimal_terminal_tab(
                                        Some(AgentCli::Claude),
                                        window,
                                        cx,
                                    )
                                }))
                                .child(
                                    svg()
                                        .path(CLAUDE_ICON)
                                        .size(px(18.0))
                                        .text_color(rgb(colors.text)),
                                )
                                .child("Claude"),
                        )
                        .child(
                            div()
                                .id("minimal-new-jcode-tab")
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(colors.text))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_minimal_terminal_tab(
                                        Some(AgentCli::Jcode),
                                        window,
                                        cx,
                                    )
                                }))
                                .child(
                                    svg()
                                        .path(JCODE_ICON)
                                        .size(px(18.0))
                                        .text_color(rgb(colors.text)),
                                )
                                .child("JCode"),
                        )
                        .child(
                            div()
                                .id("minimal-new-copilot-tab")
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(colors.text))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_minimal_terminal_tab(
                                        Some(AgentCli::Copilot),
                                        window,
                                        cx,
                                    )
                                }))
                                .child(
                                    svg()
                                        .path(COPILOT_ICON)
                                        .size(px(18.0))
                                        .text_color(rgb(colors.text)),
                                )
                                .child("Copilot"),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_minimal_window_controls(&self, colors: ThemeColors) -> gpui::AnyElement {
        div()
            .h_full()
            .flex()
            .items_center()
            .when(!cfg!(target_os = "macos"), |titlebar| {
                titlebar.child(
                    div()
                        .id("minimal-window-minimize")
                        .w(px(38.0))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .cursor_pointer()
                        .hover(|style| {
                            style
                                .bg(rgb(colors.surface_high))
                                .text_color(rgb(colors.text))
                        })
                        .window_control_area(WindowControlArea::Min)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, window, _| window.minimize_window())
                        .child("−"),
                )
            })
            .when(!cfg!(target_os = "macos"), |titlebar| {
                titlebar.child(
                    div()
                        .id("minimal-window-maximize")
                        .w(px(38.0))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(rgb(colors.muted))
                        .cursor_pointer()
                        .hover(|style| {
                            style
                                .bg(rgb(colors.surface_high))
                                .text_color(rgb(colors.text))
                        })
                        .window_control_area(WindowControlArea::Max)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, window, _| window.zoom_window())
                        .child("□"),
                )
            })
            .when(!cfg!(target_os = "macos"), |titlebar| {
                titlebar.child(
                    div()
                        .id("minimal-window-close")
                        .w(px(42.0))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x5a252b)).text_color(rgb(0xffffff)))
                        .window_control_area(WindowControlArea::Close)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, window, _| window.remove_window())
                        .child("×"),
                )
            })
            .into_any_element()
    }

    fn render_minimal_product_nav(
        &mut self,
        colors: ThemeColors,
        titlebar: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let projects_active = matches!(self.minimal_route, MinimalRoute::Projects { .. });
        let sessions_active = matches!(
            self.minimal_route,
            MinimalRoute::Sessions { .. } | MinimalRoute::Cli { .. }
        );
        let create_session_for = match &self.minimal_route {
            MinimalRoute::Cli { project_id, .. } => Some(project_id.clone()),
            MinimalRoute::Sessions { project_id } => project_id.clone(),
            MinimalRoute::Projects { .. } => None,
        };
        let connected = self.model.connected;
        let remote_active = self.active_endpoint == ChatEndpoint::Remote;
        let runtime_label = if remote_active { "Remote" } else { "Local" };

        div()
            .id("minimal-product-nav")
            .h(px(62.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .when(titlebar, |nav| {
                nav.on_mouse_down(MouseButton::Left, |event, window, _| {
                    if event.click_count >= 2 {
                        if cfg!(target_os = "macos") {
                            window.titlebar_double_click();
                        } else {
                            window.zoom_window();
                        }
                    } else {
                        window.start_window_move();
                    }
                })
            })
            .child(
                div()
                    .h_full()
                    .min_w_0()
                    .flex_1()
                    .pl(px(if titlebar && cfg!(target_os = "macos") {
                        86.0
                    } else {
                        22.0
                    }))
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .h_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .pr_4()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(19.0))
                            .text_color(rgb(colors.text))
                            .when(titlebar, |brand| {
                                brand.window_control_area(WindowControlArea::Drag)
                            })
                            .child(xd_mark(colors.accent))
                            .child("xd"),
                    )
                    .child(
                        div()
                            .id("minimal-projects-tab")
                            .h(px(38.0))
                            .px_4()
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(rgb(if projects_active {
                                colors.surface_high
                            } else {
                                colors.surface
                            }))
                            .text_base()
                            .font_weight(if projects_active {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(rgb(if projects_active {
                                colors.text
                            } else {
                                colors.muted
                            }))
                            .cursor_pointer()
                            .hover(|style| {
                                style
                                    .bg(rgb(colors.surface_high))
                                    .text_color(rgb(colors.text))
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| this.show_minimal_projects(cx)))
                            .child("Projects"),
                    )
                    .child(
                        div()
                            .id("minimal-sessions-tab")
                            .h(px(38.0))
                            .px_4()
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(rgb(if sessions_active {
                                colors.surface_high
                            } else {
                                colors.surface
                            }))
                            .text_base()
                            .font_weight(if sessions_active {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(rgb(if sessions_active {
                                colors.text
                            } else {
                                colors.muted
                            }))
                            .cursor_pointer()
                            .hover(|style| {
                                style
                                    .bg(rgb(colors.surface_high))
                                    .text_color(rgb(colors.text))
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_minimal_sessions(window, cx)
                            }))
                            .child("Sessions"),
                    )
                    .child(
                        div()
                            .id("minimal-global-create")
                            .ml_1()
                            .size(px(38.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(rgb(colors.accent))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.accent_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(project_id) = create_session_for.clone() {
                                    this.begin_chat_create(project_id, window, cx);
                                } else {
                                    this.begin_workspace_create(window, cx);
                                }
                            }))
                            .child(plus_icon(colors.accent_text)),
                    )
                    .child(
                        div()
                            .h_full()
                            .min_w(px(20.0))
                            .flex_1()
                            .when(titlebar, |spacer| {
                                spacer.window_control_area(WindowControlArea::Drag)
                            }),
                    )
                    .child(
                        div()
                            .id("minimal-runtime")
                            .h(px(34.0))
                            .px_3()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_full()
                            .bg(rgb(colors.background))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(colors.text))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if remote_active {
                                    this.disconnect_remote_runtime(cx);
                                } else {
                                    this.open_remote(window, cx);
                                }
                            }))
                            .child(div().size(px(8.0)).rounded_full().bg(rgb(if connected {
                                0x36c75c
                            } else {
                                0xb74c58
                            })))
                            .child(runtime_label),
                    )
                    .child(
                        div()
                            .id("minimal-theme")
                            .size(px(34.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_lg()
                            .text_color(rgb(colors.muted))
                            .cursor_pointer()
                            .hover(|style| {
                                style
                                    .bg(rgb(colors.surface_high))
                                    .text_color(rgb(colors.text))
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_minimal_theme_popup(window, cx)
                            }))
                            .child("⚙"),
                    ),
            )
            .when(titlebar && !cfg!(target_os = "macos"), |nav| {
                nav.child(self.render_minimal_window_controls(colors))
            })
            .into_any_element()
    }

    fn render_minimal_titlebar(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.render_minimal_product_nav(colors, true, cx)
    }

    fn render_minimal_context_toolbar(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (project_id, route_agent, cli_active) = match &self.minimal_route {
            MinimalRoute::Projects { project_id } => (project_id.clone(), None, false),
            MinimalRoute::Sessions { project_id } => (project_id.clone(), None, false),
            MinimalRoute::Cli {
                project_id, agent, ..
            } => (Some(project_id.clone()), Some(*agent), true),
        };
        let project_name = project_id
            .as_deref()
            .and_then(|project_id| {
                self.model
                    .folders
                    .iter()
                    .find(|folder| folder.id == project_id)
            })
            .map(|folder| folder.name.clone())
            .unwrap_or_else(|| "xd".into());
        let context_label = if cli_active {
            project_name
        } else {
            "Projects".into()
        };
        let selected_agent = self
            .terminal_panel
            .as_ref()
            .and_then(TerminalPanel::selected)
            .and_then(|terminal| terminal.agent)
            .or(route_agent);
        let has_terminal = self
            .terminal_panel
            .as_ref()
            .and_then(TerminalPanel::selected)
            .is_some();

        div()
            .id("minimal-context-toolbar")
            .h(px(54.0))
            .w_full()
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.sidebar))
            .when(cli_active, |toolbar| {
                toolbar.child(
                    div()
                        .id("minimal-board-back")
                        .size(px(32.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .text_xl()
                        .text_color(rgb(colors.text))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(colors.surface_high)))
                        .on_click(cx.listener(|this, _, _, cx| this.show_minimal_projects(cx)))
                        .child("←"),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(colors.text))
                    .child(
                        div()
                            .min_w_0()
                            .max_w(px(320.0))
                            .overflow_hidden()
                            .child(context_label),
                    )
                    .when(cli_active, |breadcrumbs| {
                        breadcrumbs
                            .child(div().text_color(rgb(colors.muted)).child("/"))
                            .child("Sessions")
                            .child(div().text_color(rgb(colors.muted)).child("⌄"))
                    }),
            )
            .child(div().flex_1())
            .when_some(selected_agent, |toolbar, agent| {
                toolbar.child(
                    div()
                        .id("minimal-session-agent")
                        .h(px(32.0))
                        .px_3()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .font_family(MONO)
                        .text_xs()
                        .text_color(rgb(colors.text))
                        .child(
                            svg()
                                .path(match agent {
                                    AgentCli::Codex => CODEX_ICON,
                                    AgentCli::Claude => CLAUDE_ICON,
                                    AgentCli::Jcode => JCODE_ICON,
                                    AgentCli::Copilot => COPILOT_ICON,
                                })
                                .size(px(15.0))
                                .text_color(rgb(colors.muted)),
                        )
                        .child(agent.protocol_name()),
                )
            })
            .when(has_terminal, |toolbar| {
                toolbar
                    .child(
                        div()
                            .id("minimal-context-terminal")
                            .h(px(30.0))
                            .px_3()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(rgb(colors.surface_high))
                            .font_family(MONO)
                            .text_xs()
                            .text_color(rgb(colors.text))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.selected_surface)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_minimal_terminal_tab(None, window, cx);
                            }))
                            .child(">_"),
                    )
                    .child(div().mx_1().w(px(1.0)).h(px(20.0)).bg(rgb(colors.border)))
                    .child(
                        div()
                            .id("minimal-stop-terminal")
                            .h(px(32.0))
                            .px_2()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xc2524a))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.send_terminal_input(&[3], cx)),
                            )
                            .child(
                                div()
                                    .size(px(12.0))
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(0xc2524a)),
                            )
                            .child("Stop"),
                    )
            })
            .into_any_element()
    }

    fn render_minimal_session_board(
        &mut self,
        colors: ThemeColors,
        active_chat_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let projects = project_cards(&self.model.folders, &self.model.chats);
        let mut groups = Vec::with_capacity(projects.len());
        let mut session_instance = 0usize;

        for (project_index, project) in projects.iter().enumerate() {
            let collapsed = self.collapsed_folders.contains(&project.id);
            let sessions = project_sessions(&project.id, &self.model.chats);
            let mut cards = Vec::with_capacity(sessions.len());

            for session in sessions {
                let instance = session_instance;
                session_instance += 1;
                let selected = active_chat_id == Some(session.id.as_str());
                let emphasized = selected;
                let project_id = project.id.clone();
                let chat_id = session.id.clone();
                let context_chat_id = session.id.clone();
                let context_title = session.title.clone();
                let agent = session.agent;
                let done = !session.working && self.model.unread_chats.contains(&session.id);
                let status_color = if session.working {
                    colors.accent_ink
                } else if done {
                    DONE_STATUS_COLOR
                } else {
                    colors.muted
                };
                let status = if session.working {
                    div()
                        .mt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(status_color))
                        .child(session_status_icon(status_color))
                        .child("Working")
                        .child(working_dots(instance, status_color))
                        .into_any_element()
                } else if done {
                    div()
                        .mt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(status_color))
                        .child(session_status_icon(status_color))
                        .child("Done")
                        .into_any_element()
                } else {
                    div()
                        .mt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(status_color))
                        .child(session_status_icon(status_color))
                        .child("Idle")
                        .into_any_element()
                };

                cards.push(
                    div()
                        .id(("minimal-session-card", instance))
                        .w_full()
                        .min_h(px(104.0))
                        .px_3()
                        .py_3()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(if emphasized {
                            colors.selected_border
                        } else {
                            colors.border
                        }))
                        .bg(rgb(if emphasized {
                            colors.selected_surface
                        } else {
                            colors.surface
                        }))
                        .cursor_pointer()
                        .hover(|style| {
                            style
                                .bg(rgb(if emphasized {
                                    colors.selected_surface
                                } else {
                                    colors.surface_high
                                }))
                                .border_color(rgb(if emphasized {
                                    colors.selected_border
                                } else {
                                    colors.accent
                                }))
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_minimal_session(
                                project_id.clone(),
                                chat_id.clone(),
                                agent,
                                window,
                                cx,
                            );
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                this.open_session_context_menu(
                                    context_chat_id.clone(),
                                    context_title.clone(),
                                    event.position,
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .child(
                            div().flex().items_start().gap_2().child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_base()
                                    .line_height(px(22.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.text))
                                    .child(session.title),
                            ),
                        )
                        .child(
                            div()
                                .mt_2()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_sm()
                                .text_color(rgb(colors.muted))
                                .child(
                                    svg()
                                        .path(GIT_BRANCH_ICON)
                                        .size(px(14.0))
                                        .flex_none()
                                        .text_color(rgb(colors.muted)),
                                )
                                .child(div().min_w_0().flex_1().truncate().child(session.branch)),
                        )
                        .child(status),
                );
            }

            let collapse_project = project.id.clone();
            let create_project = project.id.clone();
            let header = div()
                .id(("minimal-session-group-header", project_index))
                .h(px(28.0))
                .w_full()
                .px_1()
                .flex()
                .items_center()
                .gap_2()
                .rounded_md()
                .cursor_pointer()
                .hover(|style| style.bg(rgb(colors.surface_high)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_folder_collapsed(collapse_project.clone(), cx);
                }))
                .child(
                    div()
                        .w(px(12.0))
                        .flex_none()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .child(if collapsed { "›" } else { "⌄" }),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(colors.text))
                        .child(project.name.to_uppercase()),
                )
                .child(
                    div()
                        .id(("minimal-session-group-add", project_index))
                        .size(px(26.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(colors.border)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.begin_chat_create(create_project.clone(), window, cx);
                        }))
                        .child(plus_icon(colors.text)),
                );

            groups.push(
                div()
                    .id(("minimal-session-group", project_index))
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .when(!collapsed, |group| {
                        group.child(
                            div()
                                .mt_2()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(cards)
                                .when(project.sessions == 0, |list| {
                                    list.child(
                                        div()
                                            .px_3()
                                            .py_3()
                                            .text_sm()
                                            .text_color(rgb(colors.muted))
                                            .child("No sessions yet."),
                                    )
                                }),
                        )
                    }),
            );
        }

        div()
            .id("minimal-session-board")
            .w(px(268.0))
            .h_full()
            .min_h_0()
            .flex_none()
            .overflow_y_scroll()
            .px_3()
            .pt_4()
            .pb_5()
            .flex()
            .flex_col()
            .gap_4()
            .border_r_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.sidebar))
            .children(groups)
            .when(projects.is_empty(), |board| {
                board.child(
                    div()
                        .px_2()
                        .py_4()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .child("No workspaces yet."),
                )
            })
            .into_any_element()
    }

    fn render_minimal_home(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let projects = project_cards(&self.model.folders, &self.model.chats);
        let requested_project_id = match &self.minimal_route {
            MinimalRoute::Projects { project_id } => project_id.as_ref(),
            MinimalRoute::Sessions { project_id } => project_id.as_ref(),
            MinimalRoute::Cli { project_id, .. } => Some(project_id),
        };
        let selected_project_id = requested_project_id
            .filter(|selected| projects.iter().any(|project| &project.id == *selected))
            .cloned()
            .or_else(|| projects.first().map(|project| project.id.clone()));
        let selected_project = selected_project_id
            .as_deref()
            .and_then(|selected| projects.iter().find(|project| project.id == selected));
        let selected_project_name = selected_project
            .map(|project| project.name.clone())
            .unwrap_or_else(|| "Welcome to xd".into());
        let selected_project_rename =
            selected_project.map(|project| (project.id.clone(), project.name.clone()));
        let selected_project_delete = selected_project.map(|project| project.id.clone());
        let sessions = selected_project_id
            .as_deref()
            .map(|project_id| project_sessions(project_id, &self.model.chats))
            .unwrap_or_default();

        let project_rows =
            projects
                .iter()
                .enumerate()
                .map(|(index, project)| {
                    let project_id = project.id.clone();
                    let selected = selected_project_id.as_deref() == Some(project.id.as_str());
                    div()
                        .id(("minimal-project", index))
                        .w_full()
                        .px_3()
                        .py_3()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(if selected {
                            colors.selected_border
                        } else {
                            colors.border
                        }))
                        .bg(rgb(if selected {
                            colors.selected_surface
                        } else {
                            colors.surface
                        }))
                        .cursor_pointer()
                        .hover(|style| {
                            style
                                .bg(rgb(colors.surface_high))
                                .border_color(rgb(colors.accent))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.minimal_route = MinimalRoute::Projects {
                                project_id: Some(project_id.clone()),
                            };
                            cx.notify();
                        }))
                        .child(svg().path(FOLDER_ICON).size(px(18.0)).text_color(rgb(
                            if selected {
                                colors.accent
                            } else {
                                colors.muted
                            },
                        )))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(colors.text))
                                        .child(project.name.clone()),
                                )
                                .child(div().mt_1().text_xs().text_color(rgb(colors.muted)).child(
                                    format!(
                                        "{} session{}",
                                        project.sessions,
                                        if project.sessions == 1 { "" } else { "s" }
                                    ),
                                )),
                        )
                        .when(project.working > 0, |row| {
                            row.child(working_dots(index, colors.accent))
                        })
                })
                .collect::<Vec<_>>();

        let session_rows = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let project_id = selected_project_id.clone().unwrap_or_default();
                let chat_id = session.id.clone();
                let context_chat_id = session.id.clone();
                let context_title = session.title.clone();
                let agent = session.agent;
                let done = !session.working && self.model.unread_chats.contains(&session.id);
                let icon = match agent {
                    AgentCli::Codex => CODEX_ICON,
                    AgentCli::Claude => CLAUDE_ICON,
                    AgentCli::Jcode => JCODE_ICON,
                    AgentCli::Copilot => COPILOT_ICON,
                };
                let status = if session.working {
                    div().child("Working").into_any_element()
                } else if done {
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(DONE_STATUS_COLOR))
                        .child("Done")
                        .into_any_element()
                } else {
                    div().child("Idle").into_any_element()
                };
                div()
                    .id(("minimal-session", index))
                    .w_full()
                    .max_w(px(760.0))
                    .px_4()
                    .py_4()
                    .flex()
                    .items_center()
                    .gap_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.surface))
                    .cursor_pointer()
                    .hover(|style| {
                        style
                            .bg(rgb(colors.surface_high))
                            .border_color(rgb(colors.accent))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_minimal_session(
                            project_id.clone(),
                            chat_id.clone(),
                            agent,
                            window,
                            cx,
                        );
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_session_context_menu(
                                context_chat_id.clone(),
                                context_title.clone(),
                                event.position,
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(
                        div()
                            .size(px(38.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(rgb(colors.background))
                            .child(svg().path(icon).size(px(22.0)).text_color(rgb(colors.text))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.text))
                                    .child(session.title.clone()),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(rgb(colors.muted))
                                    .child(format!("{} CLI", agent.label()))
                                    .child("·")
                                    .child(status),
                            ),
                    )
                    .when(session.working, |row| {
                        row.child(working_dots(index + projects.len(), colors.accent))
                    })
                    .child(div().text_lg().text_color(rgb(colors.muted)).child("›"))
            })
            .collect::<Vec<_>>();

        let project_sidebar = div()
            .id("minimal-project-list")
            .w(px(268.0))
            .h_full()
            .min_h_0()
            .flex_none()
            .overflow_y_scroll()
            .px_3()
            .pt_4()
            .pb_5()
            .flex()
            .flex_col()
            .gap_2()
            .border_r_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.sidebar))
            .child(
                div()
                    .h(px(30.0))
                    .w_full()
                    .px_1()
                    .mb_1()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors.muted))
                            .child("PROJECTS"),
                    )
                    .child(
                        div()
                            .id("minimal-new-project")
                            .size(px(26.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.begin_workspace_create(window, cx)
                            }))
                            .child(plus_icon(colors.text)),
                    ),
            )
            .children(project_rows)
            .when(projects.is_empty(), |list| {
                list.child(
                    div()
                        .px_2()
                        .py_4()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .child("No projects yet."),
                )
            });

        let project_content = div()
            .id("minimal-project-content")
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .px_8()
            .py_7()
            .bg(rgb(colors.background))
            .child(
                div()
                    .w_full()
                    .max_w(px(900.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors.muted))
                            .child("PROJECT"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_3xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(colors.text))
                                    .child(selected_project_name),
                            )
                            .when_some(
                                selected_project_rename,
                                |heading, (folder_id, project_name)| {
                                    heading.child(
                                        div()
                                            .id("minimal-rename-project")
                                            .size(px(34.0))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_full()
                                            .border_1()
                                            .border_color(rgb(colors.border))
                                            .bg(rgb(colors.surface))
                                            .text_lg()
                                            .text_color(rgb(colors.muted))
                                            .cursor_pointer()
                                            .hover(|style| {
                                                style
                                                    .bg(rgb(colors.surface_high))
                                                    .text_color(rgb(colors.text))
                                            })
                                            .on_click(cx.listener(
                                                move |this, _, window, cx| {
                                                    this.begin_sidebar_edit(
                                                        SidebarTarget::Folder(folder_id.clone()),
                                                        project_name.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            ))
                                            .child("✎"),
                                    )
                                },
                            )
                            .when_some(
                                selected_project_delete,
                                |heading, folder_id| {
                                    heading.child(
                                        div()
                                            .id("minimal-delete-project")
                                            .size(px(34.0))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_full()
                                            .border_1()
                                            .border_color(rgb(colors.border))
                                            .bg(rgb(colors.surface))
                                            .text_color(rgb(0xd86f7c))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x5a252b)))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.begin_sidebar_delete(
                                                    SidebarTarget::Folder(folder_id.clone()),
                                                    window,
                                                    cx,
                                                );
                                            }))
                                            .child(
                                                svg()
                                                    .path(TRASH_ICON)
                                                    .size(px(17.0))
                                                    .text_color(rgb(0xd86f7c)),
                                            ),
                                    )
                                },
                            ),
                    )
                    .child(
                        div()
                            .mt_8()
                            .mb_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.text))
                                    .child("Sessions"),
                            )
                            .when_some(selected_project_id.clone(), |row, folder_id| {
                                row.child(
                                    div()
                                        .id("minimal-new-session")
                                        .px_3()
                                        .py_2()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .rounded_lg()
                                        .bg(rgb(colors.accent))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(colors.accent_text))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(colors.accent_hover)))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.begin_chat_create(folder_id.clone(), window, cx);
                                        }))
                                        .child(plus_icon(colors.accent_text))
                                        .child("New session"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .children(session_rows)
                            .when(
                                selected_project_id.is_some() && sessions.is_empty(),
                                |list| {
                                    list.child(
                                        div()
                                            .w_full()
                                            .max_w(px(760.0))
                                            .p_6()
                                            .rounded_xl()
                                            .border_1()
                                            .border_color(rgb(colors.border))
                                            .text_sm()
                                            .text_color(rgb(colors.muted))
                                            .child(
                                                "No sessions yet. Start one with Codex, Claude, JCode, or Copilot.",
                                            ),
                                    )
                                },
                            )
                            .when(projects.is_empty(), |list| {
                                list.child(
                                    div()
                                        .max_w(px(360.0))
                                        .text_sm()
                                        .text_color(rgb(colors.muted))
                                        .child(
                                            "Create a workspace, then start a Codex, Claude, JCode, or Copilot session.",
                                        ),
                                )
                            }),
                    ),
            );
        let toolbar = self.render_minimal_context_toolbar(colors, cx);

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(colors.background))
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(project_sidebar)
                    .child(project_content),
            )
            .into_any_element()
    }

    fn render_minimal_cli(
        &mut self,
        colors: ThemeColors,
        _project_id: String,
        chat_id: String,
        _agent: AgentCli,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let toolbar = self.render_minimal_context_toolbar(colors, cx);
        let board = self.render_minimal_session_board(colors, Some(chat_id.as_str()), cx);
        let terminal = self.render_minimal_terminal(colors, window, cx);

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(colors.background))
            .child(toolbar)
            .child(
                div().flex_1().min_h_0().flex().child(board).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .min_h_0()
                        .p_4()
                        .bg(rgb(colors.background))
                        .child(terminal),
                ),
            )
            .into_any_element()
    }

    fn render_minimal_empty_sessions(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let toolbar = self.render_minimal_context_toolbar(colors, cx);
        let board = self.render_minimal_session_board(colors, None, cx);

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(colors.background))
            .child(toolbar)
            .child(
                div().flex_1().min_h_0().flex().child(board).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .child("No session selected."),
                ),
            )
            .into_any_element()
    }

    fn render_minimal(
        &mut self,
        custom_titlebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.restore_minimal_popup_focus(window);
        let minimal_popup_open = self.minimal_popup_is_open();
        let minimal_popup_focus = self.minimal_popup_focus.clone();
        let colors = self.settings.theme.colors();
        let route = self.minimal_route.clone();
        let content = match route {
            MinimalRoute::Projects { .. } => self.render_minimal_home(colors, cx),
            MinimalRoute::Sessions { .. } => self.render_minimal_empty_sessions(colors, cx),
            MinimalRoute::Cli {
                project_id,
                chat_id,
                agent,
            } => self.render_minimal_cli(colors, project_id, chat_id, agent, window, cx),
        };
        let product_nav = if custom_titlebar {
            self.render_minimal_titlebar(colors, cx)
        } else {
            self.render_minimal_product_nav(colors, false, cx)
        };
        let session_context_overlay = self.session_context_menu.clone().map(|menu| {
            let rename_chat_id = menu.chat_id.clone();
            let rename_title = menu.title.clone();
            let delete_chat_id = menu.chat_id.clone();
            div()
                .occlude()
                .absolute()
                .inset_0()
                .track_focus(&minimal_popup_focus)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_session_context_menu(window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, window, cx| this.close_session_context_menu(window, cx)),
                )
                .child(
                    div()
                        .occlude()
                        .absolute()
                        .left(menu.position.x)
                        .top(menu.position.y)
                        .w(px(188.0))
                        .p_1()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .id("minimal-rename-session")
                                .w_full()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(colors.text))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin_sidebar_edit(
                                        SidebarTarget::Chat(rename_chat_id.clone()),
                                        rename_title.clone(),
                                        window,
                                        cx,
                                    );
                                }))
                                .child("Rename session"),
                        )
                        .child(
                            div()
                                .id("minimal-delete-session")
                                .w_full()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(0xd86f7c))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x5a252b)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin_sidebar_delete(
                                        SidebarTarget::Chat(delete_chat_id.clone()),
                                        window,
                                        cx,
                                    );
                                }))
                                .child("Delete session"),
                        ),
                )
                .into_any_element()
        });
        let theme_overlay = self.minimal_theme_open.then(|| {
            let allow_all_permissions = self.settings.allow_all_permissions;
            let rows = ThemePreset::ALL
                .into_iter()
                .enumerate()
                .map(|(index, theme)| {
                    let selected = self.settings.theme == theme;
                    let swatch = theme.colors();
                    div()
                        .id(("minimal-theme-choice", index))
                        .w_full()
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(colors.text))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(colors.surface_high)))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.choose_minimal_theme(theme, cx)),
                        )
                        .child(
                            div()
                                .size(px(18.0))
                                .rounded_full()
                                .border_1()
                                .border_color(rgb(swatch.border))
                                .bg(rgb(swatch.background)),
                        )
                        .child(div().flex_1().child(theme.label()))
                        .when(selected, |row| {
                            row.child(div().text_color(rgb(colors.accent)).child("✓"))
                        })
                })
                .collect::<Vec<_>>();
            div()
                .occlude()
                .absolute()
                .inset_0()
                .track_focus(&minimal_popup_focus)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.close_minimal_theme_popup(window, cx)
                    }),
                )
                .child(
                    div()
                        .occlude()
                        .absolute()
                        .top(px(68.0))
                        .right(px(22.0))
                        .w(px(300.0))
                        .p_2()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .px_3()
                                .pt_2()
                                .pb_1()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(colors.muted))
                                .child("Appearance"),
                        )
                        .children(rows)
                        .child(div().mx_2().my_2().h(px(1.0)).bg(rgb(colors.border)))
                        .child(
                            div()
                                .id("minimal-all-permissions")
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_minimal_all_permissions(cx)
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(colors.text))
                                                .child("All permissions"),
                                        )
                                        .child(
                                            div()
                                                .mt_1()
                                                .text_xs()
                                                .text_color(rgb(colors.muted))
                                                .child("New agent tabs skip approvals and sandboxing when the CLI supports it."),
                                        ),
                                )
                                .child(
                                    div()
                                        .w(px(38.0))
                                        .h(px(22.0))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(rgb(if allow_all_permissions {
                                            colors.accent
                                        } else {
                                            colors.surface_high
                                        }))
                                        .child(
                                            div()
                                                .mt(px(3.0))
                                                .ml(px(if allow_all_permissions {
                                                    19.0
                                                } else {
                                                    3.0
                                                }))
                                                .size(px(16.0))
                                                .rounded_full()
                                                .bg(rgb(if allow_all_permissions {
                                                    colors.accent_text
                                                } else {
                                                    colors.muted
                                                })),
                                        ),
                                ),
                        ),
                )
                .into_any_element()
        });
        let remote_overlay = self.remote_panel.clone().map(|panel| {
            let can_connect =
                !panel.submitting && SshCommand::parse(panel.command.trim()).is_ok();
            div()
                .occlude()
                .absolute()
                .inset_0()
                .track_focus(&minimal_popup_focus)
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(470.0))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(colors.text))
                                .child("Connect over SSH"),
                        )
                        .child(
                            div()
                                .mb_2()
                                .text_sm()
                                .text_color(rgb(colors.muted))
                                .child(
                                    "Enter the SSH command you normally use. This replaces the local runtime until you disconnect.",
                                ),
                        )
                        .child(
                            div()
                                .h(px(42.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.background))
                                .child(self.remote_ssh_input.clone()),
                        )
                        .when_some(panel.error.clone(), |card, error| {
                            card.child(div().text_sm().text_color(rgb(0xd86f7c)).child(error))
                        })
                        .child(
                            div()
                                .mt_2()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("minimal-cancel-remote")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(colors.muted))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(colors.surface_high)))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.close_remote(cx)),
                                        )
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("minimal-connect-remote")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if can_connect {
                                            colors.accent
                                        } else {
                                            colors.surface_high
                                        }))
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(if can_connect {
                                            colors.accent_text
                                        } else {
                                            colors.muted
                                        }))
                                        .when(can_connect, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(colors.accent_hover)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_connect {
                                                this.connect_remote_machine(cx);
                                            }
                                        }))
                                        .child(if panel.submitting {
                                            "Connecting…"
                                        } else {
                                            "Connect"
                                        }),
                                ),
                        ),
                )
                .into_any_element()
        });
        let workspace_overlay = self.creating_workspace.then(|| {
            let can_save =
                !self.workspace_create_submitting && !self.workspace_create_name.trim().is_empty();
            div()
                .occlude()
                .absolute()
                .inset_0()
                .track_focus(&minimal_popup_focus)
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(500.0))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(colors.text))
                                .child("New project"),
                        )
                        .child(
                            div()
                                .h(px(42.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.background))
                                .child(self.workspace_create_input.clone()),
                        )
                        .child(
                            div()
                                .h(px(42.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.background))
                                .child(self.workspace_repo_input.clone()),
                        )
                        .child(
                            div()
                                .h(px(42.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.background))
                                .child(self.workspace_clone_input.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors.muted))
                                .child("Use either an existing repository path or a clone URL."),
                        )
                        .child(
                            div()
                                .mt_2()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("minimal-cancel-project")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(colors.muted))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(colors.surface_high)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_workspace_create(cx)
                                        }))
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("minimal-save-project")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if can_save {
                                            colors.accent
                                        } else {
                                            colors.surface_high
                                        }))
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(if can_save {
                                            colors.accent_text
                                        } else {
                                            colors.muted
                                        }))
                                        .when(can_save, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(colors.accent_hover)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_save {
                                                this.save_workspace_create(cx)
                                            }
                                        }))
                                        .child(if self.workspace_create_submitting {
                                            "Creating…"
                                        } else {
                                            "Create project"
                                        }),
                                ),
                        ),
                )
                .into_any_element()
        });
        let sidebar_rename_overlay = self.sidebar_edit.clone().and_then(|edit| {
            let (heading, cancel_id, save_id) = match &edit.target {
                SidebarTarget::Folder(_) => (
                    "Rename project",
                    "minimal-cancel-project-rename",
                    "minimal-save-project-rename",
                ),
                SidebarTarget::Chat(_) => (
                    "Rename session",
                    "minimal-cancel-session-rename",
                    "minimal-save-session-rename",
                ),
            };
            let name = edit.text.trim();
            let can_save = !edit.submitting && !name.is_empty() && name != edit.original;
            Some(
                div()
                    .occlude()
                    .absolute()
                    .inset_0()
                    .track_focus(&minimal_popup_focus)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x00000099))
                    .child(
                        div()
                            .w(px(430.0))
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .rounded_xl()
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.surface))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.text))
                                    .child(heading),
                            )
                            .child(
                                div()
                                    .h(px(42.0))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgb(colors.border))
                                    .bg(rgb(colors.background))
                                    .child(self.sidebar_edit_input.clone()),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .flex()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id(cancel_id)
                                            .px_4()
                                            .py_2()
                                            .rounded_lg()
                                            .text_sm()
                                            .text_color(rgb(colors.muted))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(colors.surface_high)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.cancel_sidebar_edit(cx)
                                            }))
                                            .child("Cancel"),
                                    )
                                    .child(
                                        div()
                                            .id(save_id)
                                            .px_4()
                                            .py_2()
                                            .rounded_lg()
                                            .bg(rgb(if can_save {
                                                colors.accent
                                            } else {
                                                colors.surface_high
                                            }))
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if can_save {
                                                colors.accent_text
                                            } else {
                                                colors.muted
                                            }))
                                            .when(can_save, |button| {
                                                button.cursor_pointer().hover(|style| {
                                                    style.bg(rgb(colors.accent_hover))
                                                })
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if can_save {
                                                    this.save_sidebar_edit(cx)
                                                }
                                            }))
                                            .child(if edit.submitting {
                                                "Renaming…"
                                            } else {
                                                "Rename"
                                            }),
                                    ),
                            ),
                    )
                    .into_any_element(),
            )
        });
        let workspace_delete_overlay = self.pending_sidebar_delete.clone().and_then(|target| {
            let SidebarTarget::Folder(folder_id) = target else {
                return None;
            };
            let project_name = self
                .model
                .folders
                .iter()
                .find(|folder| folder.id == folder_id)
                .map(|folder| folder.name.clone())?;
            let submitting = self.sidebar_delete_submitting;
            Some(
                div()
                    .occlude()
                    .absolute()
                    .inset_0()
                    .track_focus(&minimal_popup_focus)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x00000099))
                    .child(
                        div()
                            .w(px(450.0))
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .rounded_xl()
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.surface))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.text))
                                    .child("Delete project?"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(colors.muted))
                                    .child(format!(
                                        "This removes {project_name} and all its sessions from xd. Its files will be moved to the workspace trash."
                                    )),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .flex()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("minimal-cancel-project-delete")
                                            .px_4()
                                            .py_2()
                                            .rounded_lg()
                                            .text_sm()
                                            .text_color(rgb(colors.muted))
                                            .when(!submitting, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(rgb(colors.surface_high)))
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.cancel_sidebar_delete(cx)
                                            }))
                                            .child("Cancel"),
                                    )
                                    .child(
                                        div()
                                            .id("minimal-confirm-project-delete")
                                            .px_4()
                                            .py_2()
                                            .rounded_lg()
                                            .bg(rgb(if submitting {
                                                colors.surface_high
                                            } else {
                                                0x9f3544
                                            }))
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if submitting {
                                                colors.muted
                                            } else {
                                                0xffffff
                                            }))
                                            .when(!submitting, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(rgb(0xb84252)))
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.confirm_sidebar_delete(cx)
                                            }))
                                            .child(if submitting {
                                                "Deleting…"
                                            } else {
                                                "Delete project"
                                            }),
                                    ),
                            ),
                )
                .into_any_element(),
            )
        });
        let chat_delete_overlay = self.pending_sidebar_delete.clone().and_then(|target| {
            let SidebarTarget::Chat(chat_id) = target else {
                return None;
            };
            let session_name = self
                .model
                .chats
                .iter()
                .find(|chat| chat.id == chat_id)
                .and_then(|chat| chat.title.as_deref())
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("New Session")
                .to_owned();
            let submitting = self.sidebar_delete_submitting;
            Some(
                div()
                    .occlude()
                    .absolute()
                    .inset_0()
                    .track_focus(&minimal_popup_focus)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x00000099))
                    .child(
                        div()
                            .w(px(450.0))
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .rounded_xl()
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.surface))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.text))
                                    .child("Delete session?"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(colors.muted))
                                    .child(format!(
                                        "This permanently removes {session_name} and its conversation from xd. Project files are not deleted."
                                    )),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .flex()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("minimal-cancel-session-delete")
                                            .px_4()
                                            .py_2()
                                            .rounded_lg()
                                            .text_sm()
                                            .text_color(rgb(colors.muted))
                                            .when(!submitting, |button| {
                                                button.cursor_pointer().hover(|style| {
                                                    style.bg(rgb(colors.surface_high))
                                                })
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.cancel_sidebar_delete(cx)
                                            }))
                                            .child("Cancel"),
                                    )
                                    .child(
                                        div()
                                            .id("minimal-confirm-session-delete")
                                            .px_4()
                                            .py_2()
                                            .rounded_lg()
                                            .bg(rgb(if submitting {
                                                colors.surface_high
                                            } else {
                                                0x9f3544
                                            }))
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if submitting {
                                                colors.muted
                                            } else {
                                                0xffffff
                                            }))
                                            .when(!submitting, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(rgb(0xb84252)))
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.confirm_sidebar_delete(cx)
                                            }))
                                            .child(if submitting {
                                                "Deleting…"
                                            } else {
                                                "Delete session"
                                            }),
                                    ),
                            ),
                    )
                    .into_any_element(),
            )
        });
        let chat_overlay = self.creating_chat_folder.is_some().then(|| {
            let can_save = !self.chat_create_submitting
                && !self.chat_create_worktrees_loading
                && self.chat_create_worktree.is_some();
            let selected_worktree = self.chat_create_worktree.clone();
            let can_create_worktree = self.chat_create_can_new_worktree;
            div()
                .occlude()
                .absolute()
                .inset_0()
                .track_focus(&minimal_popup_focus)
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(520.0))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(colors.text))
                                .child("New agent session"),
                        )
                        .child(
                            div()
                                .h(px(42.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.background))
                                .child(self.chat_create_input.clone()),
                        )
                        .child(
                            div().flex().gap_2().children(
                                [
                                    AgentCli::Codex,
                                    AgentCli::Claude,
                                    AgentCli::Jcode,
                                    AgentCli::Copilot,
                                ]
                                .into_iter()
                                .enumerate()
                                .map(|(index, agent)| {
                                    let selected = self.minimal_new_session_agent == agent;
                                    div()
                                        .id(("minimal-session-agent", index))
                                        .flex_1()
                                        .px_3()
                                        .py_2()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(if selected {
                                            colors.accent
                                        } else {
                                            colors.border
                                        }))
                                        .bg(rgb(if selected {
                                            colors.surface_high
                                        } else {
                                            colors.background
                                        }))
                                        .text_sm()
                                        .text_color(rgb(colors.text))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(colors.surface_high)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.minimal_new_session_agent = agent;
                                            cx.notify();
                                        }))
                                        .child(
                                            svg()
                                                .path(match agent {
                                                    AgentCli::Codex => CODEX_ICON,
                                                    AgentCli::Claude => CLAUDE_ICON,
                                                    AgentCli::Jcode => JCODE_ICON,
                                                    AgentCli::Copilot => COPILOT_ICON,
                                                })
                                                .size(px(18.0))
                                                .text_color(rgb(colors.text)),
                                        )
                                        .child(agent.label())
                                }),
                            ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors.muted))
                                .child("The selected CLI opens directly in this project."),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(colors.text))
                                .child("Worktree"),
                        )
                        .child(
                            div()
                                .id("minimal-session-worktrees")
                                .max_h(px(236.0))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when(self.chat_create_worktrees_loading, |list| {
                                    list.child(
                                        div()
                                            .px_3()
                                            .py_3()
                                            .text_sm()
                                            .text_color(rgb(colors.muted))
                                            .child("Loading worktrees…"),
                                    )
                                })
                                .when(!self.chat_create_worktrees_loading, |list| {
                                    let selected =
                                        matches!(selected_worktree, Some(NewSessionWorktree::New));
                                    list.child(
                                        div()
                                            .id("minimal-session-new-worktree")
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(rgb(if selected {
                                                colors.accent
                                            } else {
                                                colors.border
                                            }))
                                            .bg(rgb(if selected {
                                                colors.surface_high
                                            } else {
                                                colors.background
                                            }))
                                            .text_sm()
                                            .text_color(rgb(if can_create_worktree {
                                                colors.text
                                            } else {
                                                colors.muted
                                            }))
                                            .when(can_create_worktree, |row| {
                                                row.cursor_pointer()
                                                    .hover(|style| {
                                                        style.bg(rgb(colors.surface_high))
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.chat_create_worktree =
                                                            Some(NewSessionWorktree::New);
                                                        cx.notify();
                                                    }))
                                            })
                                            .child("Create new worktree"),
                                    )
                                })
                                .when(!self.chat_create_worktrees_loading, |list| {
                                    list.child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(colors.muted))
                                            .child("Existing worktrees"),
                                    )
                                })
                                .when(!self.chat_create_worktrees_loading, |list| {
                                    list.children(
                                        self.chat_create_worktrees.iter().enumerate().map(
                                            |(index, worktree)| {
                                                let path = worktree.path.clone();
                                                let selected = matches!(
                                                    &selected_worktree,
                                                    Some(NewSessionWorktree::Existing(selected))
                                                        if selected == &path
                                                );
                                                let label =
                                                    worktree.branch.clone().unwrap_or_else(|| {
                                                        Path::new(&path)
                                                            .file_name()
                                                            .and_then(|name| name.to_str())
                                                            .unwrap_or("Project directory")
                                                            .to_owned()
                                                    });
                                                let selected_path = path.clone();
                                                div()
                                                    .id(("minimal-session-worktree", index))
                                                    .px_3()
                                                    .py_2()
                                                    .rounded_lg()
                                                    .border_1()
                                                    .border_color(rgb(if selected {
                                                        colors.accent
                                                    } else {
                                                        colors.border
                                                    }))
                                                    .bg(rgb(if selected {
                                                        colors.surface_high
                                                    } else {
                                                        colors.background
                                                    }))
                                                    .cursor_pointer()
                                                    .hover(|style| {
                                                        style.bg(rgb(colors.surface_high))
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.chat_create_worktree =
                                                            Some(NewSessionWorktree::Existing(
                                                                selected_path.clone(),
                                                            ));
                                                        cx.notify();
                                                    }))
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(rgb(colors.text))
                                                            .child(label),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(colors.muted))
                                                            .child(path),
                                                    )
                                            },
                                        ),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .mt_2()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("minimal-cancel-session")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(colors.muted))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(colors.surface_high)))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.cancel_chat_create(cx)
                                            }),
                                        )
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("minimal-save-session")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if can_save {
                                            colors.accent
                                        } else {
                                            colors.surface_high
                                        }))
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(if can_save {
                                            colors.accent_text
                                        } else {
                                            colors.muted
                                        }))
                                        .when(can_save, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(colors.accent_hover)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_save {
                                                this.save_minimal_chat_create(cx)
                                            }
                                        }))
                                        .child(if self.chat_create_submitting {
                                            "Creating…"
                                        } else {
                                            "Create session"
                                        }),
                                ),
                        ),
                )
                .into_any_element()
        });
        let error_banner = self.model.connection_error.clone().map(|error| {
            div()
                .absolute()
                .bottom(px(20.0))
                .left(px(50.0))
                .right(px(50.0))
                .mx_auto()
                .max_w(px(760.0))
                .px_4()
                .py_3()
                .rounded_lg()
                .border_1()
                .border_color(rgb(0x8f3f4b))
                .bg(rgb(colors.surface))
                .text_sm()
                .text_color(rgb(0xd86f7c))
                .child(error)
                .into_any_element()
        });

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .key_context("XdDesktop")
            .bg(rgb(colors.background))
            .font_family(UI_FONT)
            .on_action(cx.listener(|_, _: &CopyRenderedSelection, _, cx| {
                if let Some(text) = TextSelection::selected(cx) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }))
            .on_action(cx.listener(|this, _: &CloseSearch, window, cx| {
                if this.remote_panel.is_some() {
                    this.close_remote(cx);
                } else if this.session_context_menu.is_some() {
                    this.close_session_context_menu(window, cx);
                } else if this.pending_sidebar_delete.is_some() {
                    this.cancel_sidebar_delete(cx);
                } else if this.sidebar_edit.is_some() {
                    this.cancel_sidebar_edit(cx);
                } else if this.creating_workspace {
                    this.cancel_workspace_create(cx);
                } else if this.creating_chat_folder.is_some() {
                    this.cancel_chat_create(cx);
                } else if this.minimal_new_tab_open {
                    this.minimal_new_tab_open = false;
                    cx.notify();
                } else if this.minimal_theme_open {
                    this.minimal_theme_open = false;
                    cx.notify();
                } else if matches!(
                    this.minimal_route,
                    MinimalRoute::Sessions { .. } | MinimalRoute::Cli { .. }
                ) {
                    this.show_minimal_projects(cx);
                }
            }))
            .child(product_nav)
            .child(content)
            .when_some(session_context_overlay, |root, overlay| root.child(overlay))
            .when_some(theme_overlay, |root, overlay| root.child(overlay))
            .when_some(remote_overlay, |root, overlay| root.child(overlay))
            .when_some(workspace_overlay, |root, overlay| root.child(overlay))
            .when_some(sidebar_rename_overlay, |root, overlay| root.child(overlay))
            .when_some(workspace_delete_overlay, |root, overlay| {
                root.child(overlay)
            })
            .when_some(chat_delete_overlay, |root, overlay| root.child(overlay))
            .when_some(chat_overlay, |root, overlay| root.child(overlay))
            .when_some(error_banner, |root, banner| root.child(banner))
            .when(!minimal_popup_open, |root| {
                root.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(6.0))
                        .right(px(6.0))
                        .h(px(6.0))
                        .cursor(CursorStyle::ResizeUpDown)
                        .on_mouse_down(MouseButton::Left, |_, window, _| {
                            window.start_window_resize(ResizeEdge::Top)
                        }),
                )
            })
            .when(!minimal_popup_open, |root| {
                root.child(
                    div()
                        .absolute()
                        .bottom(px(0.0))
                        .left(px(6.0))
                        .right(px(6.0))
                        .h(px(6.0))
                        .cursor(CursorStyle::ResizeUpDown)
                        .on_mouse_down(MouseButton::Left, |_, window, _| {
                            window.start_window_resize(ResizeEdge::Bottom)
                        }),
                )
            })
            .when(!minimal_popup_open, |root| {
                root.child(
                    div()
                        .absolute()
                        .top(px(6.0))
                        .bottom(px(6.0))
                        .left(px(0.0))
                        .w(px(6.0))
                        .cursor(CursorStyle::ResizeLeftRight)
                        .on_mouse_down(MouseButton::Left, |_, window, _| {
                            window.start_window_resize(ResizeEdge::Left)
                        }),
                )
            })
            .when(!minimal_popup_open, |root| {
                root.child(
                    div()
                        .absolute()
                        .top(px(6.0))
                        .right(px(0.0))
                        .bottom(px(6.0))
                        .w(px(6.0))
                        .cursor(CursorStyle::ResizeLeftRight)
                        .on_mouse_down(MouseButton::Left, |_, window, _| {
                            window.start_window_resize(ResizeEdge::Right)
                        }),
                )
            })
            .into_any_element()
    }
}

impl Render for XdDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.expire_action_error(cx);
        let client_decorations = matches!(window.window_decorations(), Decorations::Client { .. });
        let custom_titlebar = client_decorations || cfg!(target_os = "macos");
        window.set_client_inset(if client_decorations { px(6.0) } else { px(0.0) });
        self.render_minimal(custom_titlebar, window, cx)
    }
}

/// Highlight the source portion of a diff line while leaving its `+`, `-`, or
/// context marker in the diff color supplied by the surrounding element.

fn parse_unified_diff(output: &str) -> Result<(Vec<DiffFile>, bool), String> {
    const MAX_FILES: usize = 500;
    const MAX_LINES: usize = 1_600;
    const MAX_LINE_CHARS: usize = 4_096;

    let mut files = Vec::new();
    let mut current: Option<DiffFile> = None;
    let mut rendered_lines = 0_usize;
    let mut truncated = false;
    for line in output.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            if files.len() >= MAX_FILES {
                truncated = true;
                break;
            }
            let path = header
                .split_once(" b/")
                .map(|(_, path)| path)
                .or_else(|| header.split_whitespace().last())
                .unwrap_or("changed file")
                .trim_matches('"')
                .trim_start_matches("b/")
                .to_owned();
            current = Some(DiffFile {
                path,
                additions: 0,
                deletions: 0,
                lines: Vec::new(),
                lazy_read: None,
                loaded: true,
                loading: false,
                error: None,
            });
        }
        let Some(file) = &mut current else {
            continue;
        };
        // Every file is drawn under a header naming it, so Git's own file
        // headers say nothing the reader cannot already see.
        if line.starts_with("diff --git ")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
        {
            continue;
        }
        if rendered_lines >= MAX_LINES {
            truncated = true;
            continue;
        }
        if line.starts_with('+') {
            file.additions = file.additions.saturating_add(1);
        } else if line.starts_with('-') {
            file.deletions = file.deletions.saturating_add(1);
        }
        if line.chars().count() > MAX_LINE_CHARS {
            truncated = true;
        }
        file.lines.push(DiffLine);
        rendered_lines += 1;
    }
    if let Some(file) = current {
        files.push(file);
    }
    if files.is_empty() && !output.trim().is_empty() {
        return Err("Git returned a diff that xd could not parse.".into());
    }
    Ok((files, truncated))
}

fn parse_diff_file_list(output: &str, branch: bool) -> Result<(Vec<DiffFile>, bool), String> {
    const MAX_FILES: usize = 500;
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let mut truncated = false;
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        if files.len() >= MAX_FILES {
            truncated = true;
            break;
        }
        let (status, raw_path) = if branch {
            let mut fields = line.split('\t');
            let status = fields.next().unwrap_or_default().trim();
            let path = fields.next_back().unwrap_or_default();
            (status, path)
        } else {
            if line.len() < 4 || line.as_bytes().get(2) != Some(&b' ') {
                return Err("Git returned a file list that xd could not parse.".into());
            }
            let status = &line[..2];
            let mut path = &line[3..];
            if (status.starts_with('R') || status.starts_with('C'))
                && let Some((_, renamed)) = path.rsplit_once(" -> ")
            {
                path = renamed;
            }
            (status, path)
        };
        if status.is_empty() || raw_path.is_empty() {
            return Err("Git returned a file list that xd could not parse.".into());
        }
        let path = decode_git_path(raw_path)?;
        if path.is_empty() || !seen.insert(path.clone()) {
            continue;
        }
        files.push(DiffFile {
            path,
            additions: 0,
            deletions: 0,
            lines: Vec::new(),
            lazy_read: Some(
                if branch || status != "??" {
                    if branch {
                        "branch-file"
                    } else {
                        "working-file"
                    }
                } else {
                    "untracked-file"
                }
                .to_owned(),
            ),
            loaded: false,
            loading: false,
            error: None,
        });
    }
    Ok((files, truncated))
}

fn decode_git_path(value: &str) -> Result<String, String> {
    if !value.starts_with('"') {
        return Ok(value.to_owned());
    }
    if !value.ends_with('"') || value.len() < 2 {
        return Err("Git returned an invalid quoted file path.".into());
    }
    let input = &value.as_bytes()[1..value.len() - 1];
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] != b'\\' {
            output.push(input[index]);
            index += 1;
            continue;
        }
        index += 1;
        let Some(escaped) = input.get(index).copied() else {
            return Err("Git returned an invalid quoted file path.".into());
        };
        if (b'0'..=b'7').contains(&escaped) {
            let mut value = 0_u8;
            let mut digits = 0;
            while digits < 3 && index < input.len() && (b'0'..=b'7').contains(&input[index]) {
                value = value.saturating_mul(8).saturating_add(input[index] - b'0');
                index += 1;
                digits += 1;
            }
            output.push(value);
            continue;
        }
        output.push(match escaped {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            b'"' => b'"',
            b'\\' => b'\\',
            _ => return Err("Git returned an invalid quoted file path.".into()),
        });
        index += 1;
    }
    String::from_utf8(output).map_err(|_| "Git returned an invalid UTF-8 file path.".into())
}

/// The vendor mark for an agent, and the colour it is drawn in. GPUI paints an
/// SVG as a mask, so the colour lives here rather than in the file.

/// Serves the agent marks compiled into the binary, so a bundle needs no icon
/// theme on the host.
struct EmbeddedIcons;

impl AssetSource for EmbeddedIcons {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            CLAUDE_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/claude.svg").as_slice(),
            )),
            CODEX_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/codex.svg").as_slice(),
            )),
            JCODE_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/jcode.svg").as_slice(),
            )),
            COPILOT_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/copilot.svg").as_slice(),
            )),
            XD_MARK_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/xd-mark.svg").as_slice(),
            )),
            SEND_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/send.svg").as_slice(),
            )),
            STOP_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/stop.svg").as_slice(),
            )),
            FOLDER_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/folder.svg").as_slice(),
            )),
            FILE_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/file.svg").as_slice(),
            )),
            GIT_BRANCH_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/git-branch.svg").as_slice(),
            )),
            TRASH_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/trash.svg").as_slice(),
            )),
            _ => None,
        })
    }

    fn list(&self, _: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(vec![
            CLAUDE_ICON.into(),
            CODEX_ICON.into(),
            JCODE_ICON.into(),
            COPILOT_ICON.into(),
            XD_MARK_ICON.into(),
            SEND_ICON.into(),
            STOP_ICON.into(),
            FOLDER_ICON.into(),
            FILE_ICON.into(),
            GIT_BRANCH_ICON.into(),
            TRASH_ICON.into(),
        ])
    }
}

fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_millis(match attempt {
        0 => 0,
        1 => 250,
        2 => 500,
        3 => 1_000,
        4 => 2_000,
        _ => 5_000,
    })
}

fn workflow_status_terminal(status: &Value) -> bool {
    status.get("ok").and_then(Value::as_bool) == Some(true)
        && status.get("state").and_then(Value::as_str) == Some("completed")
}

fn workflow_clock_active(status: &Value) -> bool {
    status.get("ok").and_then(Value::as_bool) == Some(true)
        && !workflow_status_terminal(status)
        && status
            .get("started_at")
            .and_then(Value::as_i64)
            .is_some_and(|started_at| started_at >= 0)
}

fn workflow_row_indices(model: &AppModel, marker: &str) -> Vec<usize> {
    let live_offset = model.messages.len();
    model
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            (message.role == "tool" && message.content == marker).then_some(index)
        })
        .chain(
            model
                .live_items
                .iter()
                .enumerate()
                .filter_map(|(index, message)| {
                    (message.role == "tool" && message.content == marker)
                        .then_some(live_offset + index)
                }),
        )
        .collect()
}

#[cfg(target_os = "linux")]
fn notify_turn_finished(title: &str) {
    let title = title
        .chars()
        .filter(|character| !character.is_control())
        .take(120)
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!("{title} finished");
    if let Ok(mut child) = Command::new("notify-send")
        .args(["--app-name=xd", "--", "xd", &body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(target_os = "macos")]
fn notify_turn_finished(title: &str) {
    let body = format!(
        "{} finished",
        title
            .chars()
            .filter(|character| !character.is_control())
            .take(120)
            .collect::<String>()
    );
    if let Ok(mut child) = Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title \"xd\"",
            "-e",
            "end run",
            "--",
            &body,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn notify_turn_finished(_: &str) {}

fn optional_trimmed(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

/// The title to create a chat under, given whatever is in the field.
///
/// The field opens holding [`DEFAULT_CHAT_TITLE`], selected. Clearing it is a
/// way of asking for that default back, not an error to be corrected: naming a
/// chat before it exists is busywork, and the name is renamed later or never.
fn chat_create_title(entered: &str) -> &str {
    optional_trimmed(entered).unwrap_or(DEFAULT_CHAT_TITLE)
}

fn new_session_worktree_state(value: &Value) -> (Vec<Worktree>, bool, Option<NewSessionWorktree>) {
    let effective = value
        .get("effective_workdir")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut worktrees = value
        .get("worktrees")
        .cloned()
        .and_then(|worktrees| serde_json::from_value::<Vec<Worktree>>(worktrees).ok())
        .unwrap_or_default();
    if worktrees.is_empty() && !effective.is_empty() {
        worktrees.push(Worktree {
            path: effective.to_owned(),
            branch: None,
            detached: false,
            main: true,
            current: true,
        });
    }
    let selected = worktrees
        .iter()
        .find(|worktree| worktree.current)
        .or_else(|| worktrees.iter().find(|worktree| worktree.main))
        .or_else(|| worktrees.first())
        .map(|worktree| NewSessionWorktree::Existing(worktree.path.clone()));
    let can_create = value
        .get("can_create_worktree")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (worktrees, can_create, selected)
}

fn pairing_details(value: &Value) -> Result<(String, u16, String), String> {
    let host = value
        .get("host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "The host returned an invalid pairing address.".to_owned())?;
    let port = value
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .ok_or_else(|| "The host returned an invalid pairing port.".to_owned())?;
    let code = value
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty())
        .ok_or_else(|| "The host returned an invalid pairing code.".to_owned())?;
    Ok((host.to_owned(), port, code.to_owned()))
}

fn scoped_element_id(scope: &str, index: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    scope.hash(&mut hasher);
    index.hash(&mut hasher);
    hasher.finish()
}

/// Flattens the rendered text blocks into the text Ctrl+C should receive and
/// records where each separately laid-out block lives inside it.

fn sidebar_edit_applied(model: &AppModel, edit: &SidebarEdit) -> bool {
    if !edit.submitting {
        return false;
    }
    let authoritative = match &edit.target {
        SidebarTarget::Folder(folder_id) => model
            .folders
            .iter()
            .find(|folder| &folder.id == folder_id)
            .map(|folder| folder.name.as_str()),
        SidebarTarget::Chat(chat_id) => model
            .chats
            .iter()
            .find(|chat| &chat.id == chat_id)
            .and_then(|chat| chat.title.as_deref()),
    };
    authoritative == Some(edit.text.trim())
}

fn sidebar_move_applied(
    model: &AppModel,
    target: &SidebarTarget,
    destination: Option<&str>,
) -> bool {
    match target {
        SidebarTarget::Folder(folder_id) => model
            .folders
            .iter()
            .find(|folder| &folder.id == folder_id)
            .is_some_and(|folder| folder.parent.as_deref() == destination),
        SidebarTarget::Chat(chat_id) => destination.is_some_and(|destination| {
            model
                .chats
                .iter()
                .find(|chat| &chat.id == chat_id)
                .is_some_and(|chat| chat.folder == destination)
        }),
    }
}

fn strip_source_build_controls(text: &str) -> String {
    let mut clean = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == 0x1b {
            if bytes.next() == Some(b'[') {
                for next in bytes.by_ref() {
                    if (0x40..=0x7e).contains(&next) {
                        break;
                    }
                }
            }
        } else if byte == b'\n' || byte == b'\t' || byte >= 0x20 {
            clean.push(byte);
        }
    }
    String::from_utf8_lossy(&clean).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xd_desktop::model::{ChatSummary, Folder};

    #[gpui::test]
    fn bundled_ui_font_can_be_registered(cx: &mut gpui::TestAppContext) {
        assert_eq!(UI_FONT, "DM Sans");
        assert!(EMBEDDED_UI_FONT.starts_with(&[0x00, 0x01, 0x00, 0x00]));
        install_embedded_fonts(cx.text_system()).expect("register bundled UI font");
    }

    #[test]
    fn working_dots_cycle_from_dim_to_three_lit() {
        assert_eq!(working_dot_alphas(0), [0x4d, 0x4d, 0x4d]);
        assert_eq!(working_dot_alphas(1), [0xff, 0x4d, 0x4d]);
        assert_eq!(working_dot_alphas(2), [0xff, 0xff, 0x4d]);
        assert_eq!(working_dot_alphas(3), [0xff, 0xff, 0xff]);
        assert_eq!(working_dot_alphas(4), [0x4d, 0x4d, 0x4d]);
    }

    #[test]
    fn agent_tabs_keep_one_stable_identity_per_backend() {
        assert_eq!(
            terminal_session_id("chat-1", Some(AgentCli::Claude), false, 1),
            terminal_session_id("chat-1", Some(AgentCli::Claude), false, 2)
        );
        assert_ne!(
            terminal_session_id("chat-1", Some(AgentCli::Claude), false, 1),
            terminal_session_id("chat-1", Some(AgentCli::Codex), false, 1)
        );
        assert_ne!(
            terminal_session_id("chat-1", None, false, 1),
            terminal_session_id("chat-1", None, false, 2)
        );
    }

    #[test]
    fn claude_session_identity_is_stable_and_uuid_shaped() {
        let first = stable_agent_session_id("chat-1:claude");
        let second = stable_agent_session_id("chat-1:claude");
        assert_eq!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "4");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn working_dots_are_mounted_as_a_native_animation() {
        assert!(
            working_dots(0, 0xa8a8ad)
                .downcast_mut::<gpui::AnimationElement<gpui::Div>>()
                .is_some()
        );
    }

    #[test]
    fn desktop_platforms_mount_the_in_app_titlebar() {
        let source = include_str!("main.rs");
        let titlebar = source
            .split_once("fn render_minimal_window_controls")
            .expect("minimal window controls renderer")
            .1
            .split_once("fn render_minimal_context_toolbar")
            .expect("end of minimal product chrome")
            .0;

        assert!(titlebar.contains("window_control_area(WindowControlArea::Drag)"));
        assert!(titlebar.contains("window_control_area(WindowControlArea::Min)"));
        assert!(titlebar.contains("window_control_area(WindowControlArea::Max)"));
        assert!(titlebar.contains("window_control_area(WindowControlArea::Close)"));

        let render = source
            .split_once("impl Render for XdDesktop")
            .expect("desktop render implementation")
            .1
            .split_once("fn load_png_attachments")
            .expect("end of desktop render implementation")
            .0;
        assert!(render.contains("client_decorations || cfg!(target_os = \"macos\")"));
        assert!(render.contains("self.render_minimal(custom_titlebar, window, cx)"));

        let startup = source
            .rsplit_once("WindowOptions {")
            .expect("desktop window options")
            .1;
        assert!(startup.contains("appears_transparent: true"));
        assert!(startup.contains("traffic_light_position: cfg!(target_os = \"macos\")"));
    }

    #[test]
    fn minimal_chrome_uses_the_xirp_two_level_header() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let product_nav = production
            .split_once("fn render_minimal_product_nav(")
            .expect("product navigation renderer")
            .1
            .split_once("fn render_minimal_titlebar(")
            .expect("end of product navigation renderer")
            .0;
        for behavior in [
            "minimal-product-nav",
            ".child(\"Projects\")",
            ".child(\"Sessions\")",
            "this.show_minimal_projects(cx)",
            "this.show_minimal_sessions(window, cx)",
            "this.begin_workspace_create(window, cx)",
            "minimal-runtime",
            "minimal-theme",
        ] {
            assert!(product_nav.contains(behavior), "missing {behavior}");
        }

        let context_toolbar = production
            .split_once("fn render_minimal_context_toolbar(")
            .expect("session context toolbar")
            .1
            .split_once("fn render_minimal_session_board(")
            .expect("end of session context toolbar")
            .0;
        for behavior in [
            "minimal-context-toolbar",
            "minimal-session-agent",
            "minimal-context-terminal",
            "this.open_minimal_terminal_tab(None, window, cx)",
            "this.send_terminal_input(&[3], cx)",
            ".child(\"Stop\")",
        ] {
            assert!(context_toolbar.contains(behavior), "missing {behavior}");
        }

        let render = production
            .split_once("fn render_minimal(")
            .expect("minimal root renderer")
            .1;
        assert!(render.contains("self.render_minimal_titlebar(colors, cx)"));
        assert!(render.contains("self.render_minimal_product_nav(colors, false, cx)"));
    }

    #[test]
    fn minimal_settings_exposes_the_all_permissions_toggle() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let settings = production
            .split_once("let theme_overlay = self.minimal_theme_open.then(||")
            .expect("minimal settings overlay")
            .1
            .split_once("let remote_overlay")
            .expect("end of minimal settings overlay")
            .0;

        for behavior in [
            ".child(\"All permissions\")",
            "self.settings.allow_all_permissions",
            "this.toggle_minimal_all_permissions(cx)",
        ] {
            assert!(settings.contains(behavior), "missing {behavior}");
        }
    }

    #[test]
    fn minimal_settings_closes_when_the_backdrop_is_clicked() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let settings = production
            .split_once("let theme_overlay = self.minimal_theme_open.then(||")
            .expect("minimal settings overlay")
            .1
            .split_once("let remote_overlay")
            .expect("end of minimal settings overlay")
            .0;

        assert!(settings.contains("this.close_minimal_theme_popup(window, cx)"));
        assert!(settings.contains("cx.stop_propagation()"));
    }

    #[test]
    fn minimal_popups_occlude_the_surfaces_behind_them() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let render = production
            .split_once("fn render_minimal(")
            .expect("minimal root renderer")
            .1;
        let overlays = [
            ("let session_context_overlay =", "let theme_overlay ="),
            ("let theme_overlay =", "let remote_overlay ="),
            ("let remote_overlay =", "let workspace_overlay ="),
            ("let workspace_overlay =", "let sidebar_rename_overlay ="),
            (
                "let sidebar_rename_overlay =",
                "let workspace_delete_overlay =",
            ),
            (
                "let workspace_delete_overlay =",
                "let chat_delete_overlay =",
            ),
            ("let chat_delete_overlay =", "let chat_overlay ="),
            ("let chat_overlay =", "let error_banner ="),
        ];

        for (start, end) in overlays {
            let overlay = render
                .split_once(start)
                .unwrap_or_else(|| panic!("missing {start}"))
                .1
                .split_once(end)
                .unwrap_or_else(|| panic!("missing {end}"))
                .0;
            assert!(
                overlay.contains(".occlude()"),
                "{start} must block pointer events from reaching content behind it"
            );
        }
    }

    #[test]
    fn minimal_popups_take_keyboard_focus_from_the_surface_behind_them() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;

        assert!(production.contains("minimal_popup_focus: FocusHandle"));
        assert!(production.contains("minimal_popup_previous_focus: Option<WeakFocusHandle>"));

        for (start, end) in [
            ("fn begin_workspace_create(", "fn workspace_create_changed("),
            ("fn begin_chat_create(", "fn chat_create_changed("),
            ("fn begin_sidebar_delete(", "fn cancel_sidebar_delete("),
            (
                "fn open_session_context_menu(",
                "fn close_session_context_menu(",
            ),
            ("fn open_remote(", "fn close_remote("),
        ] {
            let opener = production
                .split_once(start)
                .unwrap_or_else(|| panic!("missing {start}"))
                .1
                .split_once(end)
                .unwrap_or_else(|| panic!("missing {end}"))
                .0;
            assert!(
                opener.contains("window: &mut Window"),
                "{start} must receive the window so it can move focus"
            );
            assert!(
                opener.contains("focus_minimal_popup("),
                "{start} must take focus away from the terminal or composer behind it"
            );
        }

        let render = production
            .split_once("fn render_minimal(")
            .expect("minimal root renderer")
            .1;
        let theme_overlay = render
            .split_once("let theme_overlay =")
            .expect("theme overlay")
            .1
            .split_once("let remote_overlay =")
            .expect("end of theme overlay")
            .0;
        assert!(
            theme_overlay.contains(".inset_0()"),
            "the settings popover needs a full-window pointer shield"
        );
        assert!(
            theme_overlay.contains(".track_focus(&minimal_popup_focus)"),
            "the settings popover needs a focused modal scope"
        );
    }

    #[test]
    fn remote_setup_is_one_persisted_ssh_command_without_pairing_fields() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let overlay = production
            .split_once("let remote_overlay =")
            .expect("remote overlay")
            .1
            .split_once("let workspace_overlay =")
            .expect("end of remote overlay")
            .0;

        assert!(overlay.contains("Connect over SSH"));
        assert!(overlay.contains("self.remote_ssh_input.clone()"));
        assert!(!overlay.contains("Pairing code"));
        assert!(!overlay.contains("remote_port_input"));
        assert!(production.contains("settings.remote_ssh_command = Some"));
    }

    #[gpui::test]
    fn minimal_popup_focus_is_taken_and_restored(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        cx.update_window_entity(&desktop, |desktop, window, cx| {
            let terminal_focus = desktop.terminal_input.read(cx).focus_handle(cx);
            window.focus(&terminal_focus);

            desktop.begin_workspace_create(window, cx);
            let workspace_focus = desktop.workspace_create_input.read(cx).focus_handle(cx);
            assert!(workspace_focus.is_focused(window));
            assert!(!terminal_focus.is_focused(window));

            desktop.cancel_workspace_create(cx);
            desktop.restore_minimal_popup_focus(window);
            assert!(terminal_focus.is_focused(window));

            window.blur();
            desktop.begin_sidebar_delete(SidebarTarget::Chat("chat".into()), window, cx);
            assert!(desktop.minimal_popup_focus.is_focused(window));
            desktop.cancel_sidebar_delete(cx);
            desktop.restore_minimal_popup_focus(window);
            assert!(window.focused(cx).is_none());
        });
    }

    #[gpui::test]
    fn switching_sessions_reuses_the_hydrated_terminal_panel(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.chats = vec![
                ChatSummary {
                    id: "chat-a".into(),
                    folder: "project".into(),
                    title: Some("A".into()),
                    backend: "codex".into(),
                    branch: None,
                    working: false,
                    terminal_working: false,
                },
                ChatSummary {
                    id: "chat-b".into(),
                    folder: "project".into(),
                    title: Some("B".into()),
                    backend: "claude".into(),
                    branch: None,
                    working: false,
                    terminal_working: false,
                },
            ];
            desktop.model.selected_chat = Some("chat-a".into());
            desktop.minimal_route = MinimalRoute::Cli {
                project_id: "project".into(),
                chat_id: "chat-a".into(),
                agent: AgentCli::Codex,
            };

            let mut screen = TerminalScreen::new(80, 24);
            screen.feed(b"cached output");
            let mut panel = XdDesktop::new_agent_terminal_panel("chat-a".into(), AgentCli::Codex);
            panel.loading = false;
            panel.auto_open = false;
            panel.selected = Some("terminal-a".into());
            panel.sessions.push(TerminalTab {
                id: "terminal-a".into(),
                title: "Codex".into(),
                agent: Some(AgentCli::Codex),
                sequence: Some(7),
                screen,
            });
            desktop.terminal_panel = Some(panel);

            desktop.select_minimal_session("project".into(), "chat-b".into(), AgentCli::Claude, cx);
            desktop.select_minimal_session("project".into(), "chat-a".into(), AgentCli::Codex, cx);

            let panel = desktop.terminal_panel.as_ref().expect("restored panel");
            assert_eq!(panel.chat_id, "chat-a");
            assert!(!panel.loading);
            assert_eq!(panel.selected.as_deref(), Some("terminal-a"));
            assert_eq!(panel.sessions[0].sequence, Some(7));
            assert_eq!(panel.sessions[0].screen.rendered().text, "cached output");
            desktop.terminal_panel = None;
            desktop.terminal_panel_cache.clear();
        });
    }

    #[gpui::test]
    fn selecting_a_tree_primed_session_waits_for_its_workdir(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.chats = vec![ChatSummary {
                id: "chat".into(),
                folder: "project".into(),
                title: Some("Session".into()),
                backend: "codex".into(),
                branch: None,
                working: false,
                terminal_working: false,
            }];

            // Tree loading primes an empty, non-loading panel before the chat
            // response carrying workdir has been requested.
            desktop.prime_terminal_cache(desktop.active_endpoint);
            assert!(
                !desktop
                    .terminal_panel_cache
                    .get(&(desktop.active_endpoint, "chat".into()))
                    .unwrap()
                    .loading
            );

            desktop.select_minimal_session("project".into(), "chat".into(), AgentCli::Codex, cx);

            let panel = desktop.terminal_panel.as_mut().unwrap();
            assert!(panel.loading);
            assert_eq!(panel.error, None);
            assert!(panel.sessions.is_empty());

            // Clicking the same card retries hydration and clears the stale
            // error produced by older builds instead of preserving it.
            panel.loading = false;
            panel.error = Some("The session working directory is still loading.".into());
            desktop.select_minimal_session("project".into(), "chat".into(), AgentCli::Codex, cx);
            let panel = desktop.terminal_panel.as_ref().unwrap();
            assert!(panel.loading);
            assert_eq!(panel.error, None);
            desktop.terminal_panel = None;
            desktop.terminal_panel_cache.clear();
        });
    }

    #[gpui::test]
    fn background_terminal_output_keeps_a_cached_session_current(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.selected_chat = Some("chat-b".into());
            let mut panel = XdDesktop::new_agent_terminal_panel("chat-a".into(), AgentCli::Codex);
            panel.loading = false;
            panel.auto_open = false;
            panel.selected = Some("terminal-a".into());
            panel.sessions.push(TerminalTab {
                id: "terminal-a".into(),
                title: "Codex".into(),
                agent: Some(AgentCli::Codex),
                sequence: Some(7),
                screen: TerminalScreen::new(80, 24),
            });
            desktop
                .terminal_panel_cache
                .insert((ChatEndpoint::Local, "chat-a".into()), panel);

            desktop.handle_event(
                "terminal-output",
                serde_json::json!({
                    "chat": "chat-a",
                    "terminal": "terminal-a",
                    "sequence": 8,
                    "data": STANDARD.encode(b"live output"),
                }),
                None,
                cx,
            );

            let panel = desktop
                .terminal_panel_cache
                .get(&(ChatEndpoint::Local, "chat-a".into()))
                .expect("cached panel");
            assert_eq!(panel.sessions[0].sequence, Some(8));
            assert_eq!(panel.sessions[0].screen.rendered().text, "live output");
            desktop.terminal_panel_cache.clear();
        });
    }

    #[test]
    fn a_new_chat_takes_the_default_title_unless_one_was_typed() {
        assert_eq!(chat_create_title("Ship the parser"), "Ship the parser");
        // Nobody has to name a chat: an untouched, cleared, or blank field all
        // mean the same thing.
        assert_eq!(chat_create_title(""), DEFAULT_CHAT_TITLE);
        assert_eq!(chat_create_title("   "), DEFAULT_CHAT_TITLE);
        assert_eq!(chat_create_title("  Ship it  "), "Ship it");
    }

    #[test]
    fn new_session_worktrees_default_to_current_and_fall_back_to_the_project_directory() {
        let (worktrees, can_create, selected) = new_session_worktree_state(&serde_json::json!({
            "effective_workdir": "/repo",
            "can_create_worktree": true,
            "worktrees": [
                {"path": "/repo", "branch": "main", "detached": false, "main": true, "current": false},
                {"path": "/repo-feature", "branch": "feature", "detached": false, "main": false, "current": true}
            ]
        }));
        assert!(can_create);
        assert_eq!(worktrees.len(), 2);
        assert_eq!(
            selected,
            Some(NewSessionWorktree::Existing("/repo-feature".into()))
        );

        let (worktrees, can_create, selected) = new_session_worktree_state(&serde_json::json!({
            "effective_workdir": "/plain-project",
            "can_create_worktree": false,
            "worktrees": []
        }));
        assert!(!can_create);
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, "/plain-project");
        assert_eq!(
            selected,
            Some(NewSessionWorktree::Existing("/plain-project".into()))
        );
    }

    #[test]
    fn startup_restores_exactly_one_persisted_runtime() {
        let remote = "remote/dev.example:4001";
        assert_eq!(
            persisted_runtime(Some(remote), Some(remote)),
            ChatEndpoint::Remote
        );
        assert_eq!(
            persisted_runtime(Some("local"), Some(remote)),
            ChatEndpoint::Local
        );
        assert_eq!(persisted_runtime(Some(remote), None), ChatEndpoint::Local);
        assert_eq!(persisted_runtime(None, Some(remote)), ChatEndpoint::Local);
    }

    #[gpui::test]
    fn passive_remote_tree_never_overrides_the_explicit_local_runtime(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.remote_state = RemoteState::Connected;
            desktop.handle_remote_update(
                HostUpdate::Reply {
                    kind: RequestKind::Tree,
                    body: serde_json::json!({
                        "ok": true,
                        "folders": [{"id": "folder", "name": "Remote workspace"}],
                        "chats": [{
                            "id": "remote-chat",
                            "folder": "folder",
                            "title": "Loaded on startup",
                            "backend": "codex"
                        }]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    attachments: None,
                },
                desktop.remote_generation,
                cx,
            );

            assert_eq!(desktop.active_endpoint, ChatEndpoint::Local);
            assert_eq!(desktop.model.selected_chat, None);
            assert_eq!(desktop.inactive_model.chats[0].id, "remote-chat");
        });
    }

    #[test]
    fn workspace_clone_outcomes_keep_identical_folder_ids_isolated() {
        let local = (ChatEndpoint::Local, "same-folder".to_owned());
        let remote = (ChatEndpoint::Remote, "same-folder".to_owned());
        let mut outcomes = HashMap::new();
        outcomes.insert(local, None::<String>);
        outcomes.insert(remote, Some("remote clone failed".to_owned()));
        assert_eq!(outcomes.len(), 2);
    }

    #[test]
    fn workspace_completion_never_creates_a_session_implicitly() {
        let source = include_str!("main.rs");
        let completion = source
            .split_once("fn handle_workspace_create_reply(")
            .expect("workspace completion handler")
            .1
            .split_once("fn handle_folder_clone_event(")
            .expect("clone completion handler")
            .0;
        let clone_event = source
            .split_once("fn handle_folder_clone_event(")
            .expect("clone completion handler")
            .1
            .split_once("fn apply_reply(")
            .expect("end of clone completion handler")
            .0;

        assert!(!completion.contains(".new_chat("));
        assert!(!clone_event.contains(".new_chat("));
    }

    #[test]
    fn remote_endpoint_accepts_chat_writes_but_not_machine_admin_replies() {
        assert!(XdDesktop::remote_chat_reply(&RequestKind::Send {
            chat_id: "chat".into(),
            text: "hello".into(),
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::SetDraft {
            chat_id: "chat".into(),
            text: "partial".into(),
            attachment_generation: None,
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::DiffRead {
            chat_id: "chat".into(),
            read: "working-all".into(),
            path: None,
            generation: 1,
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::TerminalOpen {
            chat_id: "chat".into(),
            reuse: false,
            agent: Some("codex".into()),
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::RenameFolder {
            folder_id: "workspace".into(),
            name: "Remote workspace".into(),
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::AgentSecrets {
            folder_id: Some("workspace".into()),
        }));
        assert!(!XdDesktop::remote_chat_reply(&RequestKind::Search {
            query: "needle".into(),
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::AgentAuth));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::AgentClis));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::AgentSecrets {
            folder_id: None,
        }));
        assert!(XdDesktop::remote_chat_reply(
            &RequestKind::SetAgentSecrets { folder_id: None }
        ));
        assert!(!XdDesktop::remote_chat_reply(&RequestKind::Devices));
        assert!(!XdDesktop::remote_chat_reply(&RequestKind::HostUpdate {
            action: "install".into(),
        }));
        assert!(!XdDesktop::local_admin_reply(&RequestKind::AgentSecrets {
            folder_id: None,
        }));
        assert!(!XdDesktop::local_admin_reply(&RequestKind::AgentSecrets {
            folder_id: Some("workspace".into()),
        }));
        assert!(XdDesktop::local_admin_reply(&RequestKind::Devices));
        assert!(!XdDesktop::local_admin_reply(&RequestKind::AgentAuth));
        assert!(!XdDesktop::local_admin_event("agent-auth-changed"));
        assert!(XdDesktop::local_admin_event("host-update"));
        assert!(!XdDesktop::local_admin_event("turn-finished"));
        assert!(XdDesktop::remote_read_event("queued"));
        assert!(XdDesktop::remote_read_event("terminal-activity"));
        assert!(XdDesktop::remote_read_event("terminal-output"));
        assert!(XdDesktop::remote_read_event("git-draft-finished"));
        assert!(XdDesktop::remote_read_event("agent-auth-changed"));
        assert!(XdDesktop::remote_read_event("agent-cli-changed"));
        assert!(!XdDesktop::remote_read_event("devices-changed"));
    }

    #[test]
    fn every_agent_mark_is_embedded_and_drawable() {
        for path in [CLAUDE_ICON, CODEX_ICON, JCODE_ICON, COPILOT_ICON] {
            let bytes = EmbeddedIcons
                .load(path)
                .expect("embedded marks load")
                .expect("a known agent has a mark on disk");
            assert!(String::from_utf8_lossy(&bytes).contains("<svg"));
        }
        assert!(EmbeddedIcons.load("icons/missing.svg").unwrap().is_none());
    }

    #[test]
    fn product_mark_is_embedded_and_not_x_shaped() {
        let bytes = EmbeddedIcons
            .load(XD_MARK_ICON)
            .expect("embedded mark loads")
            .expect("the product mark exists");
        let mark = String::from_utf8_lossy(&bytes);
        assert!(mark.contains("<rect"));
        assert!(mark.contains("M8 4h10"));
        assert!(!mark.contains("rotate"));
    }

    #[test]
    fn composer_action_icons_are_embedded_and_drawable() {
        for path in [
            "icons/send.svg",
            "icons/stop.svg",
            GIT_BRANCH_ICON,
            TRASH_ICON,
        ] {
            let bytes = EmbeddedIcons
                .load(path)
                .expect("embedded icons load")
                .unwrap_or_else(|| panic!("the composer action icon {path} is embedded"));
            assert!(String::from_utf8_lossy(&bytes).contains("<svg"));
        }
    }

    #[test]
    fn every_trash_icon_has_an_explicit_resting_color() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let mut count = 0;

        for (index, _) in production.match_indices(".path(TRASH_ICON)") {
            count += 1;
            let style = &production[index..production.len().min(index + 160)];
            assert!(
                style.contains(".text_color("),
                "trash icon {count} needs its own color because SVG styles do not inherit from the button"
            );
        }

        assert_eq!(count, 1);
    }

    #[gpui::test]
    fn background_turn_start_updates_the_session_board(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.selected_chat = Some("foreground".into());
            desktop.model.chats = vec![
                ChatSummary {
                    id: "foreground".into(),
                    folder: "workspace".into(),
                    title: Some("Foreground".into()),
                    backend: "codex".into(),
                    branch: Some("main".into()),
                    working: false,
                    terminal_working: false,
                },
                ChatSummary {
                    id: "background".into(),
                    folder: "workspace".into(),
                    title: Some("Background".into()),
                    backend: "claude".into(),
                    branch: Some("session/background".into()),
                    working: false,
                    terminal_working: false,
                },
            ];

            desktop.handle_event(
                "turn-started",
                serde_json::json!({"chat": "background"}),
                None,
                cx,
            );

            assert!(!desktop.model.working);
            assert!(
                desktop
                    .model
                    .chats
                    .iter()
                    .find(|chat| chat.id == "background")
                    .unwrap()
                    .working
            );
        });
    }

    #[gpui::test]
    fn background_terminal_activity_updates_the_session_board(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.selected_chat = Some("foreground".into());
            desktop.model.chats = vec![
                ChatSummary {
                    id: "foreground".into(),
                    folder: "workspace".into(),
                    title: Some("Foreground".into()),
                    backend: "codex".into(),
                    branch: Some("main".into()),
                    working: false,
                    terminal_working: false,
                },
                ChatSummary {
                    id: "background".into(),
                    folder: "workspace".into(),
                    title: Some("Background".into()),
                    backend: "claude".into(),
                    branch: Some("session/background".into()),
                    working: false,
                    terminal_working: false,
                },
            ];

            desktop.handle_event(
                "terminal-activity",
                serde_json::json!({
                    "chat": "background",
                    "working": false,
                    "terminal_working": true
                }),
                None,
                cx,
            );

            assert!(!desktop.model.working);
            assert!(
                desktop
                    .model
                    .chats
                    .iter()
                    .find(|chat| chat.id == "background")
                    .unwrap()
                    .terminal_working
            );

            desktop.handle_event(
                "terminal-activity",
                serde_json::json!({
                    "chat": "background",
                    "working": true,
                    "terminal_working": false
                }),
                None,
                cx,
            );

            assert!(
                !minimal::project_sessions("workspace", &desktop.model.chats)
                    .into_iter()
                    .find(|chat| chat.id == "background")
                    .unwrap()
                    .working
            );
        });
    }

    #[test]
    fn sidebar_rename_waits_for_the_authoritative_tree_name() {
        let model = AppModel {
            folders: vec![Folder {
                id: "folder-1".into(),
                name: "Renamed workspace".into(),
                parent: None,
            }],
            chats: vec![xd_desktop::model::ChatSummary {
                id: "chat-1".into(),
                folder: "folder-1".into(),
                title: Some("Old chat".into()),
                backend: "codex".into(),
                branch: None,
                working: false,
                terminal_working: false,
            }],
            ..Default::default()
        };
        let workspace = SidebarEdit {
            target: SidebarTarget::Folder("folder-1".into()),
            original: "Old workspace".into(),
            text: "Renamed workspace".into(),
            submitting: true,
        };
        let chat = SidebarEdit {
            target: SidebarTarget::Chat("chat-1".into()),
            original: "Old chat".into(),
            text: "Renamed chat".into(),
            submitting: true,
        };

        assert!(sidebar_edit_applied(&model, &workspace));
        assert!(!sidebar_edit_applied(&model, &chat));
        assert!(!sidebar_edit_applied(
            &model,
            &SidebarEdit {
                submitting: false,
                ..workspace
            }
        ));
    }

    #[test]
    fn optional_workspace_repository_ignores_only_blank_input() {
        assert_eq!(optional_trimmed("  /tmp/repo  "), Some("/tmp/repo"));
        assert_eq!(optional_trimmed(" \n\t "), None);
    }

    #[test]
    fn pairing_details_require_a_complete_bounded_response() {
        assert_eq!(
            pairing_details(&serde_json::json!({
                "host": "192.168.1.10",
                "port": 4001,
                "code": "ABCD-EFGH"
            }))
            .unwrap(),
            ("192.168.1.10".into(), 4001, "ABCD-EFGH".into())
        );
        assert!(pairing_details(&serde_json::json!({"port": 0})).is_err());
        assert!(
            pairing_details(&serde_json::json!({
                "host": "host",
                "port": 65_536,
                "code": "code"
            }))
            .is_err()
        );
    }

    #[test]
    fn host_reconnect_backoff_is_fast_then_bounded() {
        assert_eq!(reconnect_delay(0), Duration::ZERO);
        assert_eq!(reconnect_delay(1), Duration::from_millis(250));
        assert_eq!(reconnect_delay(2), Duration::from_millis(500));
        assert_eq!(reconnect_delay(4), Duration::from_secs(2));
        assert_eq!(reconnect_delay(5), Duration::from_secs(5));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_secs(5));
    }

    #[test]
    fn terminal_geometry_excludes_the_output_padding_and_stays_bounded() {
        // The terminal output uses 16 px of padding on every side. Counting
        // any of that padding as usable PTY space creates a partial extra row,
        // so following the bottom clips the first visible terminal line.
        assert_eq!(terminal_geometry(824.0, 252.0, 8.0, 19.0), (99, 11));
        assert_eq!(terminal_geometry(1.0, 1.0, 8.0, 19.0), (20, 4));
        assert_eq!(
            terminal_geometry(100_000.0, 100_000.0, 8.0, 19.0),
            (500, 200)
        );
    }

    #[test]
    fn terminal_tabs_restore_replay_and_fall_back_after_close() {
        let first = XdDesktop::terminal_tab_from_snapshot(&serde_json::json!({
            "id": "terminal-one",
            "title": "shell one",
            "columns": 40,
            "rows": 8,
            "replay": [{"data": "Zmlyc3Q="}],
        }))
        .unwrap();
        let second = XdDesktop::terminal_tab_from_snapshot(&serde_json::json!({
            "id": "terminal-two",
            "title": "shell two",
            "columns": 80,
            "rows": 12,
            "replay": [],
        }))
        .unwrap();
        assert!(first.screen.rendered().text.contains("first"));
        assert_eq!(first.screen.geometry(), (40, 8));

        let mut checkpoint_screen = TerminalScreen::new(40, 8);
        checkpoint_screen.feed(b"checkpoint");
        let checkpoint = checkpoint_screen
            .checkpoint_bytes_bounded(1024 * 1024)
            .expect("terminal checkpoint");
        let restored = XdDesktop::terminal_tab_from_snapshot(&serde_json::json!({
            "id": "terminal-checkpoint",
            "columns": 40,
            "rows": 8,
            "replay": [
                {
                    "checkpoint": STANDARD.encode(checkpoint),
                    "data": STANDARD.encode(b"wrong fallback"),
                },
                {"data": STANDARD.encode(b"!")},
            ],
        }))
        .unwrap();
        assert_eq!(restored.screen.rendered().text, "checkpoint!");

        let fallback = XdDesktop::terminal_tab_from_snapshot(&serde_json::json!({
            "id": "terminal-fallback",
            "columns": 40,
            "rows": 8,
            "replay": [{
                "checkpoint": STANDARD.encode(b"invalid checkpoint"),
                "data": STANDARD.encode(b"safe fallback"),
            }],
        }))
        .unwrap();
        assert_eq!(fallback.screen.rendered().text, "safe fallback");
        assert!(
            XdDesktop::terminal_tab_from_snapshot(&serde_json::json!({
                "id": "terminal-corrupt",
                "columns": 40,
                "rows": 8,
                "sequence": 99,
                "replay": [{
                    "checkpoint": STANDARD.encode(b"invalid checkpoint"),
                    "data": "not base64",
                }],
            }))
            .is_none()
        );

        let mut panel = TerminalPanel {
            chat_id: "chat".into(),
            agent: None,
            allow_agent_tabs: false,
            sessions: vec![first, second],
            selected: Some("terminal-one".into()),
            viewport: None,
            auto_open: false,
            opening: false,
            opening_agent: None,
            loading: false,
            pending_events: Vec::new(),
            error: Some("Terminal disconnected.".into()),
        };
        panel.remove("terminal-one");
        assert_eq!(panel.selected.as_deref(), Some("terminal-two"));
        assert_eq!(
            panel.selected().map(|session| session.title.as_str()),
            Some("Terminal")
        );

        let mut empty = XdDesktop::new_agent_terminal_panel("chat".into(), AgentCli::Codex);
        // Measuring the viewport can race the terminal-list reply. Do not
        // create a duplicate while the host may still report an existing
        // session; an empty reply enables auto-open once it arrives.
        assert!(!empty.should_auto_open());
        empty.loading = false;
        assert!(empty.should_auto_open());
        empty.opening = true;
        assert!(!empty.should_auto_open());
    }

    #[test]
    fn minimal_cli_panel_accepts_shell_and_agent_tabs() {
        let mut panel = XdDesktop::new_agent_terminal_panel("chat".into(), AgentCli::Codex);

        for (terminal, agent) in [
            ("shell", None),
            ("codex", Some("codex")),
            ("claude", Some("claude")),
            ("jcode", Some("jcode")),
            ("copilot", Some("copilot")),
        ] {
            let mut opened = serde_json::json!({
                "chat": "chat",
                "terminal": terminal,
                "title": terminal,
                "columns": 80,
                "rows": 24,
                "sequence": 0,
            });
            if let Some(agent) = agent {
                opened["agent"] = serde_json::Value::String(agent.into());
            }
            assert!(XdDesktop::apply_terminal_opened_event(&mut panel, &opened));
        }

        assert_eq!(
            panel
                .sessions
                .iter()
                .map(|session| session.agent)
                .collect::<Vec<_>>(),
            [
                None,
                Some(AgentCli::Codex),
                Some(AgentCli::Claude),
                Some(AgentCli::Jcode),
                Some(AgentCli::Copilot),
            ]
        );
    }

    #[test]
    fn unrelated_tabs_do_not_suppress_the_route_agent_auto_open() {
        let mut panel = XdDesktop::new_agent_terminal_panel("chat".into(), AgentCli::Codex);
        panel.opening = true;
        panel.opening_agent = Some(AgentCli::Codex);
        assert!(panel.opening_matches_protocol_agent(Some("codex")));
        assert!(!panel.opening_matches_protocol_agent(Some("claude")));
        assert!(!panel.opening_matches_protocol_agent(Some("unsupported")));
        assert!(!panel.opening_matches_protocol_agent(None));
        assert!(XdDesktop::apply_terminal_opened_event(
            &mut panel,
            &serde_json::json!({
                "chat": "chat",
                "terminal": "shell",
                "title": "Terminal",
                "columns": 80,
                "rows": 24,
                "sequence": 0,
            }),
        ));
        assert!(
            panel.opening,
            "an uncorrelated terminal event must not complete our pending open"
        );
        panel.finish_opening();
        panel.loading = false;
        assert!(panel.should_auto_open());

        assert!(XdDesktop::apply_terminal_opened_event(
            &mut panel,
            &serde_json::json!({
                "chat": "chat",
                "terminal": "codex",
                "title": "Codex",
                "agent": "codex",
                "columns": 80,
                "rows": 24,
                "sequence": 0,
            }),
        ));
        assert!(!panel.should_auto_open());
    }

    #[test]
    fn matching_local_terminal_open_completes_the_pending_tab() {
        let mut panel = XdDesktop::new_agent_terminal_panel("chat".into(), AgentCli::Claude);
        panel.opening = true;
        panel.opening_agent = Some(AgentCli::Claude);

        assert!(XdDesktop::apply_terminal_opened_event(
            &mut panel,
            &serde_json::json!({
                "chat": "chat",
                "terminal": "unsupported",
                "agent": "unsupported",
                "columns": 80,
                "rows": 24,
            }),
        ));
        assert!(panel.opening);

        assert!(XdDesktop::apply_terminal_opened_event(
            &mut panel,
            &serde_json::json!({
                "chat": "chat",
                "terminal": "claude",
                "title": "Claude",
                "agent": "claude",
                "columns": 80,
                "rows": 24,
            }),
        ));
        assert!(
            !panel.opening,
            "the opened event must allow another terminal or agent tab to start"
        );
    }

    #[test]
    fn restoring_mixed_tabs_prefers_the_route_agent_over_hash_order() {
        let shell = XdDesktop::terminal_tab_from_snapshot(&serde_json::json!({
            "id": "shell",
            "title": "Lunar",
            "columns": 80,
            "rows": 24,
        }))
        .unwrap();
        let codex = XdDesktop::terminal_tab_from_snapshot(&serde_json::json!({
            "id": "codex",
            "title": "Codex",
            "agent": "codex",
            "columns": 80,
            "rows": 24,
        }))
        .unwrap();
        let mut panel = XdDesktop::new_agent_terminal_panel("chat".into(), AgentCli::Codex);
        panel.sessions = vec![shell, codex];

        assert_eq!(panel.sessions[0].title, "Terminal");
        assert_eq!(
            panel.selection_after_refresh(None).as_deref(),
            Some("codex")
        );
        assert_eq!(
            panel
                .selection_after_refresh(Some("shell".into()))
                .as_deref(),
            Some("shell")
        );
    }

    #[test]
    fn minimal_terminal_plus_opens_an_agent_tab_menu() {
        let source = include_str!("main.rs");
        let terminal = source
            .split_once("fn render_minimal_terminal(")
            .expect("minimal terminal renderer")
            .1
            .split_once("fn render_minimal_titlebar")
            .expect("end of minimal terminal renderer")
            .0;

        assert!(terminal.contains("minimal-new-tab-menu"));
        for (id, label) in [
            ("minimal-new-shell-tab", "Terminal"),
            ("minimal-new-codex-tab", "Codex"),
            ("minimal-new-claude-tab", "Claude"),
            ("minimal-new-jcode-tab", "JCode"),
            ("minimal-new-copilot-tab", "Copilot"),
        ] {
            assert!(terminal.contains(id), "missing {label} tab choice");
            assert!(terminal.contains(&format!(".child(\"{label}\")")));
        }
        assert!(terminal.contains("this.toggle_minimal_new_tab_menu(window, cx)"));
        assert!(terminal.contains("this.open_minimal_terminal_tab(None, window, cx)"));
        assert!(terminal.contains("Some(AgentCli::Codex)"));
        assert!(terminal.contains("Some(AgentCli::Claude)"));
        assert!(terminal.contains("Some(AgentCli::Jcode)"));
        assert!(terminal.contains("Some(AgentCli::Copilot)"));
        assert!(terminal.contains(".absolute()"));
    }

    #[test]
    fn terminal_tab_hover_does_not_paint_a_rectangular_fill() {
        let source = include_str!("main.rs");
        let terminal = source
            .split_once("fn render_minimal_terminal(")
            .expect("minimal terminal renderer")
            .1
            .split_once("fn render_minimal_titlebar")
            .expect("end of minimal terminal renderer")
            .0;
        let tab_chrome = terminal
            .split_once("let tabs = panel")
            .expect("terminal tab builder")
            .1
            .split_once(".child(session.title.clone())")
            .expect("terminal tab title")
            .0;

        assert!(tab_chrome.contains("style.text_color(rgb(colors.text))"));
        assert!(!tab_chrome.contains("style.bg("));
    }

    #[test]
    fn terminal_measurement_and_input_do_not_expand_the_scroll_content() {
        let source = include_str!("main.rs");
        let terminal = source
            .split_once("fn render_minimal_terminal(")
            .expect("minimal terminal renderer")
            .1
            .split_once("fn render_minimal_titlebar")
            .expect("end of minimal terminal renderer")
            .0;
        let scroller = terminal
            .split_once("let output_scroller =")
            .expect("terminal output scroller builder")
            .1
            .split_once("let measurement_canvas =")
            .expect("end of terminal output scroller")
            .0;

        // GPUI includes absolute direct children in a tracked div's content
        // bounds. The full-height measurement canvas and bottom-anchored input
        // must therefore be siblings, or every fresh terminal starts scrolled
        // down into phantom overflow.
        assert!(!scroller.contains("canvas("));
        assert!(!scroller.contains(".child(terminal_input)"));
        assert!(terminal.contains(".child(output_scroller)"));
        assert!(terminal.contains(".child(terminal_input)"));
    }

    #[test]
    fn terminal_selection_uses_its_output_scroll_handle() {
        let terminal = include_str!("main.rs")
            .split_once("fn render_minimal_terminal(")
            .expect("minimal terminal renderer")
            .1
            .split_once("fn render_minimal_context_toolbar(")
            .expect("end of minimal terminal renderer")
            .0;

        assert!(terminal.contains("with_selection_scroll(self.terminal_scroll.clone())"));
    }

    #[test]
    fn terminal_rows_never_wrap_inside_the_measured_viewport() {
        let source = include_str!("main.rs");
        let terminal = source
            .split_once("fn render_minimal_terminal(")
            .expect("minimal terminal renderer")
            .1
            .split_once("fn render_minimal_titlebar")
            .expect("end of minimal terminal renderer")
            .0;
        let scroller = terminal
            .split_once("let output_scroller =")
            .expect("terminal output scroller builder")
            .1
            .split_once("let measurement_canvas =")
            .expect("end of terminal output scroller")
            .0;

        // The PTY has already wrapped every row at the reported column count.
        // Letting GPUI wrap the rendered text a second time turns one terminal
        // row into two visual rows and pushes later rows below the viewport.
        assert!(scroller.contains(".whitespace_nowrap()"));
    }

    #[test]
    fn terminal_wheel_events_are_encoded_for_tmux_scrollback() {
        assert_eq!(terminal_mouse_scroll_bytes(1.0), b"\x1b[<64;1;1M");
        assert_eq!(terminal_mouse_scroll_bytes(-1.0), b"\x1b[<65;1;1M");
        assert!(terminal_mouse_scroll_bytes(0.0).is_empty());
    }

    #[test]
    fn tui_cursor_is_inserted_before_later_terminal_styles() {
        let ansi = HighlightStyle {
            color: Some(rgb(0xff0000).into()),
            ..Default::default()
        };
        let cursor = HighlightStyle {
            color: Some(rgb(0x000000).into()),
            background_color: Some(rgb(0xffffff).into()),
            ..Default::default()
        };

        let actual = insert_terminal_cursor_highlight(
            vec![(0..2, ansi), (8..14, ansi)],
            Some((3..4, cursor)),
        );

        assert_eq!(
            actual
                .iter()
                .map(|(range, _)| range.clone())
                .collect::<Vec<_>>(),
            [0..2, 3..4, 8..14]
        );
        assert_eq!(actual[1].1, cursor);
        assert!(
            actual
                .windows(2)
                .all(|pair| pair[0].0.end <= pair[1].0.start)
        );
    }

    #[test]
    fn tui_cursor_splits_an_overlapping_terminal_style() {
        let ansi = HighlightStyle {
            color: Some(rgb(0xff0000).into()),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let cursor = HighlightStyle {
            color: Some(rgb(0x000000).into()),
            background_color: Some(rgb(0xffffff).into()),
            ..Default::default()
        };

        let actual = insert_terminal_cursor_highlight(
            vec![(0..2, ansi), (3..14, ansi)],
            Some((5..6, cursor)),
        );

        assert_eq!(
            actual
                .iter()
                .map(|(range, _)| range.clone())
                .collect::<Vec<_>>(),
            [0..2, 3..5, 5..6, 6..14]
        );
        assert_eq!(actual[1].1, ansi);
        assert_eq!(actual[2].1, cursor);
        assert_eq!(actual[3].1, ansi);
    }

    #[test]
    fn minimal_terminal_makes_detected_web_urls_clickable() {
        let source = include_str!("main.rs");
        let terminal = source
            .split_once("fn render_minimal_terminal(")
            .expect("minimal terminal renderer")
            .1
            .split_once("fn render_minimal_titlebar")
            .expect("end of minimal terminal renderer")
            .0;

        assert!(terminal.contains("markdown::web_links("));
        assert!(terminal.contains("selectable_links_in_document("));
        assert!(terminal.contains("cx.open_url(url)"));
    }

    #[test]
    fn minimal_terminal_keeps_osc8_link_targets_clickable() {
        let source = include_str!("main.rs");
        let terminal = source
            .split_once("fn render_minimal_terminal(")
            .expect("minimal terminal renderer")
            .1
            .split_once("fn render_minimal_titlebar")
            .expect("end of minimal terminal renderer")
            .0;

        assert!(terminal.contains("output.links"));
        assert!(terminal.contains("let detected_links = markdown::web_links("));
        assert!(terminal.contains("terminal_links.extend(detected_links)"));
    }

    #[test]
    fn projects_and_cli_have_distinct_browsing_surfaces() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        assert!(production.contains("fn render_minimal_session_board("));

        let board = production
            .split_once("fn render_minimal_session_board(")
            .expect("shared session board")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of shared session board")
            .0;
        for behavior in [
            "minimal-session-board",
            "minimal-session-group",
            "minimal-session-card",
            "GIT_BRANCH_ICON",
            ".child(session.branch)",
            "this.toggle_folder_collapsed",
            "this.begin_chat_create",
            ".child(\"Working\")",
            ".child(\"Idle\")",
        ] {
            assert!(board.contains(behavior), "missing {behavior}");
        }
        assert!(!board.contains("format!(\"{} CLI\", agent.label())"));

        let home = production
            .split_once("fn render_minimal_home(")
            .expect("minimal home renderer")
            .1
            .split_once("fn render_minimal_cli(")
            .expect("end of minimal home renderer")
            .0;
        let cli = production
            .split_once("fn render_minimal_cli(")
            .expect("minimal CLI renderer")
            .1
            .split_once("fn render_minimal(")
            .expect("end of minimal CLI renderer")
            .0;
        assert!(home.contains("project_cards(&self.model.folders, &self.model.chats)"));
        assert!(home.contains("minimal-project-list"));
        assert!(home.contains("minimal-project-content"));
        assert!(home.contains("minimal-new-session"));
        assert!(!home.contains("self.render_minimal_session_board("));
        assert!(cli.contains("self.render_minimal_session_board("));
        assert!(!cli.contains("minimal-cli-session-list"));
    }

    #[test]
    fn completed_unread_sessions_render_done_until_they_are_viewed() {
        let source = include_str!("main.rs");
        let board = source
            .split_once("fn render_minimal_session_board(")
            .expect("shared session board")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of shared session board")
            .0;

        assert!(board.contains("self.model.unread_chats.contains(&session.id)"));
        assert!(board.contains(".child(\"Done\")"));
        assert!(board.contains("DONE_STATUS_COLOR"));

        let home = source
            .split_once("fn render_minimal_home(")
            .expect("projects home")
            .1
            .split_once("fn render_minimal_cli(")
            .expect("end of projects home")
            .0;
        assert!(home.contains("self.model.unread_chats.contains(&session.id)"));
        assert!(home.contains(".child(\"Done\")"));
        assert!(home.contains("DONE_STATUS_COLOR"));
    }

    #[test]
    fn session_board_truncates_branch_names_inside_cards() {
        let source = include_str!("main.rs");
        let board = source
            .split_once("fn render_minimal_session_board(")
            .expect("shared session board")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of shared session board")
            .0;

        assert!(
            board.contains(".child(div().min_w_0().flex_1().truncate().child(session.branch))"),
            "branch text needs a shrinking, truncating flex child"
        );
    }

    #[test]
    fn session_board_only_emphasizes_the_selected_chat() {
        let source = include_str!("main.rs");
        let board = source
            .split_once("fn render_minimal_session_board(")
            .expect("shared session board")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of shared session board")
            .0;

        assert!(board.contains("let emphasized = selected;"));
        assert!(!board.contains("let emphasized = selected || session.working;"));
    }

    #[test]
    fn chat_cards_offer_rename_and_delete_from_a_right_click_menu() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let board = production
            .split_once("fn render_minimal_session_board(")
            .expect("shared session board")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of shared session board")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("minimal home renderer")
            .1
            .split_once("fn render_minimal_cli(")
            .expect("end of minimal home renderer")
            .0;

        for cards in [board, home] {
            assert!(cards.contains("MouseButton::Right"));
            assert!(cards.contains("this.open_session_context_menu("));
            assert!(!cards.contains("minimal-delete-session"));
        }

        assert!(production.contains(".child(\"Rename session\")"));
        assert!(production.contains(".child(\"Delete session\")"));
        assert!(production.contains("SidebarTarget::Chat"));
        assert!(production.contains("this.begin_sidebar_edit("));
        assert!(production.contains("this.begin_sidebar_delete("));
    }

    #[test]
    fn minimal_projects_exposes_workspace_rename_flow() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("minimal home renderer")
            .1
            .split_once("fn render_minimal_cli(")
            .expect("end of minimal home renderer")
            .0;
        for behavior in [
            "minimal-rename-project",
            "this.begin_sidebar_edit(",
            "SidebarTarget::Folder",
        ] {
            assert!(home.contains(behavior), "missing {behavior}");
        }

        let root = production
            .split_once("fn render_minimal(")
            .expect("minimal root renderer")
            .1;
        for behavior in [
            "let sidebar_rename_overlay =",
            "\"Rename project\"",
            ".child(heading)",
            "self.sidebar_edit_input.clone()",
            "minimal-cancel-project-rename",
            "minimal-save-project-rename",
            "this.save_sidebar_edit(cx)",
            ".when_some(sidebar_rename_overlay",
        ] {
            assert!(root.contains(behavior), "missing {behavior}");
        }
    }

    #[test]
    fn minimal_projects_exposes_confirmed_workspace_delete_flow() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("minimal home renderer")
            .1
            .split_once("fn render_minimal_cli(")
            .expect("end of minimal home renderer")
            .0;
        for behavior in [
            "minimal-delete-project",
            "this.begin_sidebar_delete(",
            "SidebarTarget::Folder",
            ".path(TRASH_ICON)",
        ] {
            assert!(home.contains(behavior), "missing {behavior}");
        }
        assert!(
            !home.contains(".child(\"Delete\")"),
            "the project delete control should use an icon instead of text"
        );

        let root = production
            .split_once("fn render_minimal(")
            .expect("minimal root renderer")
            .1;
        for behavior in [
            "let workspace_delete_overlay =",
            ".child(\"Delete project?\")",
            "minimal-cancel-project-delete",
            "minimal-confirm-project-delete",
            "this.confirm_sidebar_delete(cx)",
            ".when_some(workspace_delete_overlay",
        ] {
            assert!(root.contains(behavior), "missing {behavior}");
        }
    }

    #[test]
    fn minimal_sessions_expose_confirmed_delete_flow() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let board = production
            .split_once("fn render_minimal_session_board(")
            .expect("shared session board")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of shared session board")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("minimal home renderer")
            .1
            .split_once("fn render_minimal_cli(")
            .expect("end of minimal home renderer")
            .0;
        for surface in [board, home] {
            assert!(surface.contains("this.open_session_context_menu("));
            assert!(!surface.contains("minimal-delete-session"));
        }

        let root = production
            .split_once("fn render_minimal(")
            .expect("minimal root renderer")
            .1;
        for behavior in [
            "let chat_delete_overlay =",
            "let session_context_overlay =",
            ".child(\"Delete session\")",
            "this.begin_sidebar_delete(",
            "SidebarTarget::Chat",
            ".child(\"Delete session?\")",
            "minimal-cancel-session-delete",
            "minimal-confirm-session-delete",
            "this.confirm_sidebar_delete(cx)",
            ".when_some(chat_delete_overlay",
        ] {
            assert!(root.contains(behavior), "missing {behavior}");
        }
    }

    #[test]
    fn desktop_has_only_the_minimal_render_path() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop test module")
            .0;
        assert!(!production.contains("fn minimal_agent_desktop_enabled()"));

        let render = source
            .split_once("impl Render for XdDesktop")
            .expect("desktop render implementation")
            .1
            .split_once("fn load_png_attachments")
            .expect("end of desktop render implementation")
            .0;
        assert!(render.contains("self.expire_action_error(cx);"));
        assert!(render.contains("self.render_minimal(custom_titlebar, window, cx)"));
        for legacy_surface in [
            "let composer =",
            "let diff_pane =",
            "let file_tree =",
            ".id(\"send\")",
        ] {
            assert!(!render.contains(legacy_surface));
        }
    }

    #[test]
    fn session_title_submit_uses_the_minimal_agent_creation_flow() {
        let source = include_str!("main.rs");
        let subscription = source
            .split_once("let chat_create_input =")
            .expect("session title input")
            .1
            .split_once("let workspace_context_input =")
            .expect("end of session title input subscription")
            .0;

        assert!(
            subscription.contains("ComposerEvent::Submit => this.save_minimal_chat_create(cx)")
        );
        assert!(!subscription.contains("this.save_chat_create(cx)"));
    }

    #[test]
    fn new_session_flow_chooses_an_existing_or_new_worktree_before_creation() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;

        for behavior in [
            "chat_create_worktrees_loading",
            "NewSessionWorktree::New",
            "NewSessionWorktree::Existing",
            "Create new worktree",
            "Existing worktrees",
            "host.folder_settings(&folder_id)",
            "new_chat_with_backend_in_worktree",
        ] {
            assert!(production.contains(behavior), "missing {behavior}");
        }
    }

    #[test]
    fn terminal_copy_paste_and_interrupt_shortcuts_follow_shell_conventions() {
        let bindings = include_str!("main.rs");
        assert!(bindings.contains("ComposerEvent::PasteImage { format, bytes } =>"));
        assert!(bindings.contains("this.paste_terminal_image(*format, bytes, cx)"));
        assert!(bindings.contains(
            "KeyBinding::new(\"shift-enter\", TerminalControlJ, Some(\"TerminalInput\"))"
        ));
        assert!(
            bindings.contains("KeyBinding::new(\"ctrl-c\", Interrupt, Some(\"TerminalInput\"))")
        );
        assert!(
            bindings.contains("KeyBinding::new(\"ctrl-shift-c\", Copy, Some(\"TerminalInput\"))")
        );
        assert!(
            bindings.contains("KeyBinding::new(\"ctrl-shift-v\", Paste, Some(\"TerminalInput\"))")
        );
        assert!(!bindings.contains("KeyBinding::new(\"ctrl-v\", Paste, Some(\"TerminalInput\"))"));

        let input = include_str!("input.rs");
        let interrupt = input
            .split_once("fn interrupt(")
            .expect("terminal interrupt handler")
            .1
            .split_once("fn end_of_file(")
            .expect("end of terminal interrupt handler")
            .0;
        assert!(!interrupt.contains("TextSelection::selected"));
        assert!(interrupt.contains("self.terminal_bytes(vec![3], cx)"));
    }

    #[test]
    fn plain_terminal_tabs_delegate_startup_to_the_users_shell() {
        let production = include_str!("main.rs");
        let command = production
            .split_once("fn terminal_agent_command(")
            .expect("terminal command builder")
            .1
            .split_once("fn resize_terminal_viewport(")
            .expect("end of terminal command builder")
            .0;

        assert!(command.contains("None => AgentCommand::user_shell()"));
        assert!(!command.contains("\"sh\".into()"));
        assert!(!command.contains("[\"-l\"]"));
    }

    #[test]
    fn jcode_prompt_marker_reports_working_and_idle_states() {
        assert_eq!(
            jcode_terminal_screen_working("history\n18… 󰌘\n"),
            Some(true)
        );
        assert_eq!(
            jcode_terminal_screen_working("history\n19> 󰖟\n"),
            Some(false)
        );
        assert_eq!(
            jcode_terminal_screen_working("ordinary shell output\n"),
            None
        );
    }

    #[gpui::test]
    fn terminal_list_keeps_a_known_good_screen_when_replay_is_malformed(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.selected_chat = Some("chat".into());
            let existing = XdDesktop::terminal_tab_from_snapshot(&serde_json::json!({
                "id": "terminal-one",
                "columns": 40,
                "rows": 8,
                "sequence": 2,
                "replay": [{"data": STANDARD.encode(b"known good")}],
            }))
            .unwrap();
            let mut panel = XdDesktop::new_terminal_panel("chat".into());
            panel.auto_open = false;
            panel.selected = Some("terminal-one".into());
            panel.sessions.push(existing);
            desktop.terminal_panel = Some(panel);

            desktop.apply_terminal_list(
                &serde_json::json!({
                    "terminals": [{
                        "id": "terminal-one",
                        "columns": 40,
                        "rows": 8,
                        "sequence": 99,
                        "replay": [{
                            "checkpoint": STANDARD.encode(b"invalid checkpoint"),
                            "data": "not base64",
                        }],
                    }],
                }),
                cx,
            );

            let session = desktop
                .terminal_panel
                .as_ref()
                .unwrap()
                .sessions
                .first()
                .unwrap();
            assert_eq!(session.sequence, Some(2));
            assert_eq!(session.screen.rendered().text, "known good");
        });
    }

    #[gpui::test]
    fn terminal_list_replays_cold_pending_output_without_duplicates(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.selected_chat = Some("chat".into());
            let mut panel = XdDesktop::new_terminal_panel("chat".into());
            panel.auto_open = false;
            desktop.terminal_panel = Some(panel);

            desktop.handle_event(
                "terminal-output",
                serde_json::json!({
                    "chat": "chat",
                    "terminal": "terminal-one",
                    "sequence": 2,
                    "data": "YWZ0ZXI=",
                }),
                None,
                cx,
            );
            assert_eq!(
                desktop
                    .terminal_panel
                    .as_ref()
                    .map(|panel| panel.pending_events.len()),
                Some(1)
            );

            desktop.apply_terminal_list(
                &serde_json::json!({
                    "terminals": [{
                        "id": "terminal-one",
                        "title": "shell",
                        "columns": 40,
                        "rows": 8,
                        "sequence": 1,
                        "replay": [{"data": "YmVmb3Jl"}],
                    }],
                }),
                cx,
            );
            let session = desktop
                .terminal_panel
                .as_ref()
                .unwrap()
                .sessions
                .first()
                .unwrap();
            assert_eq!(session.sequence, Some(2));
            assert_eq!(session.screen.rendered().text.matches("after").count(), 1);

            desktop.handle_event(
                "terminal-output",
                serde_json::json!({
                    "chat": "chat",
                    "terminal": "terminal-one",
                    "sequence": 2,
                    "data": "YWZ0ZXI=",
                }),
                None,
                cx,
            );
            let session = desktop
                .terminal_panel
                .as_ref()
                .unwrap()
                .sessions
                .first()
                .unwrap();
            assert_eq!(session.screen.rendered().text.matches("after").count(), 1);
        });
    }

    #[gpui::test]
    fn terminal_close_tombstone_prevents_a_stale_list_from_resurrecting_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.selected_chat = Some("chat".into());
            let mut panel = XdDesktop::new_terminal_panel("chat".into());
            panel.auto_open = false;
            desktop.terminal_panel = Some(panel);

            desktop.handle_event(
                "terminal-closed",
                serde_json::json!({
                    "chat": "chat",
                    "terminal": "terminal-one",
                    "sequence": 2,
                }),
                None,
                cx,
            );
            desktop.apply_terminal_list(
                &serde_json::json!({
                    "terminals": [{
                        "id": "terminal-one",
                        "title": "shell",
                        "columns": 40,
                        "rows": 8,
                        "sequence": 1,
                        "replay": [{"data": "YmVmb3Jl"}],
                    }],
                }),
                cx,
            );

            assert!(desktop.terminal_panel.as_ref().unwrap().sessions.is_empty());
        });
    }

    #[gpui::test]
    fn remote_navigation_state_is_cached_and_reconciled(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, _| {
            desktop.active_endpoint = ChatEndpoint::Remote;
            desktop.settings.remote_ssh_command = Some("ssh dev.example -p 22".into());
            desktop.collapsed_folders = HashSet::from(["folder-remote".into()]);

            desktop.persist_collapsed_folders();

            assert_eq!(
                desktop
                    .settings
                    .collapsed_folder_sets
                    .get("remote/dev.example"),
                Some(&vec!["folder-remote".to_owned()])
            );

            desktop.remember_last_chat("chat-remote");
            assert_eq!(desktop.cached_last_chat().as_deref(), Some("chat-remote"));

            desktop.settings.collapsed_folder_sets.insert(
                "remote/dev.example".into(),
                vec!["folder-remote".into(), "folder-deleted-on-server".into()],
            );
            desktop.restore_collapsed_folders();
            desktop.model.folders = vec![Folder {
                id: "folder-remote".into(),
                parent: None,
                name: "Remote".into(),
            }];
            desktop.collapsed_folders.retain(|folder_id| {
                desktop
                    .model
                    .folders
                    .iter()
                    .any(|folder| &folder.id == folder_id)
            });
            desktop.persist_collapsed_folders();
            assert_eq!(
                desktop
                    .settings
                    .collapsed_folder_sets
                    .get("remote/dev.example"),
                Some(&vec!["folder-remote".to_owned()]),
                "an authoritative tree can prune stale cached folder ids"
            );
        });
    }
}

fn install_embedded_fonts(text_system: &gpui::TextSystem) -> Result<(), String> {
    text_system
        .add_fonts(vec![Cow::Borrowed(EMBEDDED_UI_FONT)])
        .map_err(|error| error.to_string())
}

fn main() {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|argument| argument == "--version" || argument == "-v")
    {
        println!("xd {}", desktop_version());
        return;
    }
    Application::new()
        .with_assets(EmbeddedIcons)
        .run(|cx: &mut App| {
            install_embedded_fonts(cx.text_system()).expect("register bundled UI font");
            cx.bind_keys([
                KeyBinding::new("ctrl-c", CopyRenderedSelection, Some("XdDesktop")),
                KeyBinding::new("cmd-c", CopyRenderedSelection, Some("XdDesktop")),
                KeyBinding::new("escape", CloseSearch, Some("XdDesktop")),
                KeyBinding::new("backspace", Backspace, Some("ComposerInput")),
                KeyBinding::new("delete", Delete, Some("ComposerInput")),
                KeyBinding::new("ctrl-backspace", DeleteWord, Some("ComposerInput")),
                KeyBinding::new("alt-backspace", DeleteWord, Some("ComposerInput")),
                KeyBinding::new("ctrl-delete", DeleteWordForward, Some("ComposerInput")),
                KeyBinding::new("alt-delete", DeleteWordForward, Some("ComposerInput")),
                KeyBinding::new("left", Left, Some("ComposerInput")),
                KeyBinding::new("right", Right, Some("ComposerInput")),
                KeyBinding::new("ctrl-left", WordLeft, Some("ComposerInput")),
                KeyBinding::new("alt-left", WordLeft, Some("ComposerInput")),
                KeyBinding::new("ctrl-right", WordRight, Some("ComposerInput")),
                KeyBinding::new("alt-right", WordRight, Some("ComposerInput")),
                KeyBinding::new("shift-left", SelectLeft, Some("ComposerInput")),
                KeyBinding::new("shift-right", SelectRight, Some("ComposerInput")),
                KeyBinding::new("ctrl-shift-left", SelectWordLeft, Some("ComposerInput")),
                KeyBinding::new("alt-shift-left", SelectWordLeft, Some("ComposerInput")),
                KeyBinding::new("ctrl-shift-right", SelectWordRight, Some("ComposerInput")),
                KeyBinding::new("alt-shift-right", SelectWordRight, Some("ComposerInput")),
                KeyBinding::new("home", Home, Some("ComposerInput")),
                KeyBinding::new("end", End, Some("ComposerInput")),
                KeyBinding::new("ctrl-a", SelectAll, Some("ComposerInput")),
                KeyBinding::new("ctrl-c", Copy, Some("ComposerInput")),
                KeyBinding::new("ctrl-x", Cut, Some("ComposerInput")),
                KeyBinding::new("ctrl-v", Paste, Some("ComposerInput")),
                KeyBinding::new("cmd-a", SelectAll, Some("ComposerInput")),
                KeyBinding::new("cmd-c", Copy, Some("ComposerInput")),
                KeyBinding::new("cmd-x", Cut, Some("ComposerInput")),
                KeyBinding::new("cmd-v", Paste, Some("ComposerInput")),
                KeyBinding::new("enter", Submit, Some("ComposerInput")),
                KeyBinding::new(
                    "ctrl-cmd-space",
                    ShowCharacterPalette,
                    Some("ComposerInput"),
                ),
                KeyBinding::new("backspace", EditorBackspace, Some("FileEditor")),
                KeyBinding::new("delete", EditorDelete, Some("FileEditor")),
                KeyBinding::new("ctrl-backspace", EditorDeleteWord, Some("FileEditor")),
                KeyBinding::new("alt-backspace", EditorDeleteWord, Some("FileEditor")),
                KeyBinding::new("ctrl-delete", EditorDeleteWordForward, Some("FileEditor")),
                KeyBinding::new("alt-delete", EditorDeleteWordForward, Some("FileEditor")),
                KeyBinding::new("left", EditorLeft, Some("FileEditor")),
                KeyBinding::new("right", EditorRight, Some("FileEditor")),
                KeyBinding::new("ctrl-left", EditorWordLeft, Some("FileEditor")),
                KeyBinding::new("alt-left", EditorWordLeft, Some("FileEditor")),
                KeyBinding::new("ctrl-right", EditorWordRight, Some("FileEditor")),
                KeyBinding::new("alt-right", EditorWordRight, Some("FileEditor")),
                KeyBinding::new("up", EditorUp, Some("FileEditor")),
                KeyBinding::new("down", EditorDown, Some("FileEditor")),
                KeyBinding::new("shift-left", EditorSelectLeft, Some("FileEditor")),
                KeyBinding::new("shift-right", EditorSelectRight, Some("FileEditor")),
                KeyBinding::new("ctrl-shift-left", EditorSelectWordLeft, Some("FileEditor")),
                KeyBinding::new("alt-shift-left", EditorSelectWordLeft, Some("FileEditor")),
                KeyBinding::new(
                    "ctrl-shift-right",
                    EditorSelectWordRight,
                    Some("FileEditor"),
                ),
                KeyBinding::new("alt-shift-right", EditorSelectWordRight, Some("FileEditor")),
                KeyBinding::new("home", EditorHome, Some("FileEditor")),
                KeyBinding::new("end", EditorEnd, Some("FileEditor")),
                KeyBinding::new("ctrl-a", EditorSelectAll, Some("FileEditor")),
                KeyBinding::new("ctrl-c", EditorCopy, Some("FileEditor")),
                KeyBinding::new("ctrl-x", EditorCut, Some("FileEditor")),
                KeyBinding::new("ctrl-v", EditorPaste, Some("FileEditor")),
                KeyBinding::new("cmd-a", EditorSelectAll, Some("FileEditor")),
                KeyBinding::new("cmd-c", EditorCopy, Some("FileEditor")),
                KeyBinding::new("cmd-x", EditorCut, Some("FileEditor")),
                KeyBinding::new("cmd-v", EditorPaste, Some("FileEditor")),
                KeyBinding::new("enter", EditorNewline, Some("FileEditor")),
                KeyBinding::new("tab", EditorTab, Some("FileEditor")),
                KeyBinding::new("ctrl-s", EditorSave, Some("FileEditor")),
                KeyBinding::new("cmd-s", EditorSave, Some("FileEditor")),
                KeyBinding::new("backspace", EditorBackspace, Some("MessageEditor")),
                KeyBinding::new("delete", EditorDelete, Some("MessageEditor")),
                KeyBinding::new("ctrl-backspace", EditorDeleteWord, Some("MessageEditor")),
                KeyBinding::new("alt-backspace", EditorDeleteWord, Some("MessageEditor")),
                KeyBinding::new(
                    "ctrl-delete",
                    EditorDeleteWordForward,
                    Some("MessageEditor"),
                ),
                KeyBinding::new("alt-delete", EditorDeleteWordForward, Some("MessageEditor")),
                KeyBinding::new("left", EditorLeft, Some("MessageEditor")),
                KeyBinding::new("right", EditorRight, Some("MessageEditor")),
                KeyBinding::new("ctrl-left", EditorWordLeft, Some("MessageEditor")),
                KeyBinding::new("alt-left", EditorWordLeft, Some("MessageEditor")),
                KeyBinding::new("ctrl-right", EditorWordRight, Some("MessageEditor")),
                KeyBinding::new("alt-right", EditorWordRight, Some("MessageEditor")),
                KeyBinding::new("up", EditorUp, Some("MessageEditor")),
                KeyBinding::new("down", EditorDown, Some("MessageEditor")),
                KeyBinding::new("shift-left", EditorSelectLeft, Some("MessageEditor")),
                KeyBinding::new("shift-right", EditorSelectRight, Some("MessageEditor")),
                KeyBinding::new(
                    "ctrl-shift-left",
                    EditorSelectWordLeft,
                    Some("MessageEditor"),
                ),
                KeyBinding::new(
                    "alt-shift-left",
                    EditorSelectWordLeft,
                    Some("MessageEditor"),
                ),
                KeyBinding::new(
                    "ctrl-shift-right",
                    EditorSelectWordRight,
                    Some("MessageEditor"),
                ),
                KeyBinding::new(
                    "alt-shift-right",
                    EditorSelectWordRight,
                    Some("MessageEditor"),
                ),
                KeyBinding::new("home", EditorHome, Some("MessageEditor")),
                KeyBinding::new("end", EditorEnd, Some("MessageEditor")),
                KeyBinding::new("ctrl-a", EditorSelectAll, Some("MessageEditor")),
                KeyBinding::new("ctrl-c", EditorCopy, Some("MessageEditor")),
                KeyBinding::new("ctrl-x", EditorCut, Some("MessageEditor")),
                KeyBinding::new("ctrl-v", EditorPaste, Some("MessageEditor")),
                KeyBinding::new("cmd-a", EditorSelectAll, Some("MessageEditor")),
                KeyBinding::new("cmd-c", EditorCopy, Some("MessageEditor")),
                KeyBinding::new("cmd-x", EditorCut, Some("MessageEditor")),
                KeyBinding::new("cmd-v", EditorPaste, Some("MessageEditor")),
                KeyBinding::new("enter", EditorSubmit, Some("MessageEditor")),
                KeyBinding::new("shift-enter", EditorNewline, Some("MessageEditor")),
                KeyBinding::new("tab", EditorTab, Some("MessageEditor")),
                KeyBinding::new("backspace", Backspace, Some("TerminalInput")),
                KeyBinding::new("delete", Delete, Some("TerminalInput")),
                KeyBinding::new("ctrl-backspace", DeleteWord, Some("TerminalInput")),
                KeyBinding::new("alt-backspace", DeleteWord, Some("TerminalInput")),
                KeyBinding::new("ctrl-delete", DeleteWordForward, Some("TerminalInput")),
                KeyBinding::new("alt-delete", DeleteWordForward, Some("TerminalInput")),
                KeyBinding::new("left", Left, Some("TerminalInput")),
                KeyBinding::new("right", Right, Some("TerminalInput")),
                KeyBinding::new("ctrl-left", WordLeft, Some("TerminalInput")),
                KeyBinding::new("alt-left", WordLeft, Some("TerminalInput")),
                KeyBinding::new("ctrl-right", WordRight, Some("TerminalInput")),
                KeyBinding::new("alt-right", WordRight, Some("TerminalInput")),
                KeyBinding::new("up", Up, Some("TerminalInput")),
                KeyBinding::new("down", Down, Some("TerminalInput")),
                KeyBinding::new("pageup", TerminalPageUp, Some("TerminalInput")),
                KeyBinding::new("pagedown", TerminalPageDown, Some("TerminalInput")),
                KeyBinding::new("home", Home, Some("TerminalInput")),
                KeyBinding::new("end", End, Some("TerminalInput")),
                KeyBinding::new("shift-enter", TerminalControlJ, Some("TerminalInput")),
                KeyBinding::new("enter", Submit, Some("TerminalInput")),
                KeyBinding::new("tab", Tab, Some("TerminalInput")),
                KeyBinding::new("shift-tab", TerminalShiftTab, Some("TerminalInput")),
                KeyBinding::new("escape", Escape, Some("TerminalInput")),
                KeyBinding::new("ctrl-c", Interrupt, Some("TerminalInput")),
                KeyBinding::new("ctrl-a", TerminalControlA, Some("TerminalInput")),
                KeyBinding::new("ctrl-b", TerminalControlB, Some("TerminalInput")),
                KeyBinding::new("ctrl-d", TerminalEndOfFile, Some("TerminalInput")),
                KeyBinding::new("ctrl-e", TerminalControlE, Some("TerminalInput")),
                KeyBinding::new("ctrl-f", TerminalControlF, Some("TerminalInput")),
                KeyBinding::new("ctrl-g", TerminalControlG, Some("TerminalInput")),
                KeyBinding::new("ctrl-h", TerminalControlH, Some("TerminalInput")),
                KeyBinding::new("ctrl-i", TerminalControlI, Some("TerminalInput")),
                KeyBinding::new("ctrl-j", TerminalControlJ, Some("TerminalInput")),
                KeyBinding::new("ctrl-k", TerminalControlK, Some("TerminalInput")),
                KeyBinding::new("ctrl-l", TerminalClearScreen, Some("TerminalInput")),
                KeyBinding::new("ctrl-m", TerminalControlM, Some("TerminalInput")),
                KeyBinding::new("ctrl-n", TerminalControlN, Some("TerminalInput")),
                KeyBinding::new("ctrl-o", TerminalControlO, Some("TerminalInput")),
                KeyBinding::new("ctrl-p", TerminalControlP, Some("TerminalInput")),
                KeyBinding::new("ctrl-q", TerminalControlQ, Some("TerminalInput")),
                KeyBinding::new("ctrl-r", TerminalReverseSearch, Some("TerminalInput")),
                KeyBinding::new("ctrl-s", TerminalControlS, Some("TerminalInput")),
                KeyBinding::new("ctrl-t", TerminalControlT, Some("TerminalInput")),
                KeyBinding::new("ctrl-u", TerminalControlU, Some("TerminalInput")),
                KeyBinding::new("ctrl-w", TerminalControlW, Some("TerminalInput")),
                KeyBinding::new("ctrl-x", TerminalControlX, Some("TerminalInput")),
                KeyBinding::new("ctrl-y", TerminalControlY, Some("TerminalInput")),
                KeyBinding::new("ctrl-z", TerminalSuspend, Some("TerminalInput")),
                KeyBinding::new("ctrl-shift-c", Copy, Some("TerminalInput")),
                KeyBinding::new("ctrl-shift-v", Paste, Some("TerminalInput")),
                KeyBinding::new("cmd-v", Paste, Some("TerminalInput")),
            ]);
            let settings = AppSettings::load();
            let bounds = Bounds::centered(
                None,
                size(
                    px(f32::from(settings.window_width.max(760))),
                    px(f32::from(settings.window_height.max(560))),
                ),
                cx,
            );
            let window_bounds = if settings.window_maximized {
                WindowBounds::Maximized(bounds)
            } else {
                WindowBounds::Windowed(bounds)
            };
            cx.open_window(
                WindowOptions {
                    focus: true,
                    window_bounds: Some(window_bounds),
                    is_resizable: true,
                    window_min_size: Some(size(px(760.0), px(560.0))),
                    window_background: WindowBackgroundAppearance::Opaque,
                    titlebar: Some(TitlebarOptions {
                        title: Some("xd".into()),
                        appears_transparent: true,
                        traffic_light_position: cfg!(target_os = "macos")
                            .then(|| point(px(12.0), px(10.0))),
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    app_id: Some(xd_desktop::channel::app_id().into()),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| XdDesktop::new(window, cx)),
            )
            .expect("open xd GPUI window");
            cx.activate(true);
        });
}

fn desktop_version() -> String {
    option_env!("XD_COMMIT")
        .filter(|commit| !commit.is_empty() && *commit != "development")
        .map(|commit| {
            format!(
                "{} ({})",
                env!("CARGO_PKG_VERSION"),
                &commit[..7.min(commit.len())]
            )
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}
