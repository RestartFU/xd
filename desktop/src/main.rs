use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    time::Duration,
};

#[cfg(target_os = "linux")]
use std::{
    process::{Command, Stdio},
    thread,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{
    App, Application, Bounds, ClickEvent, Context, CursorStyle, Decorations, Entity, Focusable,
    FontStyle, FontWeight, HighlightStyle, KeyBinding, ListAlignment, ListState, MouseButton,
    ObjectFit, PathPromptOptions, Render, ResizeEdge, StyledText, Timer, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, div, img, list,
    prelude::*, px, rgb, rgba, size,
};
use serde::Deserialize;
use serde_json::Value;
use xd_desktop::{
    activity::{ActivityCard, ActivityKind},
    daemon::{DaemonHandle, DaemonUpdate, RequestKind, StartedDaemon},
    markdown::{self, Block, CodeKind, InlineKind, InlineText},
    model::{AppModel, Attachment, Folder, Message},
};

mod input;
mod settings;
mod speech;
mod terminal;

use input::{
    Backspace, ComposerEvent, ComposerInput, Copy, Cut, Delete, Down, End, Escape, Home, Interrupt,
    Left, Paste, Right, SelectAll, SelectLeft, SelectRight, ShowCharacterPalette, Submit, Tab, Up,
};
use settings::{AccentPreset, AppSettings};
use speech::SpeechOutput;
use terminal::TerminalScreen;

// Keep the GPUI shell visually continuous with the established Crystal app.
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

gpui::actions!(xd, [OpenSearch, CloseSearch]);

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
    additions: usize,
    deletions: usize,
    lines: Vec<DiffLine>,
}

#[derive(Clone, Debug)]
struct FilePreview {
    path: String,
    content: String,
    truncated: bool,
}

#[derive(Clone, Default)]
struct DiffPanel {
    branch: bool,
    files_mode: bool,
    loading: bool,
    files: Vec<DiffFile>,
    error: Option<String>,
    truncated: bool,
    status: Option<GitStatus>,
    status_loading: bool,
    action: Option<String>,
    action_error: Option<String>,
    repo_files: Vec<String>,
    repo_files_truncated: bool,
    file_preview: Option<FilePreview>,
    file_loading: bool,
}

struct TerminalPanel {
    chat_id: String,
    terminal_id: Option<String>,
    title: String,
    screen: TerminalScreen,
    loading: bool,
    closed: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GitStatus {
    branch: String,
    upstream: String,
    ahead: u64,
    behind: u64,
    staged: u64,
    unstaged: u64,
    untracked: u64,
    conflicted: u64,
    clean: bool,
}

struct PendingSend {
    text: String,
    attachments: Vec<Attachment>,
    restore: bool,
}

struct PendingSpeech {
    chat_id: String,
    previous_assistant_id: Option<i64>,
}

#[derive(Clone)]
struct QueueEdit {
    chat_id: String,
    index: usize,
    original: String,
    text: String,
    submitting: Option<String>,
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

struct XdDesktop {
    model: AppModel,
    settings: AppSettings,
    settings_open: bool,
    speech_output: SpeechOutput,
    pending_speech: Option<PendingSpeech>,
    daemon: Option<DaemonHandle>,
    _started_daemon: Option<StartedDaemon>,
    transcript: ListState,
    composer_input: Entity<ComposerInput>,
    queue_edit_input: Entity<ComposerInput>,
    sidebar_edit_input: Entity<ComposerInput>,
    workspace_create_input: Entity<ComposerInput>,
    workspace_repo_input: Entity<ComposerInput>,
    workspace_clone_input: Entity<ComposerInput>,
    chat_create_input: Entity<ComposerInput>,
    workspace_context_input: Entity<ComposerInput>,
    workspace_workdir_input: Entity<ComposerInput>,
    workspace_repo_default_input: Entity<ComposerInput>,
    search_input: Entity<ComposerInput>,
    git_commit_input: Entity<ComposerInput>,
    repo_file_filter_input: Entity<ComposerInput>,
    terminal_input: Entity<ComposerInput>,
    composer: String,
    queue_edit: Option<QueueEdit>,
    sidebar_edit: Option<SidebarEdit>,
    sidebar_menu: Option<SidebarTarget>,
    pending_sidebar_delete: Option<SidebarTarget>,
    sidebar_delete_submitting: bool,
    sidebar_move: Option<SidebarTarget>,
    sidebar_move_submitting: bool,
    collapsed_folders: HashSet<String>,
    creating_workspace: bool,
    workspace_create_name: String,
    workspace_create_repo: String,
    workspace_create_clone: String,
    workspace_create_submitting: bool,
    workspace_clone_status: Option<String>,
    pending_clone_chats: HashSet<String>,
    workspace_clone_outcomes: HashMap<String, Option<String>>,
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
    expanded_activity: HashSet<String>,
}

impl XdDesktop {
    fn new(cx: &mut Context<Self>) -> Self {
        let composer_input = cx.new(|cx| ComposerInput::new(cx, "Message xd…"));
        cx.subscribe(&composer_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.composer_changed(text.clone(), cx),
            ComposerEvent::Submit => this.send_composer(cx),
            ComposerEvent::Bytes(_) => {}
        })
        .detach();
        let queue_edit_input = cx.new(|cx| ComposerInput::new(cx, "Edit queued message…"));
        cx.subscribe(&queue_edit_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.queue_edit_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_queue_edit(cx),
            ComposerEvent::Bytes(_) => {}
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
        let terminal_input = cx.new(ComposerInput::terminal);
        cx.subscribe(&terminal_input, |this, _, event, cx| {
            if let ComposerEvent::Bytes(bytes) = event {
                this.send_terminal_input(bytes, cx);
            }
        })
        .detach();
        let mut desktop = Self {
            model: AppModel {
                draft_revision: -1,
                ..Default::default()
            },
            settings: AppSettings::load(),
            settings_open: false,
            speech_output: SpeechOutput::default(),
            pending_speech: None,
            daemon: None,
            _started_daemon: None,
            transcript: ListState::new(0, ListAlignment::Bottom, px(700.0)),
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
            git_commit_input,
            repo_file_filter_input,
            terminal_input,
            composer: String::new(),
            queue_edit: None,
            sidebar_edit: None,
            sidebar_menu: None,
            pending_sidebar_delete: None,
            sidebar_delete_submitting: false,
            sidebar_move: None,
            sidebar_move_submitting: false,
            collapsed_folders: HashSet::new(),
            creating_workspace: false,
            workspace_create_name: String::new(),
            workspace_create_repo: String::new(),
            workspace_create_clone: String::new(),
            workspace_create_submitting: false,
            workspace_clone_status: None,
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
            expanded_activity: HashSet::new(),
        };
        desktop.connect(cx);
        desktop
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        match DaemonHandle::connect_or_start() {
            Ok((daemon, updates, started_daemon)) => {
                self.daemon = Some(daemon);
                self._started_daemon = started_daemon;
                self.model.connection_error = None;
                cx.spawn(async move |this, cx| {
                    while let Ok(update) = updates.recv().await {
                        if this
                            .update(cx, |this, cx| this.handle_daemon_update(update, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => {
                self.model.connected = false;
                self.model.connection_error = Some(error.to_string());
            }
        }
    }

    fn handle_daemon_update(&mut self, update: DaemonUpdate, cx: &mut Context<Self>) {
        match update {
            DaemonUpdate::Connected { .. } => {
                self.model.connected = true;
                self.model.connection_error = None;
                self.request_tree();
                self.request_agent_catalog();
            }
            DaemonUpdate::Disconnected { message } => {
                self.model.connected = false;
                self.model.connection_error = Some(message);
                self.sending = false;
                self.workspace_create_submitting = false;
                self.chat_create_submitting = false;
                self.workspace_context_loading = false;
                self.workspace_context_submitting = false;
                self.workspace_clone_status = None;
                self.pending_clone_chats.clear();
                self.workspace_clone_outcomes.clear();
                self.pending_speech = None;
                self.speech_output.stop();
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
                    panel.error = Some("Terminal disconnected.".into());
                }
                self.restore_pending_send(cx);
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
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            if !matches!(
                &kind,
                RequestKind::DiffRead { .. }
                    | RequestKind::GitStatus { .. }
                    | RequestKind::RepositoryFiles { .. }
                    | RequestKind::RepositoryFile { .. }
                    | RequestKind::GitCommit { .. }
                    | RequestKind::GitPush { .. }
                    | RequestKind::TerminalOpen { .. }
                    | RequestKind::TerminalList { .. }
                    | RequestKind::TerminalInput { .. }
                    | RequestKind::TerminalResize { .. }
                    | RequestKind::TerminalKill { .. }
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
                RequestKind::Messages { chat_id }
                    if self
                        .pending_speech
                        .as_ref()
                        .is_some_and(|pending| pending.chat_id == *chat_id) =>
                {
                    self.pending_speech = None;
                }
                RequestKind::NewFolder {
                    name,
                    repo,
                    repo_url,
                } if self.creating_workspace
                    && self.workspace_create_name.trim() == name
                    && optional_trimmed(&self.workspace_create_repo) == repo.as_deref()
                    && optional_trimmed(&self.workspace_create_clone) == repo_url.as_deref() =>
                {
                    self.workspace_create_submitting = false;
                }
                RequestKind::NewChat { folder_id, title }
                    if self.creating_chat_folder.as_deref() == Some(folder_id)
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
                RequestKind::DiffRead { generation, .. } if *generation == self.diff_generation => {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.loading = false;
                        diff.error = value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
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
                RequestKind::RepositoryFiles { generation, .. }
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
                RequestKind::RepositoryFile { generation, .. }
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
                RequestKind::TerminalOpen { chat_id } | RequestKind::TerminalList { chat_id }
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
                | RequestKind::TerminalResize { terminal_id }
                | RequestKind::TerminalKill { terminal_id }
                    if self
                        .terminal_panel
                        .as_ref()
                        .and_then(|panel| panel.terminal_id.as_ref())
                        == Some(terminal_id) =>
                {
                    if let Some(panel) = &mut self.terminal_panel {
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
                if let Err(error) = self.model.apply_tree(&value) {
                    self.model.connection_error = Some(format!("Invalid tree response: {error}"));
                    return;
                }
                if self
                    .sidebar_edit
                    .as_ref()
                    .is_some_and(|edit| !self.sidebar_target_exists(&edit.target))
                {
                    self.cancel_sidebar_edit(cx);
                }
                if self
                    .sidebar_menu
                    .as_ref()
                    .is_some_and(|target| !self.sidebar_target_exists(target))
                {
                    self.sidebar_menu = None;
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
                    .is_some_and(|target| !self.sidebar_target_exists(target))
                {
                    self.sidebar_move = None;
                    self.sidebar_move_submitting = false;
                }
                self.collapsed_folders.retain(|folder_id| {
                    self.model
                        .folders
                        .iter()
                        .any(|folder| &folder.id == folder_id)
                });
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
                if self.model.selected_chat.is_none() {
                    if let Some(chat_id) = self.model.chats.first().map(|chat| chat.id.clone()) {
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
            RequestKind::DiffRead {
                chat_id,
                read,
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
                    } else if let Some(daemon) = &self.daemon
                        && let Err(error) =
                            daemon.diff_read(&chat_id, "branch-all", Some(base), generation)
                    {
                        if let Some(diff) = &mut self.diff_panel {
                            diff.loading = false;
                            diff.error = Some(error);
                        }
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
                        if let Some(diff) = &mut self.diff_panel {
                            diff.status = Some(status);
                            diff.status_loading = false;
                            diff.action_error = None;
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
            RequestKind::RepositoryFiles {
                chat_id,
                generation,
            } => {
                if generation != self.diff_generation
                    || self.model.selected_chat.as_deref() != Some(chat_id.as_str())
                    || !self.diff_panel.as_ref().is_some_and(|diff| diff.files_mode)
                {
                    return;
                }
                match serde_json::from_value::<Vec<String>>(
                    value.get("files").cloned().unwrap_or_default(),
                ) {
                    Ok(files) => {
                        if let Some(diff) = &mut self.diff_panel {
                            diff.repo_files = files;
                            diff.repo_files_truncated = value
                                .get("truncated")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            diff.file_preview = None;
                            diff.loading = false;
                            diff.error = None;
                        }
                    }
                    Err(error) => {
                        if let Some(diff) = &mut self.diff_panel {
                            diff.loading = false;
                            diff.error = Some(format!("Invalid repository file list: {error}"));
                        }
                    }
                }
            }
            RequestKind::RepositoryFile {
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
                let response_path = value
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let content = value.get("content").and_then(Value::as_str);
                if response_path != path || content.is_none() {
                    if let Some(diff) = &mut self.diff_panel {
                        diff.file_loading = false;
                        diff.error = Some("Invalid repository file response.".into());
                    }
                    return;
                }
                if let Some(diff) = &mut self.diff_panel {
                    diff.file_preview = Some(FilePreview {
                        path,
                        content: content.unwrap_or_default().to_owned(),
                        truncated: value
                            .get("truncated")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                    diff.file_loading = false;
                    diff.error = None;
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
            RequestKind::Shortcuts { folder_id } => {
                if self
                    .model
                    .selected_summary()
                    .is_some_and(|chat| chat.folder == folder_id)
                {
                    self.model.apply_shortcuts(&value);
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
                        folder_id,
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
            RequestKind::NewFolder {
                name,
                repo,
                repo_url,
            } => {
                let Some(folder_id) = value.get("id").and_then(Value::as_str) else {
                    self.model.connection_error =
                        Some("The daemon returned no workspace id.".into());
                    return;
                };
                if self.creating_workspace
                    && self.workspace_create_name.trim() == name
                    && optional_trimmed(&self.workspace_create_repo) == repo.as_deref()
                    && optional_trimmed(&self.workspace_create_clone) == repo_url.as_deref()
                {
                    self.cancel_workspace_create(cx);
                }
                if value.get("cloning").and_then(Value::as_str).is_some() {
                    match self.workspace_clone_outcomes.remove(folder_id) {
                        Some(None) => {
                            self.workspace_clone_status = Some("Repository cloned".into());
                            if let Some(daemon) = &self.daemon
                                && let Err(error) = daemon.new_chat(folder_id, "New Chat")
                            {
                                self.model.connection_error = Some(error);
                            }
                        }
                        Some(Some(error)) => {
                            self.workspace_clone_status = None;
                            self.model.connection_error = Some(error);
                        }
                        None => {
                            self.pending_clone_chats.insert(folder_id.to_owned());
                            self.workspace_clone_status = Some("Cloning repository…".into());
                        }
                    }
                } else if let Some(daemon) = &self.daemon
                    && let Err(error) = daemon.new_chat(folder_id, "New Chat")
                {
                    self.model.connection_error = Some(error);
                }
            }
            RequestKind::NewChat { folder_id, title } => {
                let Some(chat_id) = value.get("id").and_then(Value::as_str) else {
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
            RequestKind::RenameFolder { folder_id, name } => {
                if self.sidebar_edit.as_ref().is_some_and(|edit| {
                    edit.target == SidebarTarget::Folder(folder_id) && edit.text.trim() == name
                }) {
                    self.cancel_sidebar_edit(cx);
                }
            }
            RequestKind::MoveFolder {
                folder_id,
                parent_id,
            } => {
                if self.sidebar_move.as_ref() == Some(&SidebarTarget::Folder(folder_id)) {
                    self.sidebar_move = None;
                    self.sidebar_move_submitting = false;
                }
                let _ = parent_id;
            }
            RequestKind::TrashFolder { folder_id } => {
                if self.pending_sidebar_delete.as_ref() == Some(&SidebarTarget::Folder(folder_id)) {
                    self.pending_sidebar_delete = None;
                    self.sidebar_delete_submitting = false;
                }
            }
            RequestKind::RenameChat { chat_id, title } => {
                if self.sidebar_edit.as_ref().is_some_and(|edit| {
                    edit.target == SidebarTarget::Chat(chat_id) && edit.text.trim() == title
                }) {
                    self.cancel_sidebar_edit(cx);
                }
            }
            RequestKind::MoveChat { chat_id, folder_id } => {
                if self.sidebar_move.as_ref() == Some(&SidebarTarget::Chat(chat_id)) {
                    self.sidebar_move = None;
                    self.sidebar_move_submitting = false;
                }
                let _ = folder_id;
            }
            RequestKind::DeleteChat { chat_id } => {
                if self.pending_sidebar_delete.as_ref() == Some(&SidebarTarget::Chat(chat_id)) {
                    self.pending_sidebar_delete = None;
                    self.sidebar_delete_submitting = false;
                }
            }
            RequestKind::Chat { chat_id } if self.chat_is_active(&chat_id) => {
                let local_attachments = self
                    .attachments_dirty
                    .then(|| self.model.draft_attachments.clone());
                self.model.apply_chat(&value);
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
            RequestKind::Messages { chat_id } if self.chat_is_active(&chat_id) => {
                if let Err(error) = self.model.apply_messages(&value) {
                    self.model.connection_error =
                        Some(format!("Invalid transcript response: {error}"));
                    return;
                }
                if !self.model.working {
                    self.model.live_text.clear();
                    self.model.live_activity.clear();
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
                self.transcript.reset(self.model.display_message_count());
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
            RequestKind::SetShortcuts => {}
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
            RequestKind::TerminalOpen { chat_id }
                if self
                    .terminal_panel
                    .as_ref()
                    .is_some_and(|panel| panel.chat_id == chat_id) =>
            {
                if let Some(panel) = &mut self.terminal_panel {
                    panel.terminal_id = value.get("id").and_then(Value::as_str).map(str::to_owned);
                    panel.loading = true;
                    panel.closed = false;
                }
                if let Some(daemon) = &self.daemon
                    && let Err(error) = daemon.terminal_list(&chat_id)
                    && let Some(panel) = &mut self.terminal_panel
                {
                    panel.loading = false;
                    panel.error = Some(error);
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
            RequestKind::TerminalKill { terminal_id } => {
                if let Some(panel) = &mut self.terminal_panel
                    && panel.terminal_id.as_deref() == Some(terminal_id.as_str())
                {
                    panel.closed = true;
                }
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
            "terminal-opened" if self.event_is_active(&body) => {
                if let Some(panel) = &mut self.terminal_panel {
                    panel.terminal_id = body
                        .get("terminal")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    panel.title = body
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Terminal")
                        .to_owned();
                    panel.closed = false;
                }
            }
            "terminal-output" if self.event_is_active(&body) => {
                if let Some(panel) = &mut self.terminal_panel
                    && panel.terminal_id.as_deref() == body.get("terminal").and_then(Value::as_str)
                    && let Some(data) = body.get("data").and_then(Value::as_str)
                    && let Ok(data) = STANDARD.decode(data)
                {
                    panel.screen.feed(&data);
                }
            }
            "terminal-resized" if self.event_is_active(&body) => {
                if let Some(panel) = &mut self.terminal_panel
                    && panel.terminal_id.as_deref() == body.get("terminal").and_then(Value::as_str)
                {
                    let columns =
                        body.get("columns").and_then(Value::as_u64).unwrap_or(120) as usize;
                    let rows = body.get("rows").and_then(Value::as_u64).unwrap_or(32) as usize;
                    panel.screen.resize(columns, rows);
                }
            }
            "terminal-closed" if self.event_is_active(&body) => {
                if let Some(panel) = &mut self.terminal_panel
                    && panel.terminal_id.as_deref() == body.get("terminal").and_then(Value::as_str)
                {
                    panel.closed = true;
                    panel.loading = false;
                }
            }
            "tree" => self.request_tree(),
            "changed" if self.event_is_active(&body) => {
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_chat(&chat_id);
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
            "turn-started" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
                self.sync_transcript_count(false);
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_messages(&chat_id);
                    self.request_chat(&chat_id);
                }
            }
            "text" | "tool" if self.event_is_active(&body) => {
                let old_count = self.model.display_message_count();
                self.model.apply_event(name, &body);
                let new_count = self.model.display_message_count();
                if new_count > old_count {
                    self.transcript
                        .splice(old_count..old_count, new_count - old_count);
                } else if new_count > 0 {
                    self.transcript.splice(new_count - 1..new_count, 1);
                }
            }
            "turn-finished" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
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
            "folder-clone" => {
                let folder_id = body.get("folder").and_then(Value::as_str);
                let preserve_outcome = folder_id
                    .is_some_and(|folder_id| self.pending_clone_chats.contains(folder_id))
                    || (self.workspace_create_submitting
                        && optional_trimmed(&self.workspace_create_clone)
                            == body.get("url").and_then(Value::as_str));
                match body.get("state").and_then(Value::as_str) {
                    Some("cloning") => {
                        self.workspace_clone_status = Some("Cloning repository…".into());
                    }
                    Some("ready") => {
                        self.workspace_clone_status = Some("Repository cloned".into());
                        if preserve_outcome && let Some(folder_id) = folder_id {
                            self.workspace_clone_outcomes
                                .insert(folder_id.to_owned(), None);
                        }
                        if folder_id
                            .is_some_and(|folder_id| self.pending_clone_chats.remove(folder_id))
                        {
                            if let (Some(daemon), Some(folder_id)) = (&self.daemon, folder_id)
                                && let Err(error) = daemon.new_chat(folder_id, "New Chat")
                            {
                                self.model.connection_error = Some(error);
                            }
                            if let Some(folder_id) = folder_id {
                                self.workspace_clone_outcomes.remove(folder_id);
                            }
                        }
                    }
                    Some("failed") => {
                        let error = body
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("Git could not clone that repository.")
                            .to_owned();
                        if preserve_outcome && let Some(folder_id) = folder_id {
                            self.workspace_clone_outcomes
                                .insert(folder_id.to_owned(), Some(error.clone()));
                        }
                        if folder_id
                            .is_some_and(|folder_id| self.pending_clone_chats.remove(folder_id))
                        {
                            if let Some(folder_id) = folder_id {
                                self.workspace_clone_outcomes.remove(folder_id);
                            }
                        }
                        self.workspace_clone_status = None;
                        self.model.connection_error = Some(error);
                    }
                    _ => {}
                }
            }
            "agent-auth-changed"
                if body.get("provider").and_then(Value::as_str)
                    == Some(self.model.backend.as_str()) =>
            {
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_chat(&chat_id);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn request_tree(&mut self) {
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.tree() {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn request_agent_catalog(&mut self) {
        if let Some(daemon) = &self.daemon
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
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.shortcuts(&folder_id)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn begin_workspace_create(&mut self, cx: &mut Context<Self>) {
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
            .daemon
            .as_ref()
            .ok_or_else(|| "xd-dev is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.new_folder(name, repo.as_deref(), repo_url.as_deref()));
        match result {
            Ok(()) => {
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
            .daemon
            .as_ref()
            .ok_or_else(|| "xd-dev is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.new_chat(&folder_id, title));
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
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.folder_context(&folder_id) {
                self.workspace_context_loading = false;
                self.model.connection_error = Some(error);
            }
        } else {
            self.workspace_context_loading = false;
            self.model.connection_error = Some("xd-dev is not connected to a daemon.".into());
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
            .daemon
            .as_ref()
            .ok_or_else(|| "xd-dev is not connected to a daemon.".to_owned())
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
        let Some(daemon) = self.daemon.as_ref() else {
            self.model.connection_error = Some("xd-dev is not connected to a daemon.".into());
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
            .daemon
            .as_ref()
            .ok_or_else(|| "xd-dev is not connected to a daemon.".to_owned())
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
                match this.daemon.as_ref() {
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
                            Some("xd-dev is not connected to a daemon.".into());
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
        if self.settings_open {
            self.search_generation = self.search_generation.saturating_add(1);
            self.search = None;
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
            cx.notify();
            return;
        }
        self.terminal_panel = None;
        self.diff_panel = Some(DiffPanel::default());
        self.refresh_diff(cx);
    }

    fn toggle_terminal_panel(&mut self, cx: &mut Context<Self>) {
        if self.terminal_panel.is_some() {
            self.terminal_panel = None;
            cx.notify();
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        self.diff_generation = self.diff_generation.saturating_add(1);
        self.diff_panel = None;
        self.terminal_panel = Some(TerminalPanel {
            chat_id: chat_id.clone(),
            terminal_id: None,
            title: "Terminal".into(),
            screen: TerminalScreen::default(),
            loading: true,
            closed: false,
            error: None,
        });
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.terminal_open(&chat_id, 68, 32)
                && let Some(panel) = &mut self.terminal_panel
            {
                panel.loading = false;
                panel.error = Some(error);
            }
        } else if let Some(panel) = &mut self.terminal_panel {
            panel.loading = false;
            panel.error = Some("xd-dev is not connected to a daemon.".into());
        }
        cx.notify();
    }

    fn send_terminal_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let Some(terminal_id) = self
            .terminal_panel
            .as_ref()
            .and_then(|panel| panel.terminal_id.clone())
        else {
            return;
        };
        if let Some(daemon) = &self.daemon
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
            .and_then(|panel| panel.terminal_id.clone())
        else {
            return;
        };
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.terminal_kill(&terminal_id)
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn restart_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        panel.terminal_id = None;
        panel.screen = TerminalScreen::new(68, 32);
        panel.loading = true;
        panel.closed = false;
        panel.error = None;
        let chat_id = panel.chat_id.clone();
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.terminal_open(&chat_id, 68, 32)
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.loading = false;
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn apply_terminal_list(&mut self, value: &Value) {
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        panel.loading = false;
        let Some(terminal) = value
            .get("terminals")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("id").and_then(Value::as_str) == panel.terminal_id.as_deref()
                })
            })
        else {
            panel.closed = true;
            return;
        };
        panel.title = terminal
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Terminal")
            .to_owned();
        panel.screen = TerminalScreen::new(
            terminal
                .get("columns")
                .and_then(Value::as_u64)
                .unwrap_or(120) as usize,
            terminal.get("rows").and_then(Value::as_u64).unwrap_or(32) as usize,
        );
        if let Some(replay) = terminal.get("replay").and_then(Value::as_array) {
            for frame in replay {
                if let Some(data) = frame
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(|data| STANDARD.decode(data).ok())
                {
                    panel.screen.feed(&data);
                } else if let (Some(columns), Some(rows)) = (
                    frame.get("columns").and_then(Value::as_u64),
                    frame.get("rows").and_then(Value::as_u64),
                ) {
                    panel.screen.resize(columns as usize, rows as usize);
                }
            }
        }
        panel.closed = false;
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
        }
        self.refresh_diff(cx);
    }

    fn read_repository_file(&mut self, path: String, cx: &mut Context<Self>) {
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
            .daemon
            .as_ref()
            .ok_or_else(|| "xd-dev is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.repository_file(&chat_id, &path, generation));
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.file_loading = false;
            diff.error = Some(error);
        }
        cx.notify();
    }

    fn close_file_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(diff) = &mut self.diff_panel {
            diff.file_preview = None;
            diff.file_loading = false;
            diff.error = None;
        }
        cx.notify();
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
            .daemon
            .as_ref()
            .ok_or_else(|| "xd-dev is not connected to a daemon.".to_owned())
            .and_then(|daemon| daemon.git_commit(&chat_id, &message, generation));
        if let Err(error) = result
            && let Some(diff) = &mut self.diff_panel
        {
            diff.action = None;
            diff.action_error = Some(error);
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
            .daemon
            .as_ref()
            .ok_or_else(|| "xd-dev is not connected to a daemon.".to_owned())
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
            diff.status_loading = true;
            diff.files.clear();
            diff.file_preview = None;
            diff.file_loading = false;
            diff.error = None;
            diff.truncated = false;
        } else {
            return;
        }
        match self.daemon.as_ref() {
            Some(daemon) => {
                let content = if files_mode {
                    daemon.repository_files(&chat_id, generation)
                } else {
                    daemon.diff_read(
                        &chat_id,
                        if branch { "base" } else { "working-all" },
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
                if let Err(error) = daemon.git_status(&chat_id, generation)
                    && let Some(diff) = &mut self.diff_panel
                {
                    diff.status_loading = false;
                    diff.action_error = Some(error);
                }
            }
            None => {
                if let Some(diff) = &mut self.diff_panel {
                    diff.loading = false;
                    diff.status_loading = false;
                    diff.error = Some("xd-dev is not connected to a daemon.".into());
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

    fn toggle_diff_file(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.collapsed_diff_files.remove(&path) {
            self.collapsed_diff_files.insert(path);
        }
        cx.notify();
    }

    fn begin_sidebar_edit(
        &mut self,
        target: SidebarTarget,
        current: String,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_menu = None;
        self.pending_sidebar_delete = None;
        self.sidebar_delete_submitting = false;
        self.sidebar_move = None;
        self.sidebar_move_submitting = false;
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

    fn toggle_sidebar_menu(&mut self, target: SidebarTarget, cx: &mut Context<Self>) {
        self.sidebar_menu = if self.sidebar_menu.as_ref() == Some(&target) {
            None
        } else {
            Some(target)
        };
        self.pending_sidebar_delete = None;
        self.sidebar_delete_submitting = false;
        self.sidebar_move = None;
        self.sidebar_move_submitting = false;
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
        let result = self.daemon.as_ref().map(|daemon| match &edit.target {
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
            None => {
                self.model.connection_error = Some("xd-dev is not connected to a daemon.".into())
            }
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
            self.pending_sidebar_delete = Some(target);
            cx.notify();
            return;
        }
        let result = self.daemon.as_ref().map(|daemon| match &target {
            SidebarTarget::Folder(folder_id) => daemon.trash_folder(folder_id),
            SidebarTarget::Chat(chat_id) => daemon.delete_chat(chat_id),
        });
        match result {
            Some(Ok(())) => self.sidebar_delete_submitting = true,
            Some(Err(error)) => self.model.connection_error = Some(error),
            None => {
                self.model.connection_error = Some("xd-dev is not connected to a daemon.".into())
            }
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
        } else {
            self.sidebar_move = Some(target);
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
        let result = self.daemon.as_ref().map(|daemon| match &target {
            SidebarTarget::Folder(folder_id) => {
                daemon.move_folder(folder_id, destination.as_deref())
            }
            SidebarTarget::Chat(chat_id) => destination
                .as_deref()
                .ok_or_else(|| "A chat needs a destination workspace.".to_owned())
                .and_then(|folder_id| daemon.move_chat(chat_id, folder_id)),
        });
        match result {
            Some(Ok(())) => self.sidebar_move_submitting = true,
            Some(Err(error)) => self.model.connection_error = Some(error),
            None => {
                self.model.connection_error = Some("xd-dev is not connected to a daemon.".into())
            }
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
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_new_worktree(&chat_id, !self.model.new_worktree)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cycle_model(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working {
            return;
        }
        let choices = self
            .model
            .agent_backends
            .iter()
            .flat_map(|backend| {
                backend
                    .models
                    .iter()
                    .map(|model| (backend.id.clone(), model.id.clone()))
            })
            .collect::<Vec<_>>();
        if choices.is_empty() {
            return;
        }
        let current = choices
            .iter()
            .position(|(backend, model)| {
                backend == &self.model.backend
                    && Some(model.as_str()) == self.model.model.as_deref()
            })
            .unwrap_or(choices.len() - 1);
        let (backend, model) = &choices[(current + 1) % choices.len()];
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_model(&chat_id, backend, model)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cycle_effort(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working {
            return;
        }
        let Some(backend) = self
            .model
            .agent_backends
            .iter()
            .find(|backend| backend.id == self.model.backend)
        else {
            return;
        };
        if backend.efforts.is_empty() {
            return;
        }
        let current = backend
            .efforts
            .iter()
            .position(|effort| effort == &self.model.effort)
            .unwrap_or(backend.efforts.len() - 1);
        let effort = backend.efforts[(current + 1) % backend.efforts.len()].clone();
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_effort(&chat_id, &effort)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cycle_access(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working {
            return;
        }
        const ACCESS: [&str; 3] = ["read-only", "edit", "full"];
        let current = ACCESS
            .iter()
            .position(|access| *access == self.model.access)
            .unwrap_or(ACCESS.len() - 1);
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_access(&chat_id, ACCESS[(current + 1) % ACCESS.len()])
        {
            self.model.connection_error = Some(error);
        }
    }

    fn toggle_plan(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working {
            return;
        }
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_plan(&chat_id, !self.model.plan)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cycle_workspace(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.has_messages || self.model.working || self.model.worktrees.len() < 2 {
            return;
        }
        let current = self
            .model
            .worktrees
            .iter()
            .position(|worktree| worktree.current)
            .unwrap_or(0);
        let next = (current + 1) % self.model.worktrees.len();
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_workspace(&chat_id, &self.model.worktrees[next].path)
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
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.remove_worktree(&chat_id, &worktree.path)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn request_chat(&mut self, chat_id: &str) {
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.chat(chat_id) {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn request_messages(&mut self, chat_id: &str) {
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.messages(chat_id) {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn select_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        if self.model.selected_chat.as_deref() == Some(chat_id.as_str()) {
            return;
        }
        self.model.select_chat(chat_id.clone());
        self.pending_speech = None;
        self.speech_output.stop();
        self.terminal_panel = None;
        self.set_composer_text(String::new(), cx);
        self.draft_dirty = false;
        self.attachments_dirty = false;
        self.pending_send = None;
        self.cancel_queue_edit(cx);
        self.sending = false;
        self.transcript.reset(0);
        self.request_chat(&chat_id);
        self.request_messages(&chat_id);
        self.request_shortcuts();
        if self.diff_panel.is_some() {
            self.repo_file_filter.clear();
            self.repo_file_filter_input
                .update(cx, |input, cx| input.set_text("", cx));
            if let Some(diff) = &mut self.diff_panel {
                diff.action = None;
                diff.action_error = None;
            }
            self.refresh_diff(cx);
        }
        cx.notify();
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
        let Some(daemon) = self.daemon.clone() else {
            self.model.connection_error = Some("xd-dev is not connected to a daemon.".into());
            return;
        };
        if let Err(error) = daemon.send_message(&chat_id, &text, &attachments) {
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

    fn send_shortcut(&mut self, prompt: String) {
        if self.sending || prompt.trim().is_empty() {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(daemon) = self.daemon.clone() else {
            self.model.connection_error = Some("xd-dev is not connected to a daemon.".into());
            return;
        };
        if let Err(error) = daemon.send_message(&chat_id, &prompt, &[]) {
            self.model.connection_error = Some(error);
            return;
        }
        self.sending = true;
        self.pending_send = Some(PendingSend {
            text: prompt,
            attachments: Vec::new(),
            restore: false,
        });
    }

    fn drop_queued(&mut self, index: usize) {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return;
        };
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.drop_queue(chat_id, index)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn begin_queue_edit(&mut self, index: usize, prompt: String, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
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
        if let Some(daemon) = &self.daemon {
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

    fn steer_queued(&mut self, index: usize, text: &str) {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return;
        };
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.steer_queue(chat_id, index, text)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cancel_turn(&mut self) {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return;
        };
        if let Some(daemon) = &self.daemon
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
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_shortcuts(folder_id.as_deref(), &shortcuts)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn set_composer_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.composer.clone_from(&text);
        self.composer_input
            .update(cx, |input, cx| input.set_text(text, cx));
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
        if let Some(daemon) = &self.daemon {
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
        cx.notify();
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

    fn message_row(
        message: &Message,
        index: usize,
        expanded: bool,
        desktop: Entity<Self>,
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
                ActivityCard::parse(&message.content),
                key,
                index,
                expanded,
                desktop,
            );
        }

        div()
            .w_full()
            .px_4()
            .py_2()
            .flex()
            .when(is_user, |row| row.justify_end())
            .child(
                div()
                    .when(!is_user, |body| body.w_full().max_w(px(1040.0)).mx_auto())
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
                        Self::markdown_content(message.markdown())
                            .text_sm()
                            .line_height(px(21.0)),
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
        let status_color = match card.kind {
            ActivityKind::Running => 0x91a7ff,
            ActivityKind::Success => 0x8bd5a0,
            ActivityKind::Failure => 0xff8f8f,
            ActivityKind::Finished => 0xaab2c0,
        };
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
                            if !this.expanded_activity.remove(&toggle_key) {
                                this.expanded_activity.insert(toggle_key.clone());
                            }
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
                    .child(card.detail),
            );
            if let Some(footer) = card.footer {
                body = body.child(
                    div()
                        .px_4()
                        .pb_3()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(footer),
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

    fn markdown_content(document: std::sync::Arc<markdown::Document>) -> gpui::Div {
        let mut content = div().w_full().flex().flex_col().gap_2();
        for block in document.blocks.iter().cloned() {
            let element = match block {
                Block::Heading { level, content } => {
                    let heading = div()
                        .mt_1()
                        .font_weight(FontWeight::BOLD)
                        .child(Self::inline_text(content));
                    match level {
                        1 => heading.text_xl().line_height(px(30.0)),
                        2 => heading.text_lg().line_height(px(27.0)),
                        _ => heading.text_base().line_height(px(24.0)),
                    }
                    .into_any_element()
                }
                Block::Paragraph(content) => div()
                    .whitespace_normal()
                    .child(Self::inline_text(content))
                    .into_any_element(),
                Block::Quote(content) => div()
                    .pl_3()
                    .py_1()
                    .border_l_2()
                    .border_color(rgb(0x59647a))
                    .text_color(rgb(MUTED))
                    .child(Self::inline_text(content))
                    .into_any_element(),
                Block::ListItem { ordered, content } => div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .w(px(18.0))
                            .flex_none()
                            .text_color(rgb(MUTED))
                            .child(if ordered { "1." } else { "•" }),
                    )
                    .child(div().flex_1().child(Self::inline_text(content)))
                    .into_any_element(),
                Block::Rule => div()
                    .w_full()
                    .h(px(1.0))
                    .my_2()
                    .bg(rgb(BORDER))
                    .into_any_element(),
                Block::Code(code) => {
                    let language = code.language.unwrap_or_else(|| "text".into());
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
                                .bg(rgb(SURFACE_HIGH))
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(language),
                        )
                        .child(
                            div()
                                .w_full()
                                .p_3()
                                .font_family("monospace")
                                .text_sm()
                                .line_height(px(20.0))
                                .whitespace_normal()
                                .child(StyledText::new(code.code).with_highlights(highlights)),
                        )
                        .into_any_element()
                }
            };
            content = content.child(element);
        }
        content
    }

    fn inline_text(content: InlineText) -> StyledText {
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
        StyledText::new(content.text).with_highlights(highlights)
    }
}

impl Render for XdDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let client_decorations = matches!(window.window_decorations(), Decorations::Client { .. });
        window.set_client_inset(if client_decorations { px(6.0) } else { px(0.0) });
        let accent = self.settings.accent.color();
        let accent_hover = self.settings.accent.hover_color();
        let messages = self.model.display_messages();
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
        let access_label = match self.model.access.as_str() {
            "full" => "Full access",
            "edit" => "Edit",
            _ => "Read only",
        };
        let status_text = if self.model.connected {
            "connected"
        } else {
            "offline"
        };
        let status_color = if self.model.connected {
            0x92d5a5
        } else {
            0xe49a9a
        };

        let sidebar_edit = self.sidebar_edit.clone();
        let sidebar_menu = self.sidebar_menu.clone();
        let sidebar_edit_input = self.sidebar_edit_input.clone();
        let sidebar_edit_focus = self.sidebar_edit_input.read(cx).focus_handle(cx);
        let pending_sidebar_delete = self.pending_sidebar_delete.clone();
        let sidebar_delete_submitting = self.sidebar_delete_submitting;
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
        let mut tree_rows = Vec::new();
        let mut chat_row_index = 0_usize;
        for (folder_row_index, folder) in self.model.folders.clone().into_iter().enumerate() {
            if self.folder_hidden_by_collapse(&folder.id) {
                continue;
            }
            let indent = if folder.parent.is_some() { 22.0 } else { 12.0 };
            let new_chat_folder_id = folder.id.clone();
            let context_folder_id = folder.id.clone();
            let defaults_folder_id = folder.id.clone();
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
                let rename_target = folder_target.clone();
                let move_target = folder_target.clone();
                let delete_target = folder_target.clone();
                let menu_target = folder_target.clone();
                let context_menu_target = folder_target.clone();
                let folder_menu_open = sidebar_menu.as_ref() == Some(&folder_target);
                let confirming_delete = pending_sidebar_delete.as_ref() == Some(&folder_target);
                let moving_folder = sidebar_move.as_ref() == Some(&folder_target);
                let editing_context =
                    workspace_context_folder.as_deref() == Some(folder.id.as_str());
                let editing_defaults = workspace_defaults
                    .as_ref()
                    .is_some_and(|defaults| defaults.folder_id == folder.id);
                tree_rows.push(
                    div()
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
                        .child(
                            div()
                                .id(("collapse-folder", folder_row_index))
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .cursor_pointer()
                                .hover(|style| style.text_color(rgb(0xb9c7ff)))
                                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                    if event.is_right_click() {
                                        this.toggle_sidebar_menu(context_menu_target.clone(), cx);
                                    } else {
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
                        .child(
                            div()
                                .id(("folder-menu", folder_row_index))
                                .w(px(24.0))
                                .h(px(24.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(if folder_menu_open { TEXT } else { MUTED }))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_sidebar_menu(menu_target.clone(), cx);
                                }))
                                .child("···"),
                        )
                        .into_any_element(),
                );
                if folder_menu_open {
                    tree_rows.push(
                        div()
                            .ml(px(indent + 10.0))
                            .mr_2()
                            .mb_1()
                            .p_1()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .rounded_md()
                            .bg(rgb(SURFACE_HIGH))
                            .child(
                                div()
                                    .id(("rename-folder", folder_row_index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(BG)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.begin_sidebar_edit(
                                            rename_target.clone(),
                                            folder_name.clone(),
                                            cx,
                                        );
                                        let focus =
                                            this.sidebar_edit_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }))
                                    .child("Edit"),
                            )
                            .child(
                                div()
                                    .id(("context-folder", folder_row_index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(if editing_context { TEXT } else { MUTED }))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(BG)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.sidebar_menu = None;
                                        this.begin_workspace_context(context_folder_id.clone(), cx);
                                        let focus =
                                            this.workspace_context_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }))
                                    .child("Context"),
                            )
                            .child(
                                div()
                                    .id(("defaults-folder", folder_row_index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(if editing_defaults { TEXT } else { MUTED }))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(BG)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.sidebar_menu = None;
                                        this.begin_workspace_defaults(
                                            defaults_folder_id.clone(),
                                            cx,
                                        );
                                    }))
                                    .child("Agent"),
                            )
                            .child(
                                div()
                                    .id(("move-folder", folder_row_index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(if moving_folder { TEXT } else { MUTED }))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(BG)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_sidebar_move(move_target.clone(), cx);
                                    }))
                                    .child("Move"),
                            )
                            .child(
                                div()
                                    .id(("trash-folder", folder_row_index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(if confirming_delete {
                                        0xefaaaa
                                    } else {
                                        MUTED
                                    }))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x3b282e)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.delete_sidebar_item(delete_target.clone(), cx);
                                    }))
                                    .child(if confirming_delete {
                                        if sidebar_delete_submitting {
                                            "…"
                                        } else {
                                            "Confirm"
                                        }
                                    } else {
                                        "Trash"
                                    }),
                            )
                            .into_any_element(),
                    );
                }
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
                                        .id(("workspace-workdir-input", folder_row_index))
                                        .track_focus(&workspace_workdir_focus)
                                        .h(px(32.0))
                                        .w_full()
                                        .min_w_0()
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
                                .child("Repository")
                                .child(
                                    div()
                                        .id(("workspace-repo-default-input", folder_row_index))
                                        .track_focus(&workspace_repo_default_focus)
                                        .h(px(32.0))
                                        .w_full()
                                        .min_w_0()
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(
                                            if workspace_repo_default_focus.is_focused(window) {
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
                                            this.workspace_defaults = None;
                                            cx.notify();
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
                let rename_target = chat_target.clone();
                let move_target = chat_target.clone();
                let delete_target = chat_target.clone();
                let menu_target = chat_target.clone();
                let context_menu_target = chat_target.clone();
                let chat_menu_open = sidebar_menu.as_ref() == Some(&chat_target);
                let confirming_delete = pending_sidebar_delete.as_ref() == Some(&chat_target);
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
                        .child(
                            div()
                                .id(("select-chat", row_id))
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                    if event.is_right_click() {
                                        this.toggle_sidebar_menu(context_menu_target.clone(), cx);
                                    } else {
                                        this.select_chat(chat_id.clone(), cx);
                                    }
                                }))
                                .child(if chat.working || unread {
                                    format!("●  {title}")
                                } else {
                                    format!("   {title}")
                                }),
                        )
                        .child(
                            div()
                                .id(("chat-menu", row_id))
                                .w(px(24.0))
                                .h(px(24.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(if chat_menu_open { TEXT } else { MUTED }))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(BG)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_sidebar_menu(menu_target.clone(), cx);
                                }))
                                .child("···"),
                        )
                        .into_any_element(),
                );
                if chat_menu_open {
                    tree_rows.push(
                        div()
                            .ml(px(indent + 22.0))
                            .mr_2()
                            .mb_1()
                            .p_1()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .rounded_md()
                            .bg(rgb(SURFACE_HIGH))
                            .child(
                                div()
                                    .id(("rename-chat", row_id))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(BG)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.begin_sidebar_edit(
                                            rename_target.clone(),
                                            title.clone(),
                                            cx,
                                        );
                                        let focus =
                                            this.sidebar_edit_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }))
                                    .child("Edit"),
                            )
                            .child(
                                div()
                                    .id(("move-chat", row_id))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(if moving_chat { TEXT } else { MUTED }))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(BG)).text_color(rgb(TEXT)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_sidebar_move(move_target.clone(), cx);
                                    }))
                                    .child("Move"),
                            )
                            .child(
                                div()
                                    .id(("delete-chat", row_id))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(if confirming_delete {
                                        0xefaaaa
                                    } else {
                                        MUTED
                                    }))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x3b282e)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.delete_sidebar_item(delete_target.clone(), cx);
                                    }))
                                    .child(if confirming_delete {
                                        if sidebar_delete_submitting {
                                            "…"
                                        } else {
                                            "Confirm"
                                        }
                                    } else {
                                        "Delete"
                                    }),
                            )
                            .into_any_element(),
                    );
                }
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
            .w(px(272.0))
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
                    .child("LOCAL")
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
                                .id("workspace-repo-input")
                                .track_focus(&workspace_repo_focus)
                                .h(px(32.0))
                                .w_full()
                                .min_w_0()
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if workspace_repo_focus.is_focused(window) {
                                    accent
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(BG))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus = this.workspace_repo_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(workspace_repo_input),
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
                format!("{} · {state}", chat.backend)
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
                            .child(div().text_xs().text_color(rgb(MUTED)).child(context)),
                    )
                    .child(
                        div()
                            .id("toggle-diff")
                            .ml_2()
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
                            .ml_2()
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
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_open_diff {
                                    this.toggle_terminal_panel(cx);
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
            );

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
                    .id("assistant-cycle")
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
                    .on_click(cx.listener(move |this, _, _, _| {
                        if can_change_agent {
                            this.cycle_model();
                        }
                    }))
                    .child(model_label),
            )
            .child(
                div()
                    .id("effort-cycle")
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
                    .on_click(cx.listener(move |this, _, _, _| {
                        if can_change_agent {
                            this.cycle_effort();
                        }
                    }))
                    .child(format!("Effort: {effort_label}")),
            )
            .child(
                div()
                    .id("access-cycle")
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
                    .on_click(cx.listener(move |this, _, _, _| {
                        if can_change_agent {
                            this.cycle_access();
                        }
                    }))
                    .child(access_label),
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
                    .id("workspace-cycle")
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
                    .on_click(cx.listener(move |this, _, _, _| {
                        if can_cycle_workspace {
                            this.cycle_workspace();
                        }
                    }))
                    .child(format!("Workspace: {workspace_label}")),
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
        let desktop = cx.entity();
        let transcript = list(self.transcript.clone(), move |index, _window, _cx| {
            let message = &messages[index];
            let key = message
                .id
                .map(|id| format!("message-{id}"))
                .unwrap_or_else(|| format!("live-{index}"));
            Self::message_row(
                message,
                index,
                expanded_activity.contains(&key),
                desktop.clone(),
            )
        })
        .size_full();

        let composer_focus = self.composer_input.read(cx).focus_handle(cx);
        let attachment_count = self.model.draft_attachments.len();
        let can_attach = attachment_count < MAX_ATTACHMENTS
            && self.model.selected_chat.is_some()
            && self.model.connected;
        let can_send = (!self.composer.trim().is_empty() || attachment_count > 0)
            && self.model.selected_chat.is_some()
            && self.model.connected
            && !self.sending;
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
        let queue_edit = self.queue_edit.clone();
        let queue_edit_input = self.queue_edit_input.clone();
        let queue_edit_focus = self.queue_edit_input.read(cx).focus_handle(cx);
        let selected_chat_id = self.model.selected_chat.clone().unwrap_or_default();
        let queue_rows = self
            .model
            .queue
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, prompt)| {
                let editing = queue_edit.as_ref().is_some_and(|edit| {
                    edit.chat_id == selected_chat_id
                        && edit.index == index
                        && edit.original == prompt
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
                                .h(px(36.0))
                                .px_2()
                                .flex()
                                .items_center()
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
                let steer_prompt = prompt.clone();
                let edit_prompt = prompt.clone();
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
                                        this.begin_queue_edit(index, edit_prompt.clone(), cx);
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
                                        this.steer_queued(index, &steer_prompt);
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
                            .child(prompt),
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

        let composer = div()
            .flex_shrink_0()
            .px_3()
            .pt_2()
            .pb_3()
            .bg(rgb(BG))
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
                        .child(error),
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
            .child(
                div()
                    .id("composer")
                    .track_focus(&composer_focus)
                    .w_full()
                    .max_w(px(1040.0))
                    .mx_auto()
                    .min_h(px(64.0))
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
                    .child(self.composer_input.clone())
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
                    .child(composer_controls),
            );

        let git_commit_input = self.git_commit_input.clone();
        let git_commit_focus = self.git_commit_input.read(cx).focus_handle(cx);
        let repo_file_filter_input = self.repo_file_filter_input.clone();
        let repo_file_filter_focus = self.repo_file_filter_input.read(cx).focus_handle(cx);
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
            let can_push = !action_running
                && status.is_some_and(|status| {
                    !status.branch.is_empty()
                        && status.branch != "(detached)"
                        && status.branch != "(initial)"
                });
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
            let mut file_sections = Vec::new();
            for (index, file) in diff.files.iter().enumerate() {
                let collapsed = self.collapsed_diff_files.contains(&file.path);
                let path = file.path.clone();
                let mut lines = Vec::new();
                if !collapsed {
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
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x78c995))
                                        .child(format!("+{}", file.additions)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0xe88e9c))
                                        .child(format!("−{}", file.deletions)),
                                ),
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
                                        .id("back-to-repository-files")
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
                                    .child("Large file preview truncated for responsive rendering."),
                            )
                        })
                        .child(
                            div()
                                .id("repository-file-preview")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .p_3()
                                .child(Self::markdown_content(document)),
                        )
                        .into_any_element()
                } else {
                    let filter = self.repo_file_filter.trim().to_ascii_lowercase();
                    let matching = diff
                        .repo_files
                        .iter()
                        .filter(|path| filter.is_empty() || path.to_ascii_lowercase().contains(&filter))
                        .count();
                    let mut rows = Vec::new();
                    for (index, path) in diff
                        .repo_files
                        .iter()
                        .filter(|path| filter.is_empty() || path.to_ascii_lowercase().contains(&filter))
                        .take(400)
                        .cloned()
                        .enumerate()
                    {
                        let selected_path = path.clone();
                        let desktop = desktop_entity.clone();
                        rows.push(
                            div()
                                .id(("repository-file", index))
                                .w_full()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(move |_, _, cx| {
                                    desktop.update(cx, |this, cx| {
                                        this.read_repository_file(selected_path.clone(), cx);
                                    });
                                })
                                .child(path)
                                .into_any_element(),
                        );
                    }
                    let list_message = diff.error.clone().unwrap_or_else(|| {
                        if loading {
                            "Loading repository files…".into()
                        } else if self.repo_file_filter.trim().is_empty() {
                            "No repository files".into()
                        } else {
                            "No matching files".into()
                        }
                    });
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
                                .child(
                                    div()
                                        .id("repository-file-filter")
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
                        .when(diff.repo_files_truncated || matching > 400, |body| {
                            body.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .text_xs()
                                    .text_color(rgb(0xe0c178))
                                    .child(if diff.repo_files_truncated {
                                        "Showing a bounded repository file set. Filter to narrow it."
                                    } else {
                                        "Showing the first 400 matches. Filter to narrow it."
                                    }),
                            )
                        })
                        .child(
                            div()
                                .id("repository-files")
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
                .w(px(460.0))
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
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(TEXT))
                                        .child("Repository changes"),
                                )
                                .child(div().text_xs().text_color(rgb(MUTED)).child(
                                    if files_mode {
                                        format!("{} repository files", diff.repo_files.len())
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
                .child(
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
                                footer
                                    .child(div().text_xs().text_color(rgb(0xe0c178)).child(warning))
                            },
                        )
                        .when_some(diff.action_error.clone(), |footer, error| {
                            footer.child(div().text_xs().text_color(rgb(0xf0a8b3)).child(error))
                        })
                        .when_some(diff.action.clone(), |footer, action| {
                            footer.child(div().text_xs().text_color(rgb(0xaec4ff)).child(action))
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
                                        .border_color(rgb(if git_commit_focus.is_focused(window) {
                                            accent
                                        } else {
                                            BORDER
                                        }))
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
                                        .id("git-commit-all")
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(rgb(if can_commit { accent } else { SURFACE_HIGH }))
                                        .text_xs()
                                        .text_color(rgb(if can_commit { 0xffffff } else { MUTED }))
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
                        ),
                )
                .into_any_element()
        });

        let terminal_input = self.terminal_input.clone();
        let terminal_focus = self.terminal_input.read(cx).focus_handle(cx);
        let terminal_pane = self.terminal_panel.as_ref().map(|panel| {
            let output = panel.screen.text();
            let active = panel.terminal_id.is_some() && !panel.closed && !panel.loading;
            div()
                .w_full()
                .h(px(320.0))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(rgb(BORDER))
                .bg(rgb(BG))
                .child(
                    div()
                        .h(px(40.0))
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
                                .text_sm()
                                .text_color(rgb(TEXT))
                                .child(panel.title.clone()),
                        )
                        .when(panel.closed, |header| {
                            header.child(
                                div()
                                    .id("restart-terminal")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(accent))
                                    .text_xs()
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.restart_terminal(cx)),
                                    )
                                    .child("Restart"),
                            )
                        })
                        .when(active, |header| {
                            header.child(
                                div()
                                    .id("kill-terminal")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(0xf1b3ba))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x3c292d)))
                                    .on_click(cx.listener(|this, _, _, cx| this.kill_terminal(cx)))
                                    .child("Kill"),
                            )
                        })
                        .child(
                            div()
                                .id("close-terminal")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.toggle_terminal_panel(cx)),
                                )
                                .child("×"),
                        ),
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
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_3()
                        .font_family("monospace")
                        .text_size(px(13.0))
                        .line_height(px(19.0))
                        .text_color(rgb(0xd8dee9))
                        .child(output),
                )
                .child(
                    div()
                        .id("terminal-input")
                        .track_focus(&terminal_focus)
                        .h(px(40.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .border_t_1()
                        .border_color(rgb(if terminal_focus.is_focused(window) {
                            accent
                        } else {
                            BORDER
                        }))
                        .bg(rgb(BG))
                        .when(active, |input| {
                            input
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus = this.terminal_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                        })
                        .child(terminal_input.clone()),
                )
                .into_any_element()
        });

        let settings_overlay = self.settings_open.then(|| {
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
                        .w(px(440.0))
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
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(div().text_xs().text_color(rgb(MUTED)).child("ACCENT COLOR"))
                                .child(div().flex().flex_wrap().gap_2().children(accent_buttons)),
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
                                    div()
                                        .min_w_0()
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
                                    div()
                                        .min_w_0()
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

        let content = div()
            .flex_1()
            .min_h_0()
            .flex()
            .relative()
            .bg(rgb(BG))
            .child(sidebar)
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
                    .when_some(terminal_pane, |column, pane| column.child(pane)),
            )
            .when_some(diff_pane, |root, pane| root.child(pane))
            .when_some(settings_overlay, |root, overlay| root.child(overlay))
            .when_some(search_overlay, |root, overlay| root.child(overlay));

        let titlebar = div()
            .h(px(34.0))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR))
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                window.start_window_move();
            })
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .px_3()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(MUTED))
                    .child("xd-dev"),
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
                if this.settings_open {
                    this.settings_open = false;
                    cx.notify();
                } else {
                    this.close_search(cx);
                }
            }))
            .bg(rgb(BG))
            .font_family("Inter")
            .when(client_decorations, |root| root.child(titlebar))
            .child(content)
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
        return Err("Git returned a diff that xd-dev could not parse.".into());
    }
    Ok((files, truncated))
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
        .args(["--app-name=xd-dev", "--", "xd-dev", &body])
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn optional_workspace_repository_ignores_only_blank_input() {
        assert_eq!(optional_trimmed("  /tmp/repo  "), Some("/tmp/repo"));
        assert_eq!(optional_trimmed(" \n\t "), None);
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
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("ctrl-k", OpenSearch, Some("XdDesktop")),
            KeyBinding::new("ctrl-f", OpenSearch, Some("XdDesktop")),
            KeyBinding::new("cmd-k", OpenSearch, Some("XdDesktop")),
            KeyBinding::new("cmd-f", OpenSearch, Some("XdDesktop")),
            KeyBinding::new("escape", CloseSearch, Some("XdDesktop")),
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
        let bounds = Bounds::centered(None, size(px(1180.0), px(780.0)), cx);
        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                is_resizable: true,
                window_min_size: Some(size(px(760.0), px(560.0))),
                window_background: WindowBackgroundAppearance::Opaque,
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("xd-dev".into()),
                ..Default::default()
            },
            |_, cx| cx.new(XdDesktop::new),
        )
        .expect("open xd GPUI window");
        cx.activate(true);
    });
}
