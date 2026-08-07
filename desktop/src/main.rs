use std::{
    collections::{HashMap, HashSet, VecDeque, hash_map::DefaultHasher},
    env, fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "linux")]
use std::{
    process::{Command, Stdio},
    thread,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{
    App, Application, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, Decorations, Entity,
    Focusable, FontStyle, FontWeight, HighlightStyle, Image, InteractiveText, KeyBinding,
    ListAlignment, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    PathPromptOptions, Point, Render, ResizeEdge, SharedString, StyledText, TextRun, Timer, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, canvas, div, img,
    list, prelude::*, px, relative, rgb, rgba, size,
};
use serde::Deserialize;
use serde_json::Value;
use xd_desktop::{
    activity::{ActivityCard, ActivityKind},
    context_usage::{self, Severity as ContextSeverity},
    daemon::{DaemonHandle, DaemonUpdate, MessageCursor, RequestKind, StartedDaemon},
    markdown::{self, Block, CodeKind, InlineKind, InlineText},
    model::{AgentBackend, AppModel, Attachment, Folder, Message, MessagePageDirection},
    remote::{self, CredentialsFile, RemoteBridge, RemoteCredentials, RemoteError, RemoteSession},
};

mod editor;
mod input;
mod presence;
mod settings;
mod source_build;
mod speech;
mod terminal;
mod voice_input;

use editor::{
    Backspace as EditorBackspace, Copy as EditorCopy, Cut as EditorCut, Delete as EditorDelete,
    Down as EditorDown, EditorEvent, End as EditorEnd, FileEditor, Home as EditorHome,
    Left as EditorLeft, Newline as EditorNewline, Paste as EditorPaste, Right as EditorRight,
    Save as EditorSave, SelectAll as EditorSelectAll, SelectLeft as EditorSelectLeft,
    SelectRight as EditorSelectRight, Submit as EditorSubmit, Tab as EditorTab, Up as EditorUp,
};
use input::{
    Backspace, ComposerEvent, ComposerInput, Copy, Cut, Delete, Down, End, Escape, Home, Interrupt,
    Left, Paste, Right, SelectAll, SelectLeft, SelectRight, ShowCharacterPalette, Submit, Tab, Up,
};
use presence::DiscordPresence;
use settings::{AccentPreset, AppSettings, GitWriter};
use source_build::{SourceBuildEvent, SourceBuildRun, SourceTarget};
use speech::SpeechOutput;
use terminal::TerminalScreen;
use voice_input::{CaptureEvent, VoiceRecorder};

// Keep the GPUI shell visually continuous across every surface.
// These are the same near-black surfaces and quiet separators used by its GTK
// stylesheet; the configurable accent remains reserved for active controls.
const BG: u32 = 0x0a0a0c;
const SIDEBAR: u32 = 0x060607;
const SURFACE: u32 = 0x101013;
const SURFACE_HIGH: u32 = 0x1a1a1e;
const BORDER: u32 = 0x2a2a2d;
const TEXT: u32 = 0xf2f2f4;
const MUTED: u32 = 0xa8a8ad;
const MAX_ATTACHMENTS: usize = 4;
const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
const MAX_CACHED_MESSAGE_IMAGES: usize = 8;
const MAX_SHORTCUTS: usize = 24;
const MAX_SHORTCUT_BYTES: usize = 4_096;
const MAX_SOURCE_BUILD_OUTPUT_BYTES: usize = 8 * 1024;
static NEXT_VOICE_REQUEST: AtomicU64 = AtomicU64::new(1);

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

fn trash_icon(color: u32) -> gpui::AnyElement {
    div()
        .relative()
        .size(px(16.0))
        .child(
            div()
                .absolute()
                .left(px(2.0))
                .top(px(4.0))
                .w(px(12.0))
                .h(px(2.0))
                .rounded_full()
                .bg(rgb(color)),
        )
        .child(
            div()
                .absolute()
                .left(px(5.0))
                .top(px(2.0))
                .w(px(6.0))
                .h(px(2.0))
                .rounded_full()
                .bg(rgb(color)),
        )
        .child(
            div()
                .absolute()
                .left(px(4.0))
                .top(px(7.0))
                .w(px(8.0))
                .h(px(7.0))
                .border_1()
                .border_t_0()
                .border_color(rgb(color))
                .rounded_b_sm(),
        )
        .into_any_element()
}

gpui::actions!(
    xd,
    [
        OpenSearch,
        CloseSearch,
        SelectModel1,
        SelectModel2,
        SelectModel3,
        SelectModel4,
        SelectModel5,
        SelectModel6,
        SelectModel7,
        SelectModel8,
        SelectModel9,
        DirectoryPrevious,
        DirectoryNext,
        DirectoryOpen,
        DirectoryParent,
        DirectoryChoose
    ]
);

#[derive(Clone, Debug, Deserialize)]
struct SearchHit {
    chat: String,
    title: String,
    role: String,
    snippet: String,
}

#[derive(Clone, Default)]
struct SearchPanel {
    query: String,
    results: Vec<SearchHit>,
    loading: bool,
}

#[derive(Clone)]
struct DirectoryBrowser {
    target: WorkspacePathTarget,
    path: Option<String>,
    entries: Vec<String>,
    selected: Option<usize>,
    loading: bool,
    error: Option<String>,
    generation: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct AuthProvider {
    provider: String,
    display_name: String,
    state: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    login_url: Option<String>,
    #[serde(default)]
    device_code: Option<String>,
    #[serde(default)]
    needs_input: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct CliVersion {
    provider: String,
    display_name: String,
    state: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SelfUpdateStatus {
    #[serde(default)]
    version: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    supported: bool,
    #[serde(default)]
    available: bool,
    #[serde(default)]
    latest: Option<String>,
    #[serde(default)]
    error: Option<String>,
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

    fn output_text(&self) -> String {
        self.output.iter().cloned().collect()
    }
}

#[derive(Default)]
enum VoiceState {
    #[default]
    Idle,
    Checking,
    NeedsModel,
    Downloading(i64),
    Recording,
    Transcribing,
    Failed(String),
}

#[derive(Default)]
struct VoiceInput {
    state: VoiceState,
    chat_id: String,
    token: String,
    base_text: String,
    partial: String,
    recorder: Option<VoiceRecorder>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffLineKind {
    Header,
    Hunk,
    Added,
    Removed,
    Context,
}

#[derive(Clone, Debug)]
struct DiffLine {
    kind: DiffLineKind,
    text: String,
}

#[derive(Clone, Debug)]
struct DiffFile {
    path: String,
    status: Option<String>,
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
    screen: TerminalScreen,
}

struct TerminalPanel {
    chat_id: String,
    sessions: Vec<TerminalTab>,
    selected: Option<String>,
    viewport: Option<(usize, usize)>,
    opening: bool,
    loading: bool,
    error: Option<String>,
}

impl TerminalPanel {
    fn selected(&self) -> Option<&TerminalTab> {
        let selected = self.selected.as_deref()?;
        self.sessions.iter().find(|session| session.id == selected)
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

fn terminal_geometry(width: f32, height: f32, cell_width: f32, line_height: f32) -> (usize, usize) {
    let content_width = (width - 24.0).max(cell_width);
    let content_height = (height - 24.0).max(line_height);
    let columns = (content_width / cell_width.max(1.0)).floor() as usize;
    let rows = (content_height / line_height.max(1.0)).floor() as usize;
    (columns.clamp(20, 500), rows.clamp(4, 200))
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GitStatus {
    branch: String,
    #[serde(default)]
    base: String,
    upstream: String,
    ahead: u64,
    behind: u64,
    staged: u64,
    unstaged: u64,
    untracked: u64,
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
    text: String,
    attachments: Vec<Attachment>,
    restore: bool,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneResizeKind {
    Sidebar,
    Diff,
    Terminal,
}

const PANE_TERMINAL: u8 = 1;
const PANE_DIFF: u8 = 4;

fn pane_state_key(endpoint: ChatEndpoint, remote: Option<(&str, u16)>, chat_id: &str) -> String {
    match endpoint {
        ChatEndpoint::Local => format!("local/{chat_id}"),
        ChatEndpoint::Remote => {
            let (host, port) = remote.unwrap_or(("remote", 0));
            format!("remote/{host}:{port}/{chat_id}")
        }
    }
}

fn pane_state_mask(diff_open: bool, terminal_open: bool) -> u8 {
    (if terminal_open { PANE_TERMINAL } else { 0 }) | (if diff_open { PANE_DIFF } else { 0 })
}

#[derive(Clone, Copy, Debug)]
struct PaneResize {
    kind: PaneResizeKind,
    origin: Point<gpui::Pixels>,
    initial_size: f32,
}

fn resized_pane_size(kind: PaneResizeKind, initial_size: f32, delta: Point<f32>) -> u16 {
    let size = match kind {
        PaneResizeKind::Sidebar => initial_size + delta.x,
        PaneResizeKind::Diff => initial_size - delta.x,
        PaneResizeKind::Terminal => initial_size - delta.y,
    };
    let (minimum, maximum) = match kind {
        PaneResizeKind::Sidebar => (220.0, 520.0),
        PaneResizeKind::Diff => (320.0, 760.0),
        PaneResizeKind::Terminal => (180.0, 640.0),
    };
    size.round().clamp(minimum, maximum) as u16
}

struct PendingSpeech {
    chat_id: String,
    previous_assistant_id: Option<i64>,
}

#[derive(Clone, Default)]
struct TranscriptSnapshot {
    messages: Arc<Vec<Message>>,
    live_text: Option<Arc<Message>>,
    live_activity: Arc<Vec<Message>>,
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

    fn sync_live_activity(&mut self, model: &AppModel) {
        self.live_activity = Arc::new(model.live_activity.clone());
    }

    fn get(&self, index: usize) -> Option<&Message> {
        if let Some(message) = self.messages.get(index) {
            return Some(message);
        }
        let mut live_index = index.saturating_sub(self.messages.len());
        if let Some(live_text) = self.live_text.as_deref() {
            if live_index == 0 {
                return Some(live_text);
            }
            live_index = live_index.saturating_sub(1);
        }
        self.live_activity.get(live_index)
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
struct ShortcutRow {
    id: u64,
    prompt: String,
    input: Entity<ComposerInput>,
}

#[derive(Clone)]
struct ShortcutPanel {
    folder_id: Option<String>,
    folder_name: Option<String>,
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
struct SidebarDrag {
    target: SidebarTarget,
    label: SharedString,
    position: Point<gpui::Pixels>,
}

impl SidebarDrag {
    fn new(target: SidebarTarget, label: impl Into<SharedString>) -> Self {
        Self {
            target,
            label: label.into(),
            position: Point::default(),
        }
    }

    fn position(mut self, position: Point<gpui::Pixels>) -> Self {
        self.position = position;
        self
    }
}

impl Render for SidebarDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .pl(self.position.x + px(10.0))
            .pt(self.position.y + px(10.0))
            .child(
                div()
                    .max_w(px(240.0))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgba(0x101013ee))
                    .shadow_md()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child(self.label.clone()),
            )
    }
}

#[derive(Clone, Debug)]
struct SidebarContextMenu {
    target: Option<SidebarTarget>,
    position: Point<gpui::Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerMenu {
    Model,
    Effort,
    Access,
    Workspace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsMenu {
    GitWriter,
    GitWriterModel,
}

#[derive(Clone, Debug)]
enum ComposerChoice {
    Model { backend: String, model: String },
    Effort(String),
    Access(String),
    Workspace(String),
}

#[derive(Clone)]
enum WorkspacePathTarget {
    CreateRepository,
    DefaultsWorkdir { folder_id: String },
    DefaultsRepository { folder_id: String },
    CreateChat { folder_id: String, title: String },
}

#[derive(Clone)]
struct SidebarEdit {
    target: SidebarTarget,
    original: String,
    text: String,
    submitting: bool,
}

#[derive(Clone)]
struct WorkspaceDefaults {
    folder_id: String,
    backend: Option<String>,
    model: Option<String>,
    effective_backend: String,
    workdir: String,
    repo: String,
    loading: bool,
    submitting: bool,
}

#[derive(Clone)]
struct SecretsPanel {
    folder_id: Option<String>,
    folder_name: Option<String>,
    names: Vec<String>,
    name: String,
    value: String,
    loading: bool,
    submitting: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct PairedDevice {
    id: String,
    name: String,
    created_at: i64,
    last_seen: i64,
    #[serde(default)]
    connected: bool,
}

#[derive(Clone, Default)]
struct DevicesPanel {
    devices: Vec<PairedDevice>,
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

impl ChatEndpoint {
    fn other(self) -> Self {
        match self {
            Self::Local => Self::Remote,
            Self::Remote => Self::Local,
        }
    }
}

#[derive(Clone, Default)]
struct RemotePanel {
    host: String,
    port: String,
    code: String,
    name: String,
    submitting: bool,
    error: Option<String>,
}

#[derive(Clone)]
enum MessageImageState {
    Loading,
    Ready(Arc<Image>),
    Unavailable,
}

#[derive(Default)]
struct MessageImageCache {
    entries: HashMap<String, MessageImageState>,
    order: VecDeque<String>,
}

#[derive(Clone)]
struct MessageImageViewer {
    image: Arc<Image>,
    number: usize,
}

impl MessageImageCache {
    fn state(&mut self, path: &str) -> Option<MessageImageState> {
        let state = self.entries.get(path)?.clone();
        self.touch(path);
        Some(state)
    }

    fn begin(&mut self, path: &str) -> bool {
        if self.entries.contains_key(path) {
            self.touch(path);
            return false;
        }
        while self.entries.len() >= MAX_CACHED_MESSAGE_IMAGES {
            let Some(index) = self.order.iter().position(|candidate| {
                !matches!(
                    self.entries.get(candidate),
                    Some(MessageImageState::Loading)
                )
            }) else {
                return false;
            };
            if let Some(evicted) = self.order.remove(index) {
                self.entries.remove(&evicted);
            }
        }
        self.entries
            .insert(path.to_owned(), MessageImageState::Loading);
        self.order.push_back(path.to_owned());
        true
    }

    fn finish(&mut self, path: &str, image: Option<Arc<Image>>) {
        if !matches!(self.entries.get(path), Some(MessageImageState::Loading)) {
            return;
        }
        self.entries.insert(
            path.to_owned(),
            image
                .map(MessageImageState::Ready)
                .unwrap_or(MessageImageState::Unavailable),
        );
        self.touch(path);
    }

    fn clear_loading(&mut self) {
        let loading = self
            .entries
            .iter()
            .filter_map(|(path, state)| {
                matches!(state, MessageImageState::Loading).then_some(path.clone())
            })
            .collect::<Vec<_>>();
        for path in loading {
            self.entries.remove(&path);
            self.order.retain(|candidate| candidate != &path);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    fn touch(&mut self, path: &str) {
        self.order.retain(|candidate| candidate != path);
        self.order.push_back(path.to_owned());
    }
}

struct XdDesktop {
    model: AppModel,
    inactive_model: AppModel,
    active_endpoint: ChatEndpoint,
    settings: AppSettings,
    settings_open: bool,
    settings_menu: Option<SettingsMenu>,
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
    voice_input: VoiceInput,
    voice_applying_text: bool,
    speech_output: SpeechOutput,
    presence: DiscordPresence,
    pending_speech: Option<PendingSpeech>,
    daemon: Option<DaemonHandle>,
    _started_daemon: Option<StartedDaemon>,
    connection_generation: u64,
    reconnect_attempt: u32,
    connecting: bool,
    connection_in_flight: bool,
    remote_credentials_file: Option<CredentialsFile>,
    remote_credentials: Option<RemoteCredentials>,
    remote_daemon: Option<DaemonHandle>,
    remote_bridge: Option<RemoteBridge>,
    remote_state: RemoteState,
    remote_error: Option<String>,
    remote_generation: u64,
    remote_reconnect_attempt: u32,
    message_images: Arc<Mutex<MessageImageCache>>,
    message_image_viewer: Option<MessageImageViewer>,
    transcript: ListState,
    transcript_snapshot: TranscriptSnapshot,
    transcript_loading: bool,
    transcript_page_loading: bool,
    transcript_refresh_pending: bool,
    transcript_has_older: bool,
    transcript_has_newer: bool,
    transcript_scroll_handler_attached: bool,
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
    search_input: Entity<ComposerInput>,
    model_search_input: Entity<ComposerInput>,
    git_commit_input: Entity<ComposerInput>,
    repo_file_filter_input: Entity<ComposerInput>,
    file_editor: Entity<FileEditor>,
    terminal_input: Entity<ComposerInput>,
    auth_input: Entity<ComposerInput>,
    secret_name_input: Entity<ComposerInput>,
    secret_value_input: Entity<ComposerInput>,
    device_name_input: Entity<ComposerInput>,
    remote_host_input: Entity<ComposerInput>,
    remote_port_input: Entity<ComposerInput>,
    remote_code_input: Entity<ComposerInput>,
    remote_name_input: Entity<ComposerInput>,
    question_input: Entity<ComposerInput>,
    source_build_input: Entity<ComposerInput>,
    composer: String,
    queue_edit: Option<QueueEdit>,
    composer_menu: Option<ComposerMenu>,
    model_search: String,
    model_filter: Option<String>,
    sidebar_edit: Option<SidebarEdit>,
    sidebar_context_menu: Option<SidebarContextMenu>,
    pending_sidebar_delete: Option<SidebarTarget>,
    sidebar_delete_submitting: bool,
    sidebar_move: Option<SidebarTarget>,
    sidebar_move_submitting: bool,
    sidebar_move_destination: Option<Option<String>>,
    collapsed_folders: HashSet<String>,
    inactive_collapsed_folders: HashSet<String>,
    creating_workspace: bool,
    workspace_create_name: String,
    workspace_create_repo: String,
    workspace_create_clone: String,
    workspace_create_submitting: bool,
    workspace_clone_status: Option<String>,
    workspace_path_generation: u64,
    directory_browser: Option<DirectoryBrowser>,
    pending_clone_requests: HashMap<ChatEndpoint, String>,
    pending_clone_chats: HashSet<(ChatEndpoint, String)>,
    workspace_clone_outcomes: HashMap<(ChatEndpoint, String), Option<String>>,
    creating_chat_folder: Option<String>,
    chat_create_title: String,
    chat_create_submitting: bool,
    workspace_context_folder: Option<String>,
    workspace_context_text: String,
    workspace_context_loading: bool,
    workspace_context_submitting: bool,
    workspace_defaults: Option<WorkspaceDefaults>,
    search: Option<SearchPanel>,
    search_generation: u64,
    diff_panel: Option<DiffPanel>,
    terminal_panel: Option<TerminalPanel>,
    terminal_cursor_visible: bool,
    diff_generation: u64,
    collapsed_diff_files: HashSet<String>,
    git_commit_message: String,
    repo_file_filter: String,
    draft_generation: u64,
    draft_dirty: bool,
    attachments_dirty: bool,
    attachment_generation: u64,
    sending: bool,
    pending_send: Option<PendingSend>,
    open_question: Option<OpenQuestion>,
    question_answer: String,
    source_build_open: bool,
    source_build_panel: SourceBuildPanel,
    source_build_run: Option<SourceBuildRun>,
    source_build_generation: u64,
    expanded_activity: Arc<HashSet<String>>,
    workflow_statuses: Arc<HashMap<String, Value>>,
    workflow_pending: Arc<HashSet<String>>,
    workflow_ticking: HashSet<String>,
    live_markdown_generation: u64,
    live_markdown_scheduled: Option<u64>,
    pane_resize: Option<PaneResize>,
    window_settings_generation: u64,
    next_shortcut_row_id: u64,
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
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let workspace_create_input = cx.new(|cx| ComposerInput::new(cx, "Workspace name…"));
        cx.subscribe(&workspace_create_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.workspace_create_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_workspace_create(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let workspace_repo_input =
            cx.new(|cx| ComposerInput::new(cx, "Existing repository path (optional)…"));
        cx.subscribe(&workspace_repo_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.workspace_repo_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_workspace_create(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let workspace_clone_input =
            cx.new(|cx| ComposerInput::new(cx, "Git clone URL (optional)…"));
        cx.subscribe(&workspace_clone_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.workspace_clone_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_workspace_create(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let chat_create_input = cx.new(|cx| ComposerInput::new(cx, "Chat title…"));
        cx.subscribe(&chat_create_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.chat_create_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_chat_create(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let workspace_context_input =
            cx.new(|cx| ComposerInput::new(cx, "Instructions inherited by this workspace…"));
        cx.subscribe(&workspace_context_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.workspace_context_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_workspace_context(cx),
            ComposerEvent::Bytes(_) => {}
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
        let search_input = cx.new(|cx| ComposerInput::new(cx, "Search conversations…"));
        cx.subscribe(&search_input, |this, _, event, cx| {
            if let ComposerEvent::Changed(text) = event {
                this.search_changed(text.clone(), cx);
            }
        })
        .detach();
        let model_search_input = cx.new(|cx| ComposerInput::new(cx, "Search models…"));
        cx.subscribe(&model_search_input, |this, _, event, cx| {
            if let ComposerEvent::Changed(text) = event {
                this.model_search = text.clone();
                cx.notify();
            }
        })
        .detach();
        let git_commit_input = cx.new(|cx| ComposerInput::new(cx, "Commit message…"));
        cx.subscribe(&git_commit_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.git_commit_changed(text.clone(), cx),
            ComposerEvent::Submit => this.commit_changes(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let repo_file_filter_input = cx.new(|cx| ComposerInput::new(cx, "Filter files…"));
        cx.subscribe(&repo_file_filter_input, |this, _, event, cx| {
            if let ComposerEvent::Changed(text) = event {
                this.repo_file_filter = text.clone();
                cx.notify();
            }
        })
        .detach();
        let file_editor = cx.new(FileEditor::new);
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
        cx.subscribe(&terminal_input, |this, _, event, cx| {
            if let ComposerEvent::Bytes(bytes) = event {
                this.send_terminal_input(bytes, cx);
            }
        })
        .detach();
        let auth_input = cx.new(|cx| ComposerInput::new(cx, "Paste authorization code…"));
        cx.subscribe(&auth_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                this.auth_input_text = text.clone();
                cx.notify();
            }
            ComposerEvent::Submit => this.submit_auth_input(cx),
            ComposerEvent::Bytes(_) => {}
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
            ComposerEvent::Bytes(_) => {}
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
            ComposerEvent::Bytes(_) => {}
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
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let remote_host_input = cx.new(|cx| ComposerInput::new(cx, "Machine address…"));
        cx.subscribe(&remote_host_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                if let Some(panel) = &mut this.remote_panel {
                    panel.host = text.clone();
                    panel.error = None;
                }
                cx.notify();
            }
            ComposerEvent::Submit => this.pair_remote_machine(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let remote_port_input = cx.new(|cx| ComposerInput::new(cx, "Port…"));
        cx.subscribe(&remote_port_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                if let Some(panel) = &mut this.remote_panel {
                    panel.port = text.clone();
                    panel.error = None;
                }
                cx.notify();
            }
            ComposerEvent::Submit => this.pair_remote_machine(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let remote_code_input = cx.new(|cx| ComposerInput::new(cx, "Pairing code…"));
        cx.subscribe(&remote_code_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                if let Some(panel) = &mut this.remote_panel {
                    panel.code = text.clone();
                    panel.error = None;
                }
                cx.notify();
            }
            ComposerEvent::Submit => this.pair_remote_machine(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let remote_name_input = cx.new(|cx| ComposerInput::new(cx, "This device name…"));
        cx.subscribe(&remote_name_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                if let Some(panel) = &mut this.remote_panel {
                    panel.name = text.clone();
                    panel.error = None;
                }
                cx.notify();
            }
            ComposerEvent::Submit => this.pair_remote_machine(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let question_input = cx.new(|cx| ComposerInput::new(cx, "Type your answer…"));
        cx.subscribe(&question_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                this.question_answer = text.clone();
                cx.notify();
            }
            ComposerEvent::Submit => this.send_question_input(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let settings = AppSettings::load();
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
            ComposerEvent::Bytes(_) => {}
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
        let collapsed_folders = settings.collapsed_folders.iter().cloned().collect();
        let (remote_credentials_file, remote_credentials, remote_error) =
            match CredentialsFile::default_path() {
                Ok(path) => {
                    let file = CredentialsFile::new(path);
                    match file.load() {
                        Ok(credentials) => (Some(file), credentials, None),
                        Err(error) => (Some(file), None, Some(error.to_string())),
                    }
                }
                Err(error) => (None, None, Some(error.to_string())),
            };
        let mut desktop = Self {
            model: AppModel {
                draft_revision: -1,
                ..Default::default()
            },
            inactive_model: AppModel {
                draft_revision: -1,
                ..Default::default()
            },
            active_endpoint: ChatEndpoint::Local,
            settings,
            settings_open: false,
            settings_menu: None,
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
            voice_input: VoiceInput::default(),
            voice_applying_text: false,
            speech_output: SpeechOutput::default(),
            presence: DiscordPresence::default(),
            pending_speech: None,
            daemon: None,
            _started_daemon: None,
            connection_generation: 0,
            reconnect_attempt: 0,
            connecting: false,
            connection_in_flight: false,
            remote_credentials_file,
            remote_credentials,
            remote_daemon: None,
            remote_bridge: None,
            remote_state: RemoteState::Unconfigured,
            remote_error,
            remote_generation: 0,
            remote_reconnect_attempt: 0,
            message_images: Arc::new(Mutex::new(MessageImageCache::default())),
            message_image_viewer: None,
            transcript: ListState::new(0, ListAlignment::Bottom, px(700.0)),
            transcript_snapshot: TranscriptSnapshot::default(),
            transcript_loading: false,
            transcript_page_loading: false,
            transcript_refresh_pending: false,
            transcript_has_older: false,
            transcript_has_newer: false,
            transcript_scroll_handler_attached: false,
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
            search_input,
            model_search_input,
            git_commit_input,
            repo_file_filter_input,
            file_editor,
            terminal_input,
            auth_input,
            secret_name_input,
            secret_value_input,
            device_name_input,
            remote_host_input,
            remote_port_input,
            remote_code_input,
            remote_name_input,
            question_input,
            source_build_input,
            composer: String::new(),
            queue_edit: None,
            composer_menu: None,
            model_search: String::new(),
            model_filter: None,
            sidebar_edit: None,
            sidebar_context_menu: None,
            pending_sidebar_delete: None,
            sidebar_delete_submitting: false,
            sidebar_move: None,
            sidebar_move_submitting: false,
            sidebar_move_destination: None,
            collapsed_folders,
            inactive_collapsed_folders: HashSet::new(),
            creating_workspace: false,
            workspace_create_name: String::new(),
            workspace_create_repo: String::new(),
            workspace_create_clone: String::new(),
            workspace_create_submitting: false,
            workspace_clone_status: None,
            workspace_path_generation: 0,
            directory_browser: None,
            pending_clone_requests: HashMap::new(),
            pending_clone_chats: HashSet::new(),
            workspace_clone_outcomes: HashMap::new(),
            creating_chat_folder: None,
            chat_create_title: String::new(),
            chat_create_submitting: false,
            workspace_context_folder: None,
            workspace_context_text: String::new(),
            workspace_context_loading: false,
            workspace_context_submitting: false,
            workspace_defaults: None,
            search: None,
            search_generation: 0,
            diff_panel: None,
            terminal_panel: None,
            terminal_cursor_visible: true,
            diff_generation: 0,
            collapsed_diff_files: HashSet::new(),
            git_commit_message: String::new(),
            repo_file_filter: String::new(),
            draft_generation: 0,
            draft_dirty: false,
            attachments_dirty: false,
            attachment_generation: 0,
            sending: false,
            pending_send: None,
            open_question: None,
            question_answer: String::new(),
            source_build_open: false,
            source_build_panel,
            source_build_run: None,
            source_build_generation: 0,
            expanded_activity: Arc::new(HashSet::new()),
            workflow_statuses: Arc::new(HashMap::new()),
            workflow_pending: Arc::new(HashSet::new()),
            workflow_ticking: HashSet::new(),
            live_markdown_generation: 0,
            live_markdown_scheduled: None,
            pane_resize: None,
            window_settings_generation: 0,
            next_shortcut_row_id: 0,
        };
        cx.observe_window_bounds(window, |this, window, cx| {
            this.window_bounds_changed(window, cx);
        })
        .detach();
        desktop.schedule_connect(Duration::ZERO, cx);
        if desktop.remote_credentials.is_some() {
            desktop.schedule_remote_connect(Duration::ZERO, cx);
        }
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

    fn active_daemon(&self) -> Option<&DaemonHandle> {
        self.endpoint_daemon(self.active_endpoint)
    }

    fn endpoint_daemon(&self, endpoint: ChatEndpoint) -> Option<&DaemonHandle> {
        match endpoint {
            ChatEndpoint::Local => self.daemon.as_ref(),
            ChatEndpoint::Remote => self.remote_daemon.as_ref(),
        }
    }

    fn secrets_daemon(&self, folder_id: Option<&str>) -> Option<&DaemonHandle> {
        if folder_id.is_some() {
            self.active_daemon()
        } else {
            self.daemon.as_ref()
        }
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
        if let Some(chat_id) = body.get("chat").and_then(Value::as_str)
            && let Some(chat) = model.chats.iter_mut().find(|chat| chat.id == chat_id)
        {
            if name == "turn-started" {
                chat.working = true;
            } else if name == "turn-finished" {
                chat.working = false;
            }
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
                | RequestKind::Search { .. }
                | RequestKind::ListDirectory { .. }
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
                | RequestKind::VoiceModel { .. }
                | RequestKind::VoiceMutation { .. }
                | RequestKind::DiffRead { .. }
                | RequestKind::GitStatus { .. }
                | RequestKind::GitState { .. }
                | RequestKind::GitDraft { .. }
                | RequestKind::GitPullRequestStatus { .. }
                | RequestKind::GitPullRequestCreate { .. }
                | RequestKind::FileBrowseList { .. }
                | RequestKind::FileBrowseRead { .. }
                | RequestKind::FileBrowseWrite { .. }
                | RequestKind::GitCommit { .. }
                | RequestKind::GitPush { .. }
                | RequestKind::TerminalOpen { .. }
                | RequestKind::TerminalList { .. }
                | RequestKind::TerminalInput { .. }
                | RequestKind::TerminalResize { .. }
                | RequestKind::TerminalKill { .. }
                | RequestKind::AgentSecrets { folder_id: Some(_) }
                | RequestKind::SetAgentSecrets { folder_id: Some(_) }
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
                | "voice"
                | "terminal-opened"
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
            RequestKind::DaemonUpdate { .. }
                | RequestKind::AgentSecrets { folder_id: None }
                | RequestKind::SetAgentSecrets { folder_id: None }
                | RequestKind::Devices
                | RequestKind::PeerPairing
                | RequestKind::PairRemote
                | RequestKind::RenameDevice { .. }
                | RequestKind::RevokeDevice { .. }
        )
    }

    fn local_admin_event(name: &str) -> bool {
        name == "daemon-update"
    }

    fn select_endpoint_chat(
        &mut self,
        endpoint: ChatEndpoint,
        chat_id: String,
        cx: &mut Context<Self>,
    ) {
        if endpoint == ChatEndpoint::Remote && self.remote_state != RemoteState::Connected {
            self.remote_error = Some("The remote machine is offline.".into());
            cx.notify();
            return;
        }
        if endpoint != self.active_endpoint {
            self.sync_draft();
            self.draft_generation = self.draft_generation.saturating_add(1);
            self.cancel_voice(false, cx);
            std::mem::swap(&mut self.model, &mut self.inactive_model);
            std::mem::swap(
                &mut self.collapsed_folders,
                &mut self.inactive_collapsed_folders,
            );
            self.active_endpoint = endpoint;
            self.model.selected_chat = None;
            self.invalidate_live_markdown_work();
            self.transcript_snapshot = TranscriptSnapshot::default();
            self.transcript.reset(0);
            self.composer_menu = None;
            self.pending_speech = None;
            self.speech_output.stop();
            self.diff_panel = None;
            self.terminal_panel = None;
            self.sidebar_edit = None;
            self.sidebar_context_menu = None;
            self.pending_sidebar_delete = None;
            self.sidebar_move = None;
            self.creating_workspace = false;
            self.creating_chat_folder = None;
            self.workspace_context_folder = None;
            self.workspace_defaults = None;
            self.directory_browser = None;
            self.workspace_clone_status = None;
            self.search = None;
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
            if let Ok(mut images) = self.message_images.lock() {
                images.clear();
            }
            self.message_image_viewer = None;
            self.request_agent_catalog();
        }
        self.select_chat(chat_id, cx);
    }

    fn schedule_connect(&mut self, delay: Duration, cx: &mut Context<Self>) {
        if self.connecting || self.endpoint_model(ChatEndpoint::Local).connected {
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
            .spawn(async { DaemonHandle::connect_or_start() });
        cx.spawn(async move |this, cx| {
            let result = connection.await;
            let _ = this.update(cx, |this, cx| {
                if this.connection_generation != generation {
                    return;
                }
                this.connecting = false;
                this.connection_in_flight = false;
                match result {
                    Ok((daemon, updates, started_daemon)) => {
                        this.daemon = Some(daemon);
                        if started_daemon.is_some() {
                            this._started_daemon = started_daemon;
                        }
                        this.reconnect_attempt = 0;
                        this.endpoint_model_mut(ChatEndpoint::Local)
                            .connection_error = None;
                        this.listen_for_daemon(updates, generation, cx);
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

    fn listen_for_daemon(
        &mut self,
        updates: async_channel::Receiver<DaemonUpdate>,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.connection_generation == generation {
                            this.handle_daemon_update(update, generation, cx);
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

    fn retry_connection(&mut self, cx: &mut Context<Self>) {
        if self.connection_in_flight {
            return;
        }
        self.connecting = false;
        self.reconnect_attempt = 0;
        self.endpoint_model_mut(ChatEndpoint::Local)
            .connection_error = Some("Reconnecting to xd…".into());
        self.schedule_connect(Duration::ZERO, cx);
        cx.notify();
    }

    fn schedule_remote_connect(&mut self, delay: Duration, cx: &mut Context<Self>) {
        if self.remote_credentials.is_none()
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
        let Some(credentials) = self.remote_credentials.clone() else {
            self.remote_state = RemoteState::Unconfigured;
            return;
        };
        let connection = cx
            .background_executor()
            .spawn(async move { remote::connect(&credentials) });
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
        session: RemoteSession,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let (daemon, updates, bridge) = session.into_parts();
        self.remote_daemon = Some(daemon);
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
        if let Some(daemon) = &self.remote_daemon
            && let Err(error) = daemon.tree()
        {
            self.remote_error = Some(error);
        }
        if let Some(daemon) = &self.remote_daemon {
            let _ = daemon.agent_catalog();
        }
    }

    fn listen_for_remote(
        &mut self,
        updates: async_channel::Receiver<DaemonUpdate>,
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
        update: DaemonUpdate,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        match update {
            DaemonUpdate::Connected { .. } => {}
            DaemonUpdate::Disconnected { message } => {
                if self.remote_generation != generation {
                    return;
                }
                self.remote_daemon = None;
                self.remote_bridge = None;
                let remote_model = self.endpoint_model_mut(ChatEndpoint::Remote);
                remote_model.connected = false;
                remote_model.connection_error = Some(format!("{message} Reconnecting…"));
                if self.active_endpoint == ChatEndpoint::Remote {
                    self.sending = false;
                    self.transcript_loading = false;
                    self.pending_speech = None;
                    Arc::make_mut(&mut self.workflow_pending).clear();
                    if let Ok(mut images) = self.message_images.lock() {
                        images.clear_loading();
                    }
                    self.message_image_viewer = None;
                    self.speech_output.stop();
                    self.cancel_voice(false, cx);
                    self.restore_pending_send(cx);
                    if let Some(diff) = &mut self.diff_panel {
                        diff.loading = false;
                        diff.status_loading = false;
                        diff.action = None;
                        diff.action_error = Some(message.clone());
                    }
                    if let Some(panel) = &mut self.terminal_panel {
                        panel.loading = false;
                        panel.opening = false;
                        panel.error = Some(message.clone());
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
            DaemonUpdate::Reply {
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
                    if !self.handle_workspace_create_reply(ChatEndpoint::Remote, &kind, &value, cx)
                    {
                        Self::apply_passive_reply(&mut self.inactive_model, &kind, value);
                    }
                }
            }
            DaemonUpdate::Event {
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
                        Self::apply_passive_event(&mut self.inactive_model, &name, &body);
                    }
                    if name == "turn-finished"
                        && let Some(daemon) = &self.remote_daemon
                    {
                        let _ = daemon.tree();
                    }
                }
            }
        }
        cx.notify();
    }

    fn remote_connection_failed(&mut self, error: RemoteError, cx: &mut Context<Self>) {
        let message = error.to_string();
        self.remote_daemon = None;
        self.remote_bridge = None;
        self.endpoint_model_mut(ChatEndpoint::Remote).connected = false;
        if message.contains("Unknown device. Pair first.") {
            if let Some(file) = &self.remote_credentials_file {
                let _ = file.clear();
            }
            self.remote_credentials = None;
            self.remote_state = RemoteState::Unconfigured;
            self.remote_error = Some("This machine revoked the saved device. Pair again.".into());
        } else {
            self.remote_state = RemoteState::Offline;
            self.remote_error = Some(format!("{message} Retrying automatically…"));
            self.remote_reconnect_attempt = self.remote_reconnect_attempt.saturating_add(1);
            self.schedule_remote_connect(reconnect_delay(self.remote_reconnect_attempt), cx);
        }
        if let Some(panel) = &mut self.remote_panel {
            panel.submitting = false;
            panel.error = self.remote_error.clone();
        }
        let remote_error = self.remote_error.clone();
        self.endpoint_model_mut(ChatEndpoint::Remote)
            .connection_error = remote_error;
    }

    fn retry_remote_connection(&mut self, cx: &mut Context<Self>) {
        if self.remote_credentials.is_none() || self.remote_state == RemoteState::Connecting {
            return;
        }
        self.remote_generation = self.remote_generation.saturating_add(1);
        if self.active_endpoint == ChatEndpoint::Remote {
            self.cancel_voice(false, cx);
        }
        self.remote_daemon = None;
        self.remote_bridge = None;
        self.remote_state = RemoteState::Offline;
        self.remote_reconnect_attempt = 0;
        self.remote_error = None;
        self.schedule_remote_connect(Duration::ZERO, cx);
        cx.notify();
    }

    fn handle_daemon_update(
        &mut self,
        update: DaemonUpdate,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        if self.active_endpoint == ChatEndpoint::Remote {
            match update {
                DaemonUpdate::Connected { .. } => {
                    self.inactive_model.connected = true;
                    self.inactive_model.connection_error = None;
                    self.connecting = false;
                    self.connection_in_flight = false;
                    self.reconnect_attempt = 0;
                    if let Some(daemon) = &self.daemon {
                        let _ = daemon.tree();
                        let _ = daemon.agent_catalog();
                        if let Some(chat_id) = self.inactive_model.selected_chat.as_deref() {
                            let _ = daemon.git_state(chat_id);
                        }
                    }
                }
                DaemonUpdate::Disconnected { message } => {
                    if self.connection_generation != generation {
                        return;
                    }
                    self.daemon = None;
                    self.inactive_model.connected = false;
                    self.inactive_model.connection_error = Some(format!("{message} Reconnecting…"));
                    self.connecting = false;
                    self.connection_in_flight = false;
                    self.schedule_reconnect(cx);
                }
                DaemonUpdate::Reply {
                    kind,
                    body,
                    attachments,
                } => {
                    if Self::local_admin_reply(&kind) {
                        self.handle_reply(kind, body, attachments, cx);
                    } else {
                        let value = Value::Object(body);
                        if !self.handle_workspace_create_reply(
                            ChatEndpoint::Local,
                            &kind,
                            &value,
                            cx,
                        ) {
                            Self::apply_passive_reply(&mut self.inactive_model, &kind, value);
                        }
                    }
                }
                DaemonUpdate::Event {
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
                            Self::apply_passive_event(&mut self.inactive_model, &name, &body);
                        }
                        if name == "turn-finished"
                            && let Some(daemon) = &self.daemon
                        {
                            let _ = daemon.tree();
                        }
                    }
                }
            }
            cx.notify();
            return;
        }
        match update {
            DaemonUpdate::Connected { .. } => {
                self.model.connected = true;
                self.connecting = false;
                self.connection_in_flight = false;
                self.reconnect_attempt = 0;
                self.model.connection_error = None;
                self.request_tree();
                self.request_agent_catalog();
                if let Some(chat_id) = self.model.selected_chat.as_deref()
                    && let Some(daemon) = self.active_daemon()
                {
                    let _ = daemon.git_state(chat_id);
                }
            }
            DaemonUpdate::Disconnected { message } => {
                if self.connection_generation != generation {
                    return;
                }
                self.daemon = None;
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
                self.pending_clone_chats
                    .retain(|(endpoint, _)| *endpoint != ChatEndpoint::Local);
                self.workspace_clone_outcomes
                    .retain(|(endpoint, _), _| *endpoint != ChatEndpoint::Local);
                self.pending_speech = None;
                Arc::make_mut(&mut self.workflow_pending).clear();
                if let Ok(mut images) = self.message_images.lock() {
                    images.clear_loading();
                }
                self.message_image_viewer = None;
                self.speech_output.stop();
                self.cancel_voice(false, cx);
                if let Some(defaults) = &mut self.workspace_defaults {
                    defaults.loading = false;
                    defaults.submitting = false;
                }
                if let Some(search) = &mut self.search {
                    search.loading = false;
                }
                if let Some(diff) = &mut self.diff_panel {
                    diff.loading = false;
                    diff.status_loading = false;
                    diff.action = None;
                    diff.file_loading = false;
                }
                if let Some(panel) = &mut self.terminal_panel {
                    panel.loading = false;
                    panel.opening = false;
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
                    panel.error = Some("The daemon disconnected. Reconnecting…".into());
                }
                if self.auth_open {
                    self.cli_versions_loading = false;
                    self.cli_versions_error = Some("Assistant versions disconnected.".into());
                }
                self.restore_pending_send(cx);
                self.schedule_reconnect(cx);
            }
            DaemonUpdate::Reply {
                kind,
                body,
                attachments,
            } => self.handle_reply(kind, body, attachments, cx),
            DaemonUpdate::Event {
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
                    | RequestKind::GitCommit { .. }
                    | RequestKind::GitPush { .. }
                    | RequestKind::TerminalOpen { .. }
                    | RequestKind::TerminalList { .. }
                    | RequestKind::TerminalInput { .. }
                    | RequestKind::TerminalResize { .. }
                    | RequestKind::TerminalKill { .. }
                    | RequestKind::VoiceModel { .. }
                    | RequestKind::VoiceMutation { .. }
                    | RequestKind::AgentSecrets { .. }
                    | RequestKind::SetAgentSecrets { .. }
                    | RequestKind::AgentClis
                    | RequestKind::DaemonUpdate { .. }
                    | RequestKind::Devices
                    | RequestKind::PeerPairing
                    | RequestKind::RenameDevice { .. }
                    | RequestKind::RevokeDevice { .. }
                    | RequestKind::WorkflowStatus { .. }
                    | RequestKind::ImageRead { .. }
                    | RequestKind::Shortcuts { .. }
                    | RequestKind::SetShortcuts { .. }
                    | RequestKind::ListDirectory { .. }
            ) {
                self.model.connection_error = Some(
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("The xd daemon rejected the request.")
                        .to_owned(),
                );
            }
            match &kind {
                RequestKind::Send { .. } => {
                    self.sending = false;
                    self.restore_pending_send(cx);
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
                    if self.creating_chat_folder.as_deref() == Some(folder_id.as_str())
                        && self.chat_create_submitting =>
                {
                    self.chat_create_submitting = false;
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
                RequestKind::Search { query }
                    if self
                        .search
                        .as_ref()
                        .is_some_and(|search| search.query == *query) =>
                {
                    if let Some(search) = &mut self.search {
                        search.loading = false;
                    }
                }
                RequestKind::ListDirectory { generation, .. }
                    if self
                        .directory_browser
                        .as_ref()
                        .is_some_and(|browser| browser.generation == *generation) =>
                {
                    if let Some(browser) = &mut self.directory_browser {
                        browser.loading = false;
                        browser.error = Some(
                            value
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("Cannot read that directory.")
                                .to_owned(),
                        );
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
                RequestKind::ImageRead { path } => {
                    if let Ok(mut images) = self.message_images.lock() {
                        images.finish(path, None);
                    }
                    self.invalidate_image_rows(path);
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
                RequestKind::TerminalOpen { chat_id, .. }
                | RequestKind::TerminalList { chat_id }
                    if self
                        .terminal_panel
                        .as_ref()
                        .is_some_and(|panel| &panel.chat_id == chat_id) =>
                {
                    if let Some(panel) = &mut self.terminal_panel {
                        panel.loading = false;
                        panel.opening = false;
                        panel.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                }
                RequestKind::TerminalInput { terminal_id }
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
                RequestKind::VoiceModel { chat_id }
                    if self.chat_is_active(chat_id)
                        && matches!(self.voice_input.state, VoiceState::Checking) =>
                {
                    self.fail_voice(
                        value
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("The daemon could not check the speech model.")
                            .to_owned(),
                        cx,
                    );
                }
                RequestKind::VoiceMutation { token, .. } if token == &self.voice_input.token => {
                    self.fail_voice(
                        value
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("The daemon rejected voice input.")
                            .to_owned(),
                        cx,
                    );
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
                RequestKind::DaemonUpdate { .. } => {
                    if let Some(panel) = &mut self.self_update_panel {
                        panel.busy = false;
                        panel.error = Some(
                            value
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("The daemon update request failed.")
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
                }
                RequestKind::RenameFolder { folder_id, .. } => {
                    if let Some(edit) = &mut self.sidebar_edit
                        && edit.target == SidebarTarget::Folder(folder_id.clone())
                    {
                        edit.submitting = false;
                    }
                }
                RequestKind::MoveFolder { folder_id, .. } => {
                    if self.sidebar_move.as_ref() == Some(&SidebarTarget::Folder(folder_id.clone()))
                    {
                        self.sidebar_move_submitting = false;
                        self.sidebar_move_destination = None;
                    }
                }
                RequestKind::RenameChat { chat_id, .. } => {
                    if let Some(edit) = &mut self.sidebar_edit
                        && edit.target == SidebarTarget::Chat(chat_id.clone())
                    {
                        edit.submitting = false;
                    }
                }
                RequestKind::MoveChat { chat_id, .. } => {
                    if self.sidebar_move.as_ref() == Some(&SidebarTarget::Chat(chat_id.clone())) {
                        self.sidebar_move_submitting = false;
                        self.sidebar_move_destination = None;
                    }
                }
                RequestKind::TrashFolder { folder_id } => {
                    if self.pending_sidebar_delete.as_ref()
                        == Some(&SidebarTarget::Folder(folder_id.clone()))
                    {
                        self.sidebar_delete_submitting = false;
                    }
                }
                RequestKind::DeleteChat { chat_id } => {
                    if self.pending_sidebar_delete.as_ref()
                        == Some(&SidebarTarget::Chat(chat_id.clone()))
                    {
                        self.sidebar_delete_submitting = false;
                    }
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
                    self.invalidate_live_markdown_work();
                    if let Ok(mut images) = self.message_images.lock() {
                        images.clear();
                    }
                    self.transcript_snapshot = TranscriptSnapshot::default();
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
                    .sidebar_context_menu
                    .as_ref()
                    .and_then(|menu| menu.target.as_ref())
                    .is_some_and(|target| !self.sidebar_target_exists(target))
                {
                    self.sidebar_context_menu = None;
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
                if self.model.selected_chat.is_none() {
                    let chat_id = (self.active_endpoint == ChatEndpoint::Local)
                        .then(|| self.settings.last_chat.clone())
                        .flatten()
                        .filter(|chat_id| self.model.chats.iter().any(|chat| &chat.id == chat_id))
                        .or_else(|| self.model.chats.first().map(|chat| chat.id.clone()));
                    if let Some(chat_id) = chat_id {
                        self.select_chat(chat_id, cx);
                    }
                }
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
            RequestKind::DaemonUpdate { .. } => self.apply_self_update(&value),
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
                if let Some(daemon) = self.secrets_daemon(folder_id.as_deref()).cloned()
                    && let Err(error) = daemon.agent_secrets(folder_id.as_deref())
                    && let Some(panel) = &mut self.secrets_panel
                {
                    panel.loading = false;
                    panel.error = Some(error);
                }
            }
            RequestKind::Devices => {
                match serde_json::from_value::<Vec<PairedDevice>>(
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
            RequestKind::VoiceModel { chat_id }
                if self.chat_is_active(&chat_id)
                    && matches!(self.voice_input.state, VoiceState::Checking) =>
            {
                if value.get("available").and_then(Value::as_bool) == Some(true) {
                    self.start_voice_recording(cx);
                } else {
                    self.voice_input.state = VoiceState::NeedsModel;
                }
            }
            RequestKind::VoiceMutation { .. } => {}
            RequestKind::Search { query } => {
                if self
                    .search
                    .as_ref()
                    .is_some_and(|search| search.query == query)
                {
                    match serde_json::from_value::<Vec<SearchHit>>(
                        value.get("results").cloned().unwrap_or_default(),
                    ) {
                        Ok(results) => {
                            if let Some(search) = &mut self.search {
                                search.results = results;
                                search.loading = false;
                            }
                        }
                        Err(error) => {
                            self.model.connection_error =
                                Some(format!("Invalid search response: {error}"));
                            if let Some(search) = &mut self.search {
                                search.loading = false;
                            }
                        }
                    }
                }
            }
            RequestKind::ListDirectory { generation, .. } => {
                if self
                    .directory_browser
                    .as_ref()
                    .is_none_or(|browser| browser.generation != generation)
                {
                    return;
                }
                let path = value.get("path").and_then(Value::as_str).map(str::to_owned);
                let entries = value
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    });
                if let Some(browser) = &mut self.directory_browser {
                    match (path, entries) {
                        (Some(path), Some(entries)) => {
                            browser.path = Some(path);
                            browser.entries = entries;
                            browser.selected = (!browser.entries.is_empty()).then_some(0);
                            browser.loading = false;
                            browser.error = None;
                        }
                        _ => {
                            browser.loading = false;
                            browser.error = Some("Daemon returned an invalid directory.".into());
                        }
                    }
                }
            }
            RequestKind::ImageRead { path } => {
                let image = attachments
                    .and_then(|mut attachments| attachments.pop())
                    .map(|attachment| attachment.preview);
                if let Ok(mut images) = self.message_images.lock() {
                    images.finish(&path, image);
                }
                self.invalidate_image_rows(&path);
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
                        if let Some(daemon) = self.active_daemon().cloned()
                            && let Err(error) = daemon.diff_read(
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
                            && let Some(daemon) = self.active_daemon().cloned()
                            && let Err(error) = daemon.git_pull_request_status(&chat_id, generation)
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
                        backend: value
                            .get("backend")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        model: value
                            .get("model")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        effective_backend: value
                            .get("effective_backend")
                            .and_then(Value::as_str)
                            .unwrap_or("claude")
                            .to_owned(),
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
                if self.creating_chat_folder.as_deref() == Some(folder_id.as_str())
                    && self.chat_create_submitting
                {
                    let title = self.chat_create_title.clone();
                    let start = value
                        .get("effective_workdir")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.open_directory_browser(
                        WorkspacePathTarget::CreateChat { folder_id, title },
                        start,
                        cx,
                    );
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
                    self.model.connection_error = Some("The daemon returned no chat id.".into());
                    return;
                };
                if self.creating_chat_folder.as_deref() == Some(folder_id.as_str())
                    && self.chat_create_title.trim() == title
                {
                    self.cancel_chat_create(cx);
                }
                self.request_tree();
                self.select_chat(chat_id.to_owned(), cx);
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
                    self.model.live_activity.clear();
                }
                self.transcript_snapshot.sync_messages(&self.model);
                self.transcript_snapshot.sync_live_activity(&self.model);
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
                self.request_workflow_statuses();
                if std::mem::take(&mut self.transcript_refresh_pending) {
                    self.request_messages(&chat_id);
                }
            }
            RequestKind::Send { chat_id, text } if self.chat_is_active(&chat_id) => {
                self.sending = false;
                self.pending_send = None;
                let queued = value
                    .get("queued")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !queued {
                    self.model.working = true;
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
            RequestKind::TerminalOpen { chat_id, .. }
                if self
                    .terminal_panel
                    .as_ref()
                    .is_some_and(|panel| panel.chat_id == chat_id) =>
            {
                let mut resize = None;
                if let Some(panel) = &mut self.terminal_panel {
                    panel.selected = value.get("id").and_then(Value::as_str).map(str::to_owned);
                    panel.opening = false;
                    panel.loading = true;
                    resize = panel
                        .selected
                        .clone()
                        .zip(panel.viewport)
                        .map(|(terminal_id, (columns, rows))| (terminal_id, columns, rows));
                }
                if let Some(daemon) = self.active_daemon().cloned() {
                    let result = resize
                        .map(|(terminal_id, columns, rows)| {
                            daemon.terminal_resize(&terminal_id, columns, rows)
                        })
                        .transpose()
                        .and_then(|_| daemon.terminal_list(&chat_id));
                    if let Err(error) = result
                        && let Some(panel) = &mut self.terminal_panel
                    {
                        panel.loading = false;
                        panel.error = Some(error);
                    }
                }
            }
            RequestKind::TerminalList { chat_id }
                if self
                    .terminal_panel
                    .as_ref()
                    .is_some_and(|panel| panel.chat_id == chat_id) =>
            {
                self.apply_terminal_list(&value);
            }
            RequestKind::TerminalInput { .. } | RequestKind::TerminalResize { .. } => {}
            RequestKind::TerminalKill { .. } => {}
            _ => {}
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
                    .unwrap_or("The xd daemon rejected the workspace.")
                    .to_owned(),
            );
            return true;
        }
        let Some(folder_id) = value.get("id").and_then(Value::as_str) else {
            self.pending_clone_requests.remove(&endpoint);
            self.endpoint_model_mut(endpoint).connection_error =
                Some("The daemon returned no workspace id.".into());
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
                    if let Some(daemon) = self.endpoint_daemon(endpoint).cloned()
                        && let Err(error) = daemon.new_chat(folder_id, "New Chat", None)
                    {
                        self.endpoint_model_mut(endpoint).connection_error = Some(error);
                    }
                }
                Some(Some(error)) => {
                    if endpoint == self.active_endpoint {
                        self.workspace_clone_status = None;
                    }
                    self.endpoint_model_mut(endpoint).connection_error = Some(error);
                }
                None => {
                    self.pending_clone_chats.insert(key);
                    if endpoint == self.active_endpoint {
                        self.workspace_clone_status = Some("Cloning repository…".into());
                    }
                }
            }
        } else if let Some(daemon) = self.endpoint_daemon(endpoint).cloned()
            && let Err(error) = daemon.new_chat(folder_id, "New Chat", None)
        {
            self.endpoint_model_mut(endpoint).connection_error = Some(error);
        }
        true
    }

    fn handle_folder_clone_event(&mut self, endpoint: ChatEndpoint, body: &Value) {
        let folder_id = body.get("folder").and_then(Value::as_str);
        let key = folder_id.map(|folder_id| (endpoint, folder_id.to_owned()));
        let event_url = body.get("url").and_then(Value::as_str);
        let preserve_outcome = key
            .as_ref()
            .is_some_and(|key| self.pending_clone_chats.contains(key))
            || self
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
                if let Some(key) = &key
                    && self.pending_clone_chats.remove(key)
                {
                    if let Some(daemon) = self.endpoint_daemon(endpoint).cloned()
                        && let Err(error) = daemon.new_chat(&key.1, "New Chat", None)
                    {
                        self.endpoint_model_mut(endpoint).connection_error = Some(error);
                    }
                    self.workspace_clone_outcomes.remove(key);
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
                if let Some(key) = &key
                    && self.pending_clone_chats.remove(key)
                {
                    self.workspace_clone_outcomes.remove(key);
                }
                if endpoint == self.active_endpoint {
                    self.workspace_clone_status = None;
                }
                self.endpoint_model_mut(endpoint).connection_error = Some(error);
            }
            _ => {}
        }
    }

    fn handle_event(
        &mut self,
        name: &str,
        body: Value,
        attachments: Option<Vec<Attachment>>,
        cx: &mut Context<Self>,
    ) {
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
            "voice" => self.handle_voice_event(&body, cx),
            "workflow-status" => {
                if let Some(marker) = body.get("text").and_then(Value::as_str).map(str::to_owned)
                    && self
                        .model
                        .messages
                        .iter()
                        .chain(self.model.live_activity.iter())
                        .any(|message| message.role == "tool" && message.content == marker)
                {
                    Arc::make_mut(&mut self.workflow_pending).remove(&marker);
                    Arc::make_mut(&mut self.workflow_statuses).insert(marker.clone(), body.clone());
                    self.invalidate_workflow_rows(&marker);
                    self.schedule_workflow_refresh(marker, cx);
                }
            }
            "terminal-opened" if self.event_is_active(&body) => {
                if let Some(panel) = &mut self.terminal_panel {
                    let terminal_id = body
                        .get("terminal")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    panel.opening = false;
                    let title = body
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Terminal")
                        .to_owned();
                    let columns =
                        body.get("columns").and_then(Value::as_u64).unwrap_or(120) as usize;
                    let rows = body.get("rows").and_then(Value::as_u64).unwrap_or(32) as usize;
                    if let Some(terminal_id) = terminal_id {
                        if !panel
                            .sessions
                            .iter()
                            .any(|session| session.id == terminal_id)
                        {
                            panel.sessions.push(TerminalTab {
                                id: terminal_id.clone(),
                                title,
                                screen: TerminalScreen::new(columns, rows),
                            });
                        }
                        panel.selected = Some(terminal_id);
                    }
                }
            }
            "terminal-output" if self.event_is_active(&body) => {
                if let Some(panel) = &mut self.terminal_panel
                    && let Some(session) = panel.sessions.iter_mut().find(|session| {
                        Some(session.id.as_str()) == body.get("terminal").and_then(Value::as_str)
                    })
                    && let Some(data) = body.get("data").and_then(Value::as_str)
                    && let Ok(data) = STANDARD.decode(data)
                {
                    session.screen.feed(&data);
                    self.terminal_cursor_visible = true;
                }
            }
            "terminal-resized" if self.event_is_active(&body) => {
                if let Some(panel) = &mut self.terminal_panel
                    && let Some(session) = panel.sessions.iter_mut().find(|session| {
                        Some(session.id.as_str()) == body.get("terminal").and_then(Value::as_str)
                    })
                {
                    let columns =
                        body.get("columns").and_then(Value::as_u64).unwrap_or(120) as usize;
                    let rows = body.get("rows").and_then(Value::as_u64).unwrap_or(32) as usize;
                    session.screen.resize(columns, rows);
                }
            }
            "terminal-closed" if self.event_is_active(&body) => {
                if let Some(panel) = &mut self.terminal_panel {
                    let closed = body.get("terminal").and_then(Value::as_str);
                    if let Some(closed) = closed {
                        panel.remove(closed);
                    }
                    panel.loading = false;
                }
            }
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
                self.composer_menu = None;
                self.clear_question(cx);
                self.model.apply_event(name, &body);
                self.invalidate_live_markdown_work();
                self.transcript_snapshot.sync_live_text(&self.model);
                self.transcript_snapshot.sync_live_activity(&self.model);
                self.sync_transcript_count(false);
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_messages(&chat_id);
                    self.request_chat(&chat_id);
                }
            }
            "text" | "tool" if self.event_is_active(&body) => {
                let old_count = self.model.display_message_count();
                self.model.apply_event(name, &body);
                if name == "text" {
                    self.transcript_snapshot.sync_live_text(&self.model);
                    self.schedule_live_markdown_parse(cx);
                } else {
                    self.transcript_snapshot.sync_live_activity(&self.model);
                }
                let new_count = self.model.display_message_count();
                if new_count > old_count {
                    self.transcript
                        .splice(old_count..old_count, new_count - old_count);
                } else if new_count > 0 {
                    self.transcript.splice(new_count - 1..new_count, 1);
                }
                if name == "tool" {
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
            "daemon-update" => self.apply_self_update(&body),
            _ => {}
        }
        cx.notify();
    }

    fn request_tree(&mut self) {
        if let Some(daemon) = self.active_daemon() {
            if let Err(error) = daemon.tree() {
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
                panel.error = Some(format!("Invalid daemon update response: {error}"));
            }
        }
    }

    fn open_self_update(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.self_update_panel = Some(SelfUpdatePanel {
            busy: true,
            ..Default::default()
        });
        self.request_self_update("check");
        cx.notify();
    }

    fn close_self_update(&mut self, cx: &mut Context<Self>) {
        self.self_update_panel = None;
        cx.notify();
    }

    fn request_self_update(&mut self, action: &str) {
        let Some(panel) = &mut self.self_update_panel else {
            return;
        };
        if panel.busy && action != "check" {
            return;
        }
        panel.busy = true;
        panel.error = None;
        match self.daemon.as_ref() {
            Some(daemon) => {
                if let Err(error) = daemon.daemon_update(action) {
                    panel.busy = false;
                    panel.error = Some(error);
                }
            }
            None => {
                panel.busy = false;
                panel.error = Some("xd is not connected to a daemon.".into());
            }
        }
    }

    fn install_self_update(&mut self, cx: &mut Context<Self>) {
        if self
            .self_update_panel
            .as_ref()
            .is_some_and(|panel| self_update_action(panel) == Some("install"))
        {
            self.request_self_update("install");
            cx.notify();
        }
    }

    fn restart_self_update(&mut self, cx: &mut Context<Self>) {
        if self
            .self_update_panel
            .as_ref()
            .is_some_and(|panel| self_update_action(panel) == Some("restart"))
        {
            self.request_self_update("restart");
            cx.notify();
        }
    }

    fn open_source_build(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.source_build_open = true;
        let focus = self.source_build_input.read(cx).focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    fn close_source_build(&mut self, cx: &mut Context<Self>) {
        self.settings.build_source = self.source_build_panel.text.clone();
        let _ = self.settings.save();
        self.source_build_open = false;
        cx.notify();
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

    fn stop_source_build(&mut self, cx: &mut Context<Self>) {
        if !self.source_build_panel.running || self.source_build_panel.stopping {
            return;
        }
        self.source_build_panel.stopping = true;
        if let Some(run) = &self.source_build_run {
            run.cancel();
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
        let active_provider = active_auth_provider(&self.model.backend, self.model.claude_mode);
        if let Some(provider) = self
            .auth_providers
            .iter()
            .find(|provider| provider.provider == active_provider)
        {
            self.model.auth_state = provider.state.clone();
        }
    }

    fn request_agent_catalog(&mut self) {
        if let Some(daemon) = self.active_daemon()
            && let Err(error) = daemon.agent_catalog()
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
        if let Some(daemon) = self.active_daemon()
            && let Err(error) = daemon.shortcuts(Some(&folder_id))
        {
            self.model.connection_error = Some(error);
        }
    }

    fn begin_workspace_create(&mut self, cx: &mut Context<Self>) {
        self.workspace_path_generation = self.workspace_path_generation.saturating_add(1);
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

    fn choose_workspace_path(&mut self, target: WorkspacePathTarget, cx: &mut Context<Self>) {
        let start = match &target {
            WorkspacePathTarget::CreateRepository => {
                optional_trimmed(&self.workspace_create_repo).map(str::to_owned)
            }
            WorkspacePathTarget::DefaultsWorkdir { folder_id } => self
                .workspace_defaults
                .as_ref()
                .filter(|defaults| &defaults.folder_id == folder_id)
                .and_then(|defaults| optional_trimmed(&defaults.workdir))
                .map(str::to_owned),
            WorkspacePathTarget::DefaultsRepository { folder_id } => self
                .workspace_defaults
                .as_ref()
                .filter(|defaults| &defaults.folder_id == folder_id)
                .and_then(|defaults| {
                    optional_trimmed(&defaults.repo).or_else(|| optional_trimmed(&defaults.workdir))
                })
                .map(str::to_owned),
            WorkspacePathTarget::CreateChat { .. } => None,
        };
        self.open_directory_browser(target, start, cx);
    }

    fn open_directory_browser(
        &mut self,
        target: WorkspacePathTarget,
        start: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.workspace_path_generation = self.workspace_path_generation.saturating_add(1);
        let generation = self.workspace_path_generation;
        self.directory_browser = Some(DirectoryBrowser {
            target,
            path: start.clone(),
            entries: Vec::new(),
            selected: None,
            loading: true,
            error: None,
            generation,
        });
        if let Some(daemon) = self.active_daemon().cloned() {
            if let Err(error) = daemon.list_directory(start.as_deref(), generation)
                && let Some(browser) = &mut self.directory_browser
            {
                browser.loading = false;
                browser.error = Some(error);
            }
        } else if let Some(browser) = &mut self.directory_browser {
            browser.loading = false;
            browser.error = Some("xd is not connected to a daemon.".into());
        }
        cx.notify();
    }

    fn show_directory(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        self.workspace_path_generation = self.workspace_path_generation.saturating_add(1);
        let generation = self.workspace_path_generation;
        let Some(browser) = &mut self.directory_browser else {
            return;
        };
        browser.loading = true;
        browser.error = None;
        browser.generation = generation;
        if let Some(daemon) = self.active_daemon().cloned() {
            if let Err(error) = daemon.list_directory(path.as_deref(), generation)
                && let Some(browser) = &mut self.directory_browser
            {
                browser.loading = false;
                browser.error = Some(error);
            }
        } else if let Some(browser) = &mut self.directory_browser {
            browser.loading = false;
            browser.error = Some("xd is not connected to a daemon.".into());
        }
        cx.notify();
    }

    fn select_directory_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(browser) = &mut self.directory_browser
            && index < browser.entries.len()
        {
            browser.selected = Some(index);
            cx.notify();
        }
    }

    fn move_directory_selection(&mut self, direction: isize, cx: &mut Context<Self>) {
        let Some(browser) = &mut self.directory_browser else {
            return;
        };
        let next = next_directory_selection(browser.selected, browser.entries.len(), direction);
        if next != browser.selected {
            browser.selected = next;
            cx.notify();
        }
    }

    fn open_selected_directory(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self
            .directory_browser
            .as_ref()
            .filter(|browser| !browser.loading)
            .and_then(|browser| browser.selected)
        else {
            return;
        };
        self.descend_directory(index, cx);
    }

    fn descend_directory(&mut self, index: usize, cx: &mut Context<Self>) {
        let path = self.directory_browser.as_ref().and_then(|browser| {
            Some(directory_child_path(
                browser.path.as_deref()?,
                browser.entries.get(index)?,
            ))
        });
        if let Some(path) = path {
            self.show_directory(Some(path), cx);
        }
    }

    fn ascend_directory(&mut self, cx: &mut Context<Self>) {
        let parent = self
            .directory_browser
            .as_ref()
            .and_then(|browser| browser.path.as_deref())
            .and_then(directory_parent_path);
        if let Some(parent) = parent {
            self.show_directory(Some(parent), cx);
        }
    }

    fn choose_current_directory(&mut self, cx: &mut Context<Self>) {
        let Some((target, path)) = self.directory_browser.as_ref().and_then(|browser| {
            browser
                .path
                .clone()
                .map(|path| (browser.target.clone(), path))
        }) else {
            return;
        };
        self.directory_browser = None;
        self.apply_workspace_path(target, path, cx);
    }

    fn apply_workspace_path(
        &mut self,
        target: WorkspacePathTarget,
        path: String,
        cx: &mut Context<Self>,
    ) {
        match target {
            WorkspacePathTarget::CreateRepository
                if self.creating_workspace && !self.workspace_create_submitting =>
            {
                self.workspace_create_repo = path.clone();
                self.workspace_create_clone.clear();
                self.workspace_repo_input
                    .update(cx, |input, cx| input.set_text(path, cx));
                self.workspace_clone_input
                    .update(cx, |input, cx| input.set_text("", cx));
            }
            WorkspacePathTarget::DefaultsWorkdir { folder_id } => {
                if let Some(defaults) = &mut self.workspace_defaults
                    && defaults.folder_id == folder_id
                    && !defaults.loading
                    && !defaults.submitting
                {
                    defaults.workdir = path.clone();
                    self.workspace_workdir_input
                        .update(cx, |input, cx| input.set_text(path, cx));
                }
            }
            WorkspacePathTarget::DefaultsRepository { folder_id } => {
                if let Some(defaults) = &mut self.workspace_defaults
                    && defaults.folder_id == folder_id
                    && !defaults.loading
                    && !defaults.submitting
                {
                    defaults.repo = path.clone();
                    self.workspace_repo_default_input
                        .update(cx, |input, cx| input.set_text(path, cx));
                }
            }
            WorkspacePathTarget::CreateChat { folder_id, title } => {
                let result = self
                    .active_daemon()
                    .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
                    .and_then(|daemon| daemon.new_chat(&folder_id, &title, Some(&path)));
                if let Err(error) = result {
                    self.chat_create_submitting = false;
                    self.model.connection_error = Some(error);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn close_directory_browser(&mut self, cx: &mut Context<Self>) {
        self.workspace_path_generation = self.workspace_path_generation.saturating_add(1);
        let target = self.directory_browser.take().map(|browser| browser.target);
        if let Some(WorkspacePathTarget::CreateChat { folder_id, title }) = target.as_ref() {
            let result = self
                .active_daemon()
                .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
                .and_then(|daemon| daemon.new_chat(folder_id, title, None));
            if let Err(error) = result {
                self.chat_create_submitting = false;
                self.model.connection_error = Some(error);
            }
        }
        if target.is_some() {
            cx.notify();
        }
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
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.new_folder(name, repo.as_deref(), repo_url.as_deref()));
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
        self.workspace_path_generation = self.workspace_path_generation.saturating_add(1);
        self.directory_browser = None;
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

    fn begin_chat_create(&mut self, folder_id: String, cx: &mut Context<Self>) {
        self.creating_chat_folder = Some(folder_id);
        self.chat_create_title.clear();
        self.chat_create_submitting = false;
        self.chat_create_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn chat_create_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if self.creating_chat_folder.is_some() && !self.chat_create_submitting {
            self.chat_create_title = text;
            cx.notify();
        }
    }

    fn save_chat_create(&mut self, cx: &mut Context<Self>) {
        let Some(folder_id) = self.creating_chat_folder.clone() else {
            return;
        };
        if self.chat_create_submitting {
            return;
        }
        let title = self.chat_create_title.trim();
        if title.is_empty() {
            self.model.connection_error = Some("A chat title cannot be empty.".into());
            cx.notify();
            return;
        }
        let result = self
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.folder_settings(&folder_id));
        match result {
            Ok(()) => {
                self.chat_create_title = title.to_owned();
                self.chat_create_submitting = true;
            }
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    fn cancel_chat_create(&mut self, cx: &mut Context<Self>) {
        self.workspace_path_generation = self.workspace_path_generation.saturating_add(1);
        self.directory_browser = None;
        self.creating_chat_folder = None;
        self.chat_create_title.clear();
        self.chat_create_submitting = false;
        self.chat_create_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn begin_workspace_context(&mut self, folder_id: String, cx: &mut Context<Self>) {
        self.workspace_context_folder = Some(folder_id.clone());
        self.workspace_context_text.clear();
        self.workspace_context_loading = true;
        self.workspace_context_submitting = false;
        self.workspace_context_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        if let Some(daemon) = self.active_daemon().cloned() {
            if let Err(error) = daemon.folder_context(&folder_id) {
                self.workspace_context_loading = false;
                self.model.connection_error = Some(error);
            }
        } else {
            self.workspace_context_loading = false;
            self.model.connection_error = Some("xd is not connected to a daemon.".into());
        }
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
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.set_folder_context(&folder_id, context.as_deref()));
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

    fn begin_workspace_defaults(&mut self, folder_id: String, cx: &mut Context<Self>) {
        self.workspace_path_generation = self.workspace_path_generation.saturating_add(1);
        let Some(daemon) = self.active_daemon().cloned() else {
            self.model.connection_error = Some("xd is not connected to a daemon.".into());
            cx.notify();
            return;
        };
        self.workspace_defaults = Some(WorkspaceDefaults {
            folder_id: folder_id.clone(),
            backend: None,
            model: None,
            effective_backend: "claude".into(),
            workdir: String::new(),
            repo: String::new(),
            loading: true,
            submitting: false,
        });
        self.workspace_workdir_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.workspace_repo_default_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        if let Err(error) = daemon.folder_settings(&folder_id) {
            self.workspace_defaults = None;
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn cancel_workspace_defaults(&mut self, cx: &mut Context<Self>) {
        self.workspace_path_generation = self.workspace_path_generation.saturating_add(1);
        self.directory_browser = None;
        self.workspace_defaults = None;
        cx.notify();
    }

    fn select_workspace_backend(&mut self, backend: Option<String>, cx: &mut Context<Self>) {
        if let Some(defaults) = &mut self.workspace_defaults
            && !defaults.loading
            && !defaults.submitting
        {
            defaults.backend = backend;
            defaults.model = None;
            cx.notify();
        }
    }

    fn select_workspace_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        if let Some(defaults) = &mut self.workspace_defaults
            && !defaults.loading
            && !defaults.submitting
        {
            defaults.model = model;
            cx.notify();
        }
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

    fn save_workspace_defaults(&mut self, cx: &mut Context<Self>) {
        let Some(defaults) = self.workspace_defaults.clone() else {
            return;
        };
        if defaults.loading || defaults.submitting {
            return;
        }
        let result = self
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| {
                daemon.set_folder_settings(
                    &defaults.folder_id,
                    defaults.backend.as_deref(),
                    defaults.model.as_deref(),
                    optional_trimmed(&defaults.workdir),
                    optional_trimmed(&defaults.repo),
                )
            });
        match result {
            Ok(()) => {
                if let Some(current) = &mut self.workspace_defaults {
                    current.submitting = true;
                }
            }
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.search_generation = self.search_generation.saturating_add(1);
        self.search = Some(SearchPanel::default());
        self.search_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        let focus = self.search_input.read(cx).focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.saturating_add(1);
        self.search = None;
        cx.notify();
    }

    fn search_changed(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(search) = &mut self.search else {
            return;
        };
        search.query = text;
        search.results.clear();
        self.search_generation = self.search_generation.saturating_add(1);
        let generation = self.search_generation;
        if search.query.trim().is_empty() {
            search.loading = false;
            cx.notify();
            return;
        }
        search.loading = true;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(150)).await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                let Some(query) = this.search.as_ref().map(|search| search.query.clone()) else {
                    return;
                };
                match this.active_daemon().cloned() {
                    Some(daemon) => {
                        if let Err(error) = daemon.search(query.trim()) {
                            this.model.connection_error = Some(error);
                            if let Some(search) = &mut this.search {
                                search.loading = false;
                            }
                        }
                    }
                    None => {
                        this.model.connection_error =
                            Some("xd is not connected to a daemon.".into());
                        if let Some(search) = &mut this.search {
                            search.loading = false;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn activate_search_result(&mut self, chat_id: String, cx: &mut Context<Self>) {
        self.close_search(cx);
        self.select_chat(chat_id, cx);
    }

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        self.settings_menu = None;
        if self.settings_open {
            self.search_generation = self.search_generation.saturating_add(1);
            self.search = None;
        }
        cx.notify();
    }

    fn toggle_settings_menu(&mut self, menu: SettingsMenu, cx: &mut Context<Self>) {
        self.settings_menu = (self.settings_menu != Some(menu)).then_some(menu);
        cx.notify();
    }

    fn set_git_writer(&mut self, writer: GitWriter, cx: &mut Context<Self>) {
        self.settings.git_writer = writer;
        self.settings.git_writer_model = None;
        self.settings_menu = None;
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn set_git_writer_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.settings.git_writer_model = Some(model);
        self.settings_menu = None;
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn toggle_auth(&mut self, cx: &mut Context<Self>) {
        self.auth_open = !self.auth_open;
        if self.auth_open {
            self.settings_open = false;
            self.auth_providers.clear();
            self.cli_versions.clear();
            self.cli_versions_loading = true;
            self.cli_versions_error = None;
            match self.active_daemon().cloned() {
                Some(daemon) => {
                    if let Err(error) = daemon.agent_auth() {
                        self.model.connection_error = Some(error);
                    }
                    if let Err(error) = daemon.agent_clis() {
                        self.cli_versions_loading = false;
                        self.cli_versions_error = Some(error);
                    }
                }
                None => {
                    self.cli_versions_loading = false;
                    self.cli_versions_error = Some("The selected machine is not connected.".into());
                }
            }
        }
        cx.notify();
    }

    fn refresh_cli_versions(&mut self, cx: &mut Context<Self>) {
        if self.cli_versions_loading {
            return;
        }
        self.cli_versions_loading = true;
        self.cli_versions_error = None;
        match self.active_daemon().cloned() {
            Some(daemon) => {
                if let Err(error) = daemon.agent_clis() {
                    self.cli_versions_loading = false;
                    self.cli_versions_error = Some(error);
                }
            }
            None => {
                self.cli_versions_loading = false;
                self.cli_versions_error = Some("The selected machine is not connected.".into());
            }
        }
        cx.notify();
    }

    fn open_secrets(
        &mut self,
        folder_id: Option<String>,
        folder_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_open = false;
        self.secrets_panel = Some(SecretsPanel {
            folder_id: folder_id.clone(),
            folder_name,
            names: Vec::new(),
            name: String::new(),
            value: String::new(),
            loading: true,
            submitting: false,
            error: None,
        });
        self.secret_name_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.secret_value_input
            .update(cx, |input, cx| input.set_text("", cx));
        match self.secrets_daemon(folder_id.as_deref()).cloned() {
            Some(daemon) => {
                if let Err(error) = daemon.agent_secrets(folder_id.as_deref())
                    && let Some(panel) = &mut self.secrets_panel
                {
                    panel.loading = false;
                    panel.error = Some(error);
                }
            }
            None => {
                if let Some(panel) = &mut self.secrets_panel {
                    panel.loading = false;
                    panel.error = Some("xd is not connected to a daemon.".into());
                }
            }
        }
        let focus = self.secret_name_input.read(cx).focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    fn close_secrets(&mut self, cx: &mut Context<Self>) {
        self.secrets_panel = None;
        self.secret_name_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.secret_value_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    fn open_devices(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.devices_panel = Some(DevicesPanel {
            loading: true,
            ..Default::default()
        });
        self.request_devices();
        cx.notify();
    }

    fn open_share(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.share_panel = Some(SharePanel::default());
        cx.notify();
    }

    fn open_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = false;
        let host = self
            .remote_credentials
            .as_ref()
            .map(|credentials| credentials.host.clone())
            .unwrap_or_default();
        let port = self
            .remote_credentials
            .as_ref()
            .map(|credentials| credentials.port.to_string())
            .unwrap_or_else(|| "4001".into());
        let name = std::env::var("HOSTNAME")
            .ok()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Desktop".into());
        self.remote_panel = Some(RemotePanel {
            host: host.clone(),
            port: port.clone(),
            code: String::new(),
            name: name.clone(),
            submitting: false,
            error: self.remote_error.clone(),
        });
        self.remote_host_input
            .update(cx, |input, cx| input.set_text(host, cx));
        self.remote_port_input
            .update(cx, |input, cx| input.set_text(port, cx));
        self.remote_code_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.remote_name_input
            .update(cx, |input, cx| input.set_text(name, cx));
        let focus = self.remote_host_input.read(cx).focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    fn close_remote(&mut self, cx: &mut Context<Self>) {
        let canceled_pairing = self
            .remote_panel
            .as_ref()
            .is_some_and(|panel| panel.submitting);
        if canceled_pairing {
            self.remote_generation = self.remote_generation.saturating_add(1);
            self.remote_state = if self.remote_credentials.is_some() {
                RemoteState::Offline
            } else {
                RemoteState::Unconfigured
            };
        }
        self.remote_panel = None;
        if canceled_pairing && self.remote_credentials.is_some() {
            self.schedule_remote_connect(Duration::ZERO, cx);
        }
        cx.notify();
    }

    fn pair_remote_machine(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.remote_panel.clone() else {
            return;
        };
        if panel.submitting {
            return;
        }
        let port = match panel
            .port
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
        {
            Some(port) => port,
            None => {
                if let Some(panel) = &mut self.remote_panel {
                    panel.error = Some("Remote port must be from 1 to 65535.".into());
                }
                cx.notify();
                return;
            }
        };
        if panel.host.trim().is_empty()
            || panel.code.split_whitespace().collect::<String>().is_empty()
            || panel.name.trim().is_empty()
        {
            if let Some(panel) = &mut self.remote_panel {
                panel.error = Some("Address, pairing code, and device name are required.".into());
            }
            cx.notify();
            return;
        }
        let Some(credentials_file) = self.remote_credentials_file.clone() else {
            if let Some(panel) = &mut self.remote_panel {
                panel.error = Some("The remote credentials path is unavailable.".into());
            }
            cx.notify();
            return;
        };
        self.remote_generation = self.remote_generation.saturating_add(1);
        let generation = self.remote_generation;
        self.remote_daemon = None;
        self.remote_bridge = None;
        self.remote_state = RemoteState::Connecting;
        self.remote_error = None;
        if let Some(panel) = &mut self.remote_panel {
            panel.submitting = true;
            panel.error = None;
        }
        let host = panel.host;
        let code = panel.code;
        let name = panel.name;
        let pairing = cx
            .background_executor()
            .spawn(async move { remote::pair(&host, port, &code, &name) });
        cx.spawn(async move |this, cx| {
            let result = pairing.await;
            let _ = this.update(cx, |this, cx| {
                if this.remote_generation != generation || this.remote_panel.is_none() {
                    return;
                }
                match result {
                    Ok((credentials, session)) => match credentials_file.save(&credentials) {
                        Ok(()) => {
                            this.remote_credentials = Some(credentials);
                            this.install_remote_session(session, generation, cx);
                        }
                        Err(error) => {
                            this.remote_state = RemoteState::Offline;
                            this.remote_error = Some(error.to_string());
                            if let Some(panel) = &mut this.remote_panel {
                                panel.submitting = false;
                                panel.error = this.remote_error.clone();
                            }
                        }
                    },
                    Err(error) => {
                        this.remote_state = if this.remote_credentials.is_some() {
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

    fn forget_remote_machine(&mut self, cx: &mut Context<Self>) {
        let result = self
            .remote_credentials_file
            .as_ref()
            .ok_or_else(|| "The remote credentials path is unavailable.".to_owned())
            .and_then(|file| file.clear().map_err(|error| error.to_string()));
        if let Err(error) = result {
            if let Some(panel) = &mut self.remote_panel {
                panel.error = Some(error);
            }
            cx.notify();
            return;
        }
        if self.active_endpoint == ChatEndpoint::Remote {
            let local_chat = self.inactive_model.selected_chat.clone().or_else(|| {
                self.inactive_model
                    .chats
                    .first()
                    .map(|chat| chat.id.clone())
            });
            if let Some(chat_id) = local_chat {
                self.select_endpoint_chat(ChatEndpoint::Local, chat_id, cx);
            } else {
                std::mem::swap(&mut self.model, &mut self.inactive_model);
                std::mem::swap(
                    &mut self.collapsed_folders,
                    &mut self.inactive_collapsed_folders,
                );
                self.active_endpoint = ChatEndpoint::Local;
                self.transcript_snapshot = TranscriptSnapshot::default();
                self.transcript.reset(0);
                self.set_composer_text(String::new(), cx);
            }
        }
        self.remote_generation = self.remote_generation.saturating_add(1);
        self.remote_credentials = None;
        self.remote_daemon = None;
        self.remote_bridge = None;
        self.inactive_model = AppModel {
            draft_revision: -1,
            ..Default::default()
        };
        self.remote_state = RemoteState::Unconfigured;
        self.remote_error = None;
        self.remote_reconnect_attempt = 0;
        if let Some(panel) = &mut self.remote_panel {
            panel.submitting = false;
            panel.error = None;
        }
        cx.notify();
    }

    fn close_share(&mut self, cx: &mut Context<Self>) {
        self.share_panel = None;
        cx.notify();
    }

    fn request_pairing_code(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.share_panel else {
            return;
        };
        if panel.loading {
            return;
        }
        panel.loading = true;
        panel.error = None;
        match self.daemon.as_ref() {
            Some(daemon) => {
                if let Err(error) = daemon.peer_pairing() {
                    panel.loading = false;
                    panel.error = Some(error);
                }
            }
            None => {
                panel.loading = false;
                panel.error = Some("xd is not connected to a daemon.".into());
            }
        }
        cx.notify();
    }

    fn close_devices(&mut self, cx: &mut Context<Self>) {
        self.devices_panel = None;
        self.device_name_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    fn request_devices(&mut self) {
        match self.daemon.as_ref() {
            Some(daemon) => {
                if let Err(error) = daemon.devices()
                    && let Some(panel) = &mut self.devices_panel
                {
                    panel.loading = false;
                    panel.error = Some(error);
                }
            }
            None => {
                if let Some(panel) = &mut self.devices_panel {
                    panel.loading = false;
                    panel.error = Some("xd is not connected to a daemon.".into());
                }
            }
        }
    }

    fn refresh_devices(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = &mut self.devices_panel {
            if panel.loading || panel.mutating.is_some() {
                return;
            }
            panel.loading = true;
            panel.error = None;
        }
        self.request_devices();
        cx.notify();
    }

    fn edit_device_name(
        &mut self,
        device_id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = &mut self.devices_panel else {
            return;
        };
        if panel.loading || panel.mutating.is_some() {
            return;
        }
        panel.editing_id = Some(device_id);
        panel.edit_name = name.clone();
        panel.revoke_confirmation = None;
        panel.error = None;
        self.device_name_input
            .update(cx, |input, cx| input.set_text(name, cx));
        let focus = self.device_name_input.read(cx).focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    fn cancel_device_name(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = &mut self.devices_panel {
            panel.editing_id = None;
            panel.edit_name.clear();
            panel.error = None;
        }
        self.device_name_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
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
        match self.daemon.as_ref() {
            Some(daemon) => match daemon.rename_device(&device_id, name) {
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
                    panel.error = Some("xd is not connected to a daemon.".into());
                }
            }
        }
        cx.notify();
    }

    fn revoke_device(&mut self, device_id: String, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.devices_panel else {
            return;
        };
        if panel.loading || panel.mutating.is_some() {
            return;
        }
        if panel.revoke_confirmation.as_ref() != Some(&device_id) {
            panel.revoke_confirmation = Some(device_id);
            panel.editing_id = None;
            panel.error = None;
            cx.notify();
            return;
        }
        match self.daemon.as_ref() {
            Some(daemon) => match daemon.revoke_device(&device_id) {
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
                    panel.error = Some("xd is not connected to a daemon.".into());
                }
            }
        }
        cx.notify();
    }

    fn choose_secret(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.secrets_panel else {
            return;
        };
        if panel.loading || panel.submitting {
            return;
        }
        panel.name = name.clone();
        panel.value.clear();
        panel.error = None;
        self.secret_name_input
            .update(cx, |input, cx| input.set_text(name, cx));
        self.secret_value_input
            .update(cx, |input, cx| input.set_text("", cx));
        let focus = self.secret_value_input.read(cx).focus_handle(cx);
        window.focus(&focus);
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
        match self.secrets_daemon(panel.folder_id.as_deref()).cloned() {
            Some(daemon) => match daemon.set_agent_secrets(panel.folder_id.as_deref(), &entries) {
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
                    current.error = Some("xd is not connected to a daemon.".into());
                }
            }
        }
        cx.notify();
    }

    fn remove_secret(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(panel) = self.secrets_panel.clone() else {
            return;
        };
        if panel.loading || panel.submitting {
            return;
        }
        let entries = panel
            .names
            .iter()
            .filter(|existing| **existing != name)
            .map(|existing| (existing.clone(), None))
            .collect::<Vec<_>>();
        match self.secrets_daemon(panel.folder_id.as_deref()).cloned() {
            Some(daemon) => match daemon.set_agent_secrets(panel.folder_id.as_deref(), &entries) {
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
                    current.error = Some("xd is not connected to a daemon.".into());
                }
            }
        }
        cx.notify();
    }

    fn auth_action(&mut self, provider: &str, state: &str, cx: &mut Context<Self>) {
        let Some(operation) = auth_operation(state) else {
            return;
        };
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.agent_auth_action(operation, provider, None)
        {
            self.model.connection_error = Some(error);
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
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) =
                daemon.agent_auth_action("agent-auth-input", &provider, Some(&input))
        {
            self.model.connection_error = Some(error);
            return;
        }
        self.auth_input_text.clear();
        self.auth_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    fn toggle_voice(&mut self, cx: &mut Context<Self>) {
        match self.voice_input.state {
            VoiceState::Idle | VoiceState::Failed(_) => self.check_voice_model(cx),
            VoiceState::NeedsModel => self.download_voice_model(cx),
            VoiceState::Recording => {
                if let Some(recorder) = &self.voice_input.recorder {
                    recorder.stop();
                    self.voice_input.state = VoiceState::Transcribing;
                    cx.notify();
                }
            }
            VoiceState::Checking | VoiceState::Downloading(_) | VoiceState::Transcribing => {
                self.cancel_voice(true, cx)
            }
        }
    }

    fn check_voice_model(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(daemon) = self.active_daemon().cloned() else {
            self.model.connection_error = Some("xd is not connected to a daemon.".into());
            return;
        };
        if let Err(error) = daemon.voice_model(&chat_id) {
            self.model.connection_error = Some(error);
            return;
        }
        self.voice_input = VoiceInput {
            state: VoiceState::Checking,
            chat_id,
            token: voice_request_token(),
            base_text: self.composer.clone(),
            partial: String::new(),
            recorder: None,
        };
        cx.notify();
    }

    fn download_voice_model(&mut self, cx: &mut Context<Self>) {
        let VoiceInput { chat_id, token, .. } = &self.voice_input;
        let Some(daemon) = self.active_daemon().cloned() else {
            return;
        };
        if let Err(error) = daemon.voice_action("voice-model-download", chat_id, token, None) {
            self.fail_voice(error, cx);
            return;
        }
        self.voice_input.state = VoiceState::Downloading(-1);
        cx.notify();
    }

    fn start_voice_recording(&mut self, cx: &mut Context<Self>) {
        let chat_id = self.voice_input.chat_id.clone();
        let token = self.voice_input.token.clone();
        let Some(daemon) = self.active_daemon().cloned() else {
            self.fail_voice("xd is not connected to a daemon.".into(), cx);
            return;
        };
        if let Err(error) = daemon.voice_action("voice-stream-start", &chat_id, &token, None) {
            self.fail_voice(error, cx);
            return;
        }
        let (recorder, events) = match VoiceRecorder::start() {
            Ok(recording) => recording,
            Err(error) => {
                let _ = daemon.voice_action("voice-cancel", &chat_id, &token, None);
                self.fail_voice(error, cx);
                return;
            }
        };
        self.voice_input.recorder = Some(recorder);
        self.voice_input.partial.clear();
        self.voice_input.state = VoiceState::Recording;
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| this.handle_capture_event(&token, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn handle_capture_event(&mut self, token: &str, event: CaptureEvent, cx: &mut Context<Self>) {
        if self.voice_input.token != token {
            return;
        }
        let chat_id = self.voice_input.chat_id.clone();
        let Some(daemon) = self.active_daemon().cloned() else {
            self.fail_voice("xd is not connected to a daemon.".into(), cx);
            return;
        };
        match event {
            CaptureEvent::Chunk(audio)
                if matches!(
                    self.voice_input.state,
                    VoiceState::Recording | VoiceState::Transcribing
                ) =>
            {
                if let Err(error) =
                    daemon.voice_action("voice-stream-chunk", &chat_id, token, Some(&audio))
                {
                    self.fail_voice(error, cx);
                }
            }
            CaptureEvent::Finished(audio) => {
                self.voice_input.recorder = None;
                self.voice_input.state = VoiceState::Transcribing;
                if let Err(error) =
                    daemon.voice_action("voice-stream-finish", &chat_id, token, Some(&audio))
                {
                    self.fail_voice(error, cx);
                }
            }
            CaptureEvent::Failed(error) => {
                let _ = daemon.voice_action("voice-cancel", &chat_id, token, None);
                self.fail_voice(error, cx);
            }
            CaptureEvent::Chunk(_) => {}
        }
        cx.notify();
    }

    fn handle_voice_event(&mut self, body: &Value, cx: &mut Context<Self>) {
        if body.get("request").and_then(Value::as_str) != Some(self.voice_input.token.as_str()) {
            return;
        }
        match body.get("state").and_then(Value::as_str) {
            Some("downloading") => {
                let progress = body.get("progress").and_then(Value::as_i64).unwrap_or(-1);
                self.voice_input.state = VoiceState::Downloading(progress);
            }
            Some("ready") => self.start_voice_recording(cx),
            Some("partial") => {
                let text = body
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.voice_input.partial = text.clone();
                let composer = merge_dictation(&self.voice_input.base_text, &text);
                self.apply_voice_text(composer, cx);
            }
            Some("transcribed") => {
                let text = body
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let composer = merge_dictation(&self.voice_input.base_text, &text);
                self.voice_input = VoiceInput::default();
                self.apply_voice_text(composer, cx);
            }
            Some("cancelled") => self.cancel_voice(true, cx),
            Some("error") => self.fail_voice(
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Voice recognition failed.")
                    .to_owned(),
                cx,
            ),
            _ => {}
        }
        cx.notify();
    }

    fn apply_voice_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.voice_applying_text = true;
        self.set_composer_text(text, cx);
        self.voice_applying_text = false;
        self.draft_dirty = true;
        self.schedule_draft_sync(cx);
    }

    fn fail_voice(&mut self, error: String, cx: &mut Context<Self>) {
        let expected = merge_dictation(&self.voice_input.base_text, &self.voice_input.partial);
        let base = self.voice_input.base_text.clone();
        if let Some(recorder) = &self.voice_input.recorder {
            recorder.cancel();
        }
        if self.composer == expected {
            self.apply_voice_text(base, cx);
        }
        self.voice_input = VoiceInput {
            state: VoiceState::Failed(error),
            ..VoiceInput::default()
        };
        cx.notify();
    }

    fn cancel_voice(&mut self, restore: bool, cx: &mut Context<Self>) {
        if matches!(self.voice_input.state, VoiceState::Idle) {
            return;
        }
        let expected = merge_dictation(&self.voice_input.base_text, &self.voice_input.partial);
        let base = self.voice_input.base_text.clone();
        if let Some(recorder) = &self.voice_input.recorder {
            recorder.cancel();
        }
        if let Some(daemon) = self.active_daemon().cloned()
            && !self.voice_input.token.is_empty()
        {
            let _ = daemon.voice_action(
                "voice-cancel",
                &self.voice_input.chat_id,
                &self.voice_input.token,
                None,
            );
        }
        self.voice_input = VoiceInput::default();
        if restore && self.composer == expected {
            self.apply_voice_text(base, cx);
        }
        cx.notify();
    }

    fn set_accent(&mut self, accent: AccentPreset, cx: &mut Context<Self>) {
        if self.settings.accent == accent {
            return;
        }
        self.settings.accent = accent;
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn toggle_notifications(&mut self, cx: &mut Context<Self>) {
        self.settings.notifications = !self.settings.notifications;
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn toggle_speech(&mut self, cx: &mut Context<Self>) {
        self.settings.speech = !self.settings.speech;
        if !self.settings.speech {
            self.pending_speech = None;
            self.speech_output.stop();
        }
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn toggle_diff_panel(&mut self, cx: &mut Context<Self>) {
        if self.diff_panel.is_some() {
            self.diff_generation = self.diff_generation.saturating_add(1);
            self.diff_panel = None;
            if self
                .pane_resize
                .is_some_and(|resize| resize.kind == PaneResizeKind::Diff)
            {
                self.pane_resize = None;
            }
        } else {
            self.diff_panel = Some(DiffPanel::default());
            self.refresh_diff(cx);
        }
        self.remember_panes();
        cx.notify();
    }

    fn toggle_terminal_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_panel.is_some() {
            self.terminal_panel = None;
            if self
                .pane_resize
                .is_some_and(|resize| resize.kind == PaneResizeKind::Terminal)
            {
                self.pane_resize = None;
            }
        } else if let Some(chat_id) = self.model.selected_chat.clone() {
            self.terminal_panel = Some(Self::new_terminal_panel(chat_id));
            self.terminal_cursor_visible = true;
            let focus = self.terminal_input.read(cx).focus_handle(cx);
            window.focus(&focus);
        }
        self.remember_panes();
        cx.notify();
    }

    fn new_terminal_panel(chat_id: String) -> TerminalPanel {
        TerminalPanel {
            chat_id,
            sessions: Vec::new(),
            selected: None,
            viewport: None,
            opening: false,
            loading: true,
            error: None,
        }
    }

    fn current_pane_key(&self) -> Option<String> {
        let chat_id = self.model.selected_chat.as_deref()?;
        let remote = self
            .remote_credentials
            .as_ref()
            .map(|credentials| (credentials.host.as_str(), credentials.port));
        Some(pane_state_key(self.active_endpoint, remote, chat_id))
    }

    fn remember_panes(&mut self) {
        let Some(key) = self.current_pane_key() else {
            return;
        };
        let state = pane_state_mask(self.diff_panel.is_some(), self.terminal_panel.is_some());
        self.settings.pane_states.insert(key, state);
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
    }

    fn restore_panes(&mut self, cx: &mut Context<Self>) {
        self.diff_panel = None;
        self.terminal_panel = None;
        let Some(key) = self.current_pane_key() else {
            return;
        };
        let state = self.settings.pane_states.get(&key).copied().unwrap_or(0);
        if state & PANE_DIFF != 0 {
            self.diff_panel = Some(DiffPanel::default());
            self.refresh_diff(cx);
        }
        if state & PANE_TERMINAL != 0
            && let Some(chat_id) = self.model.selected_chat.clone()
        {
            self.terminal_panel = Some(Self::new_terminal_panel(chat_id));
        }
    }

    fn begin_pane_resize(
        &mut self,
        kind: PaneResizeKind,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let initial_size = match kind {
            PaneResizeKind::Sidebar => self.settings.sidebar_width,
            PaneResizeKind::Diff => self.settings.diff_width,
            PaneResizeKind::Terminal => self.settings.terminal_height,
        } as f32;
        self.pane_resize = Some(PaneResize {
            kind,
            origin: event.position,
            initial_size,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn update_pane_resize(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(resize) = self.pane_resize else {
            return;
        };
        let delta = Point {
            x: f32::from(event.position.x - resize.origin.x),
            y: f32::from(event.position.y - resize.origin.y),
        };
        let size = resized_pane_size(resize.kind, resize.initial_size, delta);
        match resize.kind {
            PaneResizeKind::Sidebar => self.settings.sidebar_width = size,
            PaneResizeKind::Diff => self.settings.diff_width = size,
            PaneResizeKind::Terminal => self.settings.terminal_height = size,
        }
        cx.notify();
    }

    fn finish_pane_resize(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.pane_resize.take().is_some()
            && let Err(error) = self.settings.save()
        {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn resize_terminal_viewport(&mut self, columns: usize, rows: usize, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        let geometry = (columns, rows);
        if panel.viewport == Some(geometry) {
            return;
        }
        panel.viewport = Some(geometry);
        if let Some(session) = panel.selected_mut()
            && session.screen.geometry() != geometry
        {
            session.screen.resize(columns, rows);
        }
        let terminal_id = panel.selected.clone();
        let should_open = panel.sessions.is_empty() && panel.loading && !panel.opening;
        let chat_id = panel.chat_id.clone();
        if should_open {
            panel.opening = true;
        }

        let result = if let Some(daemon) = self.active_daemon().cloned() {
            if let Some(terminal_id) = terminal_id {
                daemon.terminal_resize(&terminal_id, columns, rows)
            } else if should_open {
                daemon.terminal_open(&chat_id, columns, rows, true)
            } else {
                Ok(())
            }
        } else {
            Err("xd is not connected to a daemon.".into())
        };
        if let Err(error) = result
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.loading = false;
            panel.opening = false;
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn send_terminal_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.terminal_cursor_visible = true;
        let Some(terminal_id) = self
            .terminal_panel
            .as_ref()
            .and_then(|panel| panel.selected.clone())
        else {
            return;
        };
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.terminal_input(&terminal_id, bytes)
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn kill_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(terminal_id) = self
            .terminal_panel
            .as_ref()
            .and_then(|panel| panel.selected.clone())
        else {
            return;
        };
        self.kill_terminal_id(terminal_id, cx);
    }

    fn kill_terminal_id(&mut self, terminal_id: String, cx: &mut Context<Self>) {
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.terminal_kill(&terminal_id)
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn new_terminal_session(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        if panel.opening {
            return;
        }
        let (columns, rows) = panel.viewport.unwrap_or((120, 32));
        let chat_id = panel.chat_id.clone();
        panel.opening = true;
        panel.error = None;
        let result = self
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.terminal_open(&chat_id, columns, rows, false));
        if let Err(error) = result
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.opening = false;
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn select_terminal(&mut self, terminal_id: String, cx: &mut Context<Self>) {
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
        let viewport = panel.viewport;
        if let Some((columns, rows)) = viewport
            && let Some(session) = panel.selected_mut()
        {
            session.screen.resize(columns, rows);
            if let Some(daemon) = self.active_daemon().cloned()
                && let Err(error) = daemon.terminal_resize(&terminal_id, columns, rows)
                && let Some(panel) = &mut self.terminal_panel
            {
                panel.error = Some(error);
            }
        }
        cx.notify();
    }

    fn apply_terminal_list(&mut self, value: &Value) {
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        panel.loading = false;
        panel.opening = false;
        let previous = panel.selected.clone();
        let sessions = value
            .get("terminals")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Self::terminal_tab_from_snapshot)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        panel.sessions = sessions;
        panel.selected = previous
            .filter(|selected| panel.sessions.iter().any(|session| &session.id == selected))
            .or_else(|| panel.sessions.first().map(|session| session.id.clone()));
    }

    fn terminal_tab_from_snapshot(terminal: &Value) -> Option<TerminalTab> {
        let id = terminal.get("id")?.as_str()?.to_owned();
        let title = terminal
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Terminal")
            .to_owned();
        let columns = terminal
            .get("columns")
            .and_then(Value::as_u64)
            .unwrap_or(120) as usize;
        let rows = terminal.get("rows").and_then(Value::as_u64).unwrap_or(32) as usize;
        let mut screen = TerminalScreen::new(columns, rows);
        if let Some(replay) = terminal.get("replay").and_then(Value::as_array) {
            for frame in replay {
                if let Some(data) = frame
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(|data| STANDARD.decode(data).ok())
                {
                    screen.feed(&data);
                } else if let (Some(columns), Some(rows)) = (
                    frame.get("columns").and_then(Value::as_u64),
                    frame.get("rows").and_then(Value::as_u64),
                ) {
                    screen.resize(columns as usize, rows as usize);
                }
            }
        }
        Some(TerminalTab { id, title, screen })
    }

    fn set_diff_mode(&mut self, branch: bool, cx: &mut Context<Self>) {
        if self
            .diff_panel
            .as_ref()
            .is_some_and(|diff| !diff.files_mode && diff.branch == branch)
        {
            return;
        }
        if let Some(diff) = &mut self.diff_panel {
            diff.branch = branch;
            diff.files_mode = false;
        }
        self.refresh_diff(cx);
    }

    fn set_files_mode(&mut self, cx: &mut Context<Self>) {
        if self.diff_panel.as_ref().is_some_and(|diff| diff.files_mode) {
            return;
        }
        if let Some(diff) = &mut self.diff_panel {
            diff.files_mode = true;
            diff.file_preview = None;
            diff.browse_path.clear();
            diff.browse_entries.clear();
            diff.action_error = None;
        }
        self.refresh_diff(cx);
    }

    fn load_browse_directory(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self
            .diff_panel
            .as_ref()
            .and_then(|panel| panel.file_preview.as_ref())
            .is_some_and(|preview| preview.content != preview.original)
        {
            if let Some(diff) = &mut self.diff_panel {
                diff.error = Some("Save or discard the file changes before navigating.".into());
            }
            cx.notify();
            return;
        }
        self.diff_generation = self.diff_generation.saturating_add(1);
        let generation = self.diff_generation;
        let path_changed = self
            .diff_panel
            .as_ref()
            .is_some_and(|diff| diff.browse_path != path);
        if let Some(diff) = &mut self.diff_panel {
            if !diff.files_mode {
                return;
            }
            diff.loading = true;
            diff.file_loading = false;
            diff.file_preview = None;
            diff.error = None;
        } else {
            return;
        }
        if path_changed {
            self.repo_file_filter.clear();
            self.repo_file_filter_input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        let result = self
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.file_browse_list(&chat_id, &path, generation));
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.loading = false;
            diff.error = Some(error);
        }
        cx.notify();
    }

    fn browse_up(&mut self, cx: &mut Context<Self>) {
        let Some(diff) = self.diff_panel.as_ref() else {
            return;
        };
        if diff.file_preview.is_some() {
            self.close_file_preview(cx);
            return;
        }
        if diff.browse_path.is_empty() {
            return;
        }
        self.load_browse_directory(parent_browse_path(&diff.browse_path), cx);
    }

    fn activate_browse_entry(&mut self, entry: BrowseEntry, cx: &mut Context<Self>) {
        let base = self
            .diff_panel
            .as_ref()
            .map(|diff| diff.browse_path.as_str())
            .unwrap_or_default();
        let path = join_browse_path(base, &entry.name);
        if entry.directory {
            self.load_browse_directory(path, cx);
        } else {
            self.read_browse_file(path, cx);
        }
    }

    fn read_browse_file(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let generation = self.diff_generation;
        if let Some(diff) = &mut self.diff_panel {
            if !diff.files_mode || diff.file_loading {
                return;
            }
            diff.file_loading = true;
            diff.error = None;
        } else {
            return;
        }
        let result = self
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.file_browse_read(&chat_id, &path, generation));
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.file_loading = false;
            diff.error = Some(error);
        }
        cx.notify();
    }

    fn close_file_preview(&mut self, cx: &mut Context<Self>) {
        if self
            .diff_panel
            .as_ref()
            .and_then(|panel| panel.file_preview.as_ref())
            .is_some_and(|preview| preview.content != preview.original)
        {
            if let Some(diff) = &mut self.diff_panel {
                diff.error = Some("Save or discard the file changes before closing it.".into());
            }
            cx.notify();
            return;
        }
        if let Some(diff) = &mut self.diff_panel {
            diff.file_preview = None;
            diff.file_loading = false;
            diff.error = None;
        }
        cx.notify();
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
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| {
                daemon.file_browse_write(&chat_id, &path, &original, &content, generation)
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

    fn discard_repository_changes(&mut self, cx: &mut Context<Self>) {
        let Some(original) = self
            .diff_panel
            .as_mut()
            .and_then(|panel| panel.file_preview.as_mut())
            .map(|preview| {
                preview.content = preview.original.clone();
                preview.saving = false;
                preview.original.clone()
            })
        else {
            return;
        };
        if let Some(diff) = &mut self.diff_panel {
            diff.error = None;
        }
        self.file_editor
            .update(cx, |editor, cx| editor.set_text(original, cx));
        cx.notify();
    }

    fn refresh_browse_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .diff_panel
            .as_ref()
            .and_then(|panel| panel.file_preview.as_ref())
            .map(|preview| {
                if preview.content == preview.original {
                    Ok(preview.path.clone())
                } else {
                    Err(())
                }
            })
        else {
            return;
        };
        match path {
            Ok(path) => self.read_browse_file(path, cx),
            Err(()) => {
                if let Some(diff) = &mut self.diff_panel {
                    diff.error = Some("Discard local file changes before refreshing.".into());
                }
                cx.notify();
            }
        }
    }

    fn refresh_git_status(&mut self) {
        let (Some(chat_id), Some(daemon)) = (
            self.model.selected_chat.clone(),
            self.active_daemon().cloned(),
        ) else {
            return;
        };
        if let Some(diff) = &mut self.diff_panel {
            diff.status_loading = true;
        }
        if let Err(error) = daemon.git_status(&chat_id, self.diff_generation)
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
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.git_commit(&chat_id, &message, generation));
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.action = None;
            diff.action_error = Some(error);
        }
        cx.notify();
    }

    fn draft_commit_message(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let can_draft = self.diff_panel.as_ref().is_some_and(|diff| {
            diff.action.is_none()
                && diff
                    .status
                    .as_ref()
                    .is_some_and(|status| !status.clean && status.conflicted == 0)
        });
        if !can_draft {
            return;
        }
        let generation = self.diff_generation;
        let request = format!("gpui-{generation}");
        if let Some(diff) = &mut self.diff_panel {
            diff.action = Some("Writing commit message…".into());
            diff.action_error = None;
        }
        let result = self
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| {
                daemon.git_draft(
                    &chat_id,
                    "commit",
                    &request,
                    self.settings.git_writer.backend(),
                    self.settings.git_writer_model.as_deref(),
                    generation,
                )
            });
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.action = None;
            diff.action_error = Some(error);
        }
        cx.notify();
    }

    fn draft_pull_request(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let can_draft = self.diff_panel.as_ref().is_some_and(|diff| {
            diff.action.is_none()
                && diff.pr_url.is_none()
                && diff
                    .status
                    .as_ref()
                    .is_some_and(GitStatus::can_open_pull_request)
        });
        if !can_draft {
            return;
        }
        let generation = self.diff_generation;
        let request = format!("gpui-pr-{generation}");
        if let Some(diff) = &mut self.diff_panel {
            diff.action = Some("Writing pull request…".into());
            diff.action_error = None;
            diff.pr_title = None;
            diff.pr_body.clear();
        }
        let result = self
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| {
                daemon.git_draft(
                    &chat_id,
                    "pull-request",
                    &request,
                    self.settings.git_writer.backend(),
                    self.settings.git_writer_model.as_deref(),
                    generation,
                )
            });
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.action = None;
            diff.action_error = Some(error);
        }
        cx.notify();
    }

    fn create_pull_request(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some((title, body)) = self.diff_panel.as_ref().and_then(|diff| {
            if diff.action.is_none() && diff.pr_url.is_none() {
                diff.pr_title
                    .clone()
                    .map(|title| (title, diff.pr_body.clone()))
            } else {
                None
            }
        }) else {
            return;
        };
        let generation = self.diff_generation;
        if let Some(diff) = &mut self.diff_panel {
            diff.action = Some("Creating pull request…".into());
            diff.action_error = None;
        }
        let result = self
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.git_create_pull_request(&chat_id, &title, &body, generation));
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.action = None;
            diff.action_error = Some(error);
        }
        cx.notify();
    }

    fn discard_pull_request_draft(&mut self, cx: &mut Context<Self>) {
        if let Some(diff) = &mut self.diff_panel {
            diff.pr_title = None;
            diff.pr_body.clear();
        }
        cx.notify();
    }

    fn push_changes(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let can_push = self.diff_panel.as_ref().is_some_and(|diff| {
            diff.action.is_none()
                && diff.status.as_ref().is_some_and(|status| {
                    !status.branch.is_empty()
                        && status.branch != "(detached)"
                        && status.branch != "(initial)"
                })
        });
        if !can_push {
            return;
        }
        let generation = self.diff_generation;
        if let Some(diff) = &mut self.diff_panel {
            diff.action = Some("Pushing branch…".into());
            diff.action_error = None;
        }
        let result = self
            .active_daemon()
            .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.git_push(&chat_id, generation));
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
        match self.active_daemon().cloned() {
            Some(daemon) => {
                let content = if files_mode {
                    let path = self
                        .diff_panel
                        .as_ref()
                        .map(|diff| diff.browse_path.as_str())
                        .unwrap_or_default();
                    daemon.file_browse_list(&chat_id, path, generation)
                } else {
                    daemon.diff_read(
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
                    if let Err(error) = daemon.git_status(&chat_id, generation)
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
                    diff.error = Some("xd is not connected to a daemon.".into());
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
                if let Some(diff) = &mut this.diff_panel {
                    diff.loading = false;
                    match result {
                        Ok((files, truncated)) => {
                            this.collapsed_diff_files =
                                files.iter().map(|file| file.path.clone()).collect();
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

    fn toggle_diff_file(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.collapsed_diff_files.remove(&path) {
            self.collapsed_diff_files.insert(path.clone());
            if let Some(file) = self
                .diff_panel
                .as_mut()
                .and_then(|diff| diff.files.iter_mut().find(|file| file.path == path))
                .filter(|file| file.lazy_read.is_some())
            {
                file.lines.clear();
                file.additions = 0;
                file.deletions = 0;
                file.loaded = false;
                file.error = None;
            }
            cx.notify();
            return;
        }
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
                .active_daemon()
                .ok_or_else(|| "xd is not connected to a daemon.".to_owned())
                .and_then(|daemon| {
                    daemon.diff_read(
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
        cx: &mut Context<Self>,
    ) {
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
        cx.notify();
    }

    fn open_sidebar_context_menu(
        &mut self,
        target: Option<SidebarTarget>,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.cancel_sidebar_edit(cx);
        if self.pending_sidebar_delete.as_ref() != target.as_ref() {
            self.pending_sidebar_delete = None;
            self.sidebar_delete_submitting = false;
        }
        if self.sidebar_move.as_ref() != target.as_ref() {
            self.sidebar_move = None;
            self.sidebar_move_submitting = false;
            self.sidebar_move_destination = None;
        }
        self.sidebar_context_menu = Some(SidebarContextMenu { target, position });
        cx.notify();
    }

    fn close_sidebar_context_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.sidebar_context_menu.take() {
            if self.pending_sidebar_delete.as_ref() == menu.target.as_ref() {
                self.pending_sidebar_delete = None;
                self.sidebar_delete_submitting = false;
            }
            cx.notify();
        }
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
        let result = self.active_daemon().map(|daemon| match &edit.target {
            SidebarTarget::Folder(folder_id) => daemon.rename_folder(folder_id, text),
            SidebarTarget::Chat(chat_id) => daemon.rename_chat(chat_id, text),
        });
        match result {
            Some(Ok(())) => {
                if let Some(active) = &mut self.sidebar_edit
                    && active.target == edit.target
                {
                    active.text = text.to_owned();
                    active.submitting = true;
                }
            }
            Some(Err(error)) => self.model.connection_error = Some(error),
            None => self.model.connection_error = Some("xd is not connected to a daemon.".into()),
        }
        cx.notify();
    }

    fn cancel_sidebar_edit(&mut self, cx: &mut Context<Self>) {
        self.sidebar_edit = None;
        self.sidebar_edit_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn delete_sidebar_item(&mut self, target: SidebarTarget, cx: &mut Context<Self>) {
        if self.sidebar_delete_submitting {
            return;
        }
        if self.pending_sidebar_delete.as_ref() != Some(&target) {
            self.cancel_sidebar_edit(cx);
            self.sidebar_move = None;
            self.sidebar_move_submitting = false;
            self.sidebar_move_destination = None;
            self.pending_sidebar_delete = Some(target);
            cx.notify();
            return;
        }
        let result = self.active_daemon().map(|daemon| match &target {
            SidebarTarget::Folder(folder_id) => daemon.trash_folder(folder_id),
            SidebarTarget::Chat(chat_id) => daemon.delete_chat(chat_id),
        });
        match result {
            Some(Ok(())) => self.sidebar_delete_submitting = true,
            Some(Err(error)) => self.model.connection_error = Some(error),
            None => self.model.connection_error = Some("xd is not connected to a daemon.".into()),
        }
        cx.notify();
    }

    fn toggle_sidebar_move(&mut self, target: SidebarTarget, cx: &mut Context<Self>) {
        if self.sidebar_move_submitting {
            return;
        }
        self.cancel_sidebar_edit(cx);
        self.pending_sidebar_delete = None;
        self.sidebar_delete_submitting = false;
        if self.sidebar_move.as_ref() == Some(&target) {
            self.sidebar_move = None;
            self.sidebar_move_destination = None;
        } else {
            self.sidebar_move = Some(target);
            self.sidebar_move_destination = None;
        }
        cx.notify();
    }

    fn move_sidebar_item(
        &mut self,
        target: SidebarTarget,
        destination: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_move_submitting || self.sidebar_move.as_ref() != Some(&target) {
            return;
        }
        self.submit_sidebar_move(target, destination, cx);
    }

    fn drop_sidebar_item(
        &mut self,
        target: SidebarTarget,
        destination: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_move_submitting
            || !sidebar_drop_allowed(&self.model, &target, destination.as_deref())
        {
            return;
        }
        self.cancel_sidebar_edit(cx);
        self.sidebar_context_menu = None;
        self.pending_sidebar_delete = None;
        self.sidebar_delete_submitting = false;
        self.sidebar_move = Some(target.clone());
        self.sidebar_move_destination = None;
        self.submit_sidebar_move(target, destination, cx);
    }

    fn submit_sidebar_move(
        &mut self,
        target: SidebarTarget,
        destination: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let result = self.active_daemon().map(|daemon| match &target {
            SidebarTarget::Folder(folder_id) => {
                daemon.move_folder(folder_id, destination.as_deref())
            }
            SidebarTarget::Chat(chat_id) => destination
                .as_deref()
                .ok_or_else(|| "A chat needs a destination workspace.".to_owned())
                .and_then(|folder_id| daemon.move_chat(chat_id, folder_id)),
        });
        match result {
            Some(Ok(())) => {
                self.sidebar_move_submitting = true;
                self.sidebar_move_destination = Some(destination);
            }
            Some(Err(error)) => self.model.connection_error = Some(error),
            None => self.model.connection_error = Some("xd is not connected to a daemon.".into()),
        }
        cx.notify();
    }

    fn toggle_new_worktree(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.has_messages || self.model.working {
            return;
        }
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.set_new_worktree(&chat_id, !self.model.new_worktree)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn toggle_composer_menu(&mut self, menu: ComposerMenu, cx: &mut Context<Self>) {
        let opening = self.composer_menu != Some(menu);
        self.composer_menu = if !opening { None } else { Some(menu) };
        if opening && menu == ComposerMenu::Model {
            self.model_search.clear();
            self.model_filter = Some(self.model.backend.clone());
            self.model_search_input
                .update(cx, |input, cx| input.set_text("", cx));
        }
        cx.notify();
    }

    fn set_model_filter(&mut self, backend: Option<String>, cx: &mut Context<Self>) {
        if self.composer_menu != Some(ComposerMenu::Model) {
            return;
        }
        self.model_filter = backend;
        cx.notify();
    }

    fn toggle_model_favorite(&mut self, backend: String, model: String, cx: &mut Context<Self>) {
        let valid = self.model.agent_backends.iter().any(|candidate| {
            candidate.id == backend
                && candidate
                    .models
                    .iter()
                    .any(|candidate| candidate.id == model)
        });
        if !valid {
            return;
        }
        let key = format!("{backend}/{model}");
        if self
            .settings
            .favorite_models
            .iter()
            .any(|value| value == &key)
        {
            self.settings.favorite_models.retain(|value| value != &key);
        } else {
            self.settings.favorite_models.push(key);
            self.settings.favorite_models.sort_unstable();
            self.settings.favorite_models.dedup();
        }
        if let Err(error) = self.settings.save() {
            self.model.connection_error = Some(error);
        }
        cx.notify();
    }

    fn select_model_shortcut(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.composer_menu != Some(ComposerMenu::Model) {
            return;
        }
        let Some((backend, model)) = filtered_models(
            &self.model.agent_backends,
            &self.settings.favorite_models,
            self.model_filter.as_deref(),
            &self.model_search,
        )
        .get(index)
        .cloned() else {
            return;
        };
        self.apply_composer_choice(ComposerChoice::Model { backend, model }, cx);
    }

    fn apply_composer_choice(&mut self, choice: ComposerChoice, cx: &mut Context<Self>) {
        self.composer_menu = None;
        let Some(chat_id) = self.model.selected_chat.clone() else {
            cx.notify();
            return;
        };
        if self.model.working {
            cx.notify();
            return;
        }
        let result = match choice {
            ComposerChoice::Model { backend, model }
                if self.model.agent_backends.iter().any(|candidate| {
                    candidate.id == backend
                        && candidate
                            .models
                            .iter()
                            .any(|candidate| candidate.id == model)
                }) =>
            {
                self.active_daemon()
                    .map(|daemon| daemon.set_model(&chat_id, &backend, &model))
            }
            ComposerChoice::Effort(effort)
                if self.model.agent_backends.iter().any(|backend| {
                    backend.id == self.model.backend
                        && backend.efforts.contains(&effort)
                        && !(self.model.claude_mode && effort == "ultra")
                }) =>
            {
                self.active_daemon()
                    .map(|daemon| daemon.set_effort(&chat_id, &effort))
            }
            ComposerChoice::Access(access)
                if matches!(access.as_str(), "read-only" | "edit" | "full") =>
            {
                self.active_daemon()
                    .map(|daemon| daemon.set_access(&chat_id, &access))
            }
            ComposerChoice::Workspace(path)
                if !self.model.has_messages
                    && self
                        .model
                        .worktrees
                        .iter()
                        .any(|worktree| worktree.path == path) =>
            {
                self.active_daemon()
                    .map(|daemon| daemon.set_workspace(&chat_id, &path))
            }
            _ => {
                cx.notify();
                return;
            }
        };
        match result {
            Some(Err(error)) => self.model.connection_error = Some(error),
            None => self.model.connection_error = Some("xd is not connected to a daemon.".into()),
            Some(Ok(())) => {}
        }
        cx.notify();
    }

    fn toggle_plan(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working {
            return;
        }
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.set_plan(&chat_id, !self.model.plan)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn toggle_fast(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working || self.model.backend != "codex" {
            return;
        }
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.set_fast(&chat_id, !self.model.fast)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn toggle_claude_mode(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working || self.model.backend != "codex" {
            return;
        }
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.set_claude_mode(&chat_id, !self.model.claude_mode)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn remove_selected_worktree(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.has_messages || self.model.working {
            return;
        }
        let Some(worktree) = self
            .model
            .worktrees
            .iter()
            .find(|worktree| worktree.current && !worktree.main)
        else {
            return;
        };
        let path = worktree.path.clone();
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.remove_worktree(&chat_id, &path)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn request_chat(&mut self, chat_id: &str) {
        if let Some(daemon) = self.active_daemon() {
            if let Err(error) = daemon.chat(chat_id) {
                self.model.connection_error = Some(error);
            }
        }
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
        if let Some(daemon) = self.active_daemon() {
            if let Err(error) = daemon.messages(chat_id, cursor) {
                self.model.connection_error = Some(error);
            } else {
                self.transcript_page_loading = true;
            }
        }
    }

    fn request_older_messages(&mut self) {
        if !self.transcript_has_older || self.transcript_page_loading || self.transcript_loading {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(first_id) = self.model.messages.first().and_then(|message| message.id) else {
            return;
        };
        self.request_message_page(&chat_id, MessageCursor::Before(first_id));
    }

    fn request_newer_messages(&mut self) {
        if !self.transcript_has_newer || self.transcript_page_loading || self.transcript_loading {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(last_id) = self.model.messages.last().and_then(|message| message.id) else {
            return;
        };
        self.request_message_page(&chat_id, MessageCursor::After(last_id));
    }

    fn request_workflow_statuses(&mut self) {
        let markers = self
            .model
            .messages
            .iter()
            .chain(self.model.live_activity.iter())
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
            .active_daemon()
            .ok_or_else(|| "The xd daemon is offline.".to_owned())
            .and_then(|daemon| daemon.workflow_status(&marker));
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
                        .chain(this.model.live_activity.iter())
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
                            .chain(this.model.live_activity.iter())
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

    fn select_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        if self.model.selected_chat.as_deref() == Some(chat_id.as_str()) {
            return;
        }
        self.sync_draft();
        self.draft_generation = self.draft_generation.saturating_add(1);
        if let Ok(mut images) = self.message_images.lock() {
            images.clear();
        }
        self.message_image_viewer = None;
        self.model.select_chat(chat_id.clone());
        self.invalidate_live_markdown_work();
        self.transcript_snapshot = TranscriptSnapshot::default();
        if self.active_endpoint == ChatEndpoint::Local {
            self.settings.last_chat = Some(chat_id.clone());
            if let Err(error) = self.settings.save() {
                self.model.connection_error = Some(error);
            }
        }
        self.composer_menu = None;
        self.pending_speech = None;
        self.speech_output.stop();
        self.cancel_voice(false, cx);
        self.diff_generation = self.diff_generation.saturating_add(1);
        self.diff_panel = None;
        self.terminal_panel = None;
        self.set_composer_text(String::new(), cx);
        self.draft_dirty = false;
        self.attachments_dirty = false;
        self.pending_send = None;
        Arc::make_mut(&mut self.workflow_statuses).clear();
        Arc::make_mut(&mut self.workflow_pending).clear();
        self.workflow_ticking.clear();
        self.clear_question(cx);
        self.cancel_queue_edit(cx);
        self.sending = false;
        self.transcript_loading = true;
        self.transcript_page_loading = false;
        self.transcript_refresh_pending = false;
        self.transcript_has_older = false;
        self.transcript_has_newer = false;
        self.transcript.reset(0);
        self.request_chat(&chat_id);
        self.request_message_page(&chat_id, MessageCursor::Tail);
        if let Some(daemon) = self.active_daemon()
            && let Err(error) = daemon.git_state(&chat_id)
        {
            self.model.connection_error = Some(error);
        }
        self.request_shortcuts();
        self.repo_file_filter.clear();
        self.repo_file_filter_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.restore_panes(cx);
        cx.notify();
    }

    fn send_composer(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        if !matches!(self.voice_input.state, VoiceState::Idle) {
            self.cancel_voice(false, cx);
        }
        let text = self.composer.trim().to_owned();
        let attachments = self.model.draft_attachments.clone();
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if text.is_empty() && attachments.is_empty() {
            return;
        }
        let Some(daemon) = self.active_daemon().cloned() else {
            self.model.connection_error = Some("xd is not connected to a daemon.".into());
            return;
        };
        if let Err(error) = daemon.send_message(
            &chat_id,
            &text,
            &attachments,
            self.settings.git_writer.backend(),
            self.settings.git_writer_model.as_deref(),
        ) {
            self.model.connection_error = Some(error);
            return;
        }

        self.sending = true;
        self.pending_send = Some(PendingSend {
            text,
            attachments,
            restore: true,
        });
        self.set_composer_text(String::new(), cx);
        self.model.draft_attachments.clear();
        self.draft_dirty = true;
        self.attachments_dirty = true;
        self.attachment_generation = self.attachment_generation.saturating_add(1);
        self.draft_generation = self.draft_generation.saturating_add(1);
        let _ = daemon.set_draft(&chat_id, "", Some(&[]), Some(self.attachment_generation));
        cx.notify();
    }

    fn restore_pending_send(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_send.take() else {
            return;
        };
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
        let Some(daemon) = self.active_daemon().cloned() else {
            self.model.connection_error = Some("xd is not connected to a daemon.".into());
            return false;
        };
        if let Err(error) = daemon.send_message(
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
        self.pending_send = Some(PendingSend {
            text: prompt,
            attachments: Vec::new(),
            restore: false,
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

    fn drop_queued(&mut self, index: usize) {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return;
        };
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.drop_queue(chat_id, index)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn begin_queue_edit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(prompt) = self.model.queue.get(index).cloned() else {
            return;
        };
        self.queue_edit = Some(QueueEdit {
            chat_id,
            index,
            original: prompt.clone(),
            text: prompt.clone(),
            submitting: None,
        });
        self.queue_edit_input
            .update(cx, |input, cx| input.set_text(prompt, cx));
        cx.notify();
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
        if let Some(daemon) = self.active_daemon().cloned() {
            if let Err(error) = daemon.edit_queue(&edit.chat_id, edit.index, &edit.original, text) {
                self.model.connection_error = Some(error);
            } else if let Some(active) = &mut self.queue_edit
                && active.chat_id == edit.chat_id
                && active.index == edit.index
                && active.original == edit.original
            {
                active.submitting = Some(text.to_owned());
            }
        }
    }

    fn cancel_queue_edit(&mut self, cx: &mut Context<Self>) {
        self.queue_edit = None;
        self.queue_edit_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn steer_queued(&mut self, index: usize) {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return;
        };
        let Some(text) = self.model.queue.get(index) else {
            return;
        };
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.steer_queue(chat_id, index, text)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cancel_turn(&mut self) {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return;
        };
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.cancel(chat_id)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn toggle_shortcut(&mut self, workspace: bool) {
        let prompt = self.composer.trim();
        if prompt.is_empty() {
            return;
        }
        let mut shortcuts = if workspace {
            self.model.workspace_shortcuts.clone()
        } else {
            self.model.global_shortcuts.clone()
        };
        if let Some(index) = shortcuts.iter().position(|shortcut| shortcut == prompt) {
            shortcuts.remove(index);
        } else {
            shortcuts.push(prompt.to_owned());
        }
        let folder_id = workspace
            .then(|| {
                self.model
                    .selected_summary()
                    .map(|chat| chat.folder.clone())
            })
            .flatten();
        if workspace && folder_id.is_none() {
            return;
        }
        if let Some(daemon) = self.active_daemon().cloned()
            && let Err(error) = daemon.set_shortcuts(folder_id.as_deref(), &shortcuts)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn make_shortcut_row(&mut self, prompt: String, cx: &mut Context<Self>) -> ShortcutRow {
        self.next_shortcut_row_id = self.next_shortcut_row_id.saturating_add(1);
        let id = self.next_shortcut_row_id;
        let input = cx.new(|cx| ComposerInput::new(cx, "Prompt sent when this button is pressed"));
        input.update(cx, |input, cx| input.set_text(prompt.clone(), cx));
        cx.subscribe(&input, move |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.shortcut_row_changed(id, text.clone(), cx),
            ComposerEvent::Submit => this.save_shortcut_panel(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        ShortcutRow { id, prompt, input }
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

    fn open_shortcut_panel(
        &mut self,
        folder_id: Option<String>,
        folder_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.settings_open = false;
        self.settings_menu = None;
        self.sidebar_context_menu = None;
        self.shortcut_panel = Some(ShortcutPanel {
            folder_id: folder_id.clone(),
            folder_name,
            rows: Vec::new(),
            loading: true,
            submitting: false,
            error: None,
        });
        match self.active_daemon().cloned() {
            Some(daemon) => {
                if let Err(error) = daemon.shortcuts(folder_id.as_deref())
                    && let Some(panel) = &mut self.shortcut_panel
                {
                    panel.loading = false;
                    panel.error = Some(error);
                }
            }
            None => {
                if let Some(panel) = &mut self.shortcut_panel {
                    panel.loading = false;
                    panel.error = Some("xd is not connected to a daemon.".into());
                }
            }
        }
        cx.notify();
    }

    fn shortcut_row_changed(&mut self, id: u64, text: String, cx: &mut Context<Self>) {
        if let Some(row) = self
            .shortcut_panel
            .as_mut()
            .and_then(|panel| panel.rows.iter_mut().find(|row| row.id == id))
        {
            row.prompt = text;
            if let Some(panel) = &mut self.shortcut_panel {
                panel.error = None;
            }
            cx.notify();
        }
    }

    fn add_shortcut_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcut_panel.as_ref().is_none_or(|panel| {
            panel.loading || panel.submitting || panel.rows.len() >= MAX_SHORTCUTS
        }) {
            return;
        }
        let row = self.make_shortcut_row(String::new(), cx);
        let focus = row.input.read(cx).focus_handle(cx);
        if let Some(panel) = &mut self.shortcut_panel {
            panel.rows.push(row);
        }
        window.focus(&focus);
        cx.notify();
    }

    fn remove_shortcut_row(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(panel) = &mut self.shortcut_panel
            && !panel.submitting
        {
            panel.rows.retain(|row| row.id != id);
            panel.error = None;
            cx.notify();
        }
    }

    fn save_shortcut_panel(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &self.shortcut_panel else {
            return;
        };
        if panel.loading || panel.submitting {
            return;
        }
        let prompts = panel
            .rows
            .iter()
            .map(|row| row.prompt.clone())
            .collect::<Vec<_>>();
        let shortcuts = match clean_shortcut_prompts(&prompts) {
            Ok(shortcuts) => shortcuts,
            Err(error) => {
                if let Some(panel) = &mut self.shortcut_panel {
                    panel.error = Some(error);
                }
                cx.notify();
                return;
            }
        };
        let folder_id = panel.folder_id.clone();
        let Some(daemon) = self.active_daemon().cloned() else {
            if let Some(panel) = &mut self.shortcut_panel {
                panel.error = Some("xd is not connected to a daemon.".into());
            }
            cx.notify();
            return;
        };
        match daemon.set_shortcuts(folder_id.as_deref(), &shortcuts) {
            Ok(()) => {
                if let Some(panel) = &mut self.shortcut_panel {
                    panel.submitting = true;
                    panel.error = None;
                }
            }
            Err(error) => {
                if let Some(panel) = &mut self.shortcut_panel {
                    panel.error = Some(error);
                }
            }
        }
        cx.notify();
    }

    fn close_shortcut_panel(&mut self, cx: &mut Context<Self>) {
        self.shortcut_panel = None;
        cx.notify();
    }

    fn set_composer_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.composer.clone_from(&text);
        self.composer_input
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    fn choose_command(&mut self, command: String, window: &mut Window, cx: &mut Context<Self>) {
        self.set_composer_text(format!("/{command} "), cx);
        let focus = self.composer_input.read(cx).focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    fn attach_images(&mut self, cx: &mut Context<Self>) {
        let available = MAX_ATTACHMENTS.saturating_sub(self.model.draft_attachments.len());
        if available == 0 || self.model.selected_chat.is_none() {
            return;
        }
        let existing_bytes = self
            .model
            .draft_attachments
            .iter()
            .map(|attachment| attachment.preview.bytes.len())
            .sum();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach PNG images".into()),
        });
        let load = cx.background_executor().spawn(async move {
            match receiver.await {
                Ok(Ok(Some(paths))) => load_png_attachments(paths, available, existing_bytes),
                Ok(Ok(None)) => Ok(Vec::new()),
                Ok(Err(error)) => Err(format!("Cannot open the image picker: {error}")),
                Err(_) => Err("The image picker closed unexpectedly.".into()),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(attachments) if !attachments.is_empty() => {
                    this.model.draft_attachments.extend(attachments);
                    this.attachments_dirty = true;
                    this.attachment_generation = this.attachment_generation.saturating_add(1);
                    this.schedule_draft_sync(cx);
                    cx.notify();
                }
                Ok(_) => {}
                Err(error) => {
                    this.model.connection_error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
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

    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.model.draft_attachments.len() {
            return;
        }
        self.model.draft_attachments.remove(index);
        self.attachments_dirty = true;
        self.attachment_generation = self.attachment_generation.saturating_add(1);
        self.schedule_draft_sync(cx);
        cx.notify();
    }

    fn composer_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.voice_applying_text && !matches!(self.voice_input.state, VoiceState::Idle) {
            self.cancel_voice(false, cx);
        }
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
        if let Some(daemon) = self.active_daemon().cloned() {
            let attachments = self
                .attachments_dirty
                .then_some(self.model.draft_attachments.as_slice());
            let attachment_generation = attachments.map(|_| self.attachment_generation);
            if let Err(error) =
                daemon.set_draft(&chat_id, &self.composer, attachments, attachment_generation)
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

    fn folder_is_descendant_of(&self, candidate_id: &str, ancestor_id: &str) -> bool {
        let mut current = Some(candidate_id);
        for _ in 0..=self.model.folders.len() {
            let Some(id) = current else {
                return false;
            };
            if id == ancestor_id {
                return true;
            }
            current = self
                .model
                .folders
                .iter()
                .find(|folder| folder.id == id)
                .and_then(|folder| folder.parent.as_deref());
        }
        true
    }

    fn folder_hidden_by_collapse(&self, folder_id: &str) -> bool {
        folder_hidden_by_collapse(&self.model.folders, &self.collapsed_folders, folder_id)
    }

    fn toggle_folder_collapsed(&mut self, folder_id: String, cx: &mut Context<Self>) {
        if !self.collapsed_folders.remove(&folder_id) {
            self.collapsed_folders.insert(folder_id);
        }
        self.persist_collapsed_folders();
        cx.notify();
    }

    fn persist_collapsed_folders(&mut self) {
        if self.active_endpoint == ChatEndpoint::Remote {
            return;
        }
        let mut collapsed = self.collapsed_folders.iter().cloned().collect::<Vec<_>>();
        collapsed.sort();
        if self.settings.collapsed_folders == collapsed {
            return;
        }
        self.settings.collapsed_folders = collapsed;
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

    fn invalidate_live_markdown_work(&mut self) {
        self.live_markdown_generation = self.live_markdown_generation.saturating_add(1);
        self.live_markdown_scheduled = None;
    }

    fn schedule_live_markdown_parse(&mut self, cx: &mut Context<Self>) {
        self.live_markdown_generation = self.live_markdown_generation.saturating_add(1);
        if self.live_markdown_scheduled.is_some() {
            return;
        }
        let token = self.live_markdown_generation;
        let chat_id = self.model.selected_chat.clone();
        self.live_markdown_scheduled = Some(token);
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(16)).await;
            let preparation = this
                .update(cx, |this, cx| {
                    if this.live_markdown_scheduled != Some(token)
                        || this.model.selected_chat != chat_id
                    {
                        return None;
                    }
                    this.live_markdown_scheduled = None;
                    let generation = this.live_markdown_generation;
                    let content = this.model.live_text.clone();
                    if content.is_empty() {
                        return None;
                    }
                    let label = this
                        .model
                        .selected_summary()
                        .map(|chat| chat.backend.clone());
                    let parse = cx
                        .background_executor()
                        .spawn(async move { Message::new(None, "assistant", content, label) });
                    Some((generation, parse))
                })
                .ok()
                .flatten();
            let Some((generation, parse)) = preparation else {
                return;
            };
            let message = parse.await;
            let _ = this.update(cx, |this, cx| {
                if this.live_markdown_generation != generation
                    || this.model.selected_chat != chat_id
                    || this.model.live_text != message.content
                {
                    return;
                }
                this.transcript_snapshot.live_text = Some(Arc::new(message));
                let index = this.model.messages.len();
                let anchor = this.transcript.logical_scroll_top();
                this.transcript.splice(index..index + 1, 1);
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

    fn invalidate_image_rows(&self, path: &str) {
        let indices = self
            .model
            .messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| {
                message
                    .image_paths()
                    .iter()
                    .any(|candidate| candidate == path)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if indices.is_empty() {
            return;
        }
        let anchor = self.transcript.logical_scroll_top();
        for index in indices {
            self.transcript.splice(index..index + 1, 1);
        }
        self.transcript.scroll_to(anchor);
    }

    fn open_message_image(&mut self, image: Arc<Image>, number: usize, cx: &mut Context<Self>) {
        self.message_image_viewer = Some(MessageImageViewer { image, number });
        cx.notify();
    }

    fn close_message_image(&mut self, cx: &mut Context<Self>) {
        if self.message_image_viewer.take().is_some() {
            cx.notify();
        }
    }

    fn message_image(
        path: &str,
        number: usize,
        scope: &str,
        desktop: Entity<Self>,
        daemon: Option<&DaemonHandle>,
        cache: &Arc<Mutex<MessageImageCache>>,
    ) -> gpui::AnyElement {
        let mut request = false;
        let mut state = cache.lock().ok().and_then(|mut cache| cache.state(path));
        if state.is_none()
            && daemon.is_some()
            && let Ok(mut cache) = cache.lock()
            && cache.begin(path)
        {
            request = true;
            state = Some(MessageImageState::Loading);
        }
        if request
            && let Some(daemon) = daemon
            && daemon.image_read(path).is_err()
        {
            if let Ok(mut cache) = cache.lock() {
                cache.finish(path, None);
            }
            state = Some(MessageImageState::Unavailable);
        }

        let preview = div()
            .w(px(168.0))
            .h(px(96.0))
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center();
        let preview = match state {
            Some(MessageImageState::Ready(image)) => {
                let open_image = image.clone();
                preview
                    .id(SharedString::from(format!("open-{scope}-image-{number}")))
                    .cursor_pointer()
                    .hover(|style| style.border_color(rgb(0x626268)))
                    .on_click(move |_, _, cx| {
                        desktop.update(cx, |this, cx| {
                            this.open_message_image(open_image.clone(), number, cx);
                        });
                    })
                    .child(img(image).size_full().object_fit(ObjectFit::Contain))
                    .into_any_element()
            }
            Some(MessageImageState::Loading) => preview
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Loading image…"),
                )
                .into_any_element(),
            Some(MessageImageState::Unavailable) | None => preview
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Preview unavailable"),
                )
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .child(preview)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(format!("Image #{number}")),
            )
            .into_any_element()
    }

    fn message_row(
        message: &Message,
        index: usize,
        expanded: bool,
        expanded_sections: &HashSet<String>,
        workflow_status: Option<&Value>,
        workflow_pending: bool,
        desktop: Entity<Self>,
        daemon: Option<&DaemonHandle>,
        image_cache: &Arc<Mutex<MessageImageCache>>,
    ) -> gpui::AnyElement {
        if message.role == "duration" {
            return turn_duration_label(&message.content)
                .map(|label| {
                    div()
                        .w_full()
                        .px_4()
                        .pt_1()
                        .pb_2()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(1040.0))
                                .mx_auto()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(label),
                        )
                        .into_any_element()
                })
                .unwrap_or_else(|| div().into_any_element());
        }
        let is_user = message.role == "user";
        let is_tool = message.role == "tool";
        if is_tool {
            let key = message
                .id
                .map(|id| format!("message-{id}"))
                .unwrap_or_else(|| format!("live-{index}"));
            return Self::activity_card(
                ActivityCard::parse(&message.content)
                    .with_workflow_status(workflow_status, workflow_pending),
                key,
                index,
                expanded,
                desktop.clone(),
            );
        }
        let markdown_scope = message
            .id
            .map(|id| format!("message-{id}"))
            .unwrap_or_else(|| format!("live-{index}"));
        let images = message
            .image_paths()
            .iter()
            .enumerate()
            .map(|(index, path)| {
                Self::message_image(
                    path,
                    index + 1,
                    &markdown_scope,
                    desktop.clone(),
                    daemon,
                    image_cache,
                )
            })
            .collect::<Vec<_>>();

        div()
            .w_full()
            .px_4()
            .py_2()
            .child(
                div()
                    .w_full()
                    .max_w(px(1040.0))
                    .mx_auto()
                    .flex()
                    .when(is_user, |row| row.justify_end())
                    .child(
                        div()
                            .when(!is_user, |body| body.w_full())
                            .when(is_user, |body| {
                                body.max_w(px(680.0))
                                    .px_4()
                                    .py_3()
                                    .rounded_xl()
                                    .border_1()
                                    .border_color(rgb(0x242428))
                                    .bg(rgb(SURFACE_HIGH))
                            })
                            .text_color(rgb(TEXT))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        Self::markdown_content(
                                            message.markdown(),
                                            &markdown_scope,
                                            Some(expanded_sections),
                                            Some(desktop.clone()),
                                            Some(index),
                                        )
                                        .text_sm()
                                        .line_height(px(21.0)),
                                    )
                                    .children(images),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn activity_card(
        card: ActivityCard,
        key: String,
        index: usize,
        expanded: bool,
        desktop: Entity<Self>,
    ) -> gpui::AnyElement {
        let status_color = activity_status_color(card.kind);
        let has_items = !card.items.is_empty();
        let item_rows = card
            .items
            .iter()
            .map(|item| {
                let color = activity_status_color(item.kind);
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(SURFACE_HIGH))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(rgb(color)).child("●"))
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(item.name.clone()),
                            )
                            .when_some(item.elapsed.clone(), |row, elapsed| {
                                row.child(div().text_xs().text_color(rgb(MUTED)).child(elapsed))
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(color))
                                    .child(item.status.clone()),
                            ),
                    )
                    .when_some(item.detail.clone(), |row, detail| {
                        row.child(
                            div()
                                .mt_1()
                                .ml_4()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(detail),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let toggle_key = key.clone();
        let mut body = div()
            .w_full()
            .max_w(px(920.0))
            .mx_auto()
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .overflow_hidden()
            .child(
                div()
                    .id(("activity-card", index))
                    .w_full()
                    .px_4()
                    .py_3()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                    .on_click(move |_, _, cx| {
                        desktop.update(cx, |this, cx| {
                            let anchor = this.transcript.logical_scroll_top();
                            let expanded = Arc::make_mut(&mut this.expanded_activity);
                            if !expanded.remove(&toggle_key) {
                                expanded.insert(toggle_key.clone());
                            }
                            this.transcript.splice(index..index + 1, 1);
                            this.transcript.scroll_to(anchor);
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(MUTED))
                                    .child(card.title.clone()),
                            )
                            .child(div().flex_1())
                            .child(div().text_xs().text_color(rgb(MUTED)).child(if expanded {
                                "▾"
                            } else {
                                "▸"
                            })),
                    )
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(rgb(status_color)).child("●"))
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(card.name.clone()),
                            )
                            .when_some(card.elapsed.clone(), |row, elapsed| {
                                row.child(div().text_xs().text_color(rgb(MUTED)).child(elapsed))
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(status_color))
                                    .child(card.status.clone()),
                            ),
                    ),
            );
        if expanded {
            body = body.child(
                div()
                    .w_full()
                    .px_4()
                    .pb_3()
                    .pt_1()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(card.detail)
                    .when(has_items, |details| {
                        details.mt_2().flex().flex_col().gap_2().children(item_rows)
                    }),
            );
            if let Some(footer) = card.footer {
                let url = card.url.clone();
                body = body.child(
                    div()
                        .px_4()
                        .pb_3()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(footer)
                        .when_some(url, |footer, url| {
                            footer.child(
                                div()
                                    .id(("open-workflow-run", index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_color(rgb(0x91a7ff))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                                    .child("Open in GitHub ↗"),
                            )
                        }),
                );
            }
        }
        div()
            .w_full()
            .px_6()
            .py_2()
            .text_color(rgb(TEXT))
            .child(body)
            .into_any_element()
    }

    fn markdown_content(
        document: std::sync::Arc<markdown::Document>,
        scope: &str,
        expanded_sections: Option<&HashSet<String>>,
        desktop: Option<Entity<Self>>,
        transcript_index: Option<usize>,
    ) -> gpui::Div {
        let mut content = div().w_full().flex().flex_col().gap_2();
        for (block_index, block) in document.blocks.iter().cloned().enumerate() {
            let block_id = scoped_element_id(scope, block_index);
            let element = match block {
                Block::Heading { level, content } => {
                    let heading = div()
                        .mt_1()
                        .font_weight(FontWeight::BOLD)
                        .child(Self::inline_text(content, block_id));
                    match level {
                        1 => heading.text_xl().line_height(px(30.0)),
                        2 => heading.text_lg().line_height(px(27.0)),
                        _ => heading.text_base().line_height(px(24.0)),
                    }
                    .into_any_element()
                }
                Block::Paragraph(content) => div()
                    .whitespace_normal()
                    .child(Self::inline_text(content, block_id))
                    .into_any_element(),
                Block::Quote(content) => div()
                    .pl_3()
                    .py_1()
                    .border_l_2()
                    .border_color(rgb(0x59647a))
                    .text_color(rgb(MUTED))
                    .child(Self::inline_text(content, block_id))
                    .into_any_element(),
                Block::ListItem {
                    number,
                    depth,
                    content,
                } => div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .pl(px(f32::from(depth) * 18.0))
                    .child(
                        div()
                            .min_w(px(18.0))
                            .flex_none()
                            .text_color(rgb(MUTED))
                            .child(
                                number.map_or_else(|| "•".into(), |number| format!("{number}.")),
                            ),
                    )
                    .child(div().flex_1().child(Self::inline_text(content, block_id)))
                    .into_any_element(),
                Block::Rule => div()
                    .w_full()
                    .h(px(1.0))
                    .my_2()
                    .bg(rgb(BORDER))
                    .into_any_element(),
                Block::Code(code) => {
                    let language = code.language.unwrap_or_else(|| "text".into());
                    let source = code.code;
                    let copied_source = source.clone();
                    let highlights = code.spans.into_iter().map(|span| {
                        let color = match span.kind {
                            CodeKind::Keyword => 0xc792ea,
                            CodeKind::String => 0xc3e88d,
                            CodeKind::Comment => 0x758195,
                            CodeKind::Number => 0xf78c6c,
                        };
                        (
                            span.range,
                            HighlightStyle {
                                color: Some(rgb(color).into()),
                                ..Default::default()
                            },
                        )
                    });
                    div()
                        .w_full()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .overflow_hidden()
                        .child(
                            div()
                                .w_full()
                                .px_3()
                                .py_1()
                                .flex()
                                .items_center()
                                .gap_2()
                                .bg(rgb(SURFACE_HIGH))
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(language)
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .id(("copy-code", block_id))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_color(rgb(TEXT))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(BG)))
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copied_source.clone(),
                                            ));
                                        })
                                        .child("Copy"),
                                ),
                        )
                        .child(
                            div()
                                .id(("scroll-code", block_id))
                                .w_full()
                                .overflow_scroll()
                                .p_3()
                                .font_family("monospace")
                                .text_sm()
                                .line_height(px(20.0))
                                .whitespace_nowrap()
                                .child(StyledText::new(source).with_highlights(highlights)),
                        )
                        .into_any_element()
                }
                Block::Table(table) => div()
                    .id(("scroll-table", block_id))
                    .w_full()
                    .overflow_x_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BG))
                    .p_3()
                    .font_family("monospace")
                    .text_sm()
                    .line_height(px(20.0))
                    .whitespace_nowrap()
                    .child(StyledText::new(table.text).with_highlights([(
                        table.header,
                        HighlightStyle {
                            font_weight: Some(FontWeight::BOLD),
                            ..Default::default()
                        },
                    )]))
                    .into_any_element(),
                Block::Analysis(blocks) => {
                    let section_key = format!("{scope}-analysis-{block_index}");
                    let is_expanded =
                        expanded_sections.is_some_and(|expanded| expanded.contains(&section_key));
                    let toggle_desktop = desktop.clone();
                    let toggle_key = section_key.clone();
                    let mut disclosure = div()
                        .w_full()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .overflow_hidden()
                        .child(
                            div()
                                .id(("toggle-analysis", block_id))
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .when(toggle_desktop.is_some(), |header| {
                                    header
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                })
                                .on_click(move |_, _, cx| {
                                    let Some(desktop) = toggle_desktop.as_ref() else {
                                        return;
                                    };
                                    desktop.update(cx, |this, cx| {
                                        let expanded = Arc::make_mut(&mut this.expanded_activity);
                                        if !expanded.remove(&toggle_key) {
                                            expanded.insert(toggle_key.clone());
                                        }
                                        if let Some(index) = transcript_index {
                                            let anchor = this.transcript.logical_scroll_top();
                                            this.transcript.splice(index..index + 1, 1);
                                            this.transcript.scroll_to(anchor);
                                        }
                                        cx.notify();
                                    });
                                })
                                .child(if is_expanded { "▾" } else { "▸" })
                                .child("Analysis"),
                        );
                    if is_expanded {
                        let nested = std::sync::Arc::new(markdown::Document {
                            blocks,
                            truncated: false,
                        });
                        disclosure =
                            disclosure.child(div().px_3().pb_3().child(Self::markdown_content(
                                nested,
                                &section_key,
                                expanded_sections,
                                desktop.clone(),
                                transcript_index,
                            )));
                    }
                    disclosure.into_any_element()
                }
            };
            content = content.child(element);
        }
        content
    }

    fn inline_text(content: InlineText, id: u64) -> gpui::AnyElement {
        let links = content
            .spans
            .iter()
            .filter_map(|span| span.url.clone().map(|url| (span.range.clone(), url)))
            .collect::<Vec<_>>();
        let highlights = content.spans.into_iter().map(|span| {
            let style = match span.kind {
                InlineKind::Strong => HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
                InlineKind::Emphasis => HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
                InlineKind::StrongEmphasis => HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
                InlineKind::Code => HighlightStyle {
                    color: Some(rgb(0xd8b4fe).into()),
                    background_color: Some(rgb(0x292331).into()),
                    ..Default::default()
                },
                InlineKind::Link => HighlightStyle {
                    color: Some(rgb(0x91a7ff).into()),
                    ..Default::default()
                },
            };
            (span.range, style)
        });
        let text = StyledText::new(content.text).with_highlights(highlights);
        if links.is_empty() {
            return text.into_any_element();
        }
        let ranges = links.iter().map(|(range, _)| range.clone()).collect();
        let urls = links.into_iter().map(|(_, url)| url).collect::<Vec<_>>();
        InteractiveText::new(("markdown-inline", id), text)
            .on_click(ranges, move |index, _, cx| {
                if let Some(url) = urls.get(index) {
                    cx.open_url(url);
                }
            })
            .into_any_element()
    }

    fn sidebar_context_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.sidebar_context_menu.clone()?;
        let menu_height = match &menu.target {
            None => 44.0,
            Some(SidebarTarget::Folder(_)) => 260.0,
            Some(SidebarTarget::Chat(_)) => 116.0,
        };
        let cursor_y = f32::from(menu.position.y);
        let menu_y = px(if cursor_y > 430.0 {
            (cursor_y - menu_height).max(8.0)
        } else {
            cursor_y
        });
        let mut items = Vec::new();
        match menu.target.clone() {
            None => {
                items.push(
                    div()
                        .id("context-new-workspace")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.sidebar_context_menu = None;
                            if !this.creating_workspace {
                                this.begin_workspace_create(cx);
                                let focus = this.workspace_create_input.read(cx).focus_handle(cx);
                                window.focus(&focus);
                            }
                        }))
                        .child("New Workspace")
                        .into_any_element(),
                );
            }
            Some(SidebarTarget::Folder(folder_id)) => {
                let folder = self
                    .model
                    .folders
                    .iter()
                    .find(|folder| folder.id == folder_id)?
                    .clone();
                let new_chat_id = folder.id.clone();
                items.push(
                    div()
                        .id("context-new-chat")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.sidebar_context_menu = None;
                            this.begin_chat_create(new_chat_id.clone(), cx);
                            let focus = this.chat_create_input.read(cx).focus_handle(cx);
                            window.focus(&focus);
                        }))
                        .child("New Chat")
                        .into_any_element(),
                );
                let rename_target = SidebarTarget::Folder(folder.id.clone());
                let folder_name = folder.name.clone();
                items.push(
                    div()
                        .id("context-rename-folder")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.sidebar_context_menu = None;
                            this.begin_sidebar_edit(rename_target.clone(), folder_name.clone(), cx);
                            let focus = this.sidebar_edit_input.read(cx).focus_handle(cx);
                            window.focus(&focus);
                        }))
                        .child("Rename")
                        .into_any_element(),
                );
                let context_folder = folder.id.clone();
                items.push(
                    div()
                        .id("context-folder-context")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.sidebar_context_menu = None;
                            this.begin_workspace_context(context_folder.clone(), cx);
                            let focus = this.workspace_context_input.read(cx).focus_handle(cx);
                            window.focus(&focus);
                        }))
                        .child("Context")
                        .into_any_element(),
                );
                let defaults_folder = folder.id.clone();
                items.push(
                    div()
                        .id("context-folder-agent")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sidebar_context_menu = None;
                            this.begin_workspace_defaults(defaults_folder.clone(), cx);
                        }))
                        .child("Agent Defaults")
                        .into_any_element(),
                );
                let shortcuts_folder = folder.id.clone();
                let shortcuts_name = folder.name.clone();
                items.push(
                    div()
                        .id("context-folder-shortcuts")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_shortcut_panel(
                                Some(shortcuts_folder.clone()),
                                Some(shortcuts_name.clone()),
                                cx,
                            );
                        }))
                        .child("Shortcuts")
                        .into_any_element(),
                );
                let secrets_folder = folder.id.clone();
                let secrets_name = folder.name.clone();
                items.push(
                    div()
                        .id("context-folder-secrets")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.sidebar_context_menu = None;
                            this.open_secrets(
                                Some(secrets_folder.clone()),
                                Some(secrets_name.clone()),
                                window,
                                cx,
                            );
                        }))
                        .child("Secrets")
                        .into_any_element(),
                );
                let move_target = SidebarTarget::Folder(folder.id.clone());
                items.push(
                    div()
                        .id("context-move-folder")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sidebar_context_menu = None;
                            this.toggle_sidebar_move(move_target.clone(), cx);
                        }))
                        .child("Move")
                        .into_any_element(),
                );
                let delete_target = SidebarTarget::Folder(folder.id);
                let confirming = self.pending_sidebar_delete.as_ref() == Some(&delete_target);
                let deleting = confirming && self.sidebar_delete_submitting;
                items.push(
                    div()
                        .id("context-trash-folder")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(0xefaaaa))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x3b282e)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.delete_sidebar_item(delete_target.clone(), cx);
                        }))
                        .child(if deleting {
                            "Trashing…"
                        } else if confirming {
                            "Confirm Trash"
                        } else {
                            "Trash"
                        })
                        .into_any_element(),
                );
            }
            Some(SidebarTarget::Chat(chat_id)) => {
                let chat = self
                    .model
                    .chats
                    .iter()
                    .find(|chat| chat.id == chat_id)?
                    .clone();
                let rename_target = SidebarTarget::Chat(chat.id.clone());
                let title = chat.title.clone().unwrap_or_else(|| "New Chat".into());
                items.push(
                    div()
                        .id("context-rename-chat")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.sidebar_context_menu = None;
                            this.begin_sidebar_edit(rename_target.clone(), title.clone(), cx);
                            let focus = this.sidebar_edit_input.read(cx).focus_handle(cx);
                            window.focus(&focus);
                        }))
                        .child("Rename")
                        .into_any_element(),
                );
                let move_target = SidebarTarget::Chat(chat.id.clone());
                items.push(
                    div()
                        .id("context-move-chat")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sidebar_context_menu = None;
                            this.toggle_sidebar_move(move_target.clone(), cx);
                        }))
                        .child("Move")
                        .into_any_element(),
                );
                let delete_target = SidebarTarget::Chat(chat.id);
                let confirming = self.pending_sidebar_delete.as_ref() == Some(&delete_target);
                let deleting = confirming && self.sidebar_delete_submitting;
                items.push(
                    div()
                        .id("context-delete-chat")
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(0xefaaaa))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x3b282e)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.delete_sidebar_item(delete_target.clone(), cx);
                        }))
                        .child(if deleting {
                            "Deleting…"
                        } else if confirming {
                            "Confirm Delete"
                        } else {
                            "Delete"
                        })
                        .into_any_element(),
                );
            }
        }

        Some(
            div()
                .absolute()
                .top(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close_sidebar_context_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| this.close_sidebar_context_menu(cx)),
                )
                .child(
                    div()
                        .id("sidebar-context-menu")
                        .absolute()
                        .left(menu.position.x)
                        .top(menu_y)
                        .w(px(190.0))
                        .p_1()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .children(items),
                )
                .into_any_element(),
        )
    }

    fn presence_state(&self) -> &'static str {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return "Browsing workspaces";
        };
        if self.active_endpoint == ChatEndpoint::Remote
            && self.remote_state != RemoteState::Connected
        {
            "Remote unavailable"
        } else if self
            .open_question
            .as_ref()
            .is_some_and(|question| question.chat_id == chat_id)
        {
            "Waiting for input"
        } else if self.model.working {
            "Agent working"
        } else {
            "Reviewing a conversation"
        }
    }
}

impl Render for XdDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.transcript_scroll_handler_attached {
            self.transcript_scroll_handler_attached = true;
            let desktop = cx.entity();
            self.transcript
                .set_scroll_handler(move |event, _window, cx| {
                    let near_start = event.visible_range.start <= 8;
                    let near_end = event.visible_range.end.saturating_add(8) >= event.count;
                    let _ = desktop.update(cx, |this, _cx| {
                        if near_start && this.transcript_has_older {
                            this.request_older_messages();
                        } else if near_end {
                            this.request_newer_messages();
                        }
                    });
                });
        }
        self.presence.set_state(self.presence_state());
        let client_decorations = matches!(window.window_decorations(), Decorations::Client { .. });
        window.set_client_inset(if client_decorations { px(6.0) } else { px(0.0) });
        let accent = self.settings.accent.color();
        let accent_hover = self.settings.accent.hover_color();
        let sidebar_width = self.settings.sidebar_width;
        let diff_width = self.settings.diff_width;
        let terminal_height = self.settings.terminal_height;
        let remote_active = self.active_endpoint == ChatEndpoint::Remote;
        let active_machine_label = if remote_active { "REMOTE" } else { "LOCAL" };
        let messages = self.transcript_snapshot.clone();
        let queue_count = self.model.queue.len();
        let working = self.model.working;
        let selected = self.model.selected_summary().cloned();
        let diff_open = self.diff_panel.is_some();
        let terminal_open = self.terminal_panel.is_some();
        let can_open_diff = selected.is_some();
        let new_worktree = self.model.new_worktree;
        let can_change_worktree = selected.is_some() && !self.model.has_messages && !working;
        let can_cycle_workspace =
            can_change_worktree && !new_worktree && self.model.worktrees.len() > 1;
        let can_remove_worktree = can_change_worktree
            && !new_worktree
            && self
                .model
                .worktrees
                .iter()
                .any(|worktree| worktree.current && !worktree.main);
        let workspace_label = self
            .model
            .worktrees
            .iter()
            .find(|worktree| worktree.current)
            .map(|worktree| {
                worktree.branch.clone().unwrap_or_else(|| {
                    if worktree.main {
                        "main checkout".into()
                    } else {
                        "detached worktree".into()
                    }
                })
            })
            .unwrap_or_else(|| "workspace".into());
        let model_label = self
            .model
            .selected_model_name()
            .unwrap_or(if self.model.backend.is_empty() {
                "Assistant"
            } else {
                &self.model.backend
            })
            .to_owned();
        let effort_label = if self.model.effort.is_empty() {
            "high"
        } else {
            &self.model.effort
        }
        .to_owned();
        let can_change_agent =
            selected.is_some() && !working && !self.model.agent_backends.is_empty();
        let context_meter =
            context_usage::meter(self.model.context_used, self.model.context_window);
        let fast = self.model.fast;
        let can_toggle_fast = can_change_agent && self.model.backend == "codex";
        let claude_mode = self.model.claude_mode;
        let can_toggle_claude_mode = can_change_agent && self.model.backend == "codex";
        let access_label = match self.model.access.as_str() {
            "full" => "Full access",
            "edit" => "Edit",
            _ => "Read only",
        };
        let endpoint_connecting = if remote_active {
            self.remote_state == RemoteState::Connecting
        } else {
            self.connecting
        };
        let status_text = if self.model.connected {
            "connected"
        } else if endpoint_connecting {
            "reconnecting"
        } else {
            "offline"
        };
        let status_color = if self.model.connected {
            0x92d5a5
        } else if endpoint_connecting {
            0xe8c982
        } else {
            0xe49a9a
        };

        let sidebar_edit = self.sidebar_edit.clone();
        let sidebar_edit_input = self.sidebar_edit_input.clone();
        let sidebar_edit_focus = self.sidebar_edit_input.read(cx).focus_handle(cx);
        let sidebar_move = self.sidebar_move.clone();
        let sidebar_move_submitting = self.sidebar_move_submitting;
        let creating_chat_folder = self.creating_chat_folder.clone();
        let chat_create_submitting = self.chat_create_submitting;
        let can_save_chat = creating_chat_folder.is_some()
            && !chat_create_submitting
            && !self.chat_create_title.trim().is_empty()
            && self.model.connected;
        let chat_create_input = self.chat_create_input.clone();
        let chat_create_focus = self.chat_create_input.read(cx).focus_handle(cx);
        let workspace_context_folder = self.workspace_context_folder.clone();
        let workspace_context_loading = self.workspace_context_loading;
        let workspace_context_submitting = self.workspace_context_submitting;
        let workspace_context_input = self.workspace_context_input.clone();
        let workspace_context_focus = self.workspace_context_input.read(cx).focus_handle(cx);
        let workspace_workdir_input = self.workspace_workdir_input.clone();
        let workspace_workdir_focus = self.workspace_workdir_input.read(cx).focus_handle(cx);
        let workspace_repo_default_input = self.workspace_repo_default_input.clone();
        let workspace_repo_default_focus =
            self.workspace_repo_default_input.read(cx).focus_handle(cx);
        let can_save_context = workspace_context_folder.is_some()
            && !workspace_context_loading
            && !workspace_context_submitting
            && self.model.connected;
        let workspace_defaults = self.workspace_defaults.clone();
        let root_drop_model = self.model.clone();
        let mut tree_rows = Vec::new();
        let mut chat_row_index = 0_usize;
        for (folder_row_index, folder) in self.model.folders.clone().into_iter().enumerate() {
            if self.folder_hidden_by_collapse(&folder.id) {
                continue;
            }
            let indent = if folder.parent.is_some() { 22.0 } else { 12.0 };
            let new_chat_folder_id = folder.id.clone();
            let collapse_folder_id = folder.id.clone();
            let folder_name = folder.name.clone();
            let folder_collapsed = self.collapsed_folders.contains(&folder.id);
            let folder_target = SidebarTarget::Folder(folder.id.clone());
            let editing_folder = sidebar_edit
                .as_ref()
                .is_some_and(|edit| edit.target == folder_target);
            if editing_folder {
                let can_save = sidebar_edit.as_ref().is_some_and(|edit| {
                    !edit.submitting
                        && !edit.text.trim().is_empty()
                        && edit.text.trim() != edit.original
                });
                let saving = sidebar_edit.as_ref().is_some_and(|edit| edit.submitting);
                tree_rows.push(
                    div()
                        .ml(px(indent))
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id(("folder-name-editor", folder_row_index))
                                .track_focus(&sidebar_edit_focus)
                                .h(px(30.0))
                                .min_w_0()
                                .flex_1()
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if sidebar_edit_focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(BG))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus = this.sidebar_edit_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(sidebar_edit_input.clone()),
                        )
                        .child(
                            div()
                                .id(("save-folder-name", folder_row_index))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(if can_save { TEXT } else { MUTED }))
                                .when(can_save, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_save {
                                        this.save_sidebar_edit(cx);
                                    }
                                }))
                                .child(if saving { "…" } else { "Save" }),
                        )
                        .child(
                            div()
                                .id(("cancel-folder-name", folder_row_index))
                                .px_1()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel_sidebar_edit(cx);
                                }))
                                .child("×"),
                        )
                        .into_any_element(),
                );
            } else {
                let context_menu_target = folder_target.clone();
                let dragged_folder = SidebarDrag::new(folder_target.clone(), folder_name.clone());
                let drop_folder_id = folder.id.clone();
                let drop_model = self.model.clone();
                let moving_folder = sidebar_move.as_ref() == Some(&folder_target);
                tree_rows.push(
                    div()
                        .id(("folder-row", folder_row_index))
                        .min_w_0()
                        .px_3()
                        .ml(px(indent))
                        .mr_2()
                        .pt_2()
                        .pb_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_sidebar_context_menu(
                                    Some(context_menu_target.clone()),
                                    event.position,
                                    cx,
                                );
                            }),
                        )
                        .child(
                            div()
                                .id(("collapse-folder", folder_row_index))
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .cursor_move()
                                .hover(|style| style.text_color(rgb(0xb9c7ff)))
                                .on_drag(dragged_folder, |drag: &SidebarDrag, position, _, cx| {
                                    cx.new(|_| drag.clone().position(position))
                                })
                                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                    if !event.is_right_click() {
                                        this.toggle_folder_collapsed(
                                            collapse_folder_id.clone(),
                                            cx,
                                        );
                                    }
                                }))
                                .child(format!(
                                    "{}  {folder_name}",
                                    if folder_collapsed { "▸" } else { "▾" }
                                )),
                        )
                        .child(
                            div()
                                .id(("new-chat", folder_row_index))
                                .px_2()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if !chat_create_submitting {
                                        this.begin_chat_create(new_chat_folder_id.clone(), cx);
                                        let focus =
                                            this.chat_create_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }
                                }))
                                .child("+"),
                        )
                        .can_drop(move |value, _, _| {
                            value.downcast_ref::<SidebarDrag>().is_some_and(|drag| {
                                sidebar_drop_allowed(
                                    &drop_model,
                                    &drag.target,
                                    Some(&drop_folder_id),
                                )
                            })
                        })
                        .drag_over::<SidebarDrag>(move |style, _, _, _| {
                            style.bg(rgb(SURFACE_HIGH)).border_color(rgb(accent))
                        })
                        .on_drop(cx.listener({
                            let destination = folder.id.clone();
                            move |this, drag: &SidebarDrag, _, cx| {
                                this.drop_sidebar_item(
                                    drag.target.clone(),
                                    Some(destination.clone()),
                                    cx,
                                );
                            }
                        }))
                        .into_any_element(),
                );
                if moving_folder {
                    let mut destinations = Vec::new();
                    if folder.parent.is_some() {
                        let target = folder_target.clone();
                        destinations.push(
                            div()
                                .id(("move-folder-root", folder_row_index))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x242428)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_sidebar_item(target.clone(), None, cx);
                                }))
                                .child("Workspace root")
                                .into_any_element(),
                        );
                    }
                    for (destination_index, destination) in self
                        .model
                        .folders
                        .iter()
                        .filter(|destination| {
                            folder.parent.as_deref() != Some(destination.id.as_str())
                                && !self.folder_is_descendant_of(&destination.id, &folder.id)
                        })
                        .enumerate()
                    {
                        let target = folder_target.clone();
                        let destination_id = destination.id.clone();
                        destinations.push(
                            div()
                                .id(("move-folder-destination", destination_index))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x242428)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_sidebar_item(
                                        target.clone(),
                                        Some(destination_id.clone()),
                                        cx,
                                    );
                                }))
                                .child(destination.name.clone())
                                .into_any_element(),
                        );
                    }
                    tree_rows.push(
                        div()
                            .ml(px(indent + 10.0))
                            .mr_2()
                            .mb_1()
                            .p_2()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .rounded_md()
                            .bg(rgb(SURFACE_HIGH))
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .when(sidebar_move_submitting, |panel| panel.child("Moving…"))
                            .when(
                                !sidebar_move_submitting && destinations.is_empty(),
                                |panel| panel.child("No other destination"),
                            )
                            .when(!sidebar_move_submitting, |panel| {
                                panel.children(destinations)
                            })
                            .into_any_element(),
                    );
                }
            }
            if workspace_context_folder.as_deref() == Some(folder.id.as_str()) {
                tree_rows.push(
                    div()
                        .ml(px(indent + 10.0))
                        .mr_2()
                        .mb_1()
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded_md()
                        .bg(rgb(SURFACE_HIGH))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child("Workspace instructions inherited by chats"),
                        )
                        .when(workspace_context_loading, |panel| {
                            panel.child(
                                div()
                                    .h(px(38.0))
                                    .flex()
                                    .items_center()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child("Loading context…"),
                            )
                        })
                        .when(!workspace_context_loading, |panel| {
                            panel.child(
                                div()
                                    .id(("workspace-context-input", folder_row_index))
                                    .track_focus(&workspace_context_focus)
                                    .h(px(42.0))
                                    .w_full()
                                    .min_w_0()
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(
                                        if workspace_context_focus.is_focused(window) {
                                            accent
                                        } else {
                                            BORDER
                                        },
                                    ))
                                    .bg(rgb(BG))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let focus =
                                            this.workspace_context_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }))
                                    .child(workspace_context_input.clone()),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_1()
                                .child(
                                    div()
                                        .id(("cancel-workspace-context", folder_row_index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .when(!workspace_context_submitting, |button| {
                                            button.cursor_pointer().hover(|style| {
                                                style.bg(rgb(0x242428)).text_color(rgb(TEXT))
                                            })
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !workspace_context_submitting {
                                                this.cancel_workspace_context(cx);
                                            }
                                        }))
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id(("save-workspace-context", folder_row_index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .text_color(rgb(if can_save_context {
                                            TEXT
                                        } else {
                                            MUTED
                                        }))
                                        .when(can_save_context, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(0x242428)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_save_context {
                                                this.save_workspace_context(cx);
                                            }
                                        }))
                                        .child(if workspace_context_submitting {
                                            "Saving…"
                                        } else {
                                            "Save"
                                        }),
                                ),
                        )
                        .into_any_element(),
                );
            }
            if let Some(defaults) = workspace_defaults
                .as_ref()
                .filter(|defaults| defaults.folder_id == folder.id)
            {
                let loading = defaults.loading;
                let submitting = defaults.submitting;
                let workdir_folder_id = defaults.folder_id.clone();
                let repository_folder_id = defaults.folder_id.clone();
                let selected_backend = defaults
                    .backend
                    .as_deref()
                    .unwrap_or(&defaults.effective_backend)
                    .to_owned();
                let mut backend_buttons = Vec::new();
                for (index, (id, label)) in std::iter::once((None, "Inherit".to_owned()))
                    .chain(
                        self.model
                            .agent_backends
                            .iter()
                            .map(|backend| (Some(backend.id.clone()), backend.name.clone())),
                    )
                    .enumerate()
                {
                    let selected = defaults.backend == id;
                    let value = id.clone();
                    backend_buttons.push(
                        div()
                            .id(("workspace-backend", index))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_xs()
                            .bg(rgb(if selected { accent } else { SURFACE_HIGH }))
                            .text_color(rgb(if selected { 0xffffff } else { TEXT }))
                            .when(!loading && !submitting, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x506dc7)))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !loading && !submitting {
                                    this.select_workspace_backend(value.clone(), cx);
                                }
                            }))
                            .child(label)
                            .into_any_element(),
                    );
                }
                let mut model_buttons = Vec::new();
                model_buttons.push(
                    div()
                        .id(("workspace-model", 0_usize))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_xs()
                        .bg(rgb(if defaults.model.is_none() {
                            accent
                        } else {
                            SURFACE_HIGH
                        }))
                        .text_color(rgb(if defaults.model.is_none() {
                            0xffffff
                        } else {
                            TEXT
                        }))
                        .when(!loading && !submitting, |button| {
                            button
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x506dc7)))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !loading && !submitting {
                                this.select_workspace_model(None, cx);
                            }
                        }))
                        .child("Inherit")
                        .into_any_element(),
                );
                if let Some(backend) = self
                    .model
                    .agent_backends
                    .iter()
                    .find(|backend| backend.id == selected_backend)
                {
                    for (index, model) in backend.models.iter().enumerate() {
                        let id = model.id.clone();
                        let selected = defaults.model.as_deref() == Some(id.as_str());
                        model_buttons.push(
                            div()
                                .id(("workspace-model", index + 1))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .bg(rgb(if selected { accent } else { SURFACE_HIGH }))
                                .text_color(rgb(if selected { 0xffffff } else { TEXT }))
                                .when(!loading && !submitting, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x506dc7)))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !loading && !submitting {
                                        this.select_workspace_model(Some(id.clone()), cx);
                                    }
                                }))
                                .child(model.name.clone())
                                .into_any_element(),
                        );
                    }
                }
                let can_save = !loading && !submitting && self.model.connected;
                tree_rows.push(
                    div()
                        .ml(px(indent + 10.0))
                        .mr_2()
                        .mb_1()
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .rounded_md()
                        .bg(rgb(SURFACE_HIGH))
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .when(loading, |panel| panel.child("Loading defaults…"))
                        .when(!loading, |panel| {
                            panel
                                .child("Assistant")
                                .child(div().flex().flex_wrap().gap_1().children(backend_buttons))
                                .child("Model")
                                .child(div().flex().flex_wrap().gap_1().children(model_buttons))
                                .child("Working directory")
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .id(("workspace-workdir-input", folder_row_index))
                                                .track_focus(&workspace_workdir_focus)
                                                .h(px(32.0))
                                                .min_w_0()
                                                .flex_1()
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(rgb(
                                                    if workspace_workdir_focus.is_focused(window) {
                                                        accent
                                                    } else {
                                                        BORDER
                                                    },
                                                ))
                                                .bg(rgb(BG))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    let focus = this
                                                        .workspace_workdir_input
                                                        .read(cx)
                                                        .focus_handle(cx);
                                                    window.focus(&focus);
                                                }))
                                                .child(workspace_workdir_input.clone()),
                                        )
                                        .child(
                                            div()
                                                .id(("browse-workspace-workdir", folder_row_index))
                                                .h(px(32.0))
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .rounded_md()
                                                .bg(rgb(BG))
                                                .text_color(rgb(TEXT))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(0x242428)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.choose_workspace_path(
                                                        WorkspacePathTarget::DefaultsWorkdir {
                                                            folder_id: workdir_folder_id.clone(),
                                                        },
                                                        cx,
                                                    );
                                                }))
                                                .child("Browse"),
                                        ),
                                )
                                .child("Repository")
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .id((
                                                    "workspace-repo-default-input",
                                                    folder_row_index,
                                                ))
                                                .track_focus(&workspace_repo_default_focus)
                                                .h(px(32.0))
                                                .min_w_0()
                                                .flex_1()
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(rgb(
                                                    if workspace_repo_default_focus
                                                        .is_focused(window)
                                                    {
                                                        accent
                                                    } else {
                                                        BORDER
                                                    },
                                                ))
                                                .bg(rgb(BG))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    let focus = this
                                                        .workspace_repo_default_input
                                                        .read(cx)
                                                        .focus_handle(cx);
                                                    window.focus(&focus);
                                                }))
                                                .child(workspace_repo_default_input.clone()),
                                        )
                                        .child(
                                            div()
                                                .id((
                                                    "browse-workspace-repository-default",
                                                    folder_row_index,
                                                ))
                                                .h(px(32.0))
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .rounded_md()
                                                .bg(rgb(BG))
                                                .text_color(rgb(TEXT))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(0x242428)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.choose_workspace_path(
                                                        WorkspacePathTarget::DefaultsRepository {
                                                            folder_id: repository_folder_id.clone(),
                                                        },
                                                        cx,
                                                    );
                                                }))
                                                .child("Browse"),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_color(rgb(MUTED))
                                        .child("Leave a path empty to inherit it."),
                                )
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_1()
                                .child(
                                    div()
                                        .id(("cancel-workspace-defaults", folder_row_index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x242428)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_workspace_defaults(cx);
                                        }))
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id(("save-workspace-defaults", folder_row_index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_color(rgb(if can_save { TEXT } else { MUTED }))
                                        .when(can_save, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(0x242428)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_save {
                                                this.save_workspace_defaults(cx);
                                            }
                                        }))
                                        .child(if submitting { "Saving…" } else { "Save" }),
                                ),
                        )
                        .into_any_element(),
                );
            }
            if creating_chat_folder.as_deref() == Some(folder.id.as_str()) {
                tree_rows.push(
                    div()
                        .ml(px(indent + 18.0))
                        .mr_2()
                        .mb_1()
                        .p_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .rounded_md()
                        .bg(rgb(SURFACE_HIGH))
                        .child(
                            div()
                                .id(("chat-create-input", folder_row_index))
                                .track_focus(&chat_create_focus)
                                .h(px(30.0))
                                .min_w_0()
                                .flex_1()
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if chat_create_focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(BG))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus = this.chat_create_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(chat_create_input.clone()),
                        )
                        .child(
                            div()
                                .id(("save-chat-create", folder_row_index))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(if can_save_chat { TEXT } else { MUTED }))
                                .when(can_save_chat, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x242428)))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_save_chat {
                                        this.save_chat_create(cx);
                                    }
                                }))
                                .child(if chat_create_submitting {
                                    "Creating…"
                                } else {
                                    "Save"
                                }),
                        )
                        .child(
                            div()
                                .id(("cancel-chat-create", folder_row_index))
                                .px_1()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .when(!chat_create_submitting, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x242428)))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !chat_create_submitting {
                                        this.cancel_chat_create(cx);
                                    }
                                }))
                                .child("×"),
                        )
                        .into_any_element(),
                );
            }
            if folder_collapsed {
                continue;
            }
            for chat in self
                .model
                .chats
                .iter()
                .filter(|chat| chat.folder == folder.id)
                .cloned()
            {
                let chat_id = chat.id.clone();
                let row_id = chat_row_index;
                chat_row_index += 1;
                let is_selected = self.model.selected_chat.as_deref() == Some(chat.id.as_str());
                let unread = self.model.unread_chats.contains(&chat.id);
                let title = chat.title.unwrap_or_else(|| "New Chat".into());
                let chat_target = SidebarTarget::Chat(chat.id.clone());
                let editing_chat = sidebar_edit
                    .as_ref()
                    .is_some_and(|edit| edit.target == chat_target);
                if editing_chat {
                    let can_save = sidebar_edit.as_ref().is_some_and(|edit| {
                        !edit.submitting
                            && !edit.text.trim().is_empty()
                            && edit.text.trim() != edit.original
                    });
                    let saving = sidebar_edit.as_ref().is_some_and(|edit| edit.submitting);
                    tree_rows.push(
                        div()
                            .mx_2()
                            .ml(px(indent + 10.0))
                            .mb_1()
                            .p_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded_md()
                            .bg(rgb(SURFACE_HIGH))
                            .child(
                                div()
                                    .id(("chat-title-editor", row_id))
                                    .track_focus(&sidebar_edit_focus)
                                    .h(px(30.0))
                                    .min_w_0()
                                    .flex_1()
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(if sidebar_edit_focus.is_focused(window) {
                                        accent
                                    } else {
                                        BORDER
                                    }))
                                    .bg(rgb(BG))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let focus =
                                            this.sidebar_edit_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }))
                                    .child(sidebar_edit_input.clone()),
                            )
                            .child(
                                div()
                                    .id(("save-chat-title", row_id))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(if can_save { TEXT } else { MUTED }))
                                    .when(can_save, |button| {
                                        button
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x242428)))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if can_save {
                                            this.save_sidebar_edit(cx);
                                        }
                                    }))
                                    .child(if saving { "…" } else { "Save" }),
                            )
                            .child(
                                div()
                                    .id(("cancel-chat-title", row_id))
                                    .px_1()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x242428)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_sidebar_edit(cx);
                                    }))
                                    .child("×"),
                            )
                            .into_any_element(),
                    );
                    continue;
                }
                let context_menu_target = chat_target.clone();
                let dragged_chat = SidebarDrag::new(chat_target.clone(), title.clone());
                let moving_chat = sidebar_move.as_ref() == Some(&chat_target);
                tree_rows.push(
                    div()
                        .id(("chat", row_id))
                        .min_w_0()
                        .mr_2()
                        .ml(px(indent + 10.0))
                        .mb_1()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(if is_selected { SURFACE_HIGH } else { SIDEBAR }))
                        .text_color(rgb(if is_selected || unread { TEXT } else { MUTED }))
                        .text_sm()
                        .when(unread, |row| row.font_weight(FontWeight::BOLD))
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                        .flex()
                        .items_center()
                        .gap_1()
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_sidebar_context_menu(
                                    Some(context_menu_target.clone()),
                                    event.position,
                                    cx,
                                );
                            }),
                        )
                        .child(
                            div()
                                .id(("select-chat", row_id))
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .cursor_move()
                                .on_drag(dragged_chat, |drag: &SidebarDrag, position, _, cx| {
                                    cx.new(|_| drag.clone().position(position))
                                })
                                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                    if !event.is_right_click() {
                                        this.select_chat(chat_id.clone(), cx);
                                    }
                                }))
                                .child(if chat.working || unread {
                                    format!("●  {title}")
                                } else {
                                    format!("   {title}")
                                }),
                        )
                        .into_any_element(),
                );
                if moving_chat {
                    let destinations = self
                        .model
                        .folders
                        .iter()
                        .filter(|destination| destination.id != folder.id)
                        .enumerate()
                        .map(|(destination_index, destination)| {
                            let target = chat_target.clone();
                            let destination_id = destination.id.clone();
                            div()
                                .id(("move-chat-destination", destination_index))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x242428)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_sidebar_item(
                                        target.clone(),
                                        Some(destination_id.clone()),
                                        cx,
                                    );
                                }))
                                .child(destination.name.clone())
                                .into_any_element()
                        })
                        .collect::<Vec<_>>();
                    tree_rows.push(
                        div()
                            .ml(px(indent + 22.0))
                            .mr_2()
                            .mb_1()
                            .p_2()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .rounded_md()
                            .bg(rgb(SURFACE_HIGH))
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .when(sidebar_move_submitting, |panel| panel.child("Moving…"))
                            .when(
                                !sidebar_move_submitting && destinations.is_empty(),
                                |panel| panel.child("No other workspace"),
                            )
                            .when(!sidebar_move_submitting, |panel| {
                                panel.children(destinations)
                            })
                            .into_any_element(),
                    );
                }
            }
        }

        let secondary_endpoint = self.active_endpoint.other();
        let secondary_is_remote = secondary_endpoint == ChatEndpoint::Remote;
        let show_secondary = !secondary_is_remote || self.remote_credentials.is_some();
        if show_secondary {
            let secondary_connected = if secondary_is_remote {
                self.remote_state == RemoteState::Connected
            } else {
                self.inactive_model.connected
            };
            let secondary_status = if secondary_is_remote {
                match self.remote_state {
                    RemoteState::Unconfigured => "not paired",
                    RemoteState::Connecting => "connecting",
                    RemoteState::Connected => "connected",
                    RemoteState::Offline => "offline",
                }
            } else if self.inactive_model.connected {
                "connected"
            } else if self.connecting {
                "connecting"
            } else {
                "offline"
            };
            let secondary_label = if secondary_is_remote {
                format!(
                    "REMOTE · {}",
                    self.remote_credentials
                        .as_ref()
                        .map(|credentials| credentials.host.as_str())
                        .unwrap_or("machine")
                )
            } else {
                "LOCAL · this machine".to_owned()
            };
            tree_rows.push(
                div()
                    .mt_3()
                    .mx_3()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(
                        div()
                            .size(px(7.0))
                            .rounded_full()
                            .bg(rgb(if secondary_connected { 0x65c985 } else { MUTED })),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .child(secondary_label),
                    )
                    .child(secondary_status)
                    .into_any_element(),
            );
            if secondary_connected {
                if self.inactive_model.folders.is_empty() {
                    tree_rows.push(
                        div()
                            .mx_3()
                            .py_3()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("No workspaces")
                            .into_any_element(),
                    );
                }
                for (folder_index, folder) in
                    self.inactive_model.folders.clone().into_iter().enumerate()
                {
                    let indent = if folder.parent.is_some() { 34.0 } else { 24.0 };
                    tree_rows.push(
                        div()
                            .id(("remote-folder", folder_index))
                            .ml(px(indent))
                            .mr_2()
                            .pt_2()
                            .pb_1()
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child(format!("▾  {}", folder.name))
                            .into_any_element(),
                    );
                    for chat in self
                        .inactive_model
                        .chats
                        .iter()
                        .filter(|chat| chat.folder == folder.id)
                    {
                        let chat_id = chat.id.clone();
                        let title = chat.title.clone().unwrap_or_else(|| "New Chat".into());
                        let unread = self.inactive_model.unread_chats.contains(&chat.id);
                        tree_rows.push(
                            div()
                                .id(SharedString::from(format!("remote-chat-{}", chat.id)))
                                .min_w_0()
                                .ml(px(indent + 10.0))
                                .mr_2()
                                .mb_1()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(if chat.working || unread { TEXT } else { MUTED }))
                                .when(unread, |row| row.font_weight(FontWeight::BOLD))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                    if !event.is_right_click() {
                                        this.select_endpoint_chat(
                                            secondary_endpoint,
                                            chat_id.clone(),
                                            cx,
                                        );
                                    }
                                }))
                                .child(if chat.working || unread {
                                    format!("●  {title}")
                                } else {
                                    format!("   {title}")
                                })
                                .into_any_element(),
                        );
                    }
                }
            } else {
                tree_rows.push(
                    div()
                        .mx_3()
                        .py_3()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(if secondary_is_remote {
                            self.remote_error
                                .clone()
                                .unwrap_or_else(|| "Remote workspaces will appear here.".into())
                        } else {
                            self.inactive_model
                                .connection_error
                                .clone()
                                .unwrap_or_else(|| "Local daemon is reconnecting.".into())
                        })
                        .into_any_element(),
                );
            }
        }

        let creating_workspace = self.creating_workspace;
        let workspace_create_submitting = self.workspace_create_submitting;
        let can_save_workspace = creating_workspace
            && !workspace_create_submitting
            && !self.workspace_create_name.trim().is_empty()
            && (self.workspace_create_repo.trim().is_empty()
                || self.workspace_create_clone.trim().is_empty())
            && self.model.connected;
        let workspace_create_input = self.workspace_create_input.clone();
        let workspace_create_focus = self.workspace_create_input.read(cx).focus_handle(cx);
        let workspace_repo_input = self.workspace_repo_input.clone();
        let workspace_repo_focus = self.workspace_repo_input.read(cx).focus_handle(cx);
        let workspace_clone_input = self.workspace_clone_input.clone();
        let workspace_clone_focus = self.workspace_clone_input.read(cx).focus_handle(cx);
        let workspace_clone_status = self.workspace_clone_status.clone();
        let settings_open = self.settings_open;
        let sidebar = div()
            .w(px(sidebar_width as f32))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(52.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().text_sm().text_color(rgb(TEXT)).child("Workspaces"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(status_color))
                                    .child(status_text),
                            )
                            .child(
                                div()
                                    .id("app-settings")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(if settings_open { SURFACE_HIGH } else { SIDEBAR }))
                                    .text_sm()
                                    .text_color(rgb(if settings_open { TEXT } else { MUTED }))
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_settings(cx);
                                    }))
                                    .child("⚙"),
                            ),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(active_machine_label)
                    .child(
                        div()
                            .id("new-workspace")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_color(rgb(if creating_workspace { MUTED } else { TEXT }))
                            .when(!creating_workspace, |button| {
                                button.cursor_pointer().hover(|style| {
                                    style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                })
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !creating_workspace {
                                    this.begin_workspace_create(cx);
                                    let focus =
                                        this.workspace_create_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }
                            }))
                            .child("+ New"),
                    ),
            )
            .when(creating_workspace, |sidebar| {
                sidebar.child(
                    div()
                        .mx_3()
                        .mb_2()
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded_md()
                        .bg(rgb(SURFACE_HIGH))
                        .child(
                            div()
                                .id("workspace-create-input")
                                .track_focus(&workspace_create_focus)
                                .h(px(32.0))
                                .min_w_0()
                                .flex_1()
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if workspace_create_focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(BG))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus =
                                        this.workspace_create_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(workspace_create_input),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .id("workspace-repo-input")
                                        .track_focus(&workspace_repo_focus)
                                        .h(px(32.0))
                                        .min_w_0()
                                        .flex_1()
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(
                                            if workspace_repo_focus.is_focused(window) {
                                                accent
                                            } else {
                                                BORDER
                                            },
                                        ))
                                        .bg(rgb(BG))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let focus =
                                                this.workspace_repo_input.read(cx).focus_handle(cx);
                                            window.focus(&focus);
                                        }))
                                        .child(workspace_repo_input),
                                )
                                .child(
                                    div()
                                        .id("browse-workspace-repository")
                                        .h(px(32.0))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded_md()
                                        .bg(rgb(BG))
                                        .text_xs()
                                        .text_color(rgb(TEXT))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x242428)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.choose_workspace_path(
                                                WorkspacePathTarget::CreateRepository,
                                                cx,
                                            );
                                        }))
                                        .child("Browse"),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child("or clone a remote repository"),
                        )
                        .child(
                            div()
                                .id("workspace-clone-input")
                                .track_focus(&workspace_clone_focus)
                                .h(px(32.0))
                                .w_full()
                                .min_w_0()
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if workspace_clone_focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(BG))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus =
                                        this.workspace_clone_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(workspace_clone_input),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_1()
                                .child(
                                    div()
                                        .id("cancel-workspace-create")
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .when(!workspace_create_submitting, |button| {
                                            button.cursor_pointer().hover(|style| {
                                                style.bg(rgb(0x242428)).text_color(rgb(TEXT))
                                            })
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !workspace_create_submitting {
                                                this.cancel_workspace_create(cx);
                                            }
                                        }))
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("save-workspace-create")
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .bg(rgb(if can_save_workspace {
                                            accent
                                        } else {
                                            SURFACE_HIGH
                                        }))
                                        .text_color(rgb(if can_save_workspace {
                                            0xffffff
                                        } else {
                                            MUTED
                                        }))
                                        .when(can_save_workspace, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(accent_hover)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_save_workspace {
                                                this.save_workspace_create(cx);
                                            }
                                        }))
                                        .child(if workspace_create_submitting {
                                            "Creating…"
                                        } else {
                                            "Create"
                                        }),
                                ),
                        ),
                )
            })
            .when_some(workspace_clone_status, |sidebar, status| {
                sidebar.child(
                    div()
                        .mx_3()
                        .mb_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(0x1d2a22))
                        .text_xs()
                        .text_color(rgb(0xa9d8b5))
                        .child(status),
                )
            })
            .child(
                div()
                    .id("workspace-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .can_drop(move |value, _, _| {
                        value.downcast_ref::<SidebarDrag>().is_some_and(|drag| {
                            sidebar_drop_allowed(&root_drop_model, &drag.target, None)
                        })
                    })
                    .drag_over::<SidebarDrag>(move |style, _, _, _| style.border_color(rgb(accent)))
                    .on_drop(cx.listener(|this, drag: &SidebarDrag, _, cx| {
                        if matches!(drag.target, SidebarTarget::Folder(_)) {
                            this.drop_sidebar_item(drag.target.clone(), None, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.open_sidebar_context_menu(None, event.position, cx);
                        }),
                    )
                    .children(tree_rows),
            );

        let title = selected
            .as_ref()
            .and_then(|chat| chat.title.clone())
            .unwrap_or_else(|| "Select a chat".into());
        let context = selected
            .as_ref()
            .map(|chat| {
                let state = match self.model.auth_state.as_str() {
                    "signed-in" => "signed in",
                    "signed-out" => "signed out",
                    "checking" => "checking sign-in",
                    "failed" => "sign-in check failed",
                    _ => "sign-in unknown",
                };
                let assistant = if chat.backend == "codex" && self.model.claude_mode {
                    "Claude mode"
                } else {
                    &chat.backend
                };
                format!("{assistant} · {state}")
            })
            .unwrap_or_else(|| "xd daemon".into());
        let header = div()
            .h(px(52.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(52.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px_4()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .child(div().text_sm().text_color(rgb(TEXT)).child(title))
                            .child(
                                div()
                                    .id("open-assistant-accounts")
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(rgb(TEXT)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_auth(cx);
                                    }))
                                    .child(context),
                            ),
                    )
                    .child(
                        div()
                            .ml_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("toggle-diff")
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(if diff_open { 0x26354d } else { SURFACE_HIGH }))
                                    .text_xs()
                                    .text_color(rgb(if can_open_diff { TEXT } else { MUTED }))
                                    .when(can_open_diff, |button| {
                                        button
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x242428)))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if can_open_diff {
                                            this.toggle_diff_panel(cx);
                                        }
                                    }))
                                    .child("Changes"),
                            )
                            .child(
                                div()
                                    .id("toggle-terminal")
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(if terminal_open {
                                        0x26354d
                                    } else {
                                        SURFACE_HIGH
                                    }))
                                    .text_xs()
                                    .text_color(rgb(if can_open_diff { TEXT } else { MUTED }))
                                    .when(can_open_diff, |button| {
                                        button
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x242428)))
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if can_open_diff {
                                            this.toggle_terminal_panel(window, cx);
                                        }
                                    }))
                                    .child("Terminal"),
                            )
                            .child(
                                div()
                                    .id("open-search")
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(SURFACE_HIGH))
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x242428)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_search(window, cx);
                                    }))
                                    .child("Search  Ctrl K"),
                            ),
                    ),
            );

        let menu_desktop = cx.entity();
        let composer_selector_menu = self.composer_menu.and_then(|menu| {
            let (title, choices) = match menu {
                ComposerMenu::Model if can_change_agent => {
                    let multiple_backends = self.model.agent_backends.len() > 1;
                    let mut choices = Vec::new();
                    for (backend_id, model_id) in filtered_models(
                        &self.model.agent_backends,
                        &self.settings.favorite_models,
                        self.model_filter.as_deref(),
                        &self.model_search,
                    ) {
                        let backend = self
                            .model
                            .agent_backends
                            .iter()
                            .find(|backend| backend.id == backend_id)?;
                        let model = backend.models.iter().find(|model| model.id == model_id)?;
                        let key = format!("{backend_id}/{model_id}");
                        let favorite = self
                            .settings
                            .favorite_models
                            .iter()
                            .any(|value| value == &key);
                        let label = if multiple_backends {
                            format!("{} · {}", backend.name, model.name)
                        } else {
                            model.name.clone()
                        };
                        choices.push((
                            label,
                            backend.id == self.model.backend
                                && self.model.model.as_deref() == Some(model.id.as_str()),
                            ComposerChoice::Model {
                                backend: backend_id.clone(),
                                model: model_id.clone(),
                            },
                            Some((backend_id, model_id, favorite)),
                        ));
                    }
                    ("Assistant", choices)
                }
                ComposerMenu::Effort if can_change_agent => {
                    let choices = self
                        .model
                        .agent_backends
                        .iter()
                        .find(|backend| backend.id == self.model.backend)
                        .map(|backend| {
                            backend
                                .efforts
                                .iter()
                                .filter(|effort| !(self.model.claude_mode && *effort == "ultra"))
                                .map(|effort| {
                                    (
                                        effort.clone(),
                                        effort == &self.model.effort,
                                        ComposerChoice::Effort(effort.clone()),
                                        None,
                                    )
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    ("Reasoning effort", choices)
                }
                ComposerMenu::Access if can_change_agent => (
                    "Access",
                    [
                        ("Read only", "read-only"),
                        ("Edit", "edit"),
                        ("Full access", "full"),
                    ]
                    .into_iter()
                    .map(|(label, access)| {
                        (
                            label.to_owned(),
                            self.model.access == access,
                            ComposerChoice::Access(access.to_owned()),
                            None,
                        )
                    })
                    .collect(),
                ),
                ComposerMenu::Workspace if can_cycle_workspace => {
                    let choices = self
                        .model
                        .worktrees
                        .iter()
                        .map(|worktree| {
                            let name = worktree
                                .branch
                                .clone()
                                .or_else(|| {
                                    PathBuf::from(&worktree.path)
                                        .file_name()
                                        .and_then(|name| name.to_str())
                                        .map(str::to_owned)
                                })
                                .unwrap_or_else(|| worktree.path.clone());
                            let label = if worktree.main {
                                format!("{name} · main")
                            } else {
                                name
                            };
                            (
                                label,
                                worktree.current,
                                ComposerChoice::Workspace(worktree.path.clone()),
                                None,
                            )
                        })
                        .collect::<Vec<_>>();
                    ("Workspace", choices)
                }
                _ => return None,
            };
            if choices.is_empty() && menu != ComposerMenu::Model {
                return None;
            }
            let model_filter_buttons = (menu == ComposerMenu::Model).then(|| {
                let mut buttons = Vec::new();
                let favorite_selected = self.model_filter.is_none();
                let desktop = menu_desktop.clone();
                buttons.push(
                    div()
                        .id("model-filter-favorites")
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(rgb(if favorite_selected {
                            0x26354d
                        } else {
                            SURFACE_HIGH
                        }))
                        .text_xs()
                        .text_color(rgb(if favorite_selected { TEXT } else { MUTED }))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x242428)).text_color(rgb(TEXT)))
                        .on_click(move |_, _, cx| {
                            desktop.update(cx, |this, cx| this.set_model_filter(None, cx));
                        })
                        .child("★ Starred")
                        .into_any_element(),
                );
                for (index, backend) in self.model.agent_backends.iter().enumerate() {
                    let backend_id = backend.id.clone();
                    let selected = self.model_filter.as_deref() == Some(backend.id.as_str());
                    let desktop = menu_desktop.clone();
                    buttons.push(
                        div()
                            .id(("model-filter-provider", index))
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .bg(rgb(if selected { 0x26354d } else { SURFACE_HIGH }))
                            .text_xs()
                            .text_color(rgb(if selected { TEXT } else { MUTED }))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242428)).text_color(rgb(TEXT)))
                            .on_click(move |_, _, cx| {
                                desktop.update(cx, |this, cx| {
                                    this.set_model_filter(Some(backend_id.clone()), cx);
                                });
                            })
                            .child(backend.name.clone())
                            .into_any_element(),
                    );
                }
                buttons
            });
            let choice_rows = choices
                .into_iter()
                .enumerate()
                .map(|(index, (label, selected, choice, favorite))| {
                    let desktop = menu_desktop.clone();
                    let label = if selected {
                        format!("✓  {label}")
                    } else {
                        label
                    };
                    let row = div()
                        .id(("composer-menu-choice", index))
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_lg()
                        .bg(rgb(if selected { 0x26354d } else { SURFACE_HIGH }))
                        .text_xs()
                        .text_color(rgb(if selected { TEXT } else { MUTED }))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x242428)).text_color(rgb(TEXT)))
                        .on_click(move |_, _, cx| {
                            desktop.update(cx, |this, cx| {
                                this.apply_composer_choice(choice.clone(), cx);
                            });
                        })
                        .child(div().min_w_0().flex_1().child(label))
                        .when(menu == ComposerMenu::Model && index < 9, |row| {
                            row.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(format!("Ctrl+{}", index + 1)),
                            )
                        });
                    row.when_some(favorite, |row, (backend, model, favorite)| {
                        let desktop = menu_desktop.clone();
                        row.child(
                            div()
                                .id(("favorite-model", index))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(if favorite { 0xf3c969 } else { MUTED }))
                                .hover(|style| style.bg(rgb(SURFACE)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    desktop.update(cx, |this, cx| {
                                        this.toggle_model_favorite(
                                            backend.clone(),
                                            model.clone(),
                                            cx,
                                        );
                                    });
                                })
                                .child(if favorite { "★" } else { "☆" }),
                        )
                    })
                    .into_any_element()
                })
                .collect::<Vec<_>>();
            let model_search = self.model_search_input.clone();
            let model_search_focus = self.model_search_input.read(cx).focus_handle(cx);
            let no_models = choice_rows.is_empty();
            Some(
                div()
                    .w_full()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BG))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .id("close-composer-menu")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_sm()
                                    .text_color(rgb(MUTED))
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.composer_menu = None;
                                        cx.notify();
                                    }))
                                    .child("×"),
                            ),
                    )
                    .when_some(model_filter_buttons, |menu, buttons| {
                        menu.child(div().flex().flex_wrap().gap_2().children(buttons))
                            .child(
                                div()
                                    .id("model-search-input")
                                    .track_focus(&model_search_focus)
                                    .h(px(38.0))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgb(if model_search_focus.is_focused(window) {
                                        accent
                                    } else {
                                        BORDER
                                    }))
                                    .bg(rgb(SURFACE))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let focus =
                                            this.model_search_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }))
                                    .child(model_search),
                            )
                    })
                    .child(
                        div()
                            .id("model-choice-list")
                            .max_h(px(260.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when(no_models, |list| {
                                list.child(
                                    div()
                                        .p_3()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child("No models match this view."),
                                )
                            })
                            .children(choice_rows),
                    )
                    .into_any_element(),
            )
        });

        let composer_controls = div()
            .h(px(42.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .id("assistant-menu")
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(SURFACE_HIGH))
                    .text_xs()
                    .text_color(rgb(if can_change_agent { TEXT } else { MUTED }))
                    .when(can_change_agent, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242428)))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if can_change_agent {
                            this.toggle_composer_menu(ComposerMenu::Model, cx);
                            if this.composer_menu == Some(ComposerMenu::Model) {
                                let focus = this.model_search_input.read(cx).focus_handle(cx);
                                window.focus(&focus);
                            }
                        }
                    }))
                    .child(format!("{model_label}  ▾")),
            )
            .when_some(context_meter, |controls, meter| {
                let color = match meter.severity {
                    ContextSeverity::Normal => accent,
                    ContextSeverity::Warning => 0xe1b95f,
                    ContextSeverity::Error => 0xe17272,
                };
                controls.child(
                    div()
                        .id("context-meter")
                        .relative()
                        .w(px(108.0))
                        .h(px(20.0))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .rounded_md()
                        .bg(rgb(SURFACE_HIGH))
                        .child(
                            div()
                                .absolute()
                                .left(px(0.0))
                                .top(px(0.0))
                                .bottom(px(0.0))
                                .w(relative(meter.fraction))
                                .bg(rgb(color)),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .child(meter.label),
                        ),
                )
            })
            .child(
                div()
                    .id("effort-menu")
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(SURFACE_HIGH))
                    .text_xs()
                    .text_color(rgb(if can_change_agent { TEXT } else { MUTED }))
                    .when(can_change_agent, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242428)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if can_change_agent {
                            this.toggle_composer_menu(ComposerMenu::Effort, cx);
                        }
                    }))
                    .child(format!("Effort: {effort_label}  ▾")),
            )
            .when(self.model.backend == "codex", |controls| {
                controls.child(
                    div()
                        .id("fast-toggle")
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .bg(rgb(if fast { 0x26354d } else { SURFACE_HIGH }))
                        .text_xs()
                        .text_color(rgb(if can_toggle_fast { TEXT } else { MUTED }))
                        .when(can_toggle_fast, |button| {
                            button
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x242428)))
                        })
                        .on_click(cx.listener(move |this, _, _, _| {
                            if can_toggle_fast {
                                this.toggle_fast();
                            }
                        }))
                        .child(if fast { "Fast: on" } else { "Fast: off" }),
                )
            })
            .when(self.model.backend == "codex", |controls| {
                controls.child(
                    div()
                        .id("claude-mode-toggle")
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .bg(rgb(if claude_mode { 0x26354d } else { SURFACE_HIGH }))
                        .text_xs()
                        .text_color(rgb(if can_toggle_claude_mode { TEXT } else { MUTED }))
                        .when(can_toggle_claude_mode, |button| {
                            button
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x242428)))
                        })
                        .on_click(cx.listener(move |this, _, _, _| {
                            if can_toggle_claude_mode {
                                this.toggle_claude_mode();
                            }
                        }))
                        .child(if claude_mode {
                            "Claude mode: on"
                        } else {
                            "Claude mode: off"
                        }),
                )
            })
            .child(
                div()
                    .id("access-menu")
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(SURFACE_HIGH))
                    .text_xs()
                    .text_color(rgb(if can_change_agent { TEXT } else { MUTED }))
                    .when(can_change_agent, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242428)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if can_change_agent {
                            this.toggle_composer_menu(ComposerMenu::Access, cx);
                        }
                    }))
                    .child(format!("{access_label}  ▾")),
            )
            .child(
                div()
                    .id("plan-toggle")
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(if self.model.plan {
                        0x26354d
                    } else {
                        SURFACE_HIGH
                    }))
                    .text_xs()
                    .text_color(rgb(if can_change_agent { TEXT } else { MUTED }))
                    .when(can_change_agent, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242428)))
                    })
                    .on_click(cx.listener(move |this, _, _, _| {
                        if can_change_agent {
                            this.toggle_plan();
                        }
                    }))
                    .child(if self.model.plan {
                        "Plan: on"
                    } else {
                        "Plan: off"
                    }),
            )
            .child(
                div()
                    .id("workspace-menu")
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(SURFACE_HIGH))
                    .text_xs()
                    .text_color(rgb(if can_cycle_workspace { TEXT } else { MUTED }))
                    .when(can_cycle_workspace, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242428)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if can_cycle_workspace {
                            this.toggle_composer_menu(ComposerMenu::Workspace, cx);
                        }
                    }))
                    .child(format!("Workspace: {workspace_label}  ▾")),
            )
            .when(can_remove_worktree, |controls| {
                controls.child(
                    div()
                        .id("remove-worktree")
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .bg(rgb(0x3c292d))
                        .text_xs()
                        .text_color(rgb(0xf1b3ba))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x513038)))
                        .on_click(cx.listener(|this, _, _, _| this.remove_selected_worktree()))
                        .child("Remove worktree"),
                )
            })
            .child(
                div()
                    .id("new-worktree-toggle")
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(if new_worktree { 0x26354d } else { SURFACE_HIGH }))
                    .text_xs()
                    .text_color(rgb(if can_change_worktree { TEXT } else { MUTED }))
                    .when(can_change_worktree, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x242428)))
                    })
                    .on_click(cx.listener(move |this, _, _, _| {
                        if can_change_worktree {
                            this.toggle_new_worktree();
                        }
                    }))
                    .child(if new_worktree {
                        "New worktree: on"
                    } else {
                        "New worktree: off"
                    }),
            )
            .child(
                div()
                    .id("turn-status")
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(if working { 0x26354d } else { SURFACE_HIGH }))
                    .text_xs()
                    .text_color(rgb(if working { 0xaec0ff } else { MUTED }))
                    .when(working, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x31435f)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if working {
                            this.cancel_turn();
                            cx.notify();
                        }
                    }))
                    .child(if working { "■ Stop turn" } else { "Ready" }),
            );

        let expanded_activity = self.expanded_activity.clone();
        let workflow_statuses = self.workflow_statuses.clone();
        let workflow_pending = self.workflow_pending.clone();
        let image_cache = self.message_images.clone();
        let transcript_daemon = self.active_daemon().cloned();
        let desktop = cx.entity();
        let transcript = if self.transcript_loading {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("Loading conversation…")
                .into_any_element()
        } else {
            list(self.transcript.clone(), move |index, _window, _cx| {
                let message = messages
                    .get(index)
                    .expect("transcript list index must match its snapshot");
                let key = message
                    .id
                    .map(|id| format!("message-{id}"))
                    .unwrap_or_else(|| format!("live-{index}"));
                Self::message_row(
                    message,
                    index,
                    expanded_activity.contains(&key),
                    &expanded_activity,
                    workflow_statuses.get(&message.content),
                    workflow_pending.contains(&message.content),
                    desktop.clone(),
                    transcript_daemon.as_ref(),
                    &image_cache,
                )
            })
            .size_full()
            .into_any_element()
        };

        let composer_focus = self.composer_input.read(cx).focus_handle(cx);
        let attachment_count = self.model.draft_attachments.len();
        let can_attach = attachment_count < MAX_ATTACHMENTS
            && self.model.selected_chat.is_some()
            && self.model.connected;
        let can_send = (!self.composer.trim().is_empty() || attachment_count > 0)
            && self.model.selected_chat.is_some()
            && self.model.connected
            && !self.sending;
        let can_voice = voice_input::AVAILABLE
            && self.model.selected_chat.is_some()
            && self.model.connected
            && !self.sending;
        let (voice_label, voice_active, voice_error) = match &self.voice_input.state {
            VoiceState::Idle => ("● Voice".to_owned(), false, None),
            VoiceState::Checking => ("Checking…".to_owned(), true, None),
            VoiceState::NeedsModel => ("Download base.en (141 MiB)".to_owned(), true, None),
            VoiceState::Downloading(progress) if *progress >= 0 => {
                (format!("Downloading {progress}%"), true, None)
            }
            VoiceState::Downloading(_) => ("Downloading…".to_owned(), true, None),
            VoiceState::Recording => ("■ Stop".to_owned(), true, None),
            VoiceState::Transcribing => ("Cancel voice".to_owned(), true, None),
            VoiceState::Failed(error) => ("Retry voice".to_owned(), false, Some(error.clone())),
        };
        let shortcuts_enabled =
            self.model.selected_chat.is_some() && self.model.connected && !self.sending;
        let shortcut_buttons = self
            .model
            .shortcuts
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, prompt)| {
                let label = compact_label(&prompt, 42);
                div()
                    .id(("shortcut", index))
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .text_xs()
                    .text_color(rgb(if shortcuts_enabled { TEXT } else { MUTED }))
                    .when(shortcuts_enabled, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if shortcuts_enabled {
                            this.send_shortcut(prompt.clone());
                            cx.notify();
                        }
                    }))
                    .child(label)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let command_buttons = command_suggestions(&self.model.commands, &self.composer)
            .into_iter()
            .enumerate()
            .map(|(index, command)| {
                let selected = command.clone();
                div()
                    .id(("slash-command", index))
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(SURFACE_HIGH))
                    .text_xs()
                    .text_color(rgb(TEXT))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0x242428)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_command(selected.clone(), window, cx);
                    }))
                    .child(format!("/{command}"))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let queue_edit = self.queue_edit.clone();
        let queue_edit_input = self.queue_edit_input.clone();
        let queue_edit_focus = self.queue_edit_input.read(cx).focus_handle(cx);
        let selected_chat_id = self.model.selected_chat.clone().unwrap_or_default();
        let queue_rows = self
            .model
            .queue
            .iter()
            .enumerate()
            .map(|(index, prompt)| {
                let editing = queue_edit.as_ref().is_some_and(|edit| {
                    edit.chat_id == selected_chat_id
                        && edit.index == index
                        && edit.original == prompt.as_str()
                });
                if editing {
                    let can_save = queue_edit.as_ref().is_some_and(|edit| {
                        edit.submitting.is_none()
                            && !edit.text.trim().is_empty()
                            && edit.text.trim() != edit.original
                    });
                    let saving = queue_edit
                        .as_ref()
                        .is_some_and(|edit| edit.submitting.is_some());
                    return div()
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(0x66557d))
                        .bg(rgb(0x211c2a))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0xc8b6e8))
                                        .child(format!("Editing queued {}", index + 1)),
                                )
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .id(("save-queue", index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .text_color(rgb(if can_save { 0xb9c7ff } else { MUTED }))
                                        .when(can_save, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(0x302b3a)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_save {
                                                this.save_queue_edit(cx);
                                            }
                                        }))
                                        .child(if saving { "Saving…" } else { "Save" }),
                                )
                                .child(
                                    div()
                                        .id(("cancel-queue-edit", index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x302b3a)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_queue_edit(cx);
                                        }))
                                        .child("Cancel"),
                                ),
                        )
                        .child(
                            div()
                                .id(("queue-editor", index))
                                .track_focus(&queue_edit_focus)
                                .mt_2()
                                .w_full()
                                .min_h(px(64.0))
                                .max_h(px(120.0))
                                .px_2()
                                .py_2()
                                .overflow_y_scroll()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if queue_edit_focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(SURFACE))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus = this.queue_edit_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(queue_edit_input.clone()),
                        )
                        .into_any_element();
                }
                let preview = queue_preview(prompt);
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x3a3348))
                    .bg(rgb(0x1d1a25))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xc8b6e8))
                                    .child(format!("Queued {}", index + 1)),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id(("edit-queue", index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(0xd7cede))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x302b3a)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.begin_queue_edit(index, cx);
                                        let focus = this.queue_edit_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }))
                                    .child("Edit"),
                            )
                            .child(
                                div()
                                    .id(("steer-queue", index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(0xb9c7ff))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x302b3a)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.steer_queued(index);
                                        cx.notify();
                                    }))
                                    .child("Send now"),
                            )
                            .child(
                                div()
                                    .id(("drop-queue", index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(0xefaaaa))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x3b282e)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.drop_queued(index);
                                        cx.notify();
                                    }))
                                    .child("Remove"),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .max_h(px(63.0))
                            .overflow_hidden()
                            .text_sm()
                            .line_height(px(21.0))
                            .text_color(rgb(0xd7cede))
                            .child(preview),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let draft_prompt = self.composer.trim();
        let global_saved = !draft_prompt.is_empty()
            && self
                .model
                .global_shortcuts
                .iter()
                .any(|prompt| prompt == draft_prompt);
        let workspace_saved = !draft_prompt.is_empty()
            && self
                .model
                .workspace_shortcuts
                .iter()
                .any(|prompt| prompt == draft_prompt);
        let can_edit_shortcuts = !draft_prompt.is_empty() && selected.is_some();
        let send_label = if self.sending {
            "Sending…"
        } else if working {
            "Queue"
        } else {
            "Send"
        };
        let open_question = self.open_question.clone().filter(|question| {
            self.model.selected_chat.as_deref() == Some(question.chat_id.as_str())
        });
        let question_panel = open_question.map(|question| {
            let can_answer = !self.sending;
            let option_buttons = question
                .options
                .into_iter()
                .enumerate()
                .map(|(index, option)| {
                    let answer = option.clone();
                    div()
                        .id(("question-option", index))
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(SURFACE_HIGH))
                        .text_sm()
                        .text_color(rgb(if can_answer { TEXT } else { MUTED }))
                        .when(can_answer, |button| {
                            button
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x242428)))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if can_answer {
                                this.answer_question(answer.clone(), cx);
                            }
                        }))
                        .child(option)
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            let question_input = self.question_input.clone();
            let question_input_focus = question_input.clone();
            let question_focus = self.question_input.read(cx).focus_handle(cx);
            let can_send_answer = can_answer && !self.question_answer.trim().is_empty();
            div()
                .id("agent-question")
                .w_full()
                .max_w(px(1040.0))
                .mx_auto()
                .mb_2()
                .p_3()
                .rounded_xl()
                .border_1()
                .border_color(rgb(accent))
                .bg(rgb(SURFACE))
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(TEXT))
                        .child(question.question),
                )
                .when(!option_buttons.is_empty(), |panel| {
                    panel.child(div().flex().flex_wrap().gap_2().children(option_buttons))
                })
                .when(question.accepts_input, |panel| {
                    panel.child(
                        div()
                            .id("question-input-row")
                            .track_focus(&question_focus)
                            .w_full()
                            .min_h(px(44.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(if question_focus.is_focused(window) {
                                accent
                            } else {
                                BORDER
                            }))
                            .bg(rgb(BG))
                            .on_click(move |_, window, cx| {
                                window.focus(&question_input_focus.read(cx).focus_handle(cx));
                            })
                            .child(question_input)
                            .child(
                                div()
                                    .id("send-question-answer")
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(if can_send_answer {
                                        accent
                                    } else {
                                        SURFACE_HIGH
                                    }))
                                    .text_sm()
                                    .text_color(rgb(if can_send_answer { 0xffffff } else { MUTED }))
                                    .when(can_send_answer, |button| {
                                        button
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(accent_hover)))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if can_send_answer {
                                            this.send_question_input(cx);
                                        }
                                    }))
                                    .child("Send"),
                            ),
                    )
                })
                .into_any_element()
        });
        let attachment_previews = self
            .model
            .draft_attachments
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, attachment)| {
                div()
                    .w(px(88.0))
                    .p_1()
                    .rounded_lg()
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .relative()
                            .h(px(60.0))
                            .overflow_hidden()
                            .rounded_md()
                            .bg(rgb(SURFACE_HIGH))
                            .child(
                                img(attachment.preview)
                                    .size_full()
                                    .object_fit(ObjectFit::Contain),
                            )
                            .child(
                                div()
                                    .id(("remove-attachment", index))
                                    .absolute()
                                    .top_1()
                                    .right_1()
                                    .size(px(20.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(rgb(0x1a1d24))
                                    .text_xs()
                                    .text_color(rgb(TEXT))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_attachment(index, cx)
                                    }))
                                    .child("×"),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .px_1()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .overflow_hidden()
                            .child(attachment.name),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let daemon_offline = !self.model.connected;
        let connection_in_flight = if remote_active {
            self.remote_state == RemoteState::Connecting
        } else {
            self.connection_in_flight
        };

        let composer = div()
            .flex_shrink_0()
            .px_3()
            .pt_2()
            .pb_3()
            .bg(rgb(BG))
            .when_some(question_panel, |element, panel| element.child(panel))
            .when_some(self.model.connection_error.clone(), |element, error| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(1040.0))
                        .mx_auto()
                        .mb_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(0x382126))
                        .text_xs()
                        .text_color(rgb(0xefb1b1))
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().min_w_0().flex_1().child(error))
                        .when(daemon_offline, |banner| {
                            banner.child(
                                div()
                                    .id("retry-daemon-connection")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(SURFACE_HIGH))
                                    .text_color(rgb(if connection_in_flight {
                                        MUTED
                                    } else {
                                        TEXT
                                    }))
                                    .when(!connection_in_flight, |button| {
                                        button
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x242428)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if this.active_endpoint == ChatEndpoint::Remote {
                                                    this.retry_remote_connection(cx);
                                                } else {
                                                    this.retry_connection(cx);
                                                }
                                            }))
                                    })
                                    .child(if connection_in_flight {
                                        "Connecting…"
                                    } else {
                                        "Retry now"
                                    }),
                            )
                        }),
                )
            })
            .when(queue_count > 0, |element| {
                element.child(
                    div()
                        .id("queue-panel")
                        .w_full()
                        .max_w(px(1040.0))
                        .mx_auto()
                        .mb_2()
                        .max_h(px(160.0))
                        .overflow_y_scroll()
                        .p_2()
                        .rounded_lg()
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .px_1()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(MUTED))
                                .child(format!(
                                    "{queue_count} queued message{}",
                                    if queue_count == 1 { "" } else { "s" }
                                )),
                        )
                        .children(queue_rows),
                )
            })
            .when(!command_buttons.is_empty(), |element| {
                element.child(
                    div()
                        .id("slash-command-suggestions")
                        .w_full()
                        .max_w(px(1040.0))
                        .mx_auto()
                        .mb_2()
                        .max_h(px(144.0))
                        .overflow_y_scroll()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .children(command_buttons),
                )
            })
            .when(!shortcut_buttons.is_empty(), |element| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(1040.0))
                        .mx_auto()
                        .mb_2()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(shortcut_buttons),
                )
            })
            .when(can_edit_shortcuts, |element| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(1040.0))
                        .mx_auto()
                        .mb_2()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .id("toggle-global-shortcut")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(SURFACE_HIGH))
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x242428)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_shortcut(false);
                                    cx.notify();
                                }))
                                .child(if global_saved {
                                    "Remove global shortcut"
                                } else {
                                    "Save as global shortcut"
                                }),
                        )
                        .child(
                            div()
                                .id("toggle-workspace-shortcut")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(SURFACE_HIGH))
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x242428)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_shortcut(true);
                                    cx.notify();
                                }))
                                .child(if workspace_saved {
                                    "Remove workspace shortcut"
                                } else {
                                    "Save as workspace shortcut"
                                }),
                        ),
                )
            })
            .when(attachment_count > 0, |element| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(1040.0))
                        .mx_auto()
                        .mb_2()
                        .flex()
                        .gap_2()
                        .children(attachment_previews),
                )
            })
            .when_some(voice_error, |element, error| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(1040.0))
                        .mx_auto()
                        .mb_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(0x382126))
                        .text_xs()
                        .text_color(rgb(0xefb1b1))
                        .child(error),
                )
            })
            .child(
                div()
                    .id("composer")
                    .track_focus(&composer_focus)
                    .w_full()
                    .max_w(px(1040.0))
                    .mx_auto()
                    .min_h(px(72.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(if composer_focus.is_focused(window) {
                        accent
                    } else {
                        BORDER
                    }))
                    .bg(rgb(SURFACE))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let focus = this.composer_input.read(cx).focus_handle(cx);
                        window.focus(&focus);
                    }))
                    .child(
                        div()
                            .id("attach")
                            .px_2()
                            .py_2()
                            .rounded_lg()
                            .text_sm()
                            .text_color(rgb(if can_attach { TEXT } else { MUTED }))
                            .when(can_attach, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_attach {
                                    this.attach_images(cx);
                                }
                            }))
                            .child("+ Attach"),
                    )
                    .when(voice_input::AVAILABLE, |composer| {
                        composer.child(
                            div()
                                .id("voice-input")
                                .px_2()
                                .py_2()
                                .rounded_lg()
                                .bg(rgb(if voice_active { 0x26354d } else { SURFACE }))
                                .text_sm()
                                .text_color(rgb(if can_voice { TEXT } else { MUTED }))
                                .when(can_voice, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_voice {
                                        this.toggle_voice(cx);
                                    }
                                }))
                                .child(voice_label),
                        )
                    })
                    .child(
                        div()
                            .id("message-editor-scroll")
                            .flex_1()
                            .min_w_0()
                            .min_h(px(40.0))
                            .max_h(px(160.0))
                            .py_2()
                            .overflow_y_scroll()
                            .child(self.composer_input.clone()),
                    )
                    .child(
                        div()
                            .id("send")
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .bg(rgb(if can_send { accent } else { SURFACE_HIGH }))
                            .text_sm()
                            .text_color(rgb(if can_send { 0xffffff } else { MUTED }))
                            .when(can_send, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(accent_hover)))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_send {
                                    this.send_composer(cx);
                                }
                            }))
                            .child(send_label),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(1040.0))
                    .mx_auto()
                    .mt_1()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .overflow_hidden()
                    .when_some(composer_selector_menu, |panel, menu| panel.child(menu))
                    .child(composer_controls),
            );

        let git_commit_input = self.git_commit_input.clone();
        let git_commit_focus = self.git_commit_input.read(cx).focus_handle(cx);
        let repo_file_filter_input = self.repo_file_filter_input.clone();
        let repo_file_filter_focus = self.repo_file_filter_input.read(cx).focus_handle(cx);
        let file_editor = self.file_editor.clone();
        let desktop_entity = cx.entity();
        let diff_pane = self.diff_panel.as_ref().map(|diff| {
            let branch_mode = diff.branch;
            let files_mode = diff.files_mode;
            let loading = diff.loading;
            let action_running = diff.action.is_some();
            let status = diff.status.as_ref();
            let can_commit = !self.git_commit_message.trim().is_empty()
                && !action_running
                && status.is_some_and(|status| !status.clean && status.conflicted == 0);
            let can_draft = !action_running
                && status.is_some_and(|status| !status.clean && status.conflicted == 0);
            let can_push = !action_running
                && status.is_some_and(|status| {
                    !status.branch.is_empty()
                        && status.branch != "(detached)"
                        && status.branch != "(initial)"
                });
            let can_prepare_pr = !action_running
                && status.is_some_and(GitStatus::can_open_pull_request)
                && diff.pr_url.is_none();
            let has_pr_draft = diff.pr_title.is_some();
            let pr_url = diff.pr_url.clone();
            let pr_enabled =
                !action_running && (pr_url.is_some() || (can_prepare_pr && !diff.pr_loading));
            let pr_label = if pr_url.is_some() {
                "View pull request"
            } else if diff.pr_loading {
                "Checking pull request…"
            } else if has_pr_draft {
                "Create pull request"
            } else {
                "Draft pull request"
            };
            let pr_body_preview = {
                let mut preview = diff.pr_body.chars().take(1_200).collect::<String>();
                if diff.pr_body.chars().count() > 1_200 {
                    preview.push('…');
                }
                preview
            };
            let pr_button = div()
                .id("git-pull-request")
                .w_full()
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(rgb(if pr_enabled { SURFACE_HIGH } else { BG }))
                .text_xs()
                .text_color(rgb(if pr_enabled { TEXT } else { MUTED }))
                .text_center()
                .when(pr_enabled, |button| {
                    button
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x242428)))
                })
                .when_some(pr_url.clone(), |button, url| {
                    button.on_click(move |_, _, cx| cx.open_url(&url))
                })
                .when(pr_url.is_none(), |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        if pr_enabled {
                            if has_pr_draft {
                                this.create_pull_request(cx);
                            } else {
                                this.draft_pull_request(cx);
                            }
                        }
                    }))
                })
                .child(pr_label);
            let status_label = status
                .map(|status| {
                    if status.clean {
                        format!("{} · clean", status.branch)
                    } else {
                        format!(
                            "{} · {} staged · {} modified · {} new",
                            status.branch, status.staged, status.unstaged, status.untracked
                        )
                    }
                })
                .unwrap_or_else(|| {
                    if diff.status_loading {
                        "Reading Git status…".into()
                    } else {
                        "Git status unavailable".into()
                    }
                });
            let total_additions = diff.files.iter().map(|file| file.additions).sum::<usize>();
            let total_deletions = diff.files.iter().map(|file| file.deletions).sum::<usize>();
            let loaded_files = diff.files.iter().filter(|file| file.loaded).count();
            let mut file_sections = Vec::new();
            for (index, file) in diff.files.iter().enumerate() {
                let collapsed = self.collapsed_diff_files.contains(&file.path);
                let path = file.path.clone();
                let mut lines = Vec::new();
                if !collapsed {
                    if file.loading {
                        lines.push(
                            div()
                                .id(("diff-file-loading", index))
                                .w_full()
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(rgb(0xaec4ff))
                                .child("Loading this file’s patch…")
                                .into_any_element(),
                        );
                    } else if let Some(error) = &file.error {
                        lines.push(
                            div()
                                .id(("diff-file-error", index))
                                .w_full()
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(rgb(0xf0a8b3))
                                .child(error.clone())
                                .into_any_element(),
                        );
                    } else {
                        for (line_index, line) in file.lines.iter().enumerate() {
                            let (background, color) = match line.kind {
                                DiffLineKind::Added => (0x172b20, 0xa9d8b5),
                                DiffLineKind::Removed => (0x332025, 0xf0a8b3),
                                DiffLineKind::Hunk => (0x1d2940, 0xaec4ff),
                                DiffLineKind::Header => (0x1d222b, 0xaab2c0),
                                DiffLineKind::Context => (0x14171c, 0xc9ced8),
                            };
                            lines.push(
                                div()
                                    .id(("diff-line", index * 10_000 + line_index))
                                    .w_full()
                                    .px_2()
                                    .py(px(1.0))
                                    .bg(rgb(background))
                                    .font_family("monospace")
                                    .text_xs()
                                    .line_height(px(18.0))
                                    .text_color(rgb(color))
                                    .child(line.text.clone())
                                    .into_any_element(),
                            );
                        }
                    }
                }
                file_sections.push(
                    div()
                        .id(("diff-file", index))
                        .w_full()
                        .mb_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .overflow_hidden()
                        .child(
                            div()
                                .id(("toggle-diff-file", index))
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .bg(rgb(SURFACE_HIGH))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x242428)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_diff_file(path.clone(), cx);
                                }))
                                .child(if collapsed { "▸" } else { "▾" })
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .text_sm()
                                        .text_color(rgb(TEXT))
                                        .child(file.path.clone()),
                                )
                                .when_some(file.status.clone(), |header, status| {
                                    header
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(status))
                                })
                                .when(file.loaded, |header| {
                                    header.child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x78c995))
                                            .child(format!("+{}", file.additions)),
                                    )
                                })
                                .when(file.loaded, |header| {
                                    header.child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0xe88e9c))
                                            .child(format!("−{}", file.deletions)),
                                    )
                                }),
                        )
                        .children(lines)
                        .into_any_element(),
                );
            }
            let message = diff.error.clone().unwrap_or_else(|| {
                if loading {
                    "Loading changes…".into()
                } else {
                    "No changes".into()
                }
            });
            let pane_content = if files_mode {
                if let Some(preview) = diff.file_preview.clone() {
                    let modified = preview.content != preview.original;
                    let saving = preview.saving;
                    let markdown_scope = format!("workspace-{}", preview.path);
                    let language = preview
                        .path
                        .rsplit_once('.')
                        .map(|(_, extension)| extension)
                        .filter(|extension| extension.len() <= 24);
                    let document = std::sync::Arc::new(markdown::code_document(
                        language,
                        &preview.content,
                        preview.truncated,
                    ));
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .border_b_1()
                                .border_color(rgb(BORDER))
                                .child(
                                    div()
                                        .id("back-to-workspace-files")
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .text_xs()
                                        .text_color(rgb(0xaec4ff))
                                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_file_preview(cx);
                                        }))
                                        .child("← Files"),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(rgb(TEXT))
                                        .child(preview.path),
                                )
                                .child(
                                    div()
                                        .id("refresh-workspace-file")
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.refresh_browse_file(cx);
                                        }))
                                        .child("Refresh"),
                                )
                                .when(modified, |header| {
                                    header.child(
                                        div()
                                            .id("discard-repository-file")
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.discard_repository_changes(cx);
                                            }))
                                            .child("Discard"),
                                    )
                                })
                                .child(
                                    div()
                                        .id("save-repository-file")
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(rgb(if modified && !saving {
                                            accent
                                        } else {
                                            SURFACE_HIGH
                                        }))
                                        .cursor_pointer()
                                        .text_xs()
                                        .text_color(rgb(if modified && !saving {
                                            0xffffff
                                        } else {
                                            MUTED
                                        }))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_browse_file(cx);
                                        }))
                                        .child(if saving { "Saving…" } else { "Save" }),
                                ),
                        )
                        .when(preview.truncated, |body| {
                            body.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .bg(rgb(0x332d1c))
                                    .text_xs()
                                    .text_color(rgb(0xe0c178))
                                    .child(
                                        "Large file preview truncated for responsive rendering.",
                                    ),
                            )
                        })
                        .child(if preview.truncated {
                            div()
                                .id("workspace-file-preview")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .p_3()
                                .child(Self::markdown_content(
                                    document,
                                    &markdown_scope,
                                    None,
                                    None,
                                    None,
                                ))
                                .into_any_element()
                        } else {
                            div()
                                .id("workspace-file-editor")
                                .flex_1()
                                .min_h_0()
                                .overflow_scroll()
                                .p_3()
                                .bg(rgb(0x0d0e11))
                                .child(file_editor.clone())
                                .into_any_element()
                        })
                        .into_any_element()
                } else {
                    let filter = self.repo_file_filter.trim().to_ascii_lowercase();
                    let matching = diff
                        .browse_entries
                        .iter()
                        .filter(|entry| {
                            filter.is_empty() || entry.name.to_ascii_lowercase().contains(&filter)
                        })
                        .count();
                    let mut rows = Vec::new();
                    for (index, entry) in diff
                        .browse_entries
                        .iter()
                        .filter(|entry| {
                            filter.is_empty() || entry.name.to_ascii_lowercase().contains(&filter)
                        })
                        .take(400)
                        .cloned()
                        .enumerate()
                    {
                        let label = entry.name.clone();
                        let directory = entry.directory;
                        let desktop = desktop_entity.clone();
                        rows.push(
                            div()
                                .id(("workspace-entry", index))
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(move |_, _, cx| {
                                    desktop.update(cx, |this, cx| {
                                        this.activate_browse_entry(entry.clone(), cx);
                                    });
                                })
                                .child(if directory { "▸" } else { " " })
                                .child(label)
                                .into_any_element(),
                        );
                    }
                    let list_message = diff.error.clone().unwrap_or_else(|| {
                        if loading {
                            "Loading directory…".into()
                        } else if self.repo_file_filter.trim().is_empty() {
                            "Empty directory".into()
                        } else {
                            "No matching files".into()
                        }
                    });
                    let at_root = diff.browse_path.is_empty();
                    let browse_label = if at_root {
                        "working directory".to_owned()
                    } else {
                        diff.browse_path.clone()
                    };
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .p_3()
                                .border_b_1()
                                .border_color(rgb(BORDER))
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("workspace-files-up")
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .text_xs()
                                                .text_color(rgb(if at_root { MUTED } else { TEXT }))
                                                .when(!at_root, |button| {
                                                    button
                                                        .cursor_pointer()
                                                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.browse_up(cx);
                                                }))
                                                .child("↑ Up"),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child(browse_label),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("workspace-file-filter")
                                        .track_focus(&repo_file_filter_focus)
                                        .h(px(36.0))
                                        .w_full()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(
                                            if repo_file_filter_focus.is_focused(window) {
                                                accent
                                            } else {
                                                BORDER
                                            },
                                        ))
                                        .bg(rgb(SURFACE))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let focus = this
                                                .repo_file_filter_input
                                                .read(cx)
                                                .focus_handle(cx);
                                            window.focus(&focus);
                                        }))
                                        .child(repo_file_filter_input.clone()),
                                ),
                        )
                        .when(diff.file_loading, |body| {
                            body.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .text_xs()
                                    .text_color(rgb(0xaec4ff))
                                    .child("Opening file…"),
                            )
                        })
                        .when_some(diff.error.clone(), |body, error| {
                            body.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .text_xs()
                                    .text_color(rgb(0xf0a8b3))
                                    .child(error),
                            )
                        })
                        .when(matching > 400, |body| {
                            body.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .text_xs()
                                    .text_color(rgb(0xe0c178))
                                    .child("Showing the first 400 matches. Filter to narrow it."),
                            )
                        })
                        .child(
                            div()
                                .id("workspace-files")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .p_2()
                                .when(rows.is_empty(), |body| {
                                    body.child(
                                        div()
                                            .h(px(180.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child(list_message),
                                    )
                                })
                                .children(rows),
                        )
                        .into_any_element()
                }
            } else {
                div()
                    .id("diff-files")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .when(file_sections.is_empty(), |body| {
                        body.child(
                            div()
                                .h(px(180.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .child(message),
                        )
                    })
                    .children(file_sections)
                    .into_any_element()
            };
            div()
                .w(px(diff_width as f32))
                .h_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(rgb(BORDER))
                .bg(rgb(BG))
                .child(
                    div()
                        .h(px(58.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().text_color(rgb(TEXT)).child(if files_mode {
                                    "Workspace files"
                                } else {
                                    "Repository changes"
                                }))
                                .child(div().text_xs().text_color(rgb(MUTED)).child(
                                    if files_mode {
                                        format!("{} entries", diff.browse_entries.len())
                                    } else if loaded_files < diff.files.len() {
                                        format!(
                                            "{} files · {} loaded  +{}  −{}",
                                            diff.files.len(),
                                            loaded_files,
                                            total_additions,
                                            total_deletions
                                        )
                                    } else {
                                        format!(
                                            "{} files  +{}  −{}",
                                            diff.files.len(),
                                            total_additions,
                                            total_deletions
                                        )
                                    },
                                )),
                        )
                        .child(
                            div()
                                .id("diff-working-mode")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(if !files_mode && !branch_mode {
                                    accent
                                } else {
                                    SURFACE_HIGH
                                }))
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_diff_mode(false, cx);
                                }))
                                .child("Working"),
                        )
                        .child(
                            div()
                                .id("diff-branch-mode")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(if !files_mode && branch_mode {
                                    accent
                                } else {
                                    SURFACE_HIGH
                                }))
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_diff_mode(true, cx);
                                }))
                                .child("Branch"),
                        )
                        .child(
                            div()
                                .id("diff-files-mode")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(if files_mode { accent } else { SURFACE_HIGH }))
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_files_mode(cx);
                                }))
                                .child("Files"),
                        )
                        .child(
                            div()
                                .id("refresh-diff")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.refresh_diff(cx);
                                }))
                                .child("↻"),
                        )
                        .child(
                            div()
                                .id("close-diff")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_diff_panel(cx);
                                }))
                                .child("×"),
                        ),
                )
                .when(diff.truncated, |pane| {
                    pane.child(
                        div()
                            .px_3()
                            .py_2()
                            .bg(rgb(0x332d1c))
                            .text_xs()
                            .text_color(rgb(0xe0c178))
                            .child("Large diff truncated for responsive rendering."),
                    )
                })
                .child(pane_content)
                .when(!files_mode, |pane| {
                    pane.child(
                        div()
                            .w_full()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .border_t_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(SURFACE))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child(status_label),
                                    )
                                    .when_some(status, |row, status| {
                                        row.child(div().text_xs().text_color(rgb(MUTED)).child(
                                            if status.upstream.is_empty() {
                                                "not published".into()
                                            } else {
                                                format!("↑{} ↓{}", status.ahead, status.behind)
                                            },
                                        ))
                                    }),
                            )
                            .when_some(
                                status.and_then(|status| {
                                    (status.conflicted > 0).then(|| {
                                        format!(
                                            "Resolve {} conflicted file(s) before committing.",
                                            status.conflicted
                                        )
                                    })
                                }),
                                |footer, warning| {
                                    footer.child(
                                        div().text_xs().text_color(rgb(0xe0c178)).child(warning),
                                    )
                                },
                            )
                            .when_some(diff.action_error.clone(), |footer, error| {
                                footer.child(div().text_xs().text_color(rgb(0xf0a8b3)).child(error))
                            })
                            .when_some(diff.action.clone(), |footer, action| {
                                footer
                                    .child(div().text_xs().text_color(rgb(0xaec4ff)).child(action))
                            })
                            .when_some(diff.pr_title.clone(), |footer, title| {
                                footer.child(
                                    div()
                                        .w_full()
                                        .p_3()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(BORDER))
                                        .bg(rgb(BG))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .flex_1()
                                                        .text_sm()
                                                        .text_color(rgb(TEXT))
                                                        .child(title),
                                                )
                                                .child(
                                                    div()
                                                        .id("discard-pr-draft")
                                                        .px_2()
                                                        .py_1()
                                                        .rounded_md()
                                                        .text_xs()
                                                        .text_color(rgb(MUTED))
                                                        .cursor_pointer()
                                                        .hover(|style| {
                                                            style
                                                                .bg(rgb(SURFACE_HIGH))
                                                                .text_color(rgb(TEXT))
                                                        })
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.discard_pull_request_draft(cx);
                                                        }))
                                                        .child("Discard"),
                                                ),
                                        )
                                        .when(!pr_body_preview.is_empty(), |review| {
                                            review.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(MUTED))
                                                    .child(pr_body_preview),
                                            )
                                        }),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("git-commit-input")
                                            .track_focus(&git_commit_focus)
                                            .h(px(36.0))
                                            .min_w_0()
                                            .flex_1()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(rgb(
                                                if git_commit_focus.is_focused(window) {
                                                    accent
                                                } else {
                                                    BORDER
                                                },
                                            ))
                                            .bg(rgb(BG))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                let focus =
                                                    this.git_commit_input.read(cx).focus_handle(cx);
                                                window.focus(&focus);
                                            }))
                                            .child(git_commit_input.clone()),
                                    )
                                    .child(
                                        div()
                                            .id("git-draft-commit")
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .bg(rgb(if can_draft { SURFACE_HIGH } else { BG }))
                                            .text_xs()
                                            .text_color(rgb(if can_draft { TEXT } else { MUTED }))
                                            .when(can_draft, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(rgb(0x242428)))
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if can_draft {
                                                    this.draft_commit_message(cx);
                                                }
                                            }))
                                            .child("Draft"),
                                    )
                                    .child(
                                        div()
                                            .id("git-commit-all")
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .bg(rgb(if can_commit { accent } else { SURFACE_HIGH }))
                                            .text_xs()
                                            .text_color(rgb(if can_commit {
                                                0xffffff
                                            } else {
                                                MUTED
                                            }))
                                            .when(can_commit, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(rgb(accent_hover)))
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if can_commit {
                                                    this.commit_changes(cx);
                                                }
                                            }))
                                            .child("Commit all"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("git-push")
                                    .w_full()
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(if can_push { SURFACE_HIGH } else { BG }))
                                    .text_xs()
                                    .text_color(rgb(if can_push { TEXT } else { MUTED }))
                                    .text_center()
                                    .when(can_push, |button| {
                                        button
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x242428)))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if can_push {
                                            this.push_changes(cx);
                                        }
                                    }))
                                    .child("Push branch"),
                            )
                            .child(pr_button),
                    )
                })
                .into_any_element()
        });

        let terminal_input = self.terminal_input.clone();
        let terminal_focus = self.terminal_input.read(cx).focus_handle(cx);
        let terminal_desktop = cx.entity();
        let terminal_pane = self.terminal_panel.as_ref().map(|panel| {
            let selected_id = panel.selected.clone();
            let output = panel
                .selected()
                .map(|session| session.screen.rendered_with_cursor());
            let (output_text, output_spans, output_cursor) = output
                .map(|output| (output.text, output.spans, output.cursor))
                .unwrap_or_else(|| (String::new(), Vec::new(), None));
            let mut highlights = output_spans
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
            if self.terminal_cursor_visible
                && terminal_focus.is_focused(window)
                && let Some(cursor) = output_cursor
            {
                highlights.push((
                    cursor,
                    HighlightStyle {
                        color: Some(rgb(BG).into()),
                        background_color: Some(rgb(0xd8dee9).into()),
                        ..Default::default()
                    },
                ));
            }
            let output = StyledText::new(output_text).with_highlights(highlights);
            let active = selected_id.is_some() && !panel.loading;
            let show_tabs = panel.sessions.len() > 1;
            let centered_title = (!show_tabs)
                .then(|| panel.selected().map(|session| session.title.clone()))
                .flatten();
            let terminal_tabs = if show_tabs {
                panel
                    .sessions
                    .iter()
                    .enumerate()
                    .map(|(index, session)| {
                        let id = session.id.clone();
                        let close_id = id.clone();
                        let selected = selected_id.as_deref() == Some(id.as_str());
                        div()
                            .id(("terminal-tab", index))
                            .h_full()
                            .flex_shrink_0()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .border_b_1()
                            .border_color(rgb(if selected { accent } else { BORDER }))
                            .bg(rgb(if selected { SURFACE_HIGH } else { BG }))
                            .text_xs()
                            .text_color(rgb(if selected { TEXT } else { MUTED }))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_terminal(id.clone(), cx);
                            }))
                            .child(session.title.clone())
                            .child(
                                div()
                                    .id(("close-terminal-tab", index))
                                    .px_1()
                                    .rounded_sm()
                                    .hover(|style| {
                                        style.bg(rgb(0x3c292d)).text_color(rgb(0xf1b3ba))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.kill_terminal_id(close_id.clone(), cx);
                                    }))
                                    .child("×"),
                            )
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            div()
                .w_full()
                .h(px(terminal_height as f32))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(rgb(BORDER))
                .bg(rgb(BG))
                .child(
                    div()
                        .relative()
                        .h(px(40.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .when_some(centered_title, |header, title| {
                            header.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px(px(96.0))
                                    .overflow_hidden()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(TEXT))
                                    .child(title),
                            )
                        })
                        .child(
                            div()
                                .h_full()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .items_center()
                                .overflow_hidden()
                                .children(terminal_tabs),
                        )
                        .child(
                            div()
                                .id("new-terminal-session")
                                .size(px(28.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.new_terminal_session(cx)),
                                )
                                .child(plus_icon(TEXT)),
                        )
                        .when(active, |header| {
                            header.child(
                                div()
                                    .id("kill-terminal")
                                    .size(px(28.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x3c292d)))
                                    .on_click(cx.listener(|this, _, _, cx| this.kill_terminal(cx)))
                                    .child(trash_icon(0xf1b3ba)),
                            )
                        }),
                )
                .when(panel.loading, |pane| {
                    pane.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_xs()
                            .text_color(rgb(0xaec4ff))
                            .child("Opening terminal…"),
                    )
                })
                .when(!panel.loading && panel.sessions.is_empty(), |pane| {
                    pane.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("No terminal sessions. Press + to open one."),
                    )
                })
                .when_some(panel.error.clone(), |pane, error| {
                    pane.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_xs()
                            .text_color(rgb(0xf0a8b3))
                            .child(error),
                    )
                })
                .child(
                    div()
                        .id("terminal-output")
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_3()
                        .font_family("monospace")
                        .text_size(px(13.0))
                        .line_height(px(19.0))
                        .text_color(rgb(0xd8dee9))
                        .track_focus(&terminal_focus)
                        .when(active, |output| {
                            output.cursor(CursorStyle::IBeam).on_click(cx.listener(
                                |this, _, window, cx| {
                                    let focus = this.terminal_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                },
                            ))
                        })
                        .child(
                            canvas(
                                {
                                    let desktop = terminal_desktop.clone();
                                    move |bounds, window, cx| {
                                        const SAMPLE: &str =
                                            "MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM";
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
                                        let cell_width =
                                            f32::from(line.width) / SAMPLE.len() as f32;
                                        let geometry = terminal_geometry(
                                            f32::from(bounds.size.width),
                                            f32::from(bounds.size.height),
                                            cell_width,
                                            19.0,
                                        );
                                        window.defer(cx, move |_, cx| {
                                            desktop.update(cx, |this, cx| {
                                                this.resize_terminal_viewport(
                                                    geometry.0, geometry.1, cx,
                                                );
                                            });
                                        });
                                    }
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .child(output)
                        .child(terminal_input.clone()),
                )
                .into_any_element()
        });

        let auth_input = self.auth_input.clone();
        let account_machine = if remote_active {
            format!(
                "the remote machine at {}",
                self.remote_credentials
                    .as_ref()
                    .map(|credentials| credentials.host.as_str())
                    .unwrap_or("the paired address")
            )
        } else {
            "this machine".to_owned()
        };
        let auth_overlay = self.auth_open.then(|| {
            let rows = self
                .auth_providers
                .iter()
                .enumerate()
                .map(|(index, provider)| {
                    let action_label = match provider.state.as_str() {
                        "signed-in" => "Sign Out",
                        "signing-in" => "Cancel",
                        "checking" => "Checking…",
                        "signing-out" => "Signing Out…",
                        _ => "Sign In",
                    };
                    let action_enabled = !matches!(provider.state.as_str(), "checking" | "signing-out");
                    let action_provider = provider.provider.clone();
                    let action_state = provider.state.clone();
                    let login_url = provider.login_url.clone();
                    let device_code = provider.device_code.clone();
                    let needs_input = provider.needs_input;
                    div()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(SURFACE))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child(provider.display_name.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child(provider.detail.clone().unwrap_or_else(|| provider.state.clone())),
                                        ),
                                )
                                .child(
                                    div()
                                        .id(("auth-action", index))
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if action_enabled { SURFACE_HIGH } else { BG }))
                                        .text_xs()
                                        .text_color(rgb(if action_enabled { TEXT } else { MUTED }))
                                        .when(action_enabled, |button| {
                                            button.cursor_pointer().hover(|style| style.bg(rgb(0x242428)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if action_enabled {
                                                this.auth_action(&action_provider, &action_state, cx);
                                            }
                                        }))
                                        .child(action_label),
                                ),
                        )
                        .when_some(login_url, |row, url| {
                            let opened_url = url.clone();
                            row.child(
                                div()
                                    .id(("open-auth-url", index))
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(0x26354d))
                                    .text_xs()
                                    .text_color(rgb(TEXT))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x304462)))
                                    .on_click(move |_, _, cx| cx.open_url(&opened_url))
                                    .child(format!("Open sign-in page · {url}")),
                            )
                        })
                        .when_some(device_code, |row, code| {
                            let copied_code = code.clone();
                            row.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .font_family("monospace")
                                            .text_sm()
                                            .text_color(rgb(TEXT))
                                            .child(format!("One-time code: {code}")),
                                    )
                                    .child(
                                        div()
                                            .id(("copy-auth-code", index))
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .bg(rgb(SURFACE_HIGH))
                                            .text_xs()
                                            .text_color(rgb(TEXT))
                                            .cursor_pointer()
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(copied_code.clone()));
                                            })
                                            .child("Copy"),
                                    ),
                            )
                        })
                        .when(needs_input, |row| {
                            row.child(
                                div()
                                    .h(px(42.0))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(BG))
                                    .child(auth_input.clone()),
                            )
                            .child(
                                div()
                                    .id(("submit-auth-code", index))
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(accent))
                                    .text_xs()
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.submit_auth_input(cx)))
                                    .child("Finish sign-in"),
                            )
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            let cli_rows = self
                .cli_versions
                .iter()
                .enumerate()
                .map(|(index, version)| {
                    let failed = version.state == "failed";
                    let detail = if version.state == "checking" {
                        "Checking…".to_owned()
                    } else if failed {
                        version
                            .detail
                            .clone()
                            .unwrap_or_else(|| "Version check failed".into())
                    } else if let Some(version) = &version.version {
                        version.clone()
                    } else {
                        version
                            .detail
                            .clone()
                            .unwrap_or_else(|| "Version unavailable".into())
                    };
                    div()
                        .id(("assistant-cli-version", index))
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(version.display_name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .font_family("monospace")
                                .text_xs()
                                .text_color(rgb(if failed { 0xefaaaa } else { TEXT }))
                                .child(detail),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(560.0))
                        .max_h(px(680.0))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .child(div().text_lg().text_color(rgb(TEXT)).child("Assistant Accounts"))
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                                            "Credentials stay on {account_machine} and are used only by that Rust daemon."
                                        ))),
                                )
                                .child(
                                    div()
                                        .id("close-auth")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_auth(cx)))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .p_3()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(TEXT))
                                                        .child("Bundled assistant CLIs"),
                                                )
                                                .child(div().text_xs().text_color(rgb(MUTED)).child(
                                                    "Updated only when xd updates.",
                                                )),
                                        )
                                        .child(
                                            div()
                                                .id("refresh-assistant-cli-versions")
                                                .px_3()
                                                .py_2()
                                                .rounded_lg()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .when(!self.cli_versions_loading, |button| {
                                                    button.cursor_pointer().hover(|style| {
                                                        style
                                                            .bg(rgb(SURFACE_HIGH))
                                                            .text_color(rgb(TEXT))
                                                    })
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.refresh_cli_versions(cx);
                                                }))
                                                .child(if self.cli_versions_loading {
                                                    "Checking…"
                                                } else {
                                                    "Refresh"
                                                }),
                                        ),
                                )
                                .when(cli_rows.is_empty(), |card| {
                                    card.child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child("Checking bundled versions…"),
                                    )
                                })
                                .children(cli_rows)
                                .when_some(self.cli_versions_error.clone(), |card, error| {
                                    card.child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0xefaaaa))
                                            .child(error),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .id("auth-accounts-list")
                                .min_h_0()
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .children(rows),
                        ),
                )
                .into_any_element()
        });

        let settings_overlay = self.settings_open.then(|| {
            let remote_status = match self.remote_state {
                RemoteState::Unconfigured => "Not connected",
                RemoteState::Connecting => "Connecting…",
                RemoteState::Connected => "Connected",
                RemoteState::Offline => "Offline · retrying",
            };
            let mut accent_buttons = Vec::new();
            for (index, preset) in AccentPreset::ALL.into_iter().enumerate() {
                let selected = self.settings.accent == preset;
                accent_buttons.push(
                    div()
                        .id(("accent-preset", index))
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(if selected { preset.color() } else { BORDER }))
                        .bg(rgb(if selected { SURFACE_HIGH } else { SURFACE }))
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_accent(preset, cx);
                        }))
                        .child(div().size(px(12.0)).rounded_full().bg(rgb(preset.color())))
                        .child(preset.label())
                        .into_any_element(),
                );
            }
            let writer = self.settings.git_writer;
            let mut writer_buttons = Vec::new();
            for (index, choice) in GitWriter::ALL.into_iter().enumerate() {
                let selected = writer == choice;
                writer_buttons.push(
                    div()
                        .id(("git-writer-choice", index))
                        .w_full()
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .rounded_md()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_git_writer(choice, cx);
                        }))
                        .child(choice.label())
                        .when(selected, |row| {
                            row.child(div().text_color(rgb(accent)).child("✓"))
                        })
                        .into_any_element(),
                );
            }
            let writer_backend = writer.backend();
            let writer_backend_catalog = writer_backend.and_then(|backend| {
                self.model
                    .agent_backends
                    .iter()
                    .find(|candidate| candidate.id == backend)
            });
            let selected_writer_model =
                self.settings.git_writer_model.as_deref().or_else(|| {
                    writer_backend_catalog.map(|backend| backend.default_model.as_str())
                });
            let writer_model_label = writer_backend_catalog
                .and_then(|backend| {
                    backend
                        .models
                        .iter()
                        .find(|model| Some(model.id.as_str()) == selected_writer_model)
                })
                .map(|model| model.name.clone())
                .or_else(|| selected_writer_model.map(str::to_owned))
                .unwrap_or_else(|| "Default model".into());
            let mut model_buttons = Vec::new();
            if let Some(backend) = writer_backend_catalog {
                for (index, model) in backend.models.iter().enumerate() {
                    let model_id = model.id.clone();
                    let selected = Some(model.id.as_str()) == selected_writer_model;
                    model_buttons.push(
                        div()
                            .id(("git-writer-model-choice", index))
                            .w_full()
                            .px_3()
                            .py_2()
                            .flex()
                            .items_center()
                            .justify_between()
                            .rounded_md()
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_git_writer_model(model_id.clone(), cx);
                            }))
                            .child(model.name.clone())
                            .when(selected, |row| {
                                row.child(div().text_color(rgb(accent)).child("✓"))
                            })
                            .into_any_element(),
                    );
                }
            }
            let notifications = self.settings.notifications;
            let speech = self.settings.speech;
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .id("app-settings-panel")
                        .w(px(720.0))
                        .h(px(520.0))
                        .overflow_hidden()
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_lg().text_color(rgb(TEXT)).child("App settings"))
                                .child(
                                    div()
                                        .id("close-app-settings")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.toggle_settings(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .gap_4()
                                .child(
                                    div()
                                        .id("settings-preferences-column")
                                        .w(px(316.0))
                                        .h_full()
                                        .pr_2()
                                        .overflow_y_scroll()
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(TEXT))
                                                .child("Preferences"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(MUTED))
                                                        .child("ACCENT COLOR"),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_wrap()
                                                        .gap_2()
                                                        .children(accent_buttons),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(MUTED))
                                                        .child("GIT WRITING ASSISTANT"),
                                                )
                                                .child(
                                                    div()
                                        .id("open-git-writer-menu")
                                        .w_full()
                                        .p_3()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(BORDER))
                                        .bg(rgb(SURFACE))
                                        .text_sm()
                                        .text_color(rgb(TEXT))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.toggle_settings_menu(SettingsMenu::GitWriter, cx);
                                        }))
                                        .child(writer.label())
                                        .child(div().text_color(rgb(MUTED)).child(
                                            if self.settings_menu == Some(SettingsMenu::GitWriter) {
                                                "▴"
                                            } else {
                                                "▾"
                                            },
                                        )),
                                                )
                                .when(
                                    self.settings_menu == Some(SettingsMenu::GitWriter),
                                    |section| {
                                        section.child(
                                            div()
                                                .w_full()
                                                .p_1()
                                                .rounded_lg()
                                                .border_1()
                                                .border_color(rgb(BORDER))
                                                .bg(rgb(SURFACE))
                                                .children(writer_buttons),
                                        )
                                    },
                                )
                                .when(writer_backend.is_some(), |section| {
                                    section
                                        .child(
                                            div()
                                                .mt_1()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child("MODEL"),
                                        )
                                        .child(
                                            div()
                                                .id("open-git-writer-model-menu")
                                                .w_full()
                                                .p_3()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .rounded_lg()
                                                .border_1()
                                                .border_color(rgb(BORDER))
                                                .bg(rgb(SURFACE))
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.toggle_settings_menu(
                                                        SettingsMenu::GitWriterModel,
                                                        cx,
                                                    );
                                                }))
                                                .child(writer_model_label)
                                                .child(div().text_color(rgb(MUTED)).child(
                                                    if self.settings_menu
                                                        == Some(SettingsMenu::GitWriterModel)
                                                    {
                                                        "▴"
                                                    } else {
                                                        "▾"
                                                    },
                                                )),
                                        )
                                        .when(
                                            self.settings_menu
                                                == Some(SettingsMenu::GitWriterModel),
                                            |section| {
                                                section.child(
                                                    div()
                                                        .w_full()
                                                        .p_1()
                                                        .rounded_lg()
                                                        .border_1()
                                                        .border_color(rgb(BORDER))
                                                        .bg(rgb(SURFACE))
                                                        .children(model_buttons),
                                                )
                                            },
                                        )
                                })
                                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child("Used for AI commit and pull request drafts."),
                                                ),
                                        )
                                        .child(
                            div()
                                .id("toggle-notifications")
                                .p_3()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_notifications(cx);
                                }))
                                .child(
                                    div().min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child("Background turn notifications"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child("Notify when another chat finishes."),
                                        ),
                                )
                                .child(
                                    div()
                                        .w(px(38.0))
                                        .h(px(22.0))
                                        .p(px(3.0))
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .when(!notifications, |toggle| toggle.justify_start())
                                        .rounded_full()
                                        .bg(rgb(if notifications { accent } else { BORDER }))
                                        .child(
                                            div().size(px(16.0)).rounded_full().bg(rgb(0xffffff)),
                                        ),
                                ),
                                        )
                                        .child(
                            div()
                                .id("toggle-speech")
                                .p_3()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_speech(cx);
                                }))
                                .child(
                                    div().min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child("Selective spoken replies"),
                                        )
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                                            "Speak only completed <speak> sections locally.",
                                        )),
                                )
                                .child(
                                    div()
                                        .w(px(38.0))
                                        .h(px(22.0))
                                        .p(px(3.0))
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .when(!speech, |toggle| toggle.justify_start())
                                        .rounded_full()
                                        .bg(rgb(if speech { accent } else { BORDER }))
                                        .child(
                                            div().size(px(16.0)).rounded_full().bg(rgb(0xffffff)),
                                        ),
                                ),
                                        ),
                                )
                                .child(div().w(px(1.0)).h_full().bg(rgb(BORDER)))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .h_full()
                                        .pr_1()
                                        .id("settings-application-column")
                                        .overflow_y_scroll()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(
                                            div()
                                                .mb_1()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(TEXT))
                                                .child("Application & devices"),
                                        )
                                        .child(
                            div()
                                .id("open-global-shortcuts")
                                .p_3()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_shortcut_panel(None, None, cx);
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child("Global shortcuts"),
                                        )
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                                            "Manage prompt buttons available in every workspace.",
                                        )),
                                )
                                .child(div().text_color(rgb(MUTED)).child("›")),
                                        )
                                        .when(source_build::supported(), |column| {
                                            column.child(
                                                div()
                                                    .id("open-source-build")
                                                    .p_3()
                                                    .flex()
                                                    .items_center()
                                                    .gap_3()
                                                    .rounded_lg()
                                                    .border_1()
                                                    .border_color(rgb(BORDER))
                                                    .bg(rgb(SURFACE))
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.open_source_build(window, cx);
                                                        },
                                                    ))
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .flex_1()
                                                            .flex()
                                                            .flex_col()
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .text_color(rgb(TEXT))
                                                                    .child("Build XD Source"),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(rgb(MUTED))
                                                                    .child(
                                                                        "Build and install a branch, PR, or commit.",
                                                                    ),
                                                            ),
                                                    )
                                                    .child(
                                                        div().text_color(rgb(MUTED)).child("›"),
                                                    ),
                                            )
                                        })
                                        .child(
                            div()
                                .id("open-daemon-update")
                                .p_3()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_self_update(cx);
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child("Update xd"),
                                        )
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                                            "Check, install, and explicitly restart this daemon.",
                                        )),
                                )
                                .child(div().text_color(rgb(MUTED)).child("›")),
                                        )
                                        .child(
                            div()
                                .id("open-remote-machine")
                                .p_3()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_remote(window, cx);
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child("Connect to a machine"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child(remote_status),
                                        ),
                                )
                                .child(div().text_color(rgb(MUTED)).child("›")),
                                        )
                                        .child(
                            div()
                                .id("open-add-device")
                                .p_3()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_share(cx);
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child("Add a device"),
                                        )
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                                            "Create a five-minute code for another xd app.",
                                        )),
                                )
                                .child(div().text_color(rgb(MUTED)).child("›")),
                                        )
                                        .child(
                            div()
                                .id("open-paired-devices")
                                .p_3()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_devices(cx);
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child("Paired devices"),
                                        )
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                                            "Rename or revoke devices connected to this machine.",
                                        )),
                                )
                                .child(div().text_color(rgb(MUTED)).child("›")),
                                        )
                                        .child(
                            div()
                                .id("open-agent-secrets")
                                .p_3()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_secrets(None, None, window, cx);
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child("Agent secrets"),
                                        )
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                                            "Private environment variables for local agents.",
                                        )),
                                )
                                .child(div().text_color(rgb(MUTED)).child("›")),
                                        ),
                                ),
                        ),
                )
                .into_any_element()
        });

        let secrets_overlay = self.secrets_panel.clone().map(|panel| {
            let name_input = self.secret_name_input.clone();
            let value_input = self.secret_value_input.clone();
            let name_focus = self.secret_name_input.read(cx).focus_handle(cx);
            let value_focus = self.secret_value_input.read(cx).focus_handle(cx);
            let replacing = panel
                .names
                .iter()
                .any(|name| name == panel.name.trim());
            let can_submit = self.model.connected
                && !panel.loading
                && !panel.submitting
                && !panel.name.trim().is_empty()
                && !panel.value.is_empty();
            let title = panel
                .folder_name
                .as_ref()
                .map(|name| format!("Agent Secrets · {name}"))
                .unwrap_or_else(|| "Agent Secrets · This Machine".into());
            let description = if panel.folder_id.is_some() {
                "This workspace inherits global and parent secrets. Values set here override them for this workspace and its children."
            } else {
                "Stored privately outside workspaces. Values never enter prompts or protocol replies; local agent processes receive them as environment variables."
            };
            let rows = panel
                .names
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, name)| {
                    let replace_name = name.clone();
                    let remove_name = name.clone();
                    div()
                        .p_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(SURFACE))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .font_family("monospace")
                                .text_sm()
                                .text_color(rgb(TEXT))
                                .child(name),
                        )
                        .child(
                            div()
                                .id(("replace-secret", index))
                                .px_3()
                                .py_2()
                                .rounded_lg()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.choose_secret(replace_name.clone(), window, cx);
                                }))
                                .child("Replace"),
                        )
                        .child(
                            div()
                                .id(("remove-secret", index))
                                .px_3()
                                .py_2()
                                .rounded_lg()
                                .text_xs()
                                .text_color(rgb(0xefaaaa))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x3b282e)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_secret(remove_name.clone(), cx);
                                }))
                                .child("Remove"),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(620.0))
                        .max_h(px(680.0))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(div().text_lg().text_color(rgb(TEXT)).child(title))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child(description),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("close-agent-secrets")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_secrets(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .id("agent-secret-list")
                                .min_h(px(80.0))
                                .max_h(px(300.0))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when(panel.loading, |list| {
                                    list.child(
                                        div()
                                            .p_3()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child("Loading secret names…"),
                                    )
                                })
                                .when(!panel.loading && rows.is_empty(), |list| {
                                    list.child(
                                        div()
                                            .p_3()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child("No secrets stored at this scope."),
                                    )
                                })
                                .children(rows),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child(if replacing {
                                            "REPLACE SECRET"
                                        } else {
                                            "ADD SECRET"
                                        }),
                                )
                                .child(
                                    div()
                                        .id("secret-name-input")
                                        .track_focus(&name_focus)
                                        .h(px(42.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(if name_focus.is_focused(window) {
                                            accent
                                        } else {
                                            BORDER
                                        }))
                                        .bg(rgb(SURFACE))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let focus = this
                                                .secret_name_input
                                                .read(cx)
                                                .focus_handle(cx);
                                            window.focus(&focus);
                                        }))
                                        .child(name_input),
                                )
                                .child(
                                    div()
                                        .id("secret-value-input")
                                        .track_focus(&value_focus)
                                        .h(px(42.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(if value_focus.is_focused(window) {
                                            accent
                                        } else {
                                            BORDER
                                        }))
                                        .bg(rgb(SURFACE))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let focus = this
                                                .secret_value_input
                                                .read(cx)
                                                .focus_handle(cx);
                                            window.focus(&focus);
                                        }))
                                        .child(value_input),
                                ),
                        )
                        .when_some(panel.error, |dialog, error| {
                            dialog.child(
                                div()
                                    .p_3()
                                    .rounded_lg()
                                    .bg(rgb(0x3b282e))
                                    .text_xs()
                                    .text_color(rgb(0xefaaaa))
                                    .child(error),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .child(
                                    div()
                                        .id("save-agent-secret")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if can_submit { accent } else { SURFACE_HIGH }))
                                        .text_sm()
                                        .text_color(rgb(if can_submit { 0xffffff } else { MUTED }))
                                        .when(can_submit, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(accent_hover)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_submit {
                                                this.save_secret(cx);
                                            }
                                        }))
                                        .child(if panel.submitting {
                                            "Saving…"
                                        } else if replacing {
                                            "Replace secret"
                                        } else {
                                            "Add secret"
                                        }),
                                ),
                        ),
                )
                .into_any_element()
        });

        let source_build_overlay = self.source_build_open.then(|| {
            let panel = self.source_build_panel.clone();
            let input = self.source_build_input.clone();
            let focus = self.source_build_input.read(cx).focus_handle(cx);
            let target_label = panel.target.as_ref().map(|target| target.label.clone());
            let can_build = !panel.running && panel.target.is_some() && source_build::supported();
            let running = panel.running;
            let stopping = panel.stopping;
            let output = panel.output_text();
            let activity = output
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.chars().take(180).collect::<String>());
            let status = if panel.running {
                if panel.stopping {
                    "Stopping the source build…".to_owned()
                } else {
                    activity.unwrap_or_else(|| {
                        format!(
                            "Building {}…",
                            target_label.as_deref().unwrap_or("source")
                        )
                    })
                }
            } else if let Some(message) = &panel.message {
                message.clone()
            } else if let Some(label) = &target_label {
                label.clone()
            } else if panel.text.trim().is_empty() {
                "Enter a branch, pull request, commit, or GitHub URL.".into()
            } else {
                "Source is not valid.".into()
            };
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(680.0))
                        .max_h(px(720.0))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_lg()
                                                .text_color(rgb(TEXT))
                                                .child("Build XD Source"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child(
                                                    "Fetch, build, and install a Linux nightly through Docker.",
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("close-source-build")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_source_build(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .id("source-build-input-field")
                                .track_focus(&focus)
                                .h(px(42.0))
                                .w_full()
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(if focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(if running { SURFACE_HIGH } else { SURFACE }))
                                .when(!running, |field| {
                                    field.cursor_text().on_click(cx.listener(
                                        |this, _, window, cx| {
                                            let focus = this
                                                .source_build_input
                                                .read(cx)
                                                .focus_handle(cx);
                                            window.focus(&focus);
                                        },
                                    ))
                                })
                                .child(input),
                        )
                        .child(
                            div()
                                .p_3()
                                .rounded_lg()
                                .bg(rgb(if panel.installed { 0x173423 } else { SURFACE }))
                                .text_sm()
                                .text_color(rgb(if panel.installed { 0x8bd5a0 } else { TEXT }))
                                .child(status),
                        )
                        .child(
                            div()
                                .p_3()
                                .rounded_lg()
                                .bg(rgb(0x30291b))
                                .text_xs()
                                .text_color(rgb(0xe8c780))
                                .child(
                                    "Only build source you trust. Its scripts receive Docker access to this machine.",
                                ),
                        )
                        .when(!output.trim().is_empty(), |dialog| {
                            dialog.child(
                                div()
                                    .id("source-build-output")
                                    .max_h(px(300.0))
                                    .overflow_y_scroll()
                                    .p_3()
                                    .rounded_lg()
                                    .bg(rgb(SIDEBAR))
                                    .font_family("JetBrains Mono")
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(output),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("dismiss-source-build")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_source_build(cx);
                                        }))
                                        .child("Close"),
                                )
                                .child(
                                    div()
                                        .id("run-source-build")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if running {
                                            0x6b3038
                                        } else if can_build {
                                            accent
                                        } else {
                                            SURFACE_HIGH
                                        }))
                                        .text_sm()
                                        .text_color(rgb(if running || can_build {
                                            0xffffff
                                        } else {
                                            MUTED
                                        }))
                                        .when(running || can_build, |button| {
                                            button.cursor_pointer().hover(move |style| {
                                                style.bg(rgb(if running {
                                                    0x7b3943
                                                } else {
                                                    accent_hover
                                                }))
                                            })
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if running {
                                                this.stop_source_build(cx);
                                            } else if can_build {
                                                this.start_source_build(cx);
                                            }
                                        }))
                                        .child(if running {
                                            if stopping { "Stopping…" } else { "Stop" }
                                        } else {
                                            "Build and Install"
                                        }),
                                ),
                        ),
                )
                .into_any_element()
        });

        let self_update_overlay = self.self_update_panel.clone().map(|panel| {
            let action = self_update_action(&panel);
            let can_install = action == Some("install");
            let can_restart = action == Some("restart");
            let status_text = self_update_status_text(&panel);
            let version_text = panel.status.as_ref().map(|status| {
                let mut text = format!("Running {}", status.version);
                if let Some(latest) = &status.latest {
                    text.push_str(" · latest ");
                    text.push_str(latest);
                }
                text
            });
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(520.0))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_lg()
                                        .text_color(rgb(TEXT))
                                        .child("Update xd"),
                                )
                                .child(
                                    div()
                                        .id("close-daemon-update")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_self_update(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .p_3()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .child(div().text_sm().text_color(rgb(TEXT)).child(status_text))
                                .when_some(version_text, |card, version| {
                                    card.child(
                                        div().text_xs().text_color(rgb(MUTED)).child(version),
                                    )
                                })
                                .when(can_restart, |card| {
                                    card.child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child(
                                                "Restarting drops every attached device and loses any running turn.",
                                            ),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("dismiss-daemon-update")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_self_update(cx);
                                        }))
                                        .child("Close"),
                                )
                                .child(
                                    div()
                                        .id("install-daemon-update")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if can_install { accent } else { SURFACE_HIGH }))
                                        .text_sm()
                                        .text_color(rgb(if can_install { 0xffffff } else { MUTED }))
                                        .when(can_install, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(accent_hover)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_install {
                                                this.install_self_update(cx);
                                            }
                                        }))
                                        .child(if panel.busy && !can_restart {
                                            "Working…"
                                        } else {
                                            "Install"
                                        }),
                                )
                                .when(can_restart, |actions| {
                                    actions.child(
                                        div()
                                            .id("restart-daemon-update")
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .bg(rgb(accent))
                                            .text_sm()
                                            .text_color(rgb(0xffffff))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(accent_hover)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.restart_self_update(cx);
                                            }))
                                            .child("Restart"),
                                    )
                                }),
                        ),
                )
                .into_any_element()
        });

        let devices_overlay = self.devices_panel.clone().map(|panel| {
            let name_input = self.device_name_input.clone();
            let name_focus = self.device_name_input.read(cx).focus_handle(cx);
            let rows = panel
                .devices
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, device)| {
                    let editing = panel.editing_id.as_ref() == Some(&device.id);
                    let confirming = panel.revoke_confirmation.as_ref() == Some(&device.id);
                    let busy = panel.mutating.as_ref() == Some(&device.id);
                    let rename_id = device.id.clone();
                    let rename_name = device.name.clone();
                    let revoke_id = device.id.clone();
                    div()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(SURFACE))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .text_sm()
                                        .text_color(rgb(TEXT))
                                        .child(device.name.clone()),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_full()
                                        .text_xs()
                                        .text_color(rgb(if device.connected {
                                            0x8bd5a0
                                        } else {
                                            MUTED
                                        }))
                                        .bg(rgb(if device.connected {
                                            0x173423
                                        } else {
                                            SURFACE_HIGH
                                        }))
                                        .child(if device.connected {
                                            "Connected"
                                        } else {
                                            "Offline"
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(device_time_label("Last seen", device.last_seen))
                                .child("·")
                                .child(device_time_label("Paired", device.created_at)),
                        )
                        .when(editing, |row| {
                            row.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id(("device-name-input", index))
                                            .track_focus(&name_focus)
                                            .h(px(40.0))
                                            .min_w_0()
                                            .flex_1()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(rgb(if name_focus.is_focused(window) {
                                                accent
                                            } else {
                                                BORDER
                                            }))
                                            .bg(rgb(BG))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                let focus = this
                                                    .device_name_input
                                                    .read(cx)
                                                    .focus_handle(cx);
                                                window.focus(&focus);
                                            }))
                                            .child(name_input.clone()),
                                    )
                                    .child(
                                        div()
                                            .id(("save-device-name", index))
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .bg(rgb(if busy { SURFACE_HIGH } else { accent }))
                                            .text_xs()
                                            .text_color(rgb(if busy { MUTED } else { 0xffffff }))
                                            .when(!busy, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(rgb(accent_hover)))
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if !busy {
                                                    this.save_device_name(cx);
                                                }
                                            }))
                                            .child(if busy { "Saving…" } else { "Save" }),
                                    )
                                    .child(
                                        div()
                                            .id(("cancel-device-name", index))
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .cursor_pointer()
                                            .hover(|style| {
                                                style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.cancel_device_name(cx);
                                            }))
                                            .child("Cancel"),
                                    ),
                            )
                        })
                        .when(!editing, |row| {
                            row.child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id(("rename-device", index))
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .cursor_pointer()
                                            .hover(|style| {
                                                style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                            })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.edit_device_name(
                                                    rename_id.clone(),
                                                    rename_name.clone(),
                                                    window,
                                                    cx,
                                                );
                                            }))
                                            .child("Rename"),
                                    )
                                    .child(
                                        div()
                                            .id(("revoke-device", index))
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .text_xs()
                                            .text_color(rgb(0xefaaaa))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x3b282e)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.revoke_device(revoke_id.clone(), cx);
                                            }))
                                            .child(if busy {
                                                "Revoking…"
                                            } else if confirming {
                                                "Confirm revoke"
                                            } else {
                                                "Revoke"
                                            }),
                                    ),
                            )
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(620.0))
                        .max_h(px(680.0))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_lg()
                                                .text_color(rgb(TEXT))
                                                .child("Paired devices"),
                                        )
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                                            "Devices authorized to connect to this xd daemon.",
                                        )),
                                )
                                .child(
                                    div()
                                        .id("refresh-paired-devices")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.refresh_devices(cx);
                                        }))
                                        .child(if panel.loading {
                                            "Loading…"
                                        } else {
                                            "Refresh"
                                        }),
                                )
                                .child(
                                    div()
                                        .id("close-paired-devices")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_devices(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .id("paired-device-list")
                                .min_h(px(100.0))
                                .max_h(px(460.0))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when(panel.loading && rows.is_empty(), |list| {
                                    list.child(
                                        div()
                                            .p_3()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child("Loading paired devices…"),
                                    )
                                })
                                .when(!panel.loading && rows.is_empty(), |list| {
                                    list.child(
                                        div()
                                            .p_3()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child("No devices are paired with this machine."),
                                    )
                                })
                                .children(rows),
                        )
                        .when_some(panel.error, |dialog, error| {
                            dialog.child(
                                div()
                                    .p_3()
                                    .rounded_lg()
                                    .bg(rgb(0x3b282e))
                                    .text_xs()
                                    .text_color(rgb(0xefaaaa))
                                    .child(error),
                            )
                        }),
                )
                .into_any_element()
        });

        let remote_overlay = self.remote_panel.clone().map(|panel| {
            let host_input = self.remote_host_input.clone();
            let port_input = self.remote_port_input.clone();
            let code_input = self.remote_code_input.clone();
            let name_input = self.remote_name_input.clone();
            let host_focus = self.remote_host_input.read(cx).focus_handle(cx);
            let port_focus = self.remote_port_input.read(cx).focus_handle(cx);
            let code_focus = self.remote_code_input.read(cx).focus_handle(cx);
            let name_focus = self.remote_name_input.read(cx).focus_handle(cx);
            let configured = self.remote_credentials.is_some();
            let status = match self.remote_state {
                RemoteState::Unconfigured => "No remote machine is paired.",
                RemoteState::Connecting => "Opening a secure connection…",
                RemoteState::Connected => "Connected securely.",
                RemoteState::Offline => "The paired machine is offline. Retrying automatically…",
            };
            let can_pair = !panel.submitting
                && !panel.host.trim().is_empty()
                && panel
                    .port
                    .trim()
                    .parse::<u16>()
                    .ok()
                    .is_some_and(|port| port > 0)
                && !panel.code.split_whitespace().collect::<String>().is_empty()
                && !panel.name.trim().is_empty();
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(620.0))
                        .max_h(px(760.0))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_lg()
                                                .text_color(rgb(TEXT))
                                                .child("Connect to a machine"),
                                        )
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                                            "Use another xd daemon while keeping this machine available.",
                                        )),
                                )
                                .child(
                                    div()
                                        .id("close-remote-machine")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_remote(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .p_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded_lg()
                                .bg(rgb(SURFACE))
                                .child(
                                    div()
                                        .size(px(8.0))
                                        .rounded_full()
                                        .bg(rgb(match self.remote_state {
                                            RemoteState::Connected => 0x65c985,
                                            RemoteState::Connecting => accent,
                                            RemoteState::Offline => 0xe3a45b,
                                            RemoteState::Unconfigured => MUTED,
                                        })),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .text_sm()
                                        .text_color(rgb(TEXT))
                                        .child(status),
                                )
                                .when(
                                    configured && self.remote_state != RemoteState::Connecting,
                                    |row| {
                                        row.child(
                                            div()
                                                .id("retry-remote-machine")
                                                .px_3()
                                                .py_2()
                                                .rounded_lg()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .cursor_pointer()
                                                .hover(|style| {
                                                    style
                                                        .bg(rgb(SURFACE_HIGH))
                                                        .text_color(rgb(TEXT))
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.retry_remote_connection(cx);
                                                }))
                                                .child("Reconnect"),
                                        )
                                    },
                                )
                                .when(configured, |row| {
                                    row.child(
                                        div()
                                            .id("forget-remote-machine")
                                            .px_3()
                                            .py_2()
                                            .rounded_lg()
                                            .text_xs()
                                            .text_color(rgb(0xefaaaa))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x3b282e)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.forget_remote_machine(cx);
                                            }))
                                            .child("Forget"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(MUTED))
                                .child(if configured {
                                    "PAIR A DIFFERENT MACHINE"
                                } else {
                                    "PAIR THIS DEVICE"
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .id("remote-host-field")
                                        .track_focus(&host_focus)
                                        .h(px(40.0))
                                        .min_w_0()
                                        .flex_1()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(if host_focus.is_focused(window) {
                                            accent
                                        } else {
                                            BORDER
                                        }))
                                        .bg(rgb(SURFACE))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let focus =
                                                this.remote_host_input.read(cx).focus_handle(cx);
                                            window.focus(&focus);
                                        }))
                                        .child(host_input),
                                )
                                .child(
                                    div()
                                        .id("remote-port-field")
                                        .track_focus(&port_focus)
                                        .h(px(40.0))
                                        .w(px(120.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(if port_focus.is_focused(window) {
                                            accent
                                        } else {
                                            BORDER
                                        }))
                                        .bg(rgb(SURFACE))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let focus =
                                                this.remote_port_input.read(cx).focus_handle(cx);
                                            window.focus(&focus);
                                        }))
                                        .child(port_input),
                                ),
                        )
                        .child(
                            div()
                                .id("remote-code-field")
                                .track_focus(&code_focus)
                                .h(px(40.0))
                                .w_full()
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(if code_focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(SURFACE))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus = this.remote_code_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(code_input),
                        )
                        .child(
                            div()
                                .id("remote-name-field")
                                .track_focus(&name_focus)
                                .h(px(40.0))
                                .w_full()
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(if name_focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(SURFACE))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus = this.remote_name_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(name_input),
                        )
                        .when_some(panel.error, |dialog, error| {
                            dialog.child(
                                div()
                                    .p_3()
                                    .rounded_lg()
                                    .bg(rgb(0x3b282e))
                                    .text_xs()
                                    .text_color(rgb(0xefaaaa))
                                    .child(error),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("cancel-remote-machine")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_remote(cx);
                                        }))
                                        .child("Close"),
                                )
                                .child(
                                    div()
                                        .id("pair-remote-machine")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if can_pair { accent } else { SURFACE_HIGH }))
                                        .text_sm()
                                        .text_color(rgb(if can_pair { 0xffffff } else { MUTED }))
                                        .when(can_pair, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(accent_hover)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_pair {
                                                this.pair_remote_machine(cx);
                                            }
                                        }))
                                        .child(if panel.submitting {
                                            "Pairing…"
                                        } else {
                                            "Pair device"
                                        }),
                                ),
                        ),
                )
                .into_any_element()
        });

        let share_overlay = self.share_panel.clone().map(|panel| {
            let ready = !panel.host.is_empty() && panel.port.is_some() && !panel.code.is_empty();
            let address = panel
                .port
                .map(|port| format!("{}:{port}", panel.host))
                .unwrap_or_default();
            let copied_code = panel.code.clone();
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(620.0))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_lg()
                                                .text_color(rgb(TEXT))
                                                .child("Add a device"),
                                        )
                                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                                            "Connect another xd app to this machine, its chats, and running agents.",
                                        )),
                                )
                                .child(
                                    div()
                                        .id("close-add-device")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_share(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .p_3()
                                .rounded_lg()
                                .bg(rgb(SURFACE))
                                .text_sm()
                                .text_color(rgb(if panel.error.is_some() {
                                    0xefaaaa
                                } else {
                                    MUTED
                                }))
                                .child(if let Some(error) = panel.error.clone() {
                                    error
                                } else if panel.loading {
                                    "Opening a secure listener…".into()
                                } else if ready {
                                    "Ready for one device.".into()
                                } else {
                                    "Create a code for the connecting device.".into()
                                }),
                        )
                        .when(ready, |dialog| {
                            dialog
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child("MACHINE ADDRESS AND PORT"),
                                        )
                                        .child(
                                            div()
                                                .p_3()
                                                .rounded_lg()
                                                .border_1()
                                                .border_color(rgb(BORDER))
                                                .bg(rgb(SURFACE))
                                                .text_sm()
                                                .text_color(rgb(TEXT))
                                                .child(address),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child("ONE-TIME PAIRING CODE"),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .flex_1()
                                                        .p_3()
                                                        .rounded_lg()
                                                        .border_1()
                                                        .border_color(rgb(BORDER))
                                                        .bg(rgb(SURFACE))
                                                        .text_lg()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(rgb(TEXT))
                                                        .child(panel.code.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .id("copy-pairing-code")
                                                        .px_4()
                                                        .py_3()
                                                        .rounded_lg()
                                                        .bg(rgb(SURFACE_HIGH))
                                                        .text_sm()
                                                        .text_color(rgb(TEXT))
                                                        .cursor_pointer()
                                                        .hover(|style| style.bg(rgb(BORDER)))
                                                        .on_click(move |_, _, cx| {
                                                            cx.write_to_clipboard(
                                                                ClipboardItem::new_string(
                                                                    copied_code.clone(),
                                                                ),
                                                            );
                                                        })
                                                        .child("Copy"),
                                                ),
                                        ),
                                )
                                .child(div().text_xs().text_color(rgb(MUTED)).child(
                                    "On the other device, choose “Connect to a machine”, then enter this address and code. It expires after five minutes and works once.",
                                ))
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("dismiss-add-device")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_share(cx);
                                        }))
                                        .child("Close"),
                                )
                                .child(
                                    div()
                                        .id("create-pairing-code")
                                        .px_4()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if panel.loading { SURFACE_HIGH } else { accent }))
                                        .text_sm()
                                        .text_color(rgb(if panel.loading {
                                            MUTED
                                        } else {
                                            0xffffff
                                        }))
                                        .when(!panel.loading, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(accent_hover)))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.request_pairing_code(cx);
                                        }))
                                        .child(if panel.loading {
                                            "Opening…"
                                        } else if ready {
                                            "Create another code"
                                        } else {
                                            "Create code"
                                        }),
                                ),
                        ),
                )
                .into_any_element()
        });

        let shortcut_overlay = self.shortcut_panel.clone().map(|panel| {
            let can_edit = !panel.loading && !panel.submitting;
            let can_add = can_edit && panel.rows.len() < MAX_SHORTCUTS;
            let title = panel
                .folder_name
                .as_ref()
                .map(|name| format!("Workspace Shortcuts · {name}"))
                .unwrap_or_else(|| "Global Shortcuts".into());
            let description = if panel.folder_id.is_some() {
                "These prompt buttons appear in this workspace and its children."
            } else {
                "These prompt buttons appear in every workspace on this daemon."
            };
            let rows = panel
                .rows
                .into_iter()
                .map(|row| {
                    let focus = row.input.read(cx).focus_handle(cx);
                    let input = row.input.clone();
                    let input_for_click = row.input.clone();
                    let id = row.id;
                    div()
                        .id(("shortcut-editor-row", id))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id(("shortcut-editor-input", id))
                                .track_focus(&focus)
                                .h(px(40.0))
                                .min_w_0()
                                .flex_1()
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(if focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(SURFACE))
                                .on_click(move |_, window, cx| {
                                    window.focus(&input_for_click.read(cx).focus_handle(cx));
                                })
                                .child(input),
                        )
                        .child(
                            div()
                                .id(("remove-shortcut-row", id))
                                .size(px(40.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_lg()
                                .text_sm()
                                .text_color(rgb(if can_edit { 0xefaaaa } else { MUTED }))
                                .when(can_edit, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x3b282e)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_shortcut_row(id, cx);
                                        }))
                                })
                                .child("×"),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .id("shortcut-management-panel")
                        .w(px(700.0))
                        .max_h(px(620.0))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(div().text_lg().text_color(rgb(TEXT)).child(title))
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(MUTED))
                                                .child(description),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("close-shortcut-management")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_shortcut_panel(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .id("shortcut-editor-list")
                                .min_h(px(160.0))
                                .max_h(px(390.0))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when(panel.loading, |list| {
                                    list.child(
                                        div()
                                            .p_4()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child("Loading shortcuts…"),
                                    )
                                })
                                .when(!panel.loading && rows.is_empty(), |list| {
                                    list.child(
                                        div()
                                            .p_4()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child("No shortcuts yet."),
                                    )
                                })
                                .children(rows),
                        )
                        .when_some(panel.error.clone(), |dialog, error| {
                            dialog.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(0x382126))
                                    .text_sm()
                                    .text_color(rgb(0xefb1b1))
                                    .child(error),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .id("add-shortcut-row")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(SURFACE_HIGH))
                                        .text_sm()
                                        .text_color(rgb(if can_add { TEXT } else { MUTED }))
                                        .when(can_add, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(BORDER)))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.add_shortcut_row(window, cx);
                                                }))
                                        })
                                        .child("Add Prompt"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("cancel-shortcut-management")
                                                .px_4()
                                                .py_2()
                                                .rounded_lg()
                                                .text_sm()
                                                .text_color(rgb(MUTED))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.close_shortcut_panel(cx);
                                                }))
                                                .child("Cancel"),
                                        )
                                        .child(
                                            div()
                                                .id("save-shortcut-management")
                                                .px_4()
                                                .py_2()
                                                .rounded_lg()
                                                .bg(rgb(if can_edit {
                                                    accent
                                                } else {
                                                    SURFACE_HIGH
                                                }))
                                                .text_sm()
                                                .text_color(rgb(if can_edit {
                                                    0xffffff
                                                } else {
                                                    MUTED
                                                }))
                                                .when(can_edit, |button| {
                                                    button
                                                        .cursor_pointer()
                                                        .hover(|style| style.bg(rgb(accent_hover)))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.save_shortcut_panel(cx);
                                                        }))
                                                })
                                                .child(if panel.submitting {
                                                    "Saving…"
                                                } else {
                                                    "Save"
                                                }),
                                        ),
                                ),
                        ),
                )
                .into_any_element()
        });

        let directory_overlay = self.directory_browser.clone().map(|browser| {
            let default_label = if matches!(browser.target, WorkspacePathTarget::CreateChat { .. })
            {
                "Use workspace default"
            } else {
                "Cancel"
            };
            let can_ascend = browser
                .path
                .as_deref()
                .and_then(directory_parent_path)
                .is_some()
                && !browser.loading;
            let can_choose = browser.path.is_some() && !browser.loading;
            let path = browser.path.clone().unwrap_or_else(|| "Daemon home".into());
            let rows = browser
                .entries
                .into_iter()
                .enumerate()
                .map(|(index, entry)| {
                    let selected = browser.selected == Some(index);
                    div()
                        .id(("directory-entry", index))
                        .w_full()
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .bg(rgb(if selected { SURFACE_HIGH } else { BG }))
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                            if event.click_count() >= 2 {
                                this.descend_directory(index, cx);
                            } else {
                                this.select_directory_entry(index, cx);
                            }
                        }))
                        .child("▸")
                        .child(entry)
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .id("directory-browser")
                        .w(px(620.0))
                        .h(px(460.0))
                        .flex()
                        .flex_col()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .px_4()
                                .py_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .border_b_1()
                                .border_color(rgb(BORDER))
                                .child(
                                    div()
                                        .id("directory-back")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(if can_ascend { TEXT } else { MUTED }))
                                        .when(can_ascend, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.ascend_directory(cx);
                                                }))
                                        })
                                        .child("← Back"),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .overflow_hidden()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .child(path),
                                )
                                .child(
                                    div()
                                        .id("directory-choose")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if can_choose { accent } else { SURFACE_HIGH }))
                                        .text_sm()
                                        .text_color(rgb(if can_choose { 0xffffff } else { MUTED }))
                                        .when(can_choose, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(accent_hover)))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.choose_current_directory(cx);
                                                }))
                                        })
                                        .child("Work here"),
                                ),
                        )
                        .when_some(browser.error.clone(), |panel, error| {
                            panel.child(
                                div()
                                    .mx_4()
                                    .mt_3()
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(0x382126))
                                    .text_sm()
                                    .text_color(rgb(0xefb1b1))
                                    .child(error),
                            )
                        })
                        .child(
                            div()
                                .id("directory-entries")
                                .min_h_0()
                                .flex_1()
                                .overflow_y_scroll()
                                .p_3()
                                .flex()
                                .flex_col()
                                .when(browser.loading, |list| {
                                    list.child(
                                        div()
                                            .p_4()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child("Loading folders…"),
                                    )
                                })
                                .when(!browser.loading && rows.is_empty(), |list| {
                                    list.child(
                                        div()
                                            .p_4()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child("No folders here."),
                                    )
                                })
                                .children(rows),
                        )
                        .child(
                            div()
                                .px_4()
                                .py_3()
                                .flex()
                                .items_center()
                                .justify_between()
                                .border_t_1()
                                .border_color(rgb(BORDER))
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child("↑↓ Select · Enter Open · Ctrl+Enter Work here")
                                .child(
                                    div()
                                        .id("close-directory-browser")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_directory_browser(cx);
                                        }))
                                        .child(default_label),
                                ),
                        ),
                )
                .into_any_element()
        });

        let search_overlay = self.search.clone().map(|search| {
            let search_focus = self.search_input.read(cx).focus_handle(cx);
            let mut result_rows = Vec::new();
            for (index, hit) in search.results.into_iter().enumerate() {
                let chat_id = hit.chat.clone();
                result_rows.push(
                    div()
                        .id(("search-result", index))
                        .w_full()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded_lg()
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.activate_search_result(chat_id.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(div().text_sm().text_color(rgb(TEXT)).child(hit.title))
                                .child(div().text_xs().text_color(rgb(MUTED)).child(hit.role)),
                        )
                        .child(div().text_xs().text_color(rgb(MUTED)).child(hit.snippet))
                        .into_any_element(),
                );
            }
            let has_query = !search.query.trim().is_empty();
            let empty_label = if search.loading {
                "Searching…"
            } else if has_query {
                "No matching conversations"
            } else {
                "Find a conversation by something said in it"
            };
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(76.0))
                .bg(rgba(0x00000099))
                .child(
                    div()
                        .w(px(620.0))
                        .max_h(px(560.0))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .id("search-input")
                                        .track_focus(&search_focus)
                                        .h(px(40.0))
                                        .min_w_0()
                                        .flex_1()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(rgb(if search_focus.is_focused(window) {
                                            accent
                                        } else {
                                            BORDER
                                        }))
                                        .bg(rgb(SURFACE))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let focus = this.search_input.read(cx).focus_handle(cx);
                                            window.focus(&focus);
                                        }))
                                        .child(self.search_input.clone()),
                                )
                                .child(
                                    div()
                                        .id("close-search")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_search(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .id("search-results")
                                .flex_1()
                                .min_h(px(120.0))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when(result_rows.is_empty(), |results| {
                                    results.child(
                                        div()
                                            .h(px(120.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child(empty_label),
                                    )
                                })
                                .children(result_rows),
                        ),
                )
                .into_any_element()
        });

        let message_image_overlay = self.message_image_viewer.clone().map(|viewer| {
            div()
                .id("message-image-backdrop")
                .absolute()
                .inset_0()
                .p_6()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x000000dd))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close_message_image(cx)),
                )
                .child(
                    div()
                        .id("message-image-viewer")
                        .w_full()
                        .h_full()
                        .max_w(px(1200.0))
                        .max_h(px(800.0))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(TEXT))
                                        .child(format!("Image #{}", viewer.number)),
                                )
                                .child(
                                    div()
                                        .id("close-message-image")
                                        .w(px(32.0))
                                        .h(px(32.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_lg()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT))
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_message_image(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .rounded_lg()
                                .bg(rgb(SURFACE))
                                .overflow_hidden()
                                .child(
                                    img(viewer.image).size_full().object_fit(ObjectFit::Contain),
                                ),
                        ),
                )
                .into_any_element()
        });

        let sidebar_splitter = div()
            .id("sidebar-resize")
            .w(px(5.0))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(BG))
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(|style| style.bg(rgb(BORDER)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.begin_pane_resize(PaneResizeKind::Sidebar, event, cx);
                }),
            );
        let diff_splitter = diff_open.then(|| {
            div()
                .id("diff-resize")
                .w(px(5.0))
                .h_full()
                .flex_shrink_0()
                .bg(rgb(BG))
                .cursor(CursorStyle::ResizeLeftRight)
                .hover(|style| style.bg(rgb(BORDER)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.begin_pane_resize(PaneResizeKind::Diff, event, cx);
                    }),
                )
        });
        let terminal_splitter = terminal_open.then(|| {
            div()
                .id("terminal-resize")
                .w_full()
                .h(px(5.0))
                .flex_shrink_0()
                .bg(rgb(BG))
                .cursor(CursorStyle::ResizeUpDown)
                .hover(|style| style.bg(rgb(BORDER)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.begin_pane_resize(PaneResizeKind::Terminal, event, cx);
                    }),
                )
        });
        let resize_overlay = self.pane_resize.map(|resize| {
            let cursor = match resize.kind {
                PaneResizeKind::Sidebar | PaneResizeKind::Diff => CursorStyle::ResizeLeftRight,
                PaneResizeKind::Terminal => CursorStyle::ResizeUpDown,
            };
            div()
                .absolute()
                .top(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .cursor(cursor)
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    this.update_pane_resize(event, cx)
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(XdDesktop::finish_pane_resize),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(XdDesktop::finish_pane_resize),
                )
        });
        let sidebar_context_overlay = self.sidebar_context_overlay(cx);

        let content = div()
            .flex_1()
            .min_h_0()
            .flex()
            .relative()
            .bg(rgb(BG))
            .child(sidebar)
            .child(sidebar_splitter)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(transcript))
                    .child(composer)
                    .when_some(terminal_splitter, |column, splitter| column.child(splitter))
                    .when_some(terminal_pane, |column, pane| column.child(pane)),
            )
            .when_some(diff_splitter, |root, splitter| root.child(splitter))
            .when_some(diff_pane, |root, pane| root.child(pane))
            .when_some(auth_overlay, |root, overlay| root.child(overlay))
            .when_some(settings_overlay, |root, overlay| root.child(overlay))
            .when_some(secrets_overlay, |root, overlay| root.child(overlay))
            .when_some(self_update_overlay, |root, overlay| root.child(overlay))
            .when_some(source_build_overlay, |root, overlay| root.child(overlay))
            .when_some(devices_overlay, |root, overlay| root.child(overlay))
            .when_some(remote_overlay, |root, overlay| root.child(overlay))
            .when_some(share_overlay, |root, overlay| root.child(overlay))
            .when_some(shortcut_overlay, |root, overlay| root.child(overlay))
            .when_some(directory_overlay, |root, overlay| root.child(overlay))
            .when_some(search_overlay, |root, overlay| root.child(overlay))
            .when_some(message_image_overlay, |root, overlay| root.child(overlay))
            .when_some(resize_overlay, |root, overlay| root.child(overlay));

        let titlebar = div()
            .h(px(34.0))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR))
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count >= 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .px_3()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(MUTED))
                    .child("xd"),
            )
            .child(
                div()
                    .id("window-minimize")
                    .w(px(38.0))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, _| window.minimize_window())
                    .child("−"),
            )
            .child(
                div()
                    .id("window-maximize")
                    .w(px(38.0))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, _| window.zoom_window())
                    .child("□"),
            )
            .child(
                div()
                    .id("window-close")
                    .w(px(42.0))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0x5a252b)).text_color(rgb(0xffffff)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, _| window.remove_window())
                    .child("×"),
            );

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .key_context("XdDesktop")
            .on_action(cx.listener(|this, _: &OpenSearch, window, cx| {
                this.open_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseSearch, _, cx| {
                if this.directory_browser.is_some() {
                    this.close_directory_browser(cx);
                } else if this.message_image_viewer.is_some() {
                    this.close_message_image(cx);
                } else if this.sidebar_context_menu.is_some() {
                    this.close_sidebar_context_menu(cx);
                } else if this.share_panel.is_some() {
                    this.close_share(cx);
                } else if this.shortcut_panel.is_some() {
                    this.close_shortcut_panel(cx);
                } else if this.devices_panel.is_some() {
                    this.close_devices(cx);
                } else if this.secrets_panel.is_some() {
                    this.close_secrets(cx);
                } else if this.auth_open {
                    this.auth_open = false;
                    cx.notify();
                } else if this.settings_open {
                    this.settings_open = false;
                    cx.notify();
                } else {
                    this.close_search(cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &SelectModel1, _, cx| this.select_model_shortcut(0, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectModel2, _, cx| this.select_model_shortcut(1, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectModel3, _, cx| this.select_model_shortcut(2, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectModel4, _, cx| this.select_model_shortcut(3, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectModel5, _, cx| this.select_model_shortcut(4, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectModel6, _, cx| this.select_model_shortcut(5, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectModel7, _, cx| this.select_model_shortcut(6, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectModel8, _, cx| this.select_model_shortcut(7, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectModel9, _, cx| this.select_model_shortcut(8, cx)),
            )
            .on_action(cx.listener(|this, _: &DirectoryPrevious, _, cx| {
                this.move_directory_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &DirectoryNext, _, cx| {
                this.move_directory_selection(1, cx);
            }))
            .on_action(cx.listener(|this, _: &DirectoryOpen, _, cx| {
                this.open_selected_directory(cx);
            }))
            .on_action(cx.listener(|this, _: &DirectoryParent, _, cx| {
                if this.directory_browser.is_some() {
                    this.ascend_directory(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DirectoryChoose, _, cx| {
                if this.directory_browser.is_some() {
                    this.choose_current_directory(cx);
                }
            }))
            .bg(rgb(BG))
            .font_family("Inter")
            .when(client_decorations, |root| root.child(titlebar))
            .child(content)
            .when_some(sidebar_context_overlay, |root, overlay| root.child(overlay))
            .child(
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
            .child(
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
            .child(
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
            .child(
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
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .size(px(10.0))
                    .cursor(CursorStyle::ResizeUpLeftDownRight)
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_resize(ResizeEdge::TopLeft)
                    }),
            )
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .size(px(10.0))
                    .cursor(CursorStyle::ResizeUpRightDownLeft)
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_resize(ResizeEdge::TopRight)
                    }),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .size(px(10.0))
                    .cursor(CursorStyle::ResizeUpRightDownLeft)
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_resize(ResizeEdge::BottomLeft)
                    }),
            )
            .child(
                div()
                    .absolute()
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .size(px(10.0))
                    .cursor(CursorStyle::ResizeUpLeftDownRight)
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_resize(ResizeEdge::BottomRight)
                    }),
            )
    }
}

fn load_png_attachments(
    paths: Vec<PathBuf>,
    available: usize,
    mut total_bytes: usize,
) -> Result<Vec<Attachment>, String> {
    if paths.len() > available {
        return Err(format!(
            "A message can contain at most {MAX_ATTACHMENTS} images."
        ));
    }
    let mut attachments = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        if metadata.len() > MAX_ATTACHMENT_BYTES as u64 {
            return Err("Each PNG must be 10 MiB or smaller.".into());
        }
        let bytes =
            fs::read(&path).map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        if total_bytes > MAX_TOTAL_ATTACHMENT_BYTES.saturating_sub(bytes.len()) {
            return Err("Attached images must stay under 20 MiB total.".into());
        }
        total_bytes += bytes.len();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image.png")
            .to_owned();
        attachments.push(Attachment::from_png(name, bytes)?);
    }
    Ok(attachments)
}

fn join_browse_path(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_owned()
    } else {
        format!("{base}/{name}")
    }
}

fn parent_browse_path(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_owned())
        .unwrap_or_default()
}

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
                status: None,
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
        if rendered_lines >= MAX_LINES {
            truncated = true;
            continue;
        }
        let kind = if line.starts_with("diff --git ")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("new file ")
            || line.starts_with("deleted file ")
            || line.starts_with("similarity index ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
            || line.starts_with("Binary files ")
        {
            DiffLineKind::Header
        } else if line.starts_with("@@") {
            DiffLineKind::Hunk
        } else if line.starts_with('+') {
            file.additions = file.additions.saturating_add(1);
            DiffLineKind::Added
        } else if line.starts_with('-') {
            file.deletions = file.deletions.saturating_add(1);
            DiffLineKind::Removed
        } else {
            DiffLineKind::Context
        };
        let mut text = line.chars().take(MAX_LINE_CHARS).collect::<String>();
        if line.chars().count() > MAX_LINE_CHARS {
            text.push('…');
            truncated = true;
        }
        file.lines.push(DiffLine { kind, text });
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
            status: Some(status.to_owned()),
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

fn compact_label(value: &str, limit: usize) -> String {
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

fn queue_preview(value: &str) -> String {
    const MAX_CHARS: usize = 280;
    const MAX_LINES: usize = 3;
    let mut preview = String::new();
    let mut chars = 0;
    let mut lines = 1;
    let mut truncated = false;
    for character in value.chars() {
        if chars >= MAX_CHARS - 1 || (character == '\n' && lines >= MAX_LINES) {
            truncated = true;
            break;
        }
        if character == '\n' {
            lines += 1;
        }
        preview.push(character);
        chars += 1;
    }
    if truncated {
        preview.truncate(preview.trim_end().len());
        preview.push('…');
    }
    preview
}

fn command_suggestions(commands: &[String], text: &str) -> Vec<String> {
    let Some(query) = text.strip_prefix('/') else {
        return Vec::new();
    };
    if query.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let query = query.to_lowercase();
    commands
        .iter()
        .take(200)
        .filter(|command| command.to_lowercase().starts_with(&query))
        .take(40)
        .cloned()
        .collect()
}

fn auth_operation(state: &str) -> Option<&'static str> {
    match state {
        "signed-in" => Some("agent-auth-logout"),
        "signing-in" => Some("agent-auth-cancel"),
        "checking" | "signing-out" => None,
        _ => Some("agent-auth-start"),
    }
}

fn active_auth_provider(backend: &str, claude_mode: bool) -> &str {
    if backend == "codex" && claude_mode {
        "claude-mode"
    } else {
        backend
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

fn voice_request_token() -> String {
    format!(
        "desktop-{}-{}",
        std::process::id(),
        NEXT_VOICE_REQUEST.fetch_add(1, Ordering::Relaxed)
    )
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
    let live_offset = model.messages.len() + usize::from(!model.live_text.is_empty());
    model
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            (message.role == "tool" && message.content == marker).then_some(index)
        })
        .chain(
            model
                .live_activity
                .iter()
                .enumerate()
                .filter_map(|(index, message)| {
                    (message.role == "tool" && message.content == marker)
                        .then_some(live_offset + index)
                }),
        )
        .collect()
}

fn activity_status_color(kind: ActivityKind) -> u32 {
    match kind {
        ActivityKind::Running => 0x91a7ff,
        ActivityKind::Success => 0x8bd5a0,
        ActivityKind::Failure => 0xff8f8f,
        ActivityKind::Finished => 0xaab2c0,
    }
}

fn merge_dictation(base: &str, spoken: &str) -> String {
    let spoken = spoken.trim();
    if spoken.is_empty() {
        return base.to_owned();
    }
    if base.is_empty() || base.chars().last().is_some_and(char::is_whitespace) {
        format!("{base}{spoken}")
    } else {
        format!("{base} {spoken}")
    }
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

#[cfg(not(target_os = "linux"))]
fn notify_turn_finished(_: &str) {}

fn optional_trimmed(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn directory_child_path(path: &str, entry: &str) -> String {
    if path.ends_with('/') {
        format!("{path}{entry}")
    } else {
        format!("{path}/{entry}")
    }
}

fn directory_parent_path(path: &str) -> Option<String> {
    let path = std::path::Path::new(path);
    let parent = path.parent()?;
    (parent != path).then(|| parent.to_string_lossy().into_owned())
}

fn next_directory_selection(
    selected: Option<usize>,
    entry_count: usize,
    direction: isize,
) -> Option<usize> {
    if entry_count == 0 {
        return None;
    }
    match (selected, direction.signum()) {
        (None, -1) => Some(entry_count - 1),
        (None, _) => Some(0),
        (Some(index), -1) => Some(index.saturating_sub(1)),
        (Some(index), _) => Some(index.saturating_add(1).min(entry_count - 1)),
    }
}

fn filtered_models(
    backends: &[AgentBackend],
    favorites: &[String],
    provider: Option<&str>,
    query: &str,
) -> Vec<(String, String)> {
    let needle = query.trim().to_lowercase();
    let mut visible = Vec::new();
    for backend in backends {
        if provider.is_some_and(|provider| provider != backend.id) {
            continue;
        }
        for model in &backend.models {
            let key = format!("{}/{}", backend.id, model.id);
            if provider.is_none() && !favorites.iter().any(|favorite| favorite == &key) {
                continue;
            }
            if !needle.is_empty()
                && !model.name.to_lowercase().contains(&needle)
                && !model.id.to_lowercase().contains(&needle)
                && !backend.name.to_lowercase().contains(&needle)
            {
                continue;
            }
            visible.push((backend.id.clone(), model.id.clone()));
        }
    }
    visible
}

fn clean_shortcut_prompts(prompts: &[String]) -> Result<Vec<String>, String> {
    if prompts.len() > MAX_SHORTCUTS {
        return Err(format!(
            "A shortcut list can contain at most {MAX_SHORTCUTS} prompts."
        ));
    }
    let mut cleaned = Vec::new();
    let mut seen = HashSet::new();
    for prompt in prompts {
        let prompt = prompt.trim();
        if prompt.is_empty() || !seen.insert(prompt.to_owned()) {
            continue;
        }
        if prompt.len() > MAX_SHORTCUT_BYTES {
            return Err(format!(
                "A shortcut prompt can contain at most {MAX_SHORTCUT_BYTES} bytes."
            ));
        }
        cleaned.push(prompt.to_owned());
    }
    Ok(cleaned)
}

fn turn_duration_label(value: &str) -> Option<String> {
    let seconds = value.trim().parse::<u64>().ok()?;
    let duration = if seconds >= 3_600 {
        format!("{}h {:02}m", seconds / 3_600, (seconds % 3_600) / 60)
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    };
    Some(format!("Worked for {duration}"))
}

fn pairing_details(value: &Value) -> Result<(String, u16, String), String> {
    let host = value
        .get("host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "The daemon returned an invalid pairing address.".to_owned())?;
    let port = value
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .ok_or_else(|| "The daemon returned an invalid pairing port.".to_owned())?;
    let code = value
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty())
        .ok_or_else(|| "The daemon returned an invalid pairing code.".to_owned())?;
    Ok((host.to_owned(), port, code.to_owned()))
}

fn device_time_label(label: &str, timestamp: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64;
    format!("{label} {}", relative_time(timestamp, now))
}

fn relative_time(timestamp: i64, now: i64) -> String {
    if timestamp <= 0 {
        return "unknown".into();
    }
    let seconds = now.saturating_sub(timestamp).max(0) as u64;
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3_600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3_600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

fn scoped_element_id(scope: &str, index: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    scope.hash(&mut hasher);
    index.hash(&mut hasher);
    hasher.finish()
}

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

fn sidebar_drop_allowed(
    model: &AppModel,
    target: &SidebarTarget,
    destination: Option<&str>,
) -> bool {
    match target {
        SidebarTarget::Folder(folder_id) => {
            let Some(folder) = model.folders.iter().find(|folder| &folder.id == folder_id) else {
                return false;
            };
            match destination {
                None => folder.parent.is_some(),
                Some(destination) => {
                    if folder_id == destination
                        || folder.parent.as_deref() == Some(destination)
                        || !model.folders.iter().any(|folder| folder.id == destination)
                    {
                        return false;
                    }
                    let mut current = Some(destination);
                    for _ in 0..=model.folders.len() {
                        let Some(id) = current else {
                            return true;
                        };
                        if id == folder_id {
                            return false;
                        }
                        current = model
                            .folders
                            .iter()
                            .find(|folder| folder.id == id)
                            .and_then(|folder| folder.parent.as_deref());
                    }
                    false
                }
            }
        }
        SidebarTarget::Chat(chat_id) => destination.is_some_and(|destination| {
            model.folders.iter().any(|folder| folder.id == destination)
                && model
                    .chats
                    .iter()
                    .find(|chat| &chat.id == chat_id)
                    .is_some_and(|chat| chat.folder != destination)
        }),
    }
}

fn folder_hidden_by_collapse(
    folders: &[Folder],
    collapsed: &HashSet<String>,
    folder_id: &str,
) -> bool {
    let mut current = folders
        .iter()
        .find(|folder| folder.id == folder_id)
        .and_then(|folder| folder.parent.as_deref());
    for _ in 0..=folders.len() {
        let Some(id) = current else {
            return false;
        };
        if collapsed.contains(id) {
            return true;
        }
        current = folders
            .iter()
            .find(|folder| folder.id == id)
            .and_then(|folder| folder.parent.as_deref());
    }
    true
}

fn self_update_action(panel: &SelfUpdatePanel) -> Option<&'static str> {
    let status = panel.status.as_ref()?;
    if panel.busy || !status.supported {
        return None;
    }
    if status.state == "installed" {
        return Some("restart");
    }
    (status.available || status.state == "failed").then_some("install")
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

fn self_update_status_text(panel: &SelfUpdatePanel) -> String {
    if let Some(error) = &panel.error {
        return error.clone();
    }
    let Some(status) = &panel.status else {
        return "Checking for an update…".into();
    };
    if panel.busy && status.state == "checking" {
        "Checking for an update…".into()
    } else if !status.supported {
        "This machine's installation cannot update itself. Update it the way it was installed."
            .into()
    } else {
        match status.state.as_str() {
            "installing" => "Installing. The daemon keeps running until restarted.".into(),
            "installed" => "Installed. Restart to run the new build.".into(),
            "failed" => status
                .error
                .clone()
                .unwrap_or_else(|| "The update failed.".into()),
            _ if status.available => "An update is available.".into(),
            _ => "This machine is up to date.".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_shortcuts_follow_the_visible_provider_search_and_favorites() {
        let backends = vec![
            AgentBackend {
                id: "codex".into(),
                name: "Codex".into(),
                default_model: "gpt-fast".into(),
                models: vec![
                    xd_desktop::model::AgentModel {
                        id: "gpt-fast".into(),
                        name: "GPT Fast".into(),
                    },
                    xd_desktop::model::AgentModel {
                        id: "gpt-deep".into(),
                        name: "GPT Deep".into(),
                    },
                ],
                efforts: Vec::new(),
            },
            AgentBackend {
                id: "claude".into(),
                name: "Claude Code".into(),
                default_model: "opus".into(),
                models: vec![xd_desktop::model::AgentModel {
                    id: "opus".into(),
                    name: "Opus".into(),
                }],
                efforts: Vec::new(),
            },
        ];
        assert_eq!(
            filtered_models(&backends, &[], Some("codex"), "deep"),
            vec![("codex".into(), "gpt-deep".into())]
        );
        assert_eq!(
            filtered_models(&backends, &["claude/opus".into()], None, ""),
            vec![("claude".into(), "opus".into())]
        );
        assert_eq!(
            filtered_models(&backends, &[], Some("claude"), "code"),
            vec![("claude".into(), "opus".into())]
        );
    }

    #[test]
    fn shortcut_management_matches_daemon_cleaning_and_bounds() {
        assert_eq!(
            clean_shortcut_prompts(&[
                "  Review diff  ".into(),
                "".into(),
                "Review diff".into(),
                "Run tests".into(),
            ])
            .unwrap(),
            vec!["Review diff", "Run tests"]
        );
        assert!(clean_shortcut_prompts(&vec!["x".into(); MAX_SHORTCUTS + 1]).is_err());
        assert!(clean_shortcut_prompts(&["x".repeat(MAX_SHORTCUT_BYTES + 1)]).is_err());
    }

    #[test]
    fn endpoint_routing_keeps_identical_chat_ids_isolated() {
        let tree = serde_json::json!({
            "folders": [{"id": "folder", "name": "Workspace"}],
            "chats": [{
                "id": "same-id",
                "folder": "folder",
                "title": "Chat",
                "backend": "codex"
            }]
        });
        let mut local = AppModel::default();
        let mut remote = AppModel::default();
        local.apply_tree(&tree).unwrap();
        remote.apply_tree(&tree).unwrap();

        XdDesktop::apply_passive_event(
            &mut remote,
            "turn-started",
            &serde_json::json!({"chat": "same-id"}),
        );

        assert!(!local.chats[0].working);
        assert!(remote.chats[0].working);
        assert_eq!(ChatEndpoint::Local.other(), ChatEndpoint::Remote);
        assert_eq!(ChatEndpoint::Remote.other(), ChatEndpoint::Local);
    }

    #[test]
    fn workspace_clone_tracking_keeps_identical_folder_ids_isolated() {
        let local = (ChatEndpoint::Local, "same-folder".to_owned());
        let remote = (ChatEndpoint::Remote, "same-folder".to_owned());
        let mut pending = HashSet::new();
        pending.insert(local.clone());
        assert!(pending.contains(&local));
        assert!(!pending.contains(&remote));

        let mut outcomes = HashMap::new();
        outcomes.insert(local, None::<String>);
        outcomes.insert(remote, Some("remote clone failed".to_owned()));
        assert_eq!(outcomes.len(), 2);
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
        assert!(XdDesktop::remote_chat_reply(&RequestKind::VoiceMutation {
            chat_id: "chat".into(),
            token: "voice".into(),
            operation: "voice-stream-start".into(),
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
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::RenameFolder {
            folder_id: "workspace".into(),
            name: "Remote workspace".into(),
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::AgentSecrets {
            folder_id: Some("workspace".into()),
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::Search {
            query: "needle".into(),
        }));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::AgentAuth));
        assert!(XdDesktop::remote_chat_reply(&RequestKind::AgentClis));
        assert!(!XdDesktop::remote_chat_reply(&RequestKind::AgentSecrets {
            folder_id: None,
        }));
        assert!(!XdDesktop::remote_chat_reply(&RequestKind::Devices));
        assert!(!XdDesktop::remote_chat_reply(&RequestKind::DaemonUpdate {
            action: "install".into(),
        }));
        assert!(XdDesktop::local_admin_reply(&RequestKind::AgentSecrets {
            folder_id: None,
        }));
        assert!(!XdDesktop::local_admin_reply(&RequestKind::AgentSecrets {
            folder_id: Some("workspace".into()),
        }));
        assert!(XdDesktop::local_admin_reply(&RequestKind::Devices));
        assert!(!XdDesktop::local_admin_reply(&RequestKind::AgentAuth));
        assert!(!XdDesktop::local_admin_event("agent-auth-changed"));
        assert!(XdDesktop::local_admin_event("daemon-update"));
        assert!(!XdDesktop::local_admin_event("turn-finished"));
        assert!(XdDesktop::remote_read_event("queued"));
        assert!(XdDesktop::remote_read_event("voice"));
        assert!(XdDesktop::remote_read_event("terminal-output"));
        assert!(XdDesktop::remote_read_event("git-draft-finished"));
        assert!(XdDesktop::remote_read_event("agent-auth-changed"));
        assert!(XdDesktop::remote_read_event("agent-cli-changed"));
        assert!(!XdDesktop::remote_read_event("devices-changed"));
    }

    #[test]
    fn workspace_file_navigation_stays_relative_to_the_chat() {
        assert_eq!(join_browse_path("", "src"), "src");
        assert_eq!(join_browse_path("src", "main.rs"), "src/main.rs");
        assert_eq!(parent_browse_path("src/main.rs"), "src");
        assert_eq!(parent_browse_path("src"), "");
        assert_eq!(parent_browse_path(""), "");
    }

    #[test]
    fn structured_question_events_are_bounded_and_validated() {
        let question = question_from_event(&serde_json::json!({
            "chat": "chat-1",
            "waiting": true,
            "question": "Choose a direction",
            "options": ["Fast", "Safe", "Small", "Four", "Five", "Six", "Ignored"],
            "accepts_input": true,
        }))
        .unwrap();
        assert_eq!(question.chat_id, "chat-1");
        assert_eq!(question.options.len(), 6);
        assert!(question.accepts_input);
        assert!(
            question_from_event(&serde_json::json!({
                "chat": "chat-1",
                "waiting": true,
                "options": ["Only one"],
            }))
            .is_none()
        );
        assert!(
            question_from_event(&serde_json::json!({
                "chat": "chat-1",
                "waiting": false,
                "options": ["One", "Two"],
            }))
            .is_none()
        );
    }

    #[test]
    fn assistant_account_states_choose_safe_actions() {
        assert_eq!(auth_operation("signed-out"), Some("agent-auth-start"));
        assert_eq!(auth_operation("failed"), Some("agent-auth-start"));
        assert_eq!(auth_operation("signed-in"), Some("agent-auth-logout"));
        assert_eq!(auth_operation("signing-in"), Some("agent-auth-cancel"));
        assert_eq!(auth_operation("checking"), None);
        assert_eq!(auth_operation("signing-out"), None);
        assert_eq!(active_auth_provider("codex", false), "codex");
        assert_eq!(active_auth_provider("codex", true), "claude-mode");
        assert_eq!(active_auth_provider("claude", true), "claude");
    }

    #[test]
    fn daemon_update_requires_an_explicit_safe_action() {
        let mut panel = SelfUpdatePanel {
            status: Some(SelfUpdateStatus {
                version: "old".into(),
                state: "idle".into(),
                supported: true,
                available: true,
                latest: Some("new".into()),
                error: None,
            }),
            ..Default::default()
        };
        assert_eq!(self_update_action(&panel), Some("install"));
        assert_eq!(self_update_status_text(&panel), "An update is available.");

        panel.busy = true;
        assert_eq!(self_update_action(&panel), None);
        panel.busy = false;
        panel.status.as_mut().unwrap().state = "installed".into();
        assert_eq!(self_update_action(&panel), Some("restart"));
        assert!(self_update_status_text(&panel).contains("Restart"));

        panel.status.as_mut().unwrap().supported = false;
        assert_eq!(self_update_action(&panel), None);
        assert!(self_update_status_text(&panel).contains("cannot update itself"));
    }

    #[test]
    fn live_dictation_preserves_the_existing_composer_text() {
        assert_eq!(merge_dictation("", "  open the diff  "), "open the diff");
        assert_eq!(
            merge_dictation("Please", "open the diff"),
            "Please open the diff"
        );
        assert_eq!(
            merge_dictation("Please\n", "open the diff"),
            "Please\nopen the diff"
        );
    }

    #[test]
    fn slash_commands_match_only_an_unbroken_prefix() {
        let commands = vec!["review".into(), "rename".into(), "compact".into()];
        assert_eq!(command_suggestions(&commands, "/re"), ["review", "rename"]);
        assert_eq!(command_suggestions(&commands, "/RE"), ["review", "rename"]);
        assert!(command_suggestions(&commands, "re").is_empty());
        assert!(command_suggestions(&commands, "/review now").is_empty());
    }

    #[test]
    fn workflow_refresh_targets_persisted_and_live_rows_without_moving_offsets() {
        let marker = "workflow_run\n123\nhttps://github.com/RestartFU/xd/actions/runs/123";
        let model = AppModel {
            messages: vec![
                Message::new(Some(1), "user", "run it", None),
                Message::new(Some(2), "tool", marker, None),
            ],
            live_text: "Still working".into(),
            live_activity: vec![
                Message::new(None, "tool", "read file", None),
                Message::new(None, "tool", marker, None),
            ],
            ..Default::default()
        };

        assert_eq!(workflow_row_indices(&model, marker), [1, 4]);
        assert!(workflow_clock_active(&serde_json::json!({
            "ok": true,
            "state": "in_progress",
            "started_at": 1
        })));
        assert!(!workflow_clock_active(&serde_json::json!({
            "ok": true,
            "state": "completed",
            "started_at": 1,
            "completed_at": 2
        })));
    }

    #[test]
    fn queued_message_previews_bound_text_without_changing_the_source() {
        let prompt = format!("first\nsecond\nthird\nfourth {}", "x".repeat(1_000));
        let preview = queue_preview(&prompt);

        assert_eq!(preview, "first\nsecond\nthird…");
        assert!(preview.chars().count() <= 280);
        assert_eq!(prompt.lines().count(), 4);
    }

    #[test]
    fn markdown_code_controls_have_stable_scoped_ids() {
        let first = scoped_element_id("message-1", 0);
        assert_eq!(first, scoped_element_id("message-1", 0));
        assert_ne!(first, scoped_element_id("message-2", 0));
        assert_ne!(first, scoped_element_id("message-1", 1));
    }

    #[test]
    fn live_transcript_updates_reuse_the_persisted_message_snapshot() {
        let mut model = AppModel {
            messages: vec![Message::new(Some(1), "user", "history", None)],
            live_text: "first partial".into(),
            live_activity: vec![Message::new(None, "tool", "read file", None)],
            ..Default::default()
        };
        let mut snapshot = TranscriptSnapshot::default();
        snapshot.sync_messages(&model);
        snapshot.sync_live_text(&model);
        snapshot.sync_live_activity(&model);
        let persisted = snapshot.messages.clone();

        model.live_text = "second partial".into();
        snapshot.sync_live_text(&model);

        assert!(Arc::ptr_eq(&persisted, &snapshot.messages));
        assert_eq!(snapshot.get(0).unwrap().content, "history");
        assert_eq!(snapshot.get(1).unwrap().content, "second partial");
        assert_eq!(snapshot.get(2).unwrap().content, "read file");
    }

    #[test]
    fn collapsed_workspaces_hide_every_nested_descendant() {
        let folders = vec![
            Folder {
                id: "root".into(),
                name: "Root".into(),
                parent: None,
            },
            Folder {
                id: "child".into(),
                name: "Child".into(),
                parent: Some("root".into()),
            },
            Folder {
                id: "grandchild".into(),
                name: "Grandchild".into(),
                parent: Some("child".into()),
            },
        ];
        let collapsed = HashSet::from(["root".to_owned()]);

        assert!(!folder_hidden_by_collapse(&folders, &collapsed, "root"));
        assert!(folder_hidden_by_collapse(&folders, &collapsed, "child"));
        assert!(folder_hidden_by_collapse(
            &folders,
            &collapsed,
            "grandchild"
        ));
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
                working: false,
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
    fn sidebar_move_waits_for_the_authoritative_tree_parent() {
        let model = AppModel {
            folders: vec![
                Folder {
                    id: "folder-1".into(),
                    name: "Workspace".into(),
                    parent: Some("parent-2".into()),
                },
                Folder {
                    id: "parent-2".into(),
                    name: "Parent".into(),
                    parent: None,
                },
            ],
            chats: vec![xd_desktop::model::ChatSummary {
                id: "chat-1".into(),
                folder: "folder-1".into(),
                title: Some("Chat".into()),
                backend: "codex".into(),
                working: false,
            }],
            ..Default::default()
        };

        assert!(sidebar_move_applied(
            &model,
            &SidebarTarget::Folder("folder-1".into()),
            Some("parent-2")
        ));
        assert!(!sidebar_move_applied(
            &model,
            &SidebarTarget::Folder("folder-1".into()),
            None
        ));
        assert!(sidebar_move_applied(
            &model,
            &SidebarTarget::Chat("chat-1".into()),
            Some("folder-1")
        ));
        assert!(!sidebar_move_applied(
            &model,
            &SidebarTarget::Chat("chat-1".into()),
            None
        ));
    }

    #[test]
    fn sidebar_drag_rejects_cycles_roots_and_no_op_moves() {
        let model = AppModel {
            folders: vec![
                Folder {
                    id: "root".into(),
                    name: "Root".into(),
                    parent: None,
                },
                Folder {
                    id: "child".into(),
                    name: "Child".into(),
                    parent: Some("root".into()),
                },
                Folder {
                    id: "other".into(),
                    name: "Other".into(),
                    parent: None,
                },
            ],
            chats: vec![xd_desktop::model::ChatSummary {
                id: "chat".into(),
                folder: "child".into(),
                title: Some("Chat".into()),
                backend: "codex".into(),
                working: false,
            }],
            ..Default::default()
        };

        assert!(sidebar_drop_allowed(
            &model,
            &SidebarTarget::Folder("child".into()),
            None
        ));
        assert!(sidebar_drop_allowed(
            &model,
            &SidebarTarget::Folder("child".into()),
            Some("other")
        ));
        assert!(!sidebar_drop_allowed(
            &model,
            &SidebarTarget::Folder("root".into()),
            None
        ));
        assert!(!sidebar_drop_allowed(
            &model,
            &SidebarTarget::Folder("root".into()),
            Some("child")
        ));
        assert!(!sidebar_drop_allowed(
            &model,
            &SidebarTarget::Folder("child".into()),
            Some("root")
        ));
        assert!(sidebar_drop_allowed(
            &model,
            &SidebarTarget::Chat("chat".into()),
            Some("other")
        ));
        assert!(!sidebar_drop_allowed(
            &model,
            &SidebarTarget::Chat("chat".into()),
            Some("child")
        ));
        assert!(!sidebar_drop_allowed(
            &model,
            &SidebarTarget::Chat("chat".into()),
            None
        ));
    }

    #[test]
    fn optional_workspace_repository_ignores_only_blank_input() {
        assert_eq!(optional_trimmed("  /tmp/repo  "), Some("/tmp/repo"));
        assert_eq!(optional_trimmed(" \n\t "), None);
    }

    #[test]
    fn daemon_directory_navigation_keeps_root_and_child_paths_stable() {
        assert_eq!(directory_child_path("/", "workspace"), "/workspace");
        assert_eq!(
            directory_child_path("/home/danick", "projects"),
            "/home/danick/projects"
        );
        assert_eq!(directory_parent_path("/home/danick"), Some("/home".into()));
        assert_eq!(directory_parent_path("/"), None);
        assert_eq!(next_directory_selection(None, 0, 1), None);
        assert_eq!(next_directory_selection(None, 3, 1), Some(0));
        assert_eq!(next_directory_selection(None, 3, -1), Some(2));
        assert_eq!(next_directory_selection(Some(0), 3, -1), Some(0));
        assert_eq!(next_directory_selection(Some(1), 3, 1), Some(2));
        assert_eq!(next_directory_selection(Some(2), 3, 1), Some(2));
    }

    #[test]
    fn transcript_durations_are_labeled_with_units() {
        assert_eq!(turn_duration_label("5").as_deref(), Some("Worked for 5s"));
        assert_eq!(
            turn_duration_label("65").as_deref(),
            Some("Worked for 1m 05s")
        );
        assert_eq!(
            turn_duration_label("3661").as_deref(),
            Some("Worked for 1h 01m")
        );
        assert_eq!(turn_duration_label("not-a-duration"), None);
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
    fn paired_device_times_are_human_readable() {
        let now = 1_000_000;
        assert_eq!(relative_time(0, now), "unknown");
        assert_eq!(relative_time(now - 5, now), "just now");
        assert_eq!(relative_time(now - 300, now), "5m ago");
        assert_eq!(relative_time(now - 7_200, now), "2h ago");
        assert_eq!(relative_time(now - 259_200, now), "3d ago");
        assert_eq!(relative_time(now + 10, now), "just now");
    }

    #[test]
    fn daemon_reconnect_backoff_is_fast_then_bounded() {
        assert_eq!(reconnect_delay(0), Duration::ZERO);
        assert_eq!(reconnect_delay(1), Duration::from_millis(250));
        assert_eq!(reconnect_delay(2), Duration::from_millis(500));
        assert_eq!(reconnect_delay(4), Duration::from_secs(2));
        assert_eq!(reconnect_delay(5), Duration::from_secs(5));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_secs(5));
    }

    #[test]
    fn persisted_image_cache_is_bounded_without_evicting_inflight_reads() {
        let mut cache = MessageImageCache::default();
        for index in 0..MAX_CACHED_MESSAGE_IMAGES {
            assert!(cache.begin(&format!("/paste-{index}.png")));
        }
        assert!(!cache.begin("/deferred.png"));
        cache.finish("/paste-0.png", None);
        assert!(cache.begin("/deferred.png"));
        assert_eq!(cache.entries.len(), MAX_CACHED_MESSAGE_IMAGES);
        assert!(!cache.entries.contains_key("/paste-0.png"));
        assert!(matches!(
            cache.state("/deferred.png"),
            Some(MessageImageState::Loading)
        ));
    }

    #[test]
    fn pull_requests_require_a_clean_published_feature_branch() {
        let ready = GitStatus {
            branch: "feature".into(),
            base: "main".into(),
            upstream: "origin/feature".into(),
            clean: true,
            ..Default::default()
        };
        assert!(ready.can_open_pull_request());
        assert!(
            !GitStatus {
                ahead: 1,
                ..ready.clone()
            }
            .can_open_pull_request()
        );
        assert!(
            !GitStatus {
                branch: "main".into(),
                ..ready.clone()
            }
            .can_open_pull_request()
        );
        assert!(
            !GitStatus {
                clean: false,
                ..ready
            }
            .can_open_pull_request()
        );
    }

    #[test]
    fn terminal_geometry_tracks_the_visible_pane_and_stays_bounded() {
        assert_eq!(terminal_geometry(824.0, 252.0, 8.0, 19.0), (100, 12));
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

        let mut panel = TerminalPanel {
            chat_id: "chat".into(),
            sessions: vec![first, second],
            selected: Some("terminal-one".into()),
            viewport: None,
            opening: false,
            loading: false,
            error: None,
        };
        panel.remove("terminal-one");
        assert_eq!(panel.selected.as_deref(), Some("terminal-two"));
        assert_eq!(
            panel.selected().map(|session| session.title.as_str()),
            Some("shell two")
        );
    }

    #[test]
    fn pane_resizing_uses_each_divider_direction_and_bounds() {
        assert_eq!(
            resized_pane_size(PaneResizeKind::Sidebar, 272.0, Point { x: 48.0, y: 0.0 }),
            320
        );
        assert_eq!(
            resized_pane_size(PaneResizeKind::Diff, 460.0, Point { x: 40.0, y: 0.0 }),
            420
        );
        assert_eq!(
            resized_pane_size(PaneResizeKind::Terminal, 320.0, Point { x: 0.0, y: -60.0 }),
            380
        );
        assert_eq!(
            resized_pane_size(
                PaneResizeKind::Sidebar,
                272.0,
                Point {
                    x: -1_000.0,
                    y: 0.0
                }
            ),
            220
        );
        assert_eq!(
            resized_pane_size(
                PaneResizeKind::Diff,
                460.0,
                Point {
                    x: -1_000.0,
                    y: 0.0
                }
            ),
            760
        );
    }

    #[test]
    fn pane_state_keys_keep_devices_and_chats_isolated() {
        assert_eq!(
            pane_state_key(ChatEndpoint::Local, None, "same-chat"),
            "local/same-chat"
        );
        assert_eq!(
            pane_state_key(
                ChatEndpoint::Remote,
                Some(("dev.example", 4001)),
                "same-chat"
            ),
            "remote/dev.example:4001/same-chat"
        );
        assert_ne!(
            pane_state_key(ChatEndpoint::Remote, Some(("first.example", 4001)), "chat"),
            pane_state_key(ChatEndpoint::Remote, Some(("second.example", 4001)), "chat")
        );
        assert_eq!(pane_state_mask(false, false), 0);
        assert_eq!(pane_state_mask(true, false), PANE_DIFF);
        assert_eq!(pane_state_mask(false, true), PANE_TERMINAL);
        assert_eq!(pane_state_mask(true, true), PANE_DIFF | PANE_TERMINAL);
    }

    #[test]
    fn source_build_output_is_control_clean_and_bounded() {
        let mut panel = SourceBuildPanel::default();
        panel.append_output("start\u{1b}[31m red\u{1b}[0m\r\n".into());
        assert_eq!(panel.output_text(), "start red\n");
        for _ in 0..20 {
            panel.append_output("x".repeat(1_024));
        }
        assert!(panel.output_bytes <= MAX_SOURCE_BUILD_OUTPUT_BYTES);
        assert!(panel.output_text().len() <= MAX_SOURCE_BUILD_OUTPUT_BYTES);
    }

    #[test]
    fn unified_diffs_are_split_into_bounded_collapsible_files() {
        let patch = "diff --git a/one.go b/one.go\n--- a/one.go\n+++ b/one.go\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/two.rs b/two.rs\nnew file mode 100644\n--- /dev/null\n+++ b/two.rs\n@@ -0,0 +1 @@\n+ready\n";
        let (files, truncated) = parse_unified_diff(patch).unwrap();
        assert!(!truncated);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "one.go");
        assert_eq!((files[0].additions, files[0].deletions), (1, 1));
        assert_eq!(files[1].path, "two.rs");
        assert_eq!((files[1].additions, files[1].deletions), (1, 0));
    }

    #[test]
    fn diff_file_lists_choose_safe_lazy_read_modes_and_decode_git_paths() {
        let working =
            " M tracked file.rs\n?? new.txt\nR  old.rs -> renamed.rs\n?? \"caf\\303\\251.txt\"\n";
        let (files, truncated) = parse_diff_file_list(working, false).unwrap();
        assert!(!truncated);
        assert_eq!(
            files
                .iter()
                .map(|file| (file.path.as_str(), file.lazy_read.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("tracked file.rs", Some("working-file")),
                ("new.txt", Some("untracked-file")),
                ("renamed.rs", Some("working-file")),
                ("café.txt", Some("untracked-file")),
            ]
        );

        let (branch, _) = parse_diff_file_list("M\tone.go\nR100\told.rs\tnew.rs\n", true).unwrap();
        assert_eq!(
            branch
                .iter()
                .map(|file| (file.path.as_str(), file.lazy_read.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("one.go", Some("branch-file")),
                ("new.rs", Some("branch-file")),
            ]
        );
        assert!(decode_git_path("\"unterminated").is_err());
    }
}

fn main() {
    if env::args_os()
        .skip(1)
        .any(|argument| argument == "--version" || argument == "-v")
    {
        println!("xd {}", desktop_version());
        return;
    }
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("ctrl-k", OpenSearch, Some("XdDesktop")),
            KeyBinding::new("ctrl-f", OpenSearch, Some("XdDesktop")),
            KeyBinding::new("cmd-k", OpenSearch, Some("XdDesktop")),
            KeyBinding::new("cmd-f", OpenSearch, Some("XdDesktop")),
            KeyBinding::new("escape", CloseSearch, Some("XdDesktop")),
            KeyBinding::new("ctrl-1", SelectModel1, Some("XdDesktop")),
            KeyBinding::new("ctrl-2", SelectModel2, Some("XdDesktop")),
            KeyBinding::new("ctrl-3", SelectModel3, Some("XdDesktop")),
            KeyBinding::new("ctrl-4", SelectModel4, Some("XdDesktop")),
            KeyBinding::new("ctrl-5", SelectModel5, Some("XdDesktop")),
            KeyBinding::new("ctrl-6", SelectModel6, Some("XdDesktop")),
            KeyBinding::new("ctrl-7", SelectModel7, Some("XdDesktop")),
            KeyBinding::new("ctrl-8", SelectModel8, Some("XdDesktop")),
            KeyBinding::new("ctrl-9", SelectModel9, Some("XdDesktop")),
            KeyBinding::new("up", DirectoryPrevious, Some("XdDesktop")),
            KeyBinding::new("down", DirectoryNext, Some("XdDesktop")),
            KeyBinding::new("enter", DirectoryOpen, Some("XdDesktop")),
            KeyBinding::new("backspace", DirectoryParent, Some("XdDesktop")),
            KeyBinding::new("ctrl-enter", DirectoryChoose, Some("XdDesktop")),
            KeyBinding::new("cmd-enter", DirectoryChoose, Some("XdDesktop")),
            KeyBinding::new("backspace", Backspace, Some("ComposerInput")),
            KeyBinding::new("delete", Delete, Some("ComposerInput")),
            KeyBinding::new("left", Left, Some("ComposerInput")),
            KeyBinding::new("right", Right, Some("ComposerInput")),
            KeyBinding::new("shift-left", SelectLeft, Some("ComposerInput")),
            KeyBinding::new("shift-right", SelectRight, Some("ComposerInput")),
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
            KeyBinding::new("left", EditorLeft, Some("FileEditor")),
            KeyBinding::new("right", EditorRight, Some("FileEditor")),
            KeyBinding::new("up", EditorUp, Some("FileEditor")),
            KeyBinding::new("down", EditorDown, Some("FileEditor")),
            KeyBinding::new("shift-left", EditorSelectLeft, Some("FileEditor")),
            KeyBinding::new("shift-right", EditorSelectRight, Some("FileEditor")),
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
            KeyBinding::new("left", EditorLeft, Some("MessageEditor")),
            KeyBinding::new("right", EditorRight, Some("MessageEditor")),
            KeyBinding::new("up", EditorUp, Some("MessageEditor")),
            KeyBinding::new("down", EditorDown, Some("MessageEditor")),
            KeyBinding::new("shift-left", EditorSelectLeft, Some("MessageEditor")),
            KeyBinding::new("shift-right", EditorSelectRight, Some("MessageEditor")),
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
            KeyBinding::new("left", Left, Some("TerminalInput")),
            KeyBinding::new("right", Right, Some("TerminalInput")),
            KeyBinding::new("up", Up, Some("TerminalInput")),
            KeyBinding::new("down", Down, Some("TerminalInput")),
            KeyBinding::new("home", Home, Some("TerminalInput")),
            KeyBinding::new("end", End, Some("TerminalInput")),
            KeyBinding::new("enter", Submit, Some("TerminalInput")),
            KeyBinding::new("tab", Tab, Some("TerminalInput")),
            KeyBinding::new("escape", Escape, Some("TerminalInput")),
            KeyBinding::new("ctrl-c", Interrupt, Some("TerminalInput")),
            KeyBinding::new("ctrl-v", Paste, Some("TerminalInput")),
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
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some(
                    env::var("XD_APP_ID")
                        .unwrap_or_else(|_| "com.restartfu.Xd".into())
                        .into(),
                ),
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
