#![deny(dead_code, unused_imports)]
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

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
    App, Application, AssetSource, Bounds, ClipboardItem, Context, CursorStyle, Decorations,
    Entity, FocusHandle, Focusable, FontWeight, HighlightStyle, KeyBinding, ListAlignment,
    ListOffset, ListState, MouseButton, MouseDownEvent, Pixels, Point, Render, ResizeEdge,
    ScrollHandle, ScrollWheelEvent, SharedString, StyledText, TextRun, Timer, TitlebarOptions,
    WeakFocusHandle, Window, WindowBackgroundAppearance, WindowBounds, WindowControlArea,
    WindowDecorations, WindowOptions, canvas, div, list, point, prelude::*, px, rgb, rgba, size,
    svg,
};
use serde::Deserialize;
use serde_json::Value;
use xd_desktop::{
    activity::{self, ActivityCard, ActivityKind, SubagentState, SubagentView},
    host::{HostHandle, HostUpdate, MessageCursor, NewSessionWorktree, RequestKind, StartedHost},
    markdown,
    model::{AppModel, Attachment, ChatSummary, Message, MessagePageDirection, ToolView, Worktree},
    paired::{self, RemoteBridge, RemoteError, RemoteSession},
    session_host::{AgentCommand, SessionHost, SshCommand, TMUX_CONFIGURATION},
    session_runtime::{
        SessionEndpoint, SessionEvent, SessionEventKind, SessionEventReceiver, SessionRuntime,
    },
    theme::ThemeColors,
};

mod browser;
mod editor;
mod files;
mod input;
mod minimal;
mod selection;
mod settings;
mod source_build;
mod speech;
mod terminal;
#[cfg(any(target_os = "windows", test))]
mod windows;

use browser::{BrowserEvent, BrowserPane, BrowserSession};
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
    AgentCli, MinimalRoute, SessionCard, project_cards, project_sessions, reconcile_route,
    resumable_session,
};
use selection::{TextSelection, selectable_in_document, selectable_links_in_document};
use settings::{AppSettings, BrowserState, ThemePreset};
use source_build::{SourceBuildEvent, SourceBuildRun, SourceTarget};
use speech::SpeechOutput;
use terminal::TerminalScreen;

pub(crate) const UI_FONT: &str = "Inter";
/// Static instances: GPUI renders only the default instance of a variable
/// font, so each weight the shell uses ships as its own face.
const EMBEDDED_UI_FONTS: [&[u8]; 5] = [
    include_bytes!("../../data/fonts/Inter-Regular.ttf"),
    include_bytes!("../../data/fonts/Inter-Medium.ttf"),
    include_bytes!("../../data/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../../data/fonts/Inter-Bold.ttf"),
    include_bytes!("../../data/fonts/Inter-Italic.ttf"),
];
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
const CHEVRON_RIGHT_ICON: &str = "icons/chevron-right.svg";
const CHEVRON_DOWN_ICON: &str = "icons/chevron-down.svg";
const CLOSE_ICON: &str = "icons/close.svg";
const TERMINAL_ICON: &str = "icons/terminal.svg";
const SEARCH_ICON: &str = "icons/search.svg";
const MINIMIZE_ICON: &str = "icons/minimize.svg";
const MAXIMIZE_ICON: &str = "icons/maximize.svg";
/// What a chat is called when nobody chose a name for it.
const DEFAULT_CHAT_TITLE: &str = "New Chat";
const MAX_ATTACHMENTS: usize = 4;
const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
const MAX_SOURCE_BUILD_OUTPUT_BYTES: usize = 8 * 1024;
const ACTION_ERROR_LIFETIME: Duration = Duration::from_secs(8);
const TERMINAL_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
/// Width of the catalog sidebar that holds project drawers and session cards.
const CATALOG_WIDTH: f32 = 288.0;
/// Every session row shares one height and baseline.
const CARD_HEIGHT: f32 = 30.0;
const BROWSER_DIVIDER_WIDTH: f32 = 1.0;
/// How often working dots, pulses, and running timers advance. They move a
/// few times a second instead of on every frame: a repeating animation asks
/// for a frame at display rate, and each frame re-renders the whole window.
const PULSE_INTERVAL: Duration = Duration::from_millis(400);

/// Steps of the shared pulse, advanced only while something is working.
static PULSE_FRAME: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn pulse_frame() -> usize {
    PULSE_FRAME.load(std::sync::atomic::Ordering::Relaxed)
}

/// Opacity of a pulsing dot on the current pulse step.
fn pulse_opacity() -> f32 {
    [0.35, 0.65, 1.0, 0.65][pulse_frame() % 4]
}

fn browser_pane_width(requested: u16, window_width: f32) -> f32 {
    // Keep the session usable even at the window's minimum size.
    f32::from(requested).clamp(
        280.0,
        (window_width - BROWSER_DIVIDER_WIDTH - 460.0).clamp(280.0, 1000.0),
    )
}

/// The docked pane beside a native chat keeps the chat at least 460px wide.
fn native_dock_width(requested: u16, available: f32) -> f32 {
    f32::from(requested).clamp(320.0, (available - 460.0).clamp(320.0, 900.0))
}

fn working_dot_alphas(frame: usize) -> [u8; 3] {
    let lit = frame % 4;
    std::array::from_fn(|index| if index < lit { 0xff } else { 0x4d })
}

/// Three dots that fill in one by one, in step with the shared pulse. The
/// size is inherited so the dots sit on the label's baseline.
fn working_dots(_instance: usize, color: u32) -> gpui::AnyElement {
    let highlights = working_dot_alphas(pulse_frame())
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
    div()
        .w(px(13.0))
        .flex_none()
        .child(StyledText::new("...").with_highlights(highlights))
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

fn settings_switch(colors: ThemeColors, enabled: bool) -> gpui::AnyElement {
    div()
        .w(px(38.0))
        .h(px(22.0))
        .flex_none()
        .rounded_full()
        .bg(rgb(if enabled {
            colors.accent
        } else {
            colors.surface_high
        }))
        .child(
            div()
                .mt(px(3.0))
                .ml(px(if enabled { 19.0 } else { 3.0 }))
                .size(px(16.0))
                .rounded_full()
                .bg(rgb(if enabled {
                    colors.accent_text
                } else {
                    colors.muted
                })),
        )
        .into_any_element()
}

/// What a session's signal tab says about its agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionSignal {
    Idle,
    Working,
    /// An approval or question is waiting on the user. The only amber state.
    Waiting,
    /// A finished turn the user has not opened yet.
    Done,
}

impl SessionSignal {
    fn of(working: bool, waiting: bool, unread: bool) -> Self {
        if waiting {
            Self::Waiting
        } else if working {
            Self::Working
        } else if unread {
            Self::Done
        } else {
            Self::Idle
        }
    }

    fn color(self, colors: ThemeColors) -> u32 {
        match self {
            Self::Idle => colors.faint,
            Self::Working => colors.working,
            Self::Waiting => colors.waiting,
            Self::Done => colors.done,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Working => "Working",
            Self::Waiting => "Waiting on you",
            Self::Done => "Done",
        }
    }
}

/// The small status mark at the end of a session row: a dot for work and
/// unread turns, and nothing for a quiet session so a list of them reads quiet.
/// Waiting on the user is the one state that also says so in words.
fn signal_tab(signal: SessionSignal, colors: ThemeColors) -> Option<gpui::AnyElement> {
    let color = signal.color(colors);
    let dot = |size: f32| {
        div()
            .size(px(size))
            .flex_none()
            .rounded_full()
            .bg(rgb(color))
    };
    match signal {
        SessionSignal::Idle => None,
        SessionSignal::Working => Some(dot(7.0).opacity(pulse_opacity()).into_any_element()),
        SessionSignal::Waiting => Some(pill("Waiting", color)),
        SessionSignal::Done => Some(dot(7.0).into_any_element()),
    }
}

/// The marks a collapsed project shows for its sessions that need a look.
fn drawer_tabs(signals: &[SessionSignal], colors: ThemeColors) -> gpui::AnyElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.0))
        .children(
            signals
                .iter()
                .filter(|signal| **signal != SessionSignal::Idle)
                .take(5)
                .map(|signal| {
                    div()
                        .size(px(6.0))
                        .rounded_full()
                        .bg(rgb(signal.color(colors)))
                }),
        )
        .into_any_element()
}

/// What an approval asks for, in the words of the tool asking.
fn approval_title(method: &str, tool: &str) -> String {
    match tool {
        "Bash" => "Run this command?".into(),
        "Edit" | "MultiEdit" | "NotebookEdit" => "Apply this edit?".into(),
        "Write" => "Write this file?".into(),
        "WebFetch" => "Fetch this page?".into(),
        "WebSearch" => "Search the web?".into(),
        "ExitPlanMode" => "Approve the plan?".into(),
        "" if method == "item/fileChange/requestApproval" => "Apply these changes?".into(),
        "" if method == "item/commandExecution/requestApproval" => "Run this command?".into(),
        "" => "Approval needed".into(),
        tool => format!("Allow {tool}?"),
    }
}

/// The first non-empty line of some markdown as plain text, for a one-line
/// preview: no heading or list markers, emphasis, or code ticks.
fn first_line(text: &str) -> String {
    text.lines()
        .map(|line| line.trim().trim_start_matches(['#', '-', '*', '>', ' ']))
        .find(|line| !line.is_empty() && !line.starts_with("```"))
        .unwrap_or_default()
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
}

/// A small status label on a soft tint of its own color.
fn stamp(label: impl Into<SharedString>, color: u32) -> gpui::AnyElement {
    pill(label, color)
}

fn pill(label: impl Into<SharedString>, color: u32) -> gpui::AnyElement {
    div()
        .flex_none()
        .px(px(7.0))
        .h(px(20.0))
        .flex()
        .items_center()
        .rounded_full()
        .bg(rgba((color << 8) | 0x24))
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(color))
        .child(label.into())
        .into_any_element()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// The one button vocabulary used across the shell, popovers, and dialogs.
fn ui_button(
    id: impl Into<gpui::ElementId>,
    kind: ButtonKind,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let base = div()
        .id(id)
        .h(px(30.0))
        .px_3()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .rounded(px(8.0))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer();
    match kind {
        ButtonKind::Primary => base
            .bg(rgb(colors.accent))
            .text_color(rgb(colors.accent_text))
            .hover(|style| style.bg(rgb(colors.accent_hover))),
        ButtonKind::Secondary => base
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .text_color(rgb(colors.text))
            .hover(|style| style.bg(rgb(colors.surface_high))),
        ButtonKind::Ghost => base.text_color(rgb(colors.muted)).hover(|style| {
            style
                .bg(rgb(colors.surface_high))
                .text_color(rgb(colors.text))
        }),
        ButtonKind::Danger => base
            .border_1()
            .border_color(rgb(colors.danger))
            .text_color(rgb(colors.danger))
            .hover(|style| style.bg(rgb(colors.danger_surface))),
    }
}

/// A drawn stroke icon from the embedded set, tinted to the surrounding text.
fn icon(path: &'static str, size: f32, color: u32) -> gpui::AnyElement {
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
        .into_any_element()
}

fn agent_icon(agent: AgentCli) -> &'static str {
    match agent {
        AgentCli::Codex => CODEX_ICON,
        AgentCli::Claude => CLAUDE_ICON,
        AgentCli::Jcode => JCODE_ICON,
        AgentCli::Copilot => COPILOT_ICON,
    }
}

gpui::actions!(xd, [CloseSearch, CopyRenderedSelection]);

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = xd, no_json)]
struct OpenBrowserUrl {
    url: String,
    window: gpui::AnyWindowHandle,
}

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = xd, no_json)]
struct ToggleActivity {
    id: i64,
}

/// Scrolls to an agent's card and opens it.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = xd, no_json)]
struct RevealSubagent {
    row: usize,
    card_id: i64,
}

/// Stops a running agent or sends it to the background, by its record key.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = xd, no_json)]
struct ControlSubagent {
    key: String,
    action: String,
}

/// Shows or hides the earlier steps of a work log, keyed by its head row.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = xd, no_json)]
struct ToggleWorkLog {
    id: i64,
    rows: std::ops::Range<usize>,
}

#[derive(Clone, Debug, Deserialize)]
struct AuthProvider {
    provider: String,
    state: String,
    #[serde(default)]
    needs_input: bool,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    login_url: Option<String>,
    #[serde(default)]
    device_code: Option<String>,
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
    #[serde(default)]
    version: String,
    #[serde(default)]
    latest: Option<String>,
    #[serde(default)]
    available: bool,
    #[serde(default)]
    supported: bool,
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
    patch: String,
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum NativePane {
    #[default]
    Chat,
    Files,
    Diff,
    Terminal,
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
    workdir: Option<String>,
    agent: Option<AgentCli>,
    allow_agent_tabs: bool,
    sessions: Vec<TerminalTab>,
    selected: Option<String>,
    viewport: Option<(usize, usize)>,
    auto_open: bool,
    opening: bool,
    opening_agent: Option<AgentCli>,
    opening_generation: u64,
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

    fn begin_opening(&mut self, agent: Option<AgentCli>) -> u64 {
        self.opening_generation = self.opening_generation.wrapping_add(1);
        self.opening = true;
        self.opening_agent = agent;
        self.opening_generation
    }

    fn expire_opening(&mut self, generation: u64) -> bool {
        if !self.opening || self.opening_generation != generation {
            return false;
        }
        self.finish_opening();
        true
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

fn terminal_runtime_event(event: SessionEvent) -> (ChatEndpoint, &'static str, Value) {
    let endpoint = event.endpoint().into();
    let (name, body) = match event.kind {
        SessionEventKind::Opened {
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
        SessionEventKind::Output {
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
        SessionEventKind::Resized {
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
        SessionEventKind::Activity {
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
        SessionEventKind::Closed {
            chat_id,
            terminal_id,
        } => (
            "terminal-closed",
            serde_json::json!({"chat": chat_id, "terminal": terminal_id}),
        ),
    };
    (endpoint, name, body)
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

fn global_terminal_panel_id(connection_key: &str) -> String {
    format!("global:{connection_key}")
}

fn update_terminal_activity_state(
    activity: &mut HashMap<(ChatEndpoint, String, String), bool>,
    endpoint: ChatEndpoint,
    chat_id: &str,
    terminal_id: &str,
    working: Option<bool>,
) -> bool {
    let key = (endpoint, chat_id.to_owned(), terminal_id.to_owned());
    if let Some(working) = working {
        activity.insert(key, working);
    } else {
        activity.remove(&key);
    }
    activity
        .iter()
        .any(|((candidate_endpoint, candidate_chat, _), working)| {
            *candidate_endpoint == endpoint && candidate_chat == chat_id && *working
        })
}

fn terminal_panel_cache_entry_is_live(chat_id: &str, live_chats: &HashSet<String>) -> bool {
    chat_id.starts_with("global:") || live_chats.contains(chat_id)
}

struct PendingSpeech {
    chat_id: String,
    previous_assistant_id: Option<i64>,
}

/// Plain tool calls in a row read as one work log, showing its newest steps.
const WORK_LOG_VISIBLE: usize = 6;

/// Where a row sits inside a run of consecutive plain activity, so the run reads
/// as one card instead of one card per command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorkRun {
    start: usize,
    len: usize,
    position: usize,
}

impl WorkRun {
    /// Rows the collapsed log leaves out: everything but the newest steps.
    /// The head row stays to carry the log's header.
    fn hides(self, expanded: bool) -> bool {
        !expanded
            && self.len > WORK_LOG_VISIBLE
            && self.position > 0
            && self.position < self.len - WORK_LOG_VISIBLE
    }

    fn head_shows_entry(self, expanded: bool) -> bool {
        expanded || self.len <= WORK_LOG_VISIBLE
    }

    fn hidden_count(self, expanded: bool) -> usize {
        if expanded {
            0
        } else {
            self.len.saturating_sub(WORK_LOG_VISIBLE)
        }
    }
}

/// Live replies up to this size are formatted on every flush.
const LIVE_MARKDOWN_BYTES: usize = 48 * 1024;

/// A delegated agent still at work, and where its card is.
#[derive(Clone)]
struct RunningAgent {
    row: usize,
    card_id: i64,
    view: SubagentView,
}

#[derive(Clone, Default)]
struct TranscriptSnapshot {
    messages: Arc<Vec<Message>>,
    live_text: Option<Arc<Message>>,
    live_items: Arc<Vec<Message>>,
    /// Agents still running, kept with the rows so a frame need not parse
    /// every record to find them.
    running_agents: Arc<Vec<RunningAgent>>,
}

impl TranscriptSnapshot {
    fn sync_messages(&mut self, model: &AppModel) {
        self.messages = Arc::new(model.messages.clone());
        self.sync_running_agents();
    }

    /// Rows the list draws. The list's item count must always equal this,
    /// so every change to the count is computed from the snapshot, never from
    /// the model, which can run ahead of it (live text waits for its frame).
    fn len(&self) -> usize {
        self.messages.len() + self.live_items.len() + usize::from(self.live_text.is_some())
    }

    /// Replaces a record's row with its newest snapshot, found by message id.
    /// The row count never changes.
    fn replace_record(&mut self, id: i64, text: &str) -> Option<usize> {
        let message = Message::new(Some(id), "tool", text, None);
        let row = if let Some(index) = self.messages.iter().position(|row| row.id == Some(id)) {
            Arc::make_mut(&mut self.messages)[index] = message;
            index
        } else {
            let index = self.live_items.iter().position(|row| row.id == Some(id))?;
            Arc::make_mut(&mut self.live_items)[index] = message;
            self.messages.len() + index
        };
        self.sync_running_agents();
        Some(row)
    }

    fn sync_running_agents(&mut self) {
        let rows = self.messages.iter().chain(self.live_items.iter());
        self.running_agents = Arc::new(
            rows.enumerate()
                .filter(|(_, message)| message.role == "tool")
                .filter_map(|(row, message)| match message.tool_view() {
                    Some(ToolView::Subagent(view)) if view.status == SubagentState::Running => {
                        Some(RunningAgent {
                            row,
                            card_id: message.id.unwrap_or(-(row as i64) - 1),
                            view: view.clone(),
                        })
                    }
                    _ => None,
                })
                .collect(),
        );
    }

    fn sync_live_text(&mut self, model: &AppModel) {
        // Formatted while it streams, so closing the text does not reflow it.
        // A very long reply streams as plain text to keep each flush cheap.
        self.live_text = (!model.live_text.is_empty()).then(|| {
            let label = model.live_label();
            Arc::new(if model.live_text.len() <= LIVE_MARKDOWN_BYTES {
                Message::new_streaming(None, "assistant", model.live_text.clone(), label)
            } else {
                Message::new_plain(None, "assistant", model.live_text.clone(), label)
            })
        });
    }

    fn sync_live_items(&mut self, model: &AppModel) {
        self.live_items = Arc::new(model.live_items.clone());
        self.sync_running_agents();
    }

    fn stacks_activity(&self, index: usize) -> bool {
        self.get(index).is_some_and(|message| {
            message.role == "tool" && activity::is_plain_activity(&message.content)
        })
    }

    /// The run of plain activity a row belongs to, when there is more than one.
    fn work_run(&self, index: usize) -> Option<WorkRun> {
        if !self.stacks_activity(index) {
            return None;
        }
        let mut start = index;
        while start > 0 && self.stacks_activity(start - 1) {
            start -= 1;
        }
        let mut end = index + 1;
        while self.stacks_activity(end) {
            end += 1;
        }
        (end - start > 1).then_some(WorkRun {
            start,
            len: end - start,
            position: index - start,
        })
    }

    fn row_key(&self, index: usize) -> i64 {
        self.get(index)
            .and_then(|message| message.id)
            .unwrap_or(-(index as i64) - 1)
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionDragPayload {
    chat_id: String,
    project_id: String,
    source_index: usize,
    title: String,
}

struct SessionDragPreview {
    title: String,
    position: Point<Pixels>,
    colors: ThemeColors,
}

impl Render for SessionDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x - px(110.0))
            .pt(self.position.y - px(28.0))
            .child(
                div()
                    .w(px(220.0))
                    .min_h(px(56.0))
                    .px_3()
                    .py_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(self.colors.accent))
                    .bg(rgb(self.colors.surface_high))
                    .shadow_md()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(self.colors.text))
                    .child(self.title.clone()),
            )
    }
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
    fingerprint: String,
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

impl From<ChatEndpoint> for SessionEndpoint {
    fn from(endpoint: ChatEndpoint) -> Self {
        match endpoint {
            ChatEndpoint::Local => Self::Local,
            ChatEndpoint::Remote => Self::Remote,
        }
    }
}

impl From<SessionEndpoint> for ChatEndpoint {
    fn from(endpoint: SessionEndpoint) -> Self {
        match endpoint {
            SessionEndpoint::Local => Self::Local,
            SessionEndpoint::Remote => Self::Remote,
        }
    }
}

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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct NewSessionBranch {
    name: String,
    reference: String,
    oid: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct NewSessionPullRequest {
    number: u64,
    title: String,
    oid: String,
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
    native_pane: NativePane,
    /// Drag origin and starting width while resizing the native dock.
    dock_drag: Option<(f32, u16)>,
    /// Width of the chat's reading column, measured by the workspace layout.
    chat_column_width: f32,
    native_option_open: Option<&'static str>,
    expanded_activities: HashSet<i64>,
    expanded_work_logs: HashSet<i64>,
    /// Agents asked to stop or move to the background, until their record
    /// says they did.
    subagent_actions: HashMap<String, &'static str>,
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
    remote_bridge: Option<RemoteBridge>,
    remote_state: RemoteState,
    remote_error: Option<String>,
    remote_generation: u64,
    remote_reconnect_attempt: u32,
    transcript: ListState,
    transcript_scroll_handler_attached: bool,
    /// Rows the transcript last showed, so the agents tray can leave out
    /// agents whose cards are already in view.
    transcript_visible: Range<usize>,
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
    remote_pairing_input: Entity<ComposerInput>,
    share_host_input: Entity<ComposerInput>,
    question_input: Entity<ComposerInput>,
    secret_question_input: Entity<ComposerInput>,
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
    /// Drawers other than the current project that the user opened by hand.
    opened_drawers: HashSet<String>,
    /// Filters the catalog to projects and sessions whose names match.
    catalog_find_input: Entity<ComposerInput>,
    catalog_query: String,
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
    chat_create_branches: Vec<NewSessionBranch>,
    chat_create_pull_requests: Vec<NewSessionPullRequest>,
    chat_create_sources_loading: bool,
    chat_create_sources_error: Option<String>,
    workspace_context_folder: Option<String>,
    workspace_context_text: String,
    workspace_context_loading: bool,
    workspace_context_submitting: bool,
    workspace_defaults: Option<WorkspaceDefaults>,
    diff_panel: Option<DiffPanel>,
    terminal_panel: Option<TerminalPanel>,
    browser_panes: HashMap<String, Entity<BrowserPane>>,
    browser_drag: Option<(f32, u16)>,
    browser_restore_focus: bool,
    terminal_runtime: SessionRuntime,
    terminal_panel_cache: HashMap<(ChatEndpoint, String), TerminalPanel>,
    terminal_activity_by_tab: HashMap<(ChatEndpoint, String, String), bool>,
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
    interaction_answers: HashMap<String, String>,
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
        let browser_window = window.window_handle();
        let desktop = cx.weak_entity();
        let app: &mut App = cx;
        // Link clicks can arrive without a focused GPUI element, including
        // while the native browser owns focus. Route them by their window.
        app.on_action(move |action: &OpenBrowserUrl, cx| {
            if action.window != browser_window || desktop.upgrade().is_none() {
                cx.propagate();
                return;
            }
            let desktop = desktop.clone();
            let url = action.url.clone();
            // Action dispatch already borrows the window; update after it ends.
            cx.defer(move |cx| {
                let _ = browser_window.update(cx, |_, window, cx| {
                    let _ = desktop.update(cx, |desktop, cx| {
                        desktop.open_browser_url(&url, window, cx);
                    });
                });
            });
        });
        let desktop = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = desktop.update(cx, |desktop, cx| {
                // Rendered frames can retain the native child after the view
                // entity is released. Dispose it while its parent still exists.
                for pane in desktop.browser_panes.values() {
                    pane.update(cx, |pane, _| pane.shutdown());
                }
            });
            true
        });
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
        let catalog_find_input = cx.new(|cx| ComposerInput::new(cx, "Find a session or project"));
        cx.subscribe(&catalog_find_input, |this, _, event, cx| {
            if let ComposerEvent::Changed(text) = event {
                this.catalog_query = text.trim().to_lowercase();
                cx.notify();
            }
        })
        .detach();
        let workspace_create_input = cx.new(|cx| ComposerInput::new(cx, "Workspace name…"));
        cx.subscribe(&workspace_create_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.workspace_create_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_workspace_create(cx),
            ComposerEvent::Bytes(_) | ComposerEvent::PasteImage { .. } => {}
        })
        .detach();
        let workspace_repo_input = cx.new(|cx| {
            ComposerInput::new(
                cx,
                if cfg!(target_os = "windows") {
                    "Linux repository path, e.g. /home/you/project…"
                } else {
                    "Existing repository path (optional)…"
                },
            )
        });
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
        let workspace_workdir_input = cx.new(|cx| {
            ComposerInput::new(
                cx,
                if cfg!(target_os = "windows") {
                    "Linux working directory (inherit when empty)…"
                } else {
                    "Working directory (inherit when empty)…"
                },
            )
        });
        cx.subscribe(&workspace_workdir_input, |this, _, event, cx| {
            if let ComposerEvent::Changed(text) = event {
                this.workspace_workdir_changed(text.clone(), cx);
            }
        })
        .detach();
        let workspace_repo_default_input = cx.new(|cx| {
            ComposerInput::new(
                cx,
                if cfg!(target_os = "windows") {
                    "Linux repository path (inherit when empty)…"
                } else {
                    "Repository path (inherit when empty)…"
                },
            )
        });
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
        let share_host_input =
            cx.new(|cx| ComposerInput::new(cx, "Host address (LAN or Tailscale)…"));
        cx.subscribe(&share_host_input, |this, _, event, cx| {
            if let ComposerEvent::Changed(text) = event {
                if let Some(panel) = &mut this.share_panel {
                    panel.host = text.clone();
                }
                cx.notify();
            }
        })
        .detach();
        let remote_pairing_input =
            cx.new(|cx| ComposerInput::new(cx, "Paste pairing code from host…"));
        cx.subscribe(&remote_pairing_input, |this, _, event, cx| match event {
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
        let secret_question_input = cx.new(|cx| ComposerInput::password(cx, "Type your answer…"));
        cx.subscribe(&secret_question_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => {
                this.question_answer = text.clone();
                cx.notify();
            }
            ComposerEvent::Submit => this.send_question_input(cx),
            _ => {}
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
        // Window tests must not inherit settings written by another fixture.
        let mut settings = if cfg!(test) {
            AppSettings::default()
        } else {
            AppSettings::load()
        };
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
        let remote_error = None;
        let remote_key = settings.remote_credentials.as_ref().map(|credentials| {
            connection_state_key(
                ChatEndpoint::Remote,
                Some(&format!("{}:{}", credentials.host, credentials.port)),
            )
        });
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
        let settings_load_error = settings.load_error().map(str::to_owned);
        let remote_configured = settings.remote_ssh_command.is_some();
        let minimal_popup_focus = cx.focus_handle();
        let (terminal_runtime, terminal_updates) = SessionRuntime::new();
        let mut desktop = Self {
            model: AppModel {
                draft_revision: -1,
                connection_error: settings_load_error,
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
            native_pane: NativePane::Chat,
            dock_drag: None,
            chat_column_width: 780.0,
            native_option_open: None,
            expanded_activities: HashSet::new(),
            expanded_work_logs: HashSet::new(),
            subagent_actions: HashMap::new(),
            pending_minimal_session: None,
            settings,
            settings_open: false,
            auth_open: false,
            secrets_panel: None,
            devices_panel: None,
            share_panel: None,
            shortcut_panel: None,
            remote_panel: None,
            self_update_panel: Some(SelfUpdatePanel::default()),
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
            transcript_scroll_handler_attached: false,
            transcript_visible: 0..0,
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
            remote_pairing_input,
            share_host_input,
            question_input,
            secret_question_input,
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
            opened_drawers: HashSet::new(),
            catalog_find_input,
            catalog_query: String::new(),
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
            chat_create_branches: Vec::new(),
            chat_create_pull_requests: Vec::new(),
            chat_create_sources_loading: false,
            chat_create_sources_error: None,
            workspace_context_folder: None,
            workspace_context_text: String::new(),
            workspace_context_loading: false,
            workspace_context_submitting: false,
            workspace_defaults: None,
            diff_panel: None,
            terminal_panel: None,
            browser_panes: HashMap::new(),
            browser_drag: None,
            browser_restore_focus: false,
            terminal_runtime,
            terminal_panel_cache: HashMap::new(),
            terminal_activity_by_tab: HashMap::new(),
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
            interaction_answers: HashMap::new(),
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
        // View fixtures supply their own host state. Real connections belong
        // in integration tests and must not depend on installed user tools.
        if !cfg!(test) {
            desktop.schedule_connect(Duration::ZERO, cx);
            if desktop.active_endpoint == ChatEndpoint::Remote {
                desktop.schedule_remote_connect(Duration::ZERO, cx);
            }
        }
        desktop.listen_for_terminal_runtime(terminal_updates, cx);
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(PULSE_INTERVAL).await;
                let alive = this
                    .update(cx, |this, cx| {
                        if this.anything_working() {
                            PULSE_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

    fn apply_passive_reply(model: &mut AppModel, kind: &RequestKind, body: Value) -> bool {
        match kind {
            RequestKind::Tree => model.apply_tree(&body).is_ok(),
            RequestKind::AgentCatalog => model.apply_agent_catalog(&body).is_ok(),
            _ => false,
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
                | RequestKind::NewChatSources { .. }
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
                | "record"
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
                | "interaction"
                | "interaction-resolved"
                | "interactions-cleared"
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

    fn switch_active_endpoint(&mut self, endpoint: ChatEndpoint, cx: &mut Context<Self>) -> bool {
        if endpoint == self.active_endpoint {
            return true;
        }
        if !self.flush_draft_before_navigation() {
            return false;
        }
        self.stash_terminal_panel();
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
        true
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
            || self.settings.remote_credentials.is_none()
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
        let Some(credentials) = self.settings.remote_credentials.clone() else {
            self.remote_state = RemoteState::Unconfigured;
            return;
        };
        let connection = cx
            .background_executor()
            .spawn(async move { paired::resume(credentials) });
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
        self.settings.remote_credentials = Some(session.credentials.clone());
        if let Err(error) = self.settings.save() {
            self.remote_connection_failed(RemoteError::Authentication(error), cx);
            return;
        }
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
                        let applied =
                            Self::apply_passive_reply(&mut self.inactive_model, &kind, value);
                        if tree && applied {
                            self.prime_terminal_cache(ChatEndpoint::Remote);
                            self.prune_browser_panes(ChatEndpoint::Remote, cx);
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
                        if name == "terminal-activity" {
                            self.record_terminal_tab_activity(ChatEndpoint::Remote, &body);
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
        if error.fatal() {
            self.remote_state = RemoteState::Unconfigured;
            self.remote_error = Some(message);
        } else {
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

    fn activate_remote_runtime(&mut self, cx: &mut Context<Self>) {
        if self.settings.remote_credentials.is_none() {
            self.remote_error = Some("Pair with a host first.".into());
            cx.notify();
            return;
        }
        if self.active_endpoint == ChatEndpoint::Local {
            if !self.switch_active_endpoint(ChatEndpoint::Remote, cx) {
                cx.notify();
                return;
            }
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
        if !self.switch_active_endpoint(ChatEndpoint::Local, cx) {
            cx.notify();
            return;
        }
        self.remote_generation = self.remote_generation.saturating_add(1);
        self.remote_host = None;
        self.remote_bridge = None;
        self.remote_state = if self.settings.remote_credentials.is_some() {
            RemoteState::Offline
        } else {
            RemoteState::Unconfigured
        };
        self.remote_reconnect_attempt = 0;
        self.endpoint_model_mut(ChatEndpoint::Remote).connected = false;
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
                        let _ = host.host_update("status");
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
                    if let Some(panel) = &mut self.self_update_panel {
                        panel.busy = false;
                        panel.error =
                            Some("The local update service disconnected. Reconnecting…".into());
                    }
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
                            let applied =
                                Self::apply_passive_reply(&mut self.inactive_model, &kind, value);
                            if tree && applied {
                                self.prime_terminal_cache(ChatEndpoint::Local);
                                self.prune_browser_panes(ChatEndpoint::Local, cx);
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
                            if name == "terminal-activity" {
                                self.record_terminal_tab_activity(ChatEndpoint::Local, &body);
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
                if let Some(host) = &self.host {
                    let _ = host.host_update("status");
                }
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
                } => {
                    if self.creating_chat_folder.as_deref() == Some(folder_id)
                        && self.chat_create_title.trim() == title
                    {
                        self.chat_create_submitting = false;
                    }
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
                RequestKind::NewChatSources { folder_id }
                    if self.creating_chat_folder.as_deref() == Some(folder_id.as_str()) =>
                {
                    self.chat_create_sources_loading = false;
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
                    let removed = self
                        .model
                        .chats
                        .iter()
                        .filter(|chat| chat.folder.as_str() == folder_id.as_str())
                        .map(|chat| chat.id.clone())
                        .collect::<HashSet<_>>();
                    self.stop_terminal_sessions_for_chats(self.active_endpoint, &removed);
                    self.request_tree();
                }
                RequestKind::DeleteChat { chat_id } => {
                    if self.pending_sidebar_delete.as_ref()
                        == Some(&SidebarTarget::Chat(chat_id.clone()))
                    {
                        self.sidebar_delete_submitting = false;
                    }
                    self.stop_terminal_sessions_for_chats(
                        self.active_endpoint,
                        &HashSet::from([chat_id.clone()]),
                    );
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
                self.prune_browser_panes(self.active_endpoint, cx);
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
            RequestKind::HostUpdate { action } => {
                self.apply_self_update(&value);
                if action == "restart" && !cfg!(test) {
                    if let Some(path) = xd_desktop::channel::installed_restart_path() {
                        let mut command =
                            xd_desktop::channel::restart_command(&path, std::process::id());
                        match command.spawn() {
                            Ok(_) => cx.quit(),
                            Err(error) => {
                                if let Some(panel) = &mut self.self_update_panel {
                                    panel.error = Some(format!(
                                        "Cannot restart xd: {error}. Quit and reopen the app."
                                    ));
                                }
                            }
                        }
                    } else if let Some(panel) = &mut self.self_update_panel {
                        panel.error =
                            Some("Cannot locate the app launcher. Quit and reopen xd.".into());
                    }
                }
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
                            self.share_host_input
                                .update(cx, |input, cx| input.set_text(host.clone(), cx));
                            panel.host = host;
                            panel.fingerprint = value
                                .get("fingerprint")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned();
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
                    if let Some(diff) = &mut self.diff_panel {
                        diff.patch = output.clone();
                    }
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
            RequestKind::NewChatSources { folder_id } => {
                if self.creating_chat_folder.as_deref() == Some(folder_id.as_str()) {
                    self.chat_create_branches = value
                        .get("branches")
                        .cloned()
                        .and_then(|branches| serde_json::from_value(branches).ok())
                        .unwrap_or_default();
                    self.chat_create_pull_requests = value
                        .get("pull_requests")
                        .cloned()
                        .and_then(|pull_requests| serde_json::from_value(pull_requests).ok())
                        .unwrap_or_default();
                    self.chat_create_sources_error = value
                        .get("pull_request_error")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.chat_create_sources_loading = false;
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
                // The tree supplies session_mode. Opening from the creation
                // reply alone can route a native chat through terminal setup
                // and leave its transcript unrequested when the tree arrives.
                self.pending_minimal_session = Some((folder_id, chat_id.to_owned()));
                self.request_tree();
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
                // Rows the list holds for stored messages, which is what the
                // splices below shift.
                let old_message_count = self.transcript_snapshot.messages.len();
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
                        self.transcript.reset(self.transcript_snapshot.len());
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
        if !chat_id.starts_with("global:")
            && !self
                .endpoint_model(endpoint)
                .chats
                .iter()
                .any(|chat| chat.id == chat_id)
            && !self
                .terminal_panel
                .as_ref()
                .is_some_and(|panel| panel.chat_id == chat_id)
            && !self
                .terminal_panel_cache
                .contains_key(&(endpoint, chat_id.clone()))
        {
            return;
        }
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
        if let (Some(terminal_id), Some(working)) = (terminal_id, terminal_working) {
            self.apply_terminal_tab_activity(endpoint, &chat_id, terminal_id, Some(working));
        }
        if name == "terminal-closed"
            && let Some(terminal_id) = terminal_id
        {
            self.apply_terminal_tab_activity(endpoint, &chat_id, terminal_id, None);
        }
    }

    fn apply_terminal_tab_activity(
        &mut self,
        endpoint: ChatEndpoint,
        chat_id: &str,
        terminal_id: &str,
        working: Option<bool>,
    ) {
        let terminal_working = update_terminal_activity_state(
            &mut self.terminal_activity_by_tab,
            endpoint,
            chat_id,
            terminal_id,
            working,
        );
        let unchanged = self
            .endpoint_model(endpoint)
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .is_some_and(|chat| chat.terminal_working == terminal_working);
        if !unchanged {
            self.endpoint_model_mut(endpoint)
                .apply_desktop_terminal_activity(chat_id, terminal_working);
        }
    }

    fn record_terminal_tab_activity(&mut self, endpoint: ChatEndpoint, body: &Value) {
        if let (Some(chat_id), Some(terminal_id), Some(working)) = (
            body.get("chat").and_then(Value::as_str),
            body.get("terminal").and_then(Value::as_str),
            body.get("working").and_then(Value::as_bool),
        ) {
            update_terminal_activity_state(
                &mut self.terminal_activity_by_tab,
                endpoint,
                chat_id,
                terminal_id,
                Some(working),
            );
        }
    }

    fn handle_event(
        &mut self,
        name: &str,
        body: Value,
        attachments: Option<Vec<Attachment>>,
        cx: &mut Context<Self>,
    ) {
        if self.event_is_active(&body)
            && matches!(name, "interaction-resolved" | "interactions-cleared")
        {
            if let Some(id) = body.get("request_id").and_then(Value::as_str) {
                self.interaction_answers
                    .retain(|key, _| !key.starts_with(&format!("{id}/")));
            } else {
                self.interaction_answers.clear();
            }
            self.question_answer.clear();
            self.question_input
                .update(cx, |input, cx| input.set_text("", cx));
            self.secret_question_input
                .update(cx, |input, cx| input.set_text("", cx));
        }

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
            // Approvals and questions, for this chat and for the "waiting on
            // you" marks on every other one. Without this they only appeared
            // when something else happened to reload the chat.
            "interaction" | "interaction-resolved" | "interactions-cleared" => {
                self.model.apply_event(name, &body);
            }
            "terminal-activity" => {
                self.record_terminal_tab_activity(self.active_endpoint, &body);
                self.model.apply_event(name, &body);
            }
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
                    // A background agent that starts between turns adds its card
                    // as a stored row; during a turn the live items carry it.
                    if !self.model.working {
                        self.request_messages(&chat_id);
                    }
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
                let old_count = self.transcript_snapshot.len();
                let had_live_text = self.transcript_snapshot.live_text.is_some();
                self.model.apply_event(name, &body);
                if name == "text" {
                    self.schedule_live_text_render(cx);
                } else {
                    self.transcript_snapshot.sync_live_items(&self.model);
                    self.transcript_snapshot.sync_live_text(&self.model);
                    let new_count = self.transcript_snapshot.len();
                    let closed_live_text =
                        had_live_text && self.transcript_snapshot.live_text.is_none();
                    if closed_live_text {
                        // The streamed row became an item; the tool follows it.
                        self.remeasure_rows(old_count - 1..old_count, new_count + 1 - old_count);
                    } else if new_count > old_count {
                        self.transcript
                            .splice(old_count..old_count, new_count - old_count);
                    } else if new_count > 0 {
                        self.remeasure_rows(new_count - 1..new_count, 1);
                    }
                    // A new tail row changes the run's header count and which
                    // of its earlier rows the collapsed log still shows.
                    if new_count > 0
                        && let Some(run) = self.transcript_snapshot.work_run(new_count - 1)
                        && run.position > 0
                    {
                        let first = run.start;
                        let last = new_count - 1;
                        self.remeasure_rows(first..last, last - first);
                    }
                    self.request_workflow_statuses();
                }
            }
            "record" if self.event_is_active(&body) => {
                if let Some(view) = body
                    .get("text")
                    .and_then(Value::as_str)
                    .and_then(SubagentView::parse)
                {
                    let settled = match self.subagent_actions.get(view.key.as_str()) {
                        Some(&"Moving to background") => {
                            view.background || view.status != SubagentState::Running
                        }
                        Some(_) => view.status != SubagentState::Running,
                        None => false,
                    };
                    if settled {
                        self.subagent_actions.remove(&view.key);
                    }
                }
                if self.model.apply_record(&body).is_some()
                    && let (Some(id), Some(text)) = (
                        body.get("message_id").and_then(Value::as_i64),
                        body.get("text").and_then(Value::as_str),
                    )
                    && let Some(row) = self.transcript_snapshot.replace_record(id, text)
                {
                    self.remeasure_rows(row..row + 1, 1);
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
                panel.error = status.error.clone();
                panel.status = Some(status);
            }
            Err(error) => {
                panel.busy = false;
                panel.error = Some(format!("Invalid host update response: {error}"));
            }
        }
    }

    fn request_app_update(&mut self, action: &str, cx: &mut Context<Self>) {
        let result = self
            .host
            .as_ref()
            .ok_or_else(|| {
                "The local update service is reconnecting. Try again shortly.".to_owned()
            })
            .and_then(|host| host.host_update(action));
        let panel = self.self_update_panel.get_or_insert_with(Default::default);
        panel.busy = result.is_ok();
        panel.error = result.err();
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
        self.minimal_theme_open || self.browser_occluded_by_popup()
    }

    fn browser_occluded_by_popup(&self) -> bool {
        self.minimal_new_tab_open
            || self.auth_open
            || self.native_option_open.is_some()
            || self.share_panel.is_some()
            || self.devices_panel.is_some()
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
        self.chat_create_branches.clear();
        self.chat_create_pull_requests.clear();
        self.chat_create_sources_loading = true;
        self.chat_create_sources_error = None;
        self.chat_create_input.update(cx, |input, cx| {
            input.set_text_selected(DEFAULT_CHAT_TITLE, cx)
        });
        if let Some(host) = self.active_host().cloned() {
            if let Err(error) = host.folder_settings(&folder_id) {
                self.chat_create_worktrees_loading = false;
                self.model.connection_error = Some(error);
            }
            if let Err(error) = host.new_chat_sources(&folder_id) {
                self.chat_create_sources_loading = false;
                self.chat_create_sources_error = Some(error);
            }
        } else {
            self.chat_create_worktrees_loading = false;
            self.chat_create_sources_loading = false;
            self.model.connection_error = Some("xd is not connected to a host.".into());
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
        self.chat_create_branches.clear();
        self.chat_create_pull_requests.clear();
        self.chat_create_sources_loading = false;
        self.chat_create_sources_error = None;
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
        let command = String::new();
        self.remote_panel = Some(RemotePanel {
            command: command.clone(),
            submitting: false,
            error: self.remote_error.clone(),
        });
        self.remote_pairing_input
            .update(cx, |input, cx| input.set_text(command, cx));
        let focus = self.remote_pairing_input.read(cx).focus_handle(cx);
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
            self.remote_state = if self.settings.remote_credentials.is_some() {
                RemoteState::Offline
            } else {
                RemoteState::Unconfigured
            };
        }
        self.remote_panel = None;
        if canceled_connection && self.settings.remote_credentials.is_some() {
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
        let invite = match xd_transport::PairingInvite::parse(&panel.command) {
            Ok(invite) => invite,
            Err(error) => {
                if let Some(panel) = &mut self.remote_panel {
                    panel.error = Some(error);
                }
                cx.notify();
                return;
            }
        };
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
        let connection = cx
            .background_executor()
            .spawn(async move { paired::pair(invite) });
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
            workdir: None,
            agent: None,
            allow_agent_tabs: false,
            sessions: Vec::new(),
            selected: None,
            viewport: None,
            auto_open: true,
            opening: false,
            opening_agent: None,
            opening_generation: 0,
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

    fn stop_terminal_sessions_for_chats(
        &mut self,
        endpoint: ChatEndpoint,
        chat_ids: &HashSet<String>,
    ) {
        if let Err(error) = self.terminal_runtime.kill_chats(endpoint.into(), chat_ids) {
            self.endpoint_model_mut(endpoint).connection_error = Some(error);
        }
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
        if let Err(error) = self
            .terminal_runtime
            .reconcile_chats(endpoint.into(), &live_chats)
        {
            self.endpoint_model_mut(endpoint).connection_error = Some(error);
        }
        self.terminal_panel_cache
            .retain(|(cached_endpoint, chat_id), _| {
                *cached_endpoint != endpoint
                    || terminal_panel_cache_entry_is_live(chat_id, &live_chats)
            });
        self.terminal_activity_by_tab
            .retain(|(activity_endpoint, chat_id, _), _| {
                *activity_endpoint != endpoint || (!refresh_all && live_chats.contains(chat_id))
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

    fn connection_key(&self, endpoint: ChatEndpoint) -> String {
        let remote = self
            .settings
            .remote_credentials
            .as_ref()
            .map(|credentials| format!("{}:{}", credentials.host, credentials.port))
            .or_else(|| {
                self.settings
                    .remote_ssh_command
                    .as_deref()
                    .and_then(|command| SshCommand::parse(command).ok())
                    .map(|command| command.destination().to_owned())
            });
        connection_state_key(endpoint, remote.as_deref())
    }

    fn current_connection_key(&self) -> String {
        self.connection_key(self.active_endpoint)
    }

    fn prune_browser_panes(&mut self, endpoint: ChatEndpoint, cx: &mut Context<Self>) {
        let prefix = format!("{}/chat/", self.connection_key(endpoint));
        let live_chats = self
            .endpoint_model(endpoint)
            .chats
            .iter()
            .map(|chat| chat.id.as_str())
            .collect::<HashSet<_>>();
        let removed = self
            .browser_panes
            .keys()
            .chain(self.settings.browser_sessions.keys())
            .filter(|key| {
                key.strip_prefix(&prefix)
                    .is_some_and(|chat_id| !live_chats.contains(chat_id))
            })
            .cloned()
            .collect::<HashSet<_>>();
        let mut settings_changed = false;
        for key in removed {
            if let Some(pane) = self.browser_panes.remove(&key) {
                pane.update(cx, |pane, _| pane.shutdown());
            }
            settings_changed |= self.settings.browser_sessions.remove(&key).is_some();
        }
        if settings_changed {
            let _ = self.settings.save();
        }
    }

    fn selected_minimal_project_id(&self) -> Option<String> {
        let requested = match &self.minimal_route {
            MinimalRoute::Projects { project_id } | MinimalRoute::Sessions { project_id } => {
                project_id.as_deref()
            }
            MinimalRoute::Cli { project_id, .. } => Some(project_id.as_str()),
            MinimalRoute::Terminal => return None,
        };
        requested
            .filter(|id| self.model.folders.iter().any(|folder| folder.id == *id))
            .map(str::to_owned)
            .or_else(|| self.model.folders.first().map(|folder| folder.id.clone()))
    }

    fn browser_scope_key(&self) -> String {
        let connection = self.current_connection_key();
        let chat_id = match &self.minimal_route {
            MinimalRoute::Cli { chat_id, .. } => Some(chat_id.as_str()),
            MinimalRoute::Terminal => None,
            _ => self.model.selected_chat.as_deref(),
        };
        match chat_id {
            Some(chat_id) => format!("{connection}/chat/{chat_id}"),
            None => format!("{connection}/browser"),
        }
    }

    fn browser_is_open(&self) -> bool {
        self.settings
            .browser_sessions
            .get(&self.browser_scope_key())
            .is_some_and(|state| state.open)
    }

    fn set_browser_open(&mut self, open: bool) {
        let key = self.browser_scope_key();
        let initial_url = self.settings.browser_urls.get(&key).cloned();
        self.settings
            .browser_sessions
            .entry(key)
            .or_insert_with(|| BrowserState {
                open: false,
                session: BrowserSession {
                    tabs: vec![initial_url],
                    active_tab: 0,
                },
            })
            .open = open;
    }

    fn close_browser(&mut self, cx: &mut Context<Self>) {
        self.set_browser_open(false);
        self.browser_drag = None;
        self.browser_restore_focus = true;
        for pane in self.browser_panes.values() {
            pane.update(cx, |pane, _| pane.set_visible(false));
        }
        let _ = self.settings.save();
        cx.notify();
    }

    fn toggle_browser(&mut self, cx: &mut Context<Self>) {
        if self.browser_is_open() {
            self.close_browser(cx);
        } else {
            self.set_browser_open(true);
            let _ = self.settings.save();
            cx.notify();
        }
    }

    fn open_browser_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_browser_open(true);
        self.browser_restore_focus = false;
        if let Some(pane) = self.sync_browser_pane(window, cx) {
            pane.update(cx, |pane, cx| pane.open_url(url, window, cx));
        }
        let _ = self.settings.save();
        cx.notify();
    }

    fn sync_browser_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<BrowserPane>> {
        let key = self.browser_scope_key();
        let open = self.browser_is_open();
        let visible = open && !self.browser_occluded_by_popup() && self.browser_drag.is_none();
        for (scope, pane) in &self.browser_panes {
            pane.update(cx, |pane, _| pane.set_visible(visible && scope == &key));
        }
        if !open {
            return None;
        }
        let colors = self.settings.theme.colors();
        let pane = if let Some(pane) = self.browser_panes.get(&key) {
            pane.clone()
        } else {
            let session = self
                .settings
                .browser_sessions
                .get(&key)
                .map(|state| state.session.clone())
                .unwrap_or_else(|| BrowserSession {
                    tabs: vec![self.settings.browser_urls.get(&key).cloned()],
                    active_tab: 0,
                });
            let pane = cx.new(|cx| BrowserPane::with_session(session, colors, window, cx));
            let scope = key.clone();
            cx.subscribe(&pane, move |this, _, event, cx| match event {
                BrowserEvent::SessionChanged(session) => {
                    if let Some(state) = this.settings.browser_sessions.get_mut(&scope)
                        && state.session != *session
                    {
                        state.session = session.clone();
                        let _ = this.settings.save();
                    }
                }
                BrowserEvent::Close => this.close_browser(cx),
            })
            .detach();
            self.browser_panes.insert(key, pane.clone());
            pane
        };
        pane.update(cx, |pane, cx| {
            pane.set_colors(colors, cx);
            pane.set_visible(visible);
        });
        Some(pane)
    }

    fn browser_compacts_sidebar(&self, window: &Window) -> bool {
        self.browser_is_open()
            && f32::from(window.viewport_size().width)
                - browser_pane_width(
                    self.settings.browser_width,
                    f32::from(window.viewport_size().width),
                )
                < 640.0
    }

    fn finish_dock_resize(&mut self, cx: &mut Context<Self>) {
        if self.dock_drag.take().is_some() {
            let _ = self.settings.save();
            cx.notify();
        }
    }

    fn finish_browser_resize(&mut self, cx: &mut Context<Self>) {
        if self.browser_drag.take().is_some() {
            let _ = self.settings.save();
            cx.notify();
        }
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
        updates: SessionEventReceiver,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Ok(event) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        let (endpoint, name, body) = terminal_runtime_event(event);
                        if name == "terminal-activity" {
                            if let (Some(chat_id), Some(terminal_id), Some(working)) = (
                                body.get("chat").and_then(Value::as_str),
                                body.get("terminal").and_then(Value::as_str),
                                body.get("working").and_then(Value::as_bool),
                            ) {
                                this.apply_terminal_tab_activity(
                                    endpoint,
                                    chat_id,
                                    terminal_id,
                                    Some(working),
                                );
                            }
                            cx.notify();
                        } else {
                            this.handle_terminal_screen_event(endpoint, name, &body, cx);
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

    #[allow(dead_code)]
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
                if !cfg!(target_os = "windows") {
                    std::fs::create_dir_all(&runtime)
                        .map_err(|error| format!("Cannot prepare terminal sessions: {error}."))?;
                    let configuration = runtime.join("tmux.conf");
                    std::fs::write(&configuration, TMUX_CONFIGURATION)
                        .map_err(|error| format!("Cannot configure terminal sessions: {error}."))?;
                }
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

    #[allow(dead_code)]
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
            let result = self
                .active_host()
                .ok_or_else(|| "Host disconnected.".to_owned())
                .and_then(|host| host.terminal_resize(&terminal_id, columns, rows));
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
        if let Err(error) = self
            .active_host()
            .ok_or_else(|| "Host disconnected.".to_owned())
            .and_then(|host| host.terminal_input(&terminal_id, bytes))
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
        if let Err(error) = self
            .active_host()
            .ok_or_else(|| "Host disconnected.".to_owned())
            .and_then(|host| host.terminal_kill(&terminal_id))
            && let Some(panel) = &mut self.terminal_panel
        {
            panel.error = Some(error);
        }
        cx.notify();
    }

    fn close_terminal_tab(&mut self, tab_id: String, cx: &mut Context<Self>) {
        self.kill_terminal_id(tab_id, cx);
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
        let endpoint = self.active_endpoint;
        let Some(panel) = &mut self.terminal_panel else {
            return;
        };
        if panel.opening || !panel.accepts_agent(agent) {
            return;
        }
        let (columns, rows) = panel.viewport.unwrap_or((120, 32));
        let chat_id = panel.chat_id.clone();
        panel.auto_open = false;
        let opening_generation = panel.begin_opening(agent);
        panel.error = None;
        let colors = self.settings.theme.colors();
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| match agent {
                Some(agent) => host.terminal_open_agent(
                    &chat_id,
                    columns,
                    rows,
                    reuse,
                    agent.protocol_name(),
                    self.settings.allow_all_permissions,
                    colors.text,
                    colors.background,
                ),
                None => host.terminal_open(
                    &chat_id,
                    columns,
                    rows,
                    reuse,
                    colors.text,
                    colors.background,
                ),
            });
        match result {
            Err(error) => {
                if let Some(panel) = &mut self.terminal_panel {
                    panel.finish_opening();
                    panel.error = Some(error);
                }
            }
            Ok(()) => {
                cx.spawn(async move |this, cx| {
                    Timer::after(TERMINAL_OPEN_TIMEOUT).await;
                    let _ = this.update(cx, |this, cx| {
                        let active = this.active_endpoint == endpoint
                            && this
                                .terminal_panel
                                .as_ref()
                                .is_some_and(|panel| panel.chat_id == chat_id);
                        let panel = if active {
                            this.terminal_panel.as_mut()
                        } else {
                            this.terminal_panel_cache
                                .get_mut(&(endpoint, chat_id.clone()))
                        };
                        if let Some(panel) = panel
                            && panel.expire_opening(opening_generation)
                        {
                            panel.error =
                                Some("Terminal startup timed out. Try opening it again.".into());
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Reattaches the selected card to its persistent tmux session. The PTY is
    /// window-owned, while the command remains alive in tmux between windows.
    fn refresh_terminal_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self
            .terminal_panel
            .as_ref()
            .filter(|panel| {
                panel.chat_id.starts_with("global:")
                    || self.model.selected_chat.as_deref() == Some(panel.chat_id.as_str())
            })
            .map(|panel| panel.chat_id.clone())
        else {
            return;
        };
        if let Some(panel) = &mut self.terminal_panel {
            panel.loading = true;
            panel.error = None;
        }
        if let Some(host) = self.active_host() {
            if let Err(error) = host.terminal_list(&chat_id) {
                self.model.connection_error = Some(error);
            }
        }
        cx.notify();
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
            if let Err(error) = self
                .active_host()
                .ok_or_else(|| "Host disconnected.".to_owned())
                .and_then(|host| host.terminal_resize(&terminal_id, columns, rows))
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

    fn reorder_session_from_drop(
        &mut self,
        drag: &SessionDragPayload,
        target_project_id: &str,
        target_index: usize,
        target_chat_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(reorder) =
            chat_reorder_for_drop(drag, target_project_id, target_index, target_chat_id)
        else {
            return;
        };
        let result = self
            .active_host()
            .ok_or_else(|| "xd is not connected to a host.".to_owned())
            .and_then(|host| {
                host.reorder_chat(&reorder.chat_id, &reorder.anchor_id, reorder.after)
            });
        match result {
            Ok(()) => {
                apply_optimistic_chat_reorder(&mut self.model.chats, &reorder);
            }
            Err(error) => self.model.connection_error = Some(error),
        }
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

    fn request_older_messages(&mut self) {
        if !self.transcript_has_older || self.transcript_page_loading || self.transcript_loading {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(cursor) = self
            .model
            .messages
            .first()
            .and_then(|message| message.id)
            .map(MessageCursor::Before)
        else {
            return;
        };
        self.request_message_page(&chat_id, cursor);
    }

    fn request_newer_messages(&mut self) {
        if !self.transcript_has_newer || self.transcript_page_loading || self.transcript_loading {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(cursor) = self
            .model
            .messages
            .last()
            .and_then(|message| message.id)
            .map(MessageCursor::After)
        else {
            return;
        };
        self.request_message_page(&chat_id, cursor);
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
            // Sending is a request to see the reply, wherever the reader was.
            self.pin_transcript();
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

    fn queue_composer(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.trim().to_owned();
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if text.is_empty() || !self.model.draft_attachments.is_empty() {
            return;
        }
        let Some(host) = self.active_host().cloned() else {
            self.model.connection_error = Some("xd is not connected to a host.".into());
            cx.notify();
            return;
        };
        if let Err(error) = host.queue_message(&chat_id, &text) {
            self.model.connection_error = Some(error);
            cx.notify();
            return;
        }
        self.model.queue.push(text);
        self.set_composer_text(String::new(), cx);
        self.draft_dirty = true;
        self.draft_generation = self.draft_generation.saturating_add(1);
        let _ = host.set_draft(&chat_id, "", None, None);
        cx.notify();
    }

    fn drop_queued(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(host) = self.active_host().cloned() else {
            self.model.connection_error = Some("xd is not connected to a host.".into());
            cx.notify();
            return;
        };
        match host.drop_queue(&chat_id, index) {
            Ok(()) if index < self.model.queue.len() => {
                self.model.queue.remove(index);
            }
            Ok(()) => {}
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
    }

    fn cancel_turn(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(host) = self.active_host().cloned() else {
            self.model.connection_error = Some("xd is not connected to a host.".into());
            cx.notify();
            return;
        };
        match host.cancel(&chat_id) {
            Ok(()) => {
                self.model.stop_working();
                if let Some(summary) = self.model.chats.iter_mut().find(|chat| chat.id == chat_id) {
                    summary.working = false;
                }
            }
            Err(error) => self.model.connection_error = Some(error),
        }
        cx.notify();
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
        if let Some(pending) = self
            .model
            .interactions
            .iter()
            .find(|p| p["method"] == "item/tool/requestUserInput")
            .cloned()
        {
            if let Some(id) = pending["request_id"].as_str() {
                if let Some(question) =
                    pending["params"]["questions"]
                        .as_array()
                        .and_then(|questions| {
                            questions.iter().find(|q| {
                                !self.interaction_answers.contains_key(&format!(
                                    "{id}/{}",
                                    q["id"].as_str().unwrap_or_default()
                                ))
                            })
                        })
                {
                    if let Some(qid) = question["id"].as_str() {
                        self.answer_interaction(
                            id.to_owned(),
                            qid.to_owned(),
                            self.question_answer.clone(),
                            cx,
                        );
                        return;
                    }
                }
            }
        }
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
                    let _ = this.sync_draft();
                }
            });
        })
        .detach();
    }

    fn sync_draft(&mut self) -> bool {
        if !self.draft_dirty && !self.attachments_dirty {
            return true;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return false;
        };
        let Some(host) = self.active_host().cloned() else {
            self.model.connection_error =
                Some("The draft is still local because xd is disconnected.".into());
            return false;
        };
        let attachments = self
            .attachments_dirty
            .then_some(self.model.draft_attachments.as_slice());
        let attachment_generation = attachments.map(|_| self.attachment_generation);
        if let Err(error) =
            host.set_draft(&chat_id, &self.composer, attachments, attachment_generation)
        {
            self.model.connection_error = Some(error);
            return false;
        }
        true
    }

    fn flush_draft_before_navigation(&mut self) -> bool {
        self.draft_generation = self.draft_generation.saturating_add(1);
        if !self.sync_draft() {
            return false;
        }
        self.draft_dirty = false;
        self.attachments_dirty = false;
        true
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
        let count = self.transcript_snapshot.len();
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

    /// Whether anything on screen shows motion: a turn, a session card, a
    /// terminal, or an agent still at work.
    fn anything_working(&self) -> bool {
        self.model.working
            || !self.transcript_snapshot.running_agents.is_empty()
            || self
                .model
                .chats
                .iter()
                .any(|chat| chat.working || chat.terminal_working)
    }

    fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        if self.transcript_has_newer {
            // The newest page is not loaded; the tail request lands pinned.
            if let Some(chat_id) = self.model.selected_chat.clone() {
                self.request_message_page(&chat_id, MessageCursor::Tail);
            }
        }
        self.pin_transcript();
        cx.notify();
    }

    /// Whether the transcript follows new output: a bottom-aligned list
    /// reports its end as the scroll top only while it is pinned there.
    fn transcript_pinned(&self) -> bool {
        self.transcript.logical_scroll_top().item_ix >= self.transcript.item_count()
    }

    /// Scrolls to the newest row and follows new output from there.
    fn pin_transcript(&self) {
        self.transcript.scroll_to(ListOffset {
            item_ix: self.transcript.item_count(),
            offset_in_item: px(0.0),
        });
    }

    /// Holds the transcript where the reader sees it before a row they
    /// clicked changes height. A pinned list grows upward, which would push
    /// the header they just clicked off the top.
    fn freeze_transcript(&self) {
        if self.transcript_pinned() && self.transcript.item_count() > 0 {
            let height = self.transcript.viewport_bounds().size.height;
            self.transcript.scroll_by(-height);
        }
    }

    /// Re-measures rows whose content changed without moving the reader: a
    /// pinned list stays pinned, and a reader inside a changed row keeps their
    /// place in it instead of being thrown to its top.
    fn remeasure_rows(&self, range: Range<usize>, count: usize) {
        if range.end > self.transcript.item_count() {
            return;
        }
        let anchor = self.transcript.logical_scroll_top();
        let pinned = self.transcript_pinned();
        let start = range.start;
        self.transcript.splice(range.clone(), count);
        if !pinned && range.contains(&anchor.item_ix) && anchor.item_ix - start < count {
            self.transcript.scroll_to(anchor);
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
                let index = this.transcript_snapshot.messages.len()
                    + this.transcript_snapshot.live_items.len();
                if had_live_text {
                    this.remeasure_rows(index..index + 1, 1);
                } else {
                    // A new tail row: a pinned list follows it by itself.
                    this.transcript.splice(index..index, 1);
                }
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
        for index in indices {
            self.remeasure_rows(index..index + 1, 1);
        }
    }
}

impl XdDesktop {
    fn show_minimal_projects(&mut self, cx: &mut Context<Self>) {
        let project_id = match &self.minimal_route {
            MinimalRoute::Projects { project_id } => project_id.clone(),
            MinimalRoute::Sessions { project_id } => project_id.clone(),
            MinimalRoute::Terminal => None,
            MinimalRoute::Cli { project_id, .. } => Some(project_id.clone()),
        };
        if !self.flush_draft_before_navigation() {
            cx.notify();
            return;
        }
        self.minimal_route = MinimalRoute::Projects { project_id };
        self.minimal_new_tab_open = false;
        self.stash_terminal_panel();
        self.model.selected_chat = None;
        self.sync_terminal_input_mode(cx);
        cx.notify();
    }

    fn show_minimal_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel_id = global_terminal_panel_id(&self.current_connection_key());
        let same_panel = matches!(self.minimal_route, MinimalRoute::Terminal)
            && self
                .terminal_panel
                .as_ref()
                .is_some_and(|panel| panel.chat_id == panel_id);
        if !self.flush_draft_before_navigation() {
            cx.notify();
            return;
        }
        self.minimal_route = MinimalRoute::Terminal;
        self.minimal_new_tab_open = false;
        self.model.selected_chat = None;
        if !same_panel {
            self.stash_terminal_panel();
            let key = (self.active_endpoint, panel_id.clone());
            let mut panel = self
                .terminal_panel_cache
                .remove(&key)
                .unwrap_or_else(|| Self::new_terminal_panel(panel_id));
            panel.workdir = Some(match self.active_endpoint {
                ChatEndpoint::Remote => ".".into(),
                ChatEndpoint::Local if cfg!(target_os = "windows") => "~".into(),
                ChatEndpoint::Local => env::var("HOME")
                    .or_else(|_| env::var("USERPROFILE"))
                    .unwrap_or_else(|_| ".".into()),
            });
            panel.agent = None;
            panel.allow_agent_tabs = false;
            panel.loading = false;
            panel.auto_open = panel.sessions.is_empty();
            self.terminal_panel = Some(panel);
            self.sync_terminal_input_mode(cx);
            self.terminal_scroll.scroll_to_bottom();
        }
        let focus = self.terminal_input.read(cx).focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    fn native_session(&self) -> bool {
        self.model
            .selected_chat
            .as_ref()
            .and_then(|id| self.model.session_modes.get(id))
            .is_some_and(|mode| mode == "chat")
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
        let focus = if self.native_session() {
            self.composer_input.read(cx).focus_handle(cx)
        } else {
            self.terminal_input.read(cx).focus_handle(cx)
        };
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
            self.native_pane = NativePane::Chat;
            self.diff_generation = self.diff_generation.saturating_add(1);
            self.diff_panel = None;
            self.interaction_answers.clear();
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
        if self.native_session() {
            self.stash_terminal_panel();
            self.sync_terminal_input_mode(cx);
            self.transcript_loading = !self.transcript_loaded;
            self.request_chat(&chat_id);
            self.request_messages(&chat_id);
            self.list_tree_directory(String::new(), cx);
            if let Some(host) = self.active_host() {
                let _ = host.git_state(&chat_id);
            }
            cx.notify();
            return;
        }
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
            MinimalRoute::Terminal => {
                self.minimal_route = next;
            }
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
        let panel_loading = panel.loading;
        let panel_is_empty = panel.sessions.is_empty();
        let panel_error = panel.error.clone();
        let allow_agent_tabs = panel.allow_agent_tabs;
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
                    move |index, window, cx| {
                        if let Some(url) = urls.get(index) {
                            window.dispatch_action(
                                Box::new(OpenBrowserUrl {
                                    url: url.clone(),
                                    window: window.window_handle(),
                                }),
                                cx,
                            );
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
                        colors.selected_border
                    } else {
                        colors.sidebar
                    }))
                    .text_size(px(12.0))
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
                            .text_color(rgb(colors.faint))
                            .hover(|style| {
                                style
                                    .bg(rgb(colors.surface_high))
                                    .text_color(rgb(colors.text))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_terminal_tab(close_id.clone(), cx);
                            }))
                            .child(icon(CLOSE_ICON, 12.0, colors.faint)),
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
        let viewport = div()
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
            .child(terminal_input)
            .into_any_element();

        div()
            .size_full()
            .min_h_0()
            .relative()
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.background))
            .overflow_hidden()
            .child(
                div()
                    .h(px(36.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.sidebar))
                    .children(tabs)
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("minimal-new-terminal")
                            .size(px(26.0))
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
            .when(panel_loading && panel_is_empty, |pane| {
                pane.child(
                    div()
                        .absolute()
                        .top(px(50.0))
                        .left(px(16.0))
                        .text_size(px(12.0))
                        .text_color(rgb(colors.muted))
                        .child("Starting CLI…"),
                )
            })
            .when_some(panel_error, |pane, error| {
                pane.child(
                    div()
                        .absolute()
                        .top(px(54.0))
                        .left(px(16.0))
                        .right(px(16.0))
                        .text_size(px(12.0))
                        .text_color(rgb(colors.danger))
                        .child(error),
                )
            })
            .child(viewport)
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
                        .top(px(34.0))
                        .right(px(8.0))
                        .w(px(180.0))
                        .p_1()
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .track_focus(&minimal_popup_focus)
                        .child(
                            div()
                                .id("minimal-new-shell-tab")
                                .w_full()
                                .px_3()
                                .h(px(32.0))
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded(px(5.0))
                                .text_size(px(13.0))
                                .text_color(rgb(colors.text))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_minimal_terminal_tab(None, window, cx)
                                }))
                                .child(icon(TERMINAL_ICON, 15.0, colors.text))
                                .child("Terminal"),
                        )
                        .when(allow_agent_tabs, |menu| {
                            menu.child(
                                div()
                                    .id("minimal-new-codex-tab")
                                    .w_full()
                                    .px_3()
                                    .h(px(32.0))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .rounded(px(5.0))
                                    .text_size(px(13.0))
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
                                            .size(px(15.0))
                                            .text_color(rgb(colors.text)),
                                    )
                                    .child("Codex"),
                            )
                            .child(
                                div()
                                    .id("minimal-new-claude-tab")
                                    .w_full()
                                    .px_3()
                                    .h(px(32.0))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .rounded(px(5.0))
                                    .text_size(px(13.0))
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
                                            .size(px(15.0))
                                            .text_color(rgb(colors.text)),
                                    )
                                    .child("Claude Code"),
                            )
                            .child(
                                div()
                                    .id("minimal-new-jcode-tab")
                                    .w_full()
                                    .px_3()
                                    .h(px(32.0))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .rounded(px(5.0))
                                    .text_size(px(13.0))
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
                                            .size(px(15.0))
                                            .text_color(rgb(colors.text)),
                                    )
                                    .child("JCode"),
                            )
                            .child(
                                div()
                                    .id("minimal-new-copilot-tab")
                                    .w_full()
                                    .px_3()
                                    .h(px(32.0))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .rounded(px(5.0))
                                    .text_size(px(13.0))
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
                                            .size(px(15.0))
                                            .text_color(rgb(colors.text)),
                                    )
                                    .child("GitHub Copilot"),
                            )
                        }),
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
                        .child(icon(MINIMIZE_ICON, 14.0, colors.muted)),
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
                        .child(icon(MAXIMIZE_ICON, 12.0, colors.muted)),
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
                        .hover(|style| {
                            style
                                .bg(rgb(colors.danger_surface))
                                .text_color(rgb(colors.danger))
                        })
                        .window_control_area(WindowControlArea::Close)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, window, _| window.remove_window())
                        .child(icon(CLOSE_ICON, 14.0, colors.muted)),
                )
            })
            .into_any_element()
    }

    fn render_minimal_titlebar(
        &mut self,
        colors: ThemeColors,
        titlebar: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let connected = self.model.connected;
        let remote_active = self.active_endpoint == ChatEndpoint::Remote;
        let project_name = self
            .selected_minimal_project_id()
            .and_then(|id| self.model.folders.iter().find(|folder| folder.id == id))
            .map(|folder| folder.name.clone());
        let context = match &self.minimal_route {
            MinimalRoute::Terminal => "Terminal".to_string(),
            MinimalRoute::Cli { chat_id, .. } => {
                let title = self
                    .model
                    .chats
                    .iter()
                    .find(|chat| &chat.id == chat_id)
                    .and_then(|chat| chat.title.clone())
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| DEFAULT_CHAT_TITLE.to_string());
                project_name.clone().unwrap_or(title)
            }
            MinimalRoute::Projects { .. } | MinimalRoute::Sessions { .. } => project_name
                .clone()
                .unwrap_or_else(|| "Workspace".to_string()),
        };

        div()
            .id("workbench-titlebar")
            .h(px(38.0))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .pl(px(if titlebar && cfg!(target_os = "macos") {
                86.0
            } else {
                16.0
            }))
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.sidebar))
            .when(titlebar, |bar| {
                bar.on_mouse_down(MouseButton::Left, |event, window, _| {
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
                    .flex()
                    .items_center()
                    .gap_2()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_sm()
                    .text_color(rgb(colors.text))
                    .when(titlebar, |brand| {
                        brand.window_control_area(WindowControlArea::Drag)
                    })
                    .child(xd_mark(colors.text))
                    .child("xd"),
            )
            .child(div().text_sm().text_color(rgb(colors.faint)).child("/"))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(rgb(colors.muted))
                    .when(titlebar, |context| {
                        context.window_control_area(WindowControlArea::Drag)
                    })
                    .child(context),
            )
            .child(
                ui_button("minimal-browser-toggle", ButtonKind::Ghost, colors)
                    .h(px(26.0))
                    .px_2()
                    .when(self.browser_is_open(), |button| {
                        button
                            .bg(rgb(colors.selected_surface))
                            .text_color(rgb(colors.text))
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_browser(cx)))
                    .child("Browser"),
            )
            .child(
                div()
                    .id("minimal-runtime")
                    .h(px(26.0))
                    .px_2()
                    .mr_2()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(4.0))
                    .text_xs()
                    .text_color(rgb(colors.muted))
                    .cursor_pointer()
                    .hover(|style| {
                        style
                            .bg(rgb(colors.surface_high))
                            .text_color(rgb(colors.text))
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if remote_active {
                            this.disconnect_remote_runtime(cx);
                        } else {
                            this.open_remote(window, cx);
                        }
                    }))
                    .child(div().size(px(7.0)).rounded_full().bg(rgb(if connected {
                        colors.working
                    } else {
                        colors.danger
                    })))
                    .child(match (remote_active, connected) {
                        (false, true) => "Local",
                        (false, false) => "Local · reconnecting",
                        (true, true) => "Remote",
                        (true, false) => "Remote · reconnecting",
                    }),
            )
            .when(titlebar && !cfg!(target_os = "macos"), |bar| {
                bar.child(self.render_minimal_window_controls(colors))
            })
            .into_any_element()
    }

    /// The session's header: what it is, who runs it, where, and its state.
    fn render_minimal_context_toolbar(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (chat_id, route_agent) = match &self.minimal_route {
            MinimalRoute::Cli { chat_id, agent, .. } => (Some(chat_id.clone()), Some(*agent)),
            _ => (None, None),
        };
        let chat = chat_id
            .as_deref()
            .and_then(|id| self.model.chats.iter().find(|chat| chat.id == id));
        let title = chat
            .and_then(|chat| chat.title.as_deref())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(DEFAULT_CHAT_TITLE)
            .to_owned();
        let branch = chat.and_then(|chat| chat.branch.clone());
        let signal = chat
            .map(|chat| {
                SessionSignal::of(
                    chat.working || chat.terminal_working,
                    self.model.is_waiting(&chat.id) || !self.model.interactions.is_empty(),
                    false,
                )
            })
            .unwrap_or(SessionSignal::Idle);
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
            .h(px(48.0))
            .w_full()
            .flex_none()
            .px(px(20.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.background))
            .child(
                div()
                    .min_w_0()
                    .flex_shrink()
                    .truncate()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(colors.text))
                    .child(title),
            )
            .when_some(selected_agent, |toolbar, agent| {
                toolbar.child(
                    div()
                        .id("minimal-session-agent")
                        .h(px(22.0))
                        .px(px(8.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .rounded_full()
                        .bg(rgb(colors.surface_high))
                        .text_size(px(11.5))
                        .text_color(rgb(colors.muted))
                        .child(
                            svg()
                                .path(agent_icon(agent))
                                .size(px(13.0))
                                .text_color(rgb(colors.muted)),
                        )
                        .child(agent.protocol_name()),
                )
            })
            .when_some(branch, |toolbar, branch| {
                toolbar.child(
                    div()
                        .h(px(22.0))
                        .px(px(8.0))
                        .min_w_0()
                        .flex_shrink()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .rounded_full()
                        .bg(rgb(colors.surface_high))
                        .font_family(MONO)
                        .text_size(px(11.5))
                        .text_color(rgb(colors.muted))
                        .child(
                            svg()
                                .path(GIT_BRANCH_ICON)
                                .size(px(12.0))
                                .flex_none()
                                .text_color(rgb(colors.muted)),
                        )
                        .child(div().min_w_0().truncate().child(branch)),
                )
            })
            .child(div().flex_1())
            .when(signal != SessionSignal::Idle, |toolbar| {
                toolbar.child(stamp(signal.label(), signal.color(colors)))
            })
            .when(self.native_session(), |toolbar| {
                let current = self.native_pane;
                toolbar.child(
                    div().ml_2().flex().items_center().gap(px(2.0)).children(
                        [
                            (1usize, "Changes", NativePane::Diff),
                            (2, "Files", NativePane::Files),
                            (3, "Terminal", NativePane::Terminal),
                        ]
                        .into_iter()
                        .map(|(index, label, pane)| {
                            let active = current == pane;
                            ui_button(("native-pane", index), ButtonKind::Ghost, colors)
                                .h(px(28.0))
                                .px(px(10.0))
                                .when(active, |button| {
                                    button
                                        .bg(rgb(colors.selected_surface))
                                        .text_color(rgb(colors.text))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let next = if this.native_pane == pane {
                                        NativePane::Chat
                                    } else {
                                        pane
                                    };
                                    this.show_native_pane(next, cx);
                                }))
                                .child(label)
                        }),
                    ),
                )
            })
            .when(has_terminal, |toolbar| {
                toolbar
                    .child(
                        ui_button("minimal-context-terminal", ButtonKind::Secondary, colors)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_minimal_terminal_tab(None, window, cx);
                            }))
                            .child(icon(TERMINAL_ICON, 15.0, colors.text)),
                    )
                    .child(
                        ui_button("minimal-stop-terminal", ButtonKind::Danger, colors)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.send_terminal_input(&[3], cx)),
                            )
                            .child(div().size(px(9.0)).rounded(px(1.5)).bg(rgb(colors.danger)))
                            .child("Stop"),
                    )
            })
            .into_any_element()
    }

    fn session_signal(&self, session: &SessionCard) -> SessionSignal {
        SessionSignal::of(
            session.working,
            self.model.is_waiting(&session.id),
            self.model.unread_chats.contains(&session.id),
        )
    }

    fn toggle_drawer(&mut self, project_id: String, current: bool, cx: &mut Context<Self>) {
        if current {
            self.toggle_folder_collapsed(project_id, cx);
        } else {
            if !self.opened_drawers.remove(&project_id) {
                self.opened_drawers.insert(project_id);
            }
            cx.notify();
        }
    }

    fn render_session_card(
        &mut self,
        colors: ThemeColors,
        project_id: &str,
        session_index: usize,
        session: SessionCard,
        instance: usize,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let emphasized = selected;
        let signal = self.session_signal(&session);
        let open_project_id = project_id.to_owned();
        let chat_id = session.id.clone();
        let context_chat_id = session.id.clone();
        let context_title = session.title.clone();
        let drag_payload = SessionDragPayload {
            chat_id: session.id.clone(),
            project_id: project_id.to_owned(),
            source_index: session_index,
            title: session.title.clone(),
        };
        let drop_project_id = project_id.to_owned();
        let drop_chat_id = session.id.clone();
        let agent = session.agent;

        div()
            .id(("minimal-session-card", instance))
            .relative()
            .w_full()
            .h(px(CARD_HEIGHT))
            .flex_none()
            .pl(px(10.0))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(7.0))
            .when(emphasized, |row| row.bg(rgb(colors.selected_surface)))
            .cursor_pointer()
            .hover(|style| {
                if emphasized {
                    style
                } else {
                    style.bg(rgb(colors.surface_high))
                }
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_minimal_session(
                    open_project_id.clone(),
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
            .can_drop({
                let drop_project_id = drop_project_id.clone();
                let drop_chat_id = drop_chat_id.clone();
                move |value, _, _| {
                    value
                        .downcast_ref::<SessionDragPayload>()
                        .is_some_and(|drag| {
                            drag.project_id == drop_project_id && drag.chat_id != drop_chat_id
                        })
                }
            })
            .drag_over::<SessionDragPayload>(move |style, _, _, _| {
                style
                    .border_color(rgb(colors.selected_border))
                    .bg(rgb(colors.surface_high))
            })
            .on_drop(cx.listener(move |this, drag: &SessionDragPayload, _, cx| {
                this.reorder_session_from_drop(
                    drag,
                    &drop_project_id,
                    session_index,
                    &drop_chat_id,
                    cx,
                );
            }))
            .on_drag(drag_payload, move |drag, position, _, cx| {
                cx.new(|_| SessionDragPreview {
                    title: drag.title.clone(),
                    position,
                    colors,
                })
            })
            .child(
                svg()
                    .path(agent_icon(agent))
                    .size(px(13.0))
                    .flex_none()
                    .text_color(rgb(if emphasized {
                        colors.text
                    } else {
                        colors.faint
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.0))
                    .text_color(rgb(if emphasized {
                        colors.text
                    } else {
                        colors.muted
                    }))
                    .when(emphasized, |title| title.font_weight(FontWeight::MEDIUM))
                    .child(session.title),
            )
            .when(
                signal == SessionSignal::Idle && session.branch != "Project directory",
                |row| {
                    row.child(
                        div()
                            .max_w(px(96.0))
                            .flex_none()
                            .truncate()
                            .font_family(MONO)
                            .text_size(px(10.5))
                            .text_color(rgb(colors.faint))
                            .child(session.branch),
                    )
                },
            )
            .children(signal_tab(signal, colors))
            .into_any_element()
    }

    /// The catalog: one drawer per project, each holding its session cards.
    /// The current project's drawer lifts out; the rest stay one line tall and
    /// show only their sessions' signal tabs until opened.
    fn render_catalog(
        &mut self,
        colors: ThemeColors,
        active_chat_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let projects = project_cards(&self.model.folders, &self.model.chats);
        let current_project = self.selected_minimal_project_id();
        let mut drawers = Vec::with_capacity(projects.len());
        let mut session_instance = 0usize;

        let query = self.catalog_query.clone();
        let mut matches = 0usize;
        for (project_index, project) in projects.iter().enumerate() {
            let current = current_project.as_deref() == Some(project.id.as_str())
                && !matches!(self.minimal_route, MinimalRoute::Terminal);
            let mut sessions = project_sessions(&project.id, &self.model.chats);
            let finding = !query.is_empty();
            if finding && !project.name.to_lowercase().contains(&query) {
                sessions.retain(|session| {
                    session.title.to_lowercase().contains(&query)
                        || session.branch.to_lowercase().contains(&query)
                });
                if sessions.is_empty() {
                    session_instance += project.sessions;
                    continue;
                }
            }
            matches += 1;
            let open = finding
                || if current {
                    !self.collapsed_folders.contains(&project.id)
                } else {
                    self.opened_drawers.contains(&project.id)
                };
            let signals = sessions
                .iter()
                .map(|session| self.session_signal(session))
                .collect::<Vec<_>>();
            let mut cards = Vec::new();
            if open {
                for (session_index, session) in sessions.into_iter().enumerate() {
                    let instance = session_instance;
                    session_instance += 1;
                    let selected = active_chat_id == Some(session.id.as_str());
                    cards.push(self.render_session_card(
                        colors,
                        &project.id,
                        session_index,
                        session,
                        instance,
                        selected,
                        cx,
                    ));
                }
            } else {
                session_instance += sessions.len();
            }
            // Keep instance ids stable for sessions the filter hid.
            session_instance += project.sessions.saturating_sub(signals.len());

            let toggle_project = project.id.clone();
            let select_project = project.id.clone();
            let create_project = project.id.clone();
            let name = div()
                .min_w_0()
                .flex_shrink()
                .truncate()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(if current { colors.text } else { colors.muted }))
                .child(project.name.clone());
            let header = div()
                .id(("minimal-session-group-header", project_index))
                .h(px(30.0))
                .w_full()
                .pl(px(4.0))
                .pr(px(2.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded(px(7.0))
                .group("catalog-project")
                .cursor_pointer()
                .hover(|style| style.bg(rgb(colors.surface_high)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.opened_drawers.remove(&select_project);
                    this.collapsed_folders.remove(&select_project);
                    this.persist_collapsed_folders();
                    this.minimal_route = MinimalRoute::Projects {
                        project_id: Some(select_project.clone()),
                    };
                    cx.notify();
                }))
                .child(
                    div()
                        .id(("catalog-drawer-toggle", project_index))
                        .size(px(20.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.0))
                        .text_sm()
                        .text_color(rgb(colors.faint))
                        .hover(|style| style.bg(rgb(colors.border)).text_color(rgb(colors.text)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_drawer(toggle_project.clone(), current, cx);
                        }))
                        .child(icon(
                            if open {
                                CHEVRON_DOWN_ICON
                            } else {
                                CHEVRON_RIGHT_ICON
                            },
                            12.0,
                            colors.faint,
                        )),
                )
                .child(icon(
                    FOLDER_ICON,
                    13.0,
                    if current { colors.muted } else { colors.faint },
                ))
                .child(name)
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(rgb(colors.faint))
                        .child(project.sessions.to_string()),
                )
                .child(div().flex_1())
                .when(!open, |header| header.child(drawer_tabs(&signals, colors)))
                .child(
                    div()
                        .id(("minimal-session-group-add", project_index))
                        .invisible()
                        .group_hover("catalog-project", |style| style.visible())
                        .size(px(22.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(colors.border)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.begin_chat_create(create_project.clone(), window, cx);
                        }))
                        .child(plus_icon(colors.faint)),
                );

            drawers.push(
                div()
                    .id(("minimal-session-group", project_index))
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .when(open, |drawer| {
                        drawer.child(
                            div()
                                .mt(px(1.0))
                                .mb(px(6.0))
                                .pl(px(12.0))
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap(px(1.0))
                                .children(cards)
                                .when(project.sessions == 0, |list| {
                                    list.child(
                                        div()
                                            .px_3()
                                            .py_2()
                                            .text_size(px(12.0))
                                            .text_color(rgb(colors.muted))
                                            .child("No sessions yet."),
                                    )
                                }),
                        )
                    }),
            );
        }

        let terminal_active = matches!(self.minimal_route, MinimalRoute::Terminal);
        let settings_open = self.minimal_theme_open;

        div()
            .id("minimal-session-board")
            .w(px(CATALOG_WIDTH))
            .h_full()
            .min_h_0()
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.sidebar))
            .child(
                div()
                    .h(px(40.0))
                    .flex_none()
                    .pl(px(14.0))
                    .pr(px(8.0))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(colors.faint))
                            .child("Projects"),
                    )
                    .child(
                        div()
                            .id("minimal-new-project")
                            .size(px(26.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.begin_workspace_create(window, cx)
                            }))
                            .child(plus_icon(colors.muted)),
                    ),
            )
            .child(
                div()
                    .id("catalog-find")
                    .mx(px(10.0))
                    .mb_2()
                    .h(px(30.0))
                    .px(px(9.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(8.0))
                    .bg(rgb(colors.surface))
                    .text_size(px(12.5))
                    .child(icon(SEARCH_ICON, 13.0, colors.faint))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.catalog_find_input.clone()),
                    ),
            )
            .child(
                div()
                    .id("minimal-project-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(8.0))
                    .pb_4()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .children(drawers)
                    .when(projects.is_empty(), |list| {
                        list.child(
                            div()
                                .px_3()
                                .py_3()
                                .text_size(px(12.5))
                                .line_height(px(19.0))
                                .text_color(rgb(colors.muted))
                                .child(
                                    "No projects yet. Add one with +, then start a session in it.",
                                ),
                        )
                    })
                    .when(!projects.is_empty() && matches == 0, |list| {
                        list.child(
                            div()
                                .px_3()
                                .py_3()
                                .text_size(px(12.5))
                                .line_height(px(19.0))
                                .text_color(rgb(colors.muted))
                                .child(format!(
                                    "Nothing matches \u{201c}{query}\u{201d}. Press Esc to clear."
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py(px(10.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .child(div().flex_1())
                    .child(
                        ui_button("minimal-terminal-tab", ButtonKind::Ghost, colors)
                            .when(terminal_active, |button| {
                                button
                                    .bg(rgb(colors.selected_surface))
                                    .text_color(rgb(colors.text))
                            })
                            .child(icon(
                                TERMINAL_ICON,
                                15.0,
                                if terminal_active {
                                    colors.text
                                } else {
                                    colors.muted
                                },
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_minimal_terminal(window, cx)
                            })),
                    )
                    .child(
                        ui_button("minimal-theme", ButtonKind::Ghost, colors)
                            .when(settings_open, |button| {
                                button
                                    .bg(rgb(colors.selected_surface))
                                    .text_color(rgb(colors.text))
                            })
                            .child("Settings")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_minimal_theme_popup(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The project's front: its name, its actions, and every session card at
    /// reading size. This is where "project, then session" starts.
    fn render_minimal_home(
        &mut self,
        colors: ThemeColors,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let projects = project_cards(&self.model.folders, &self.model.chats);
        let selected_project_id = self.selected_minimal_project_id();
        let selected_project = selected_project_id
            .as_deref()
            .and_then(|selected| projects.iter().find(|project| project.id == selected));
        let selected_project_rename =
            selected_project.map(|project| (project.id.clone(), project.name.clone()));
        let selected_project_delete = selected_project.map(|project| project.id.clone());
        let sessions = selected_project_id
            .as_deref()
            .map(|project_id| project_sessions(project_id, &self.model.chats))
            .unwrap_or_default();
        let resume = resumable_session(self.cached_last_chat().as_deref(), &self.model.chats)
            .and_then(|(project_id, chat_id, agent)| {
                let title = project_sessions(&project_id, &self.model.chats)
                    .into_iter()
                    .find(|session| session.id == chat_id)?
                    .title;
                Some((project_id, chat_id, agent, title))
            });
        let working = sessions.iter().filter(|session| session.working).count();
        let waiting = sessions
            .iter()
            .filter(|session| self.model.is_waiting(&session.id))
            .count();

        let session_rows = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let project_id = selected_project_id.clone().unwrap_or_default();
                let chat_id = session.id.clone();
                let context_chat_id = session.id.clone();
                let context_title = session.title.clone();
                let agent = session.agent;
                let signal = self.session_signal(session);
                let status_color = signal.color(colors);
                div()
                    .id(("minimal-session", index))
                    .relative()
                    .w_full()
                    .h(px(54.0))
                    .px(px(14.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .when(index > 0, |row| {
                        row.border_t_1().border_color(rgb(colors.border))
                    })
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(colors.surface_high)))
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
                            .size(px(28.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.0))
                            .bg(rgb(colors.surface_high))
                            .child(
                                svg()
                                    .path(agent_icon(agent))
                                    .size(px(15.0))
                                    .text_color(rgb(colors.muted)),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(colors.text))
                                    .child(session.title.clone()),
                            )
                            .child(
                                div()
                                    .mt(px(2.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_size(px(12.0))
                                    .text_color(rgb(colors.faint))
                                    .child(agent.label())
                                    .child(div().child("·"))
                                    .child(
                                        svg()
                                            .path(GIT_BRANCH_ICON)
                                            .size(px(12.0))
                                            .flex_none()
                                            .text_color(rgb(colors.faint)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .when(session.branch != "Project directory", |branch| {
                                                branch.font_family(MONO).text_size(px(11.5))
                                            })
                                            .child(session.branch.clone()),
                                    ),
                            ),
                    )
                    .child(if signal == SessionSignal::Idle {
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(rgb(colors.faint))
                            .child("Idle")
                            .into_any_element()
                    } else {
                        stamp(signal.label(), status_color)
                    })
            })
            .collect::<Vec<_>>();

        let summary = if selected_project.is_none() {
            String::new()
        } else {
            let mut parts = vec![format!(
                "{} session{}",
                sessions.len(),
                if sessions.len() == 1 { "" } else { "s" }
            )];
            if working > 0 {
                parts.push(format!("{working} working"));
            }
            if waiting > 0 {
                parts.push(format!("{waiting} waiting on you"));
            }
            parts.join(" · ")
        };

        div()
            .id("minimal-project-content")
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .bg(rgb(colors.background))
            .child(
                div()
                    .w_full()
                    .max_w(px(760.0))
                    .mx_auto()
                    .px(px(32.0))
                    .pt(px(44.0))
                    .pb_8()
                    .when_some(selected_project, |front, project| {
                        front
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(20.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(colors.text))
                                            .child(project.name.clone()),
                                    )
                                    .child(div().flex_1())
                                    .when_some(
                                        selected_project_rename.clone(),
                                        |heading, (folder_id, project_name)| {
                                            heading.child(
                                                ui_button(
                                                    "minimal-rename-project",
                                                    ButtonKind::Ghost,
                                                    colors,
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.begin_sidebar_edit(
                                                            SidebarTarget::Folder(
                                                                folder_id.clone(),
                                                            ),
                                                            project_name.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                                .child("Rename"),
                                            )
                                        },
                                    )
                                    .when_some(
                                        selected_project_delete.clone(),
                                        |heading, folder_id| {
                                            heading.child(
                                                ui_button(
                                                    "minimal-delete-project",
                                                    ButtonKind::Ghost,
                                                    colors,
                                                )
                                                .text_color(rgb(colors.danger))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.begin_sidebar_delete(
                                                            SidebarTarget::Folder(
                                                                folder_id.clone(),
                                                            ),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                                .child(
                                                    svg()
                                                        .path(TRASH_ICON)
                                                        .size(px(14.0))
                                                        .text_color(rgb(colors.danger)),
                                                )
                                                .child("Delete"),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .mt(px(4.0))
                                    .text_size(px(13.0))
                                    .text_color(rgb(colors.faint))
                                    .child(summary.clone()),
                            )
                    })
                    .when(selected_project.is_none(), |front| {
                        front
                            .child(
                                div()
                                    .text_size(px(22.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.text))
                                    .child("Welcome to xd"),
                            )
                            .child(
                                div()
                                    .mt(px(10.0))
                                    .max_w(px(460.0))
                                    .text_size(px(13.5))
                                    .line_height(px(21.0))
                                    .text_color(rgb(colors.muted))
                                    .child(
                                        "Create a workspace, then start a Codex, Claude, JCode, or Copilot session in it. Sessions keep running when you close the window.",
                                    ),
                            )
                            .child(
                                div().mt_5().flex().child(
                                    ui_button("minimal-welcome-project", ButtonKind::Primary, colors)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.begin_workspace_create(window, cx)
                                        }))
                                        .child(plus_icon(colors.accent_text))
                                        .child("New project"),
                                ),
                            )
                    })
                    .when_some(resume, |front, (project_id, chat_id, agent, title)| {
                        front.child(
                            div().mt(px(18.0)).flex().child(
                                ui_button("minimal-resume-session", ButtonKind::Secondary, colors)
                                    .max_w(px(420.0))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_minimal_session(
                                            project_id.clone(),
                                            chat_id.clone(),
                                            agent,
                                            window,
                                            cx,
                                        );
                                    }))
                                    .child(div().text_color(rgb(colors.muted)).child("Continue"))
                                    .child(div().min_w_0().truncate().child(title)),
                            ),
                        )
                    })
                    .when_some(selected_project_id.clone(), |front, folder_id| {
                        front.child(
                            div()
                                .mt(px(32.0))
                                .mb(px(10.0))
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(colors.muted))
                                        .child("Sessions"),
                                )
                                .child(
                                    ui_button("minimal-new-session", ButtonKind::Primary, colors)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.begin_chat_create(folder_id.clone(), window, cx);
                                        }))
                                        .child(plus_icon(colors.accent_text))
                                        .child("New session"),
                                ),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .when(!session_rows.is_empty(), |list| {
                                list.rounded(px(12.0))
                                    .border_1()
                                    .border_color(rgb(colors.border))
                                    .bg(rgb(colors.surface))
                                    .overflow_hidden()
                            })
                            .children(session_rows)
                            .when(
                                selected_project_id.is_some() && sessions.is_empty(),
                                |list| {
                                    list.child(
                                        div()
                                            .w_full()
                                            .p_5()
                                            .rounded(px(12.0))
                                            .border_1()
                                            .border_color(rgb(colors.border))
                                            .text_size(px(13.0))
                                            .text_color(rgb(colors.muted))
                                            .child(
                                                "No sessions yet. Start one with Codex, Claude, JCode, or Copilot.",
                                            ),
                                    )
                                },
                            ),
                    ),
            )
            .into_any_element()
    }
    fn render_native_inline(
        content: &markdown::InlineText,
        colors: ThemeColors,
        index: usize,
    ) -> gpui::AnyElement {
        let highlights = content.spans.iter().map(|span| {
            let style = match span.kind {
                markdown::InlineKind::Strong => HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
                markdown::InlineKind::Emphasis => HighlightStyle {
                    font_style: Some(gpui::FontStyle::Italic),
                    ..Default::default()
                },
                markdown::InlineKind::StrongEmphasis => HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    font_style: Some(gpui::FontStyle::Italic),
                    ..Default::default()
                },
                markdown::InlineKind::Code => HighlightStyle {
                    background_color: Some(rgb(colors.surface_high).into()),
                    ..Default::default()
                },
                markdown::InlineKind::Link => HighlightStyle {
                    color: Some(rgb(colors.accent_ink).into()),
                    underline: Some(gpui::UnderlineStyle {
                        thickness: px(1.0),
                        color: None,
                        wavy: false,
                    }),
                    ..Default::default()
                },
            };
            (span.range.clone(), style)
        });
        let value: SharedString = content.text.clone().into();
        let text = StyledText::new(value.clone()).with_highlights(highlights);
        let layout = text.layout().clone();
        let document = scoped_element_id(&format!("native-inline:{}", content.text), index);
        let (ranges, urls) = content
            .spans
            .iter()
            .filter(|span| span.kind == markdown::InlineKind::Link)
            .filter_map(|span| {
                span.url
                    .as_ref()
                    .map(|url| (span.range.clone(), url.clone()))
            })
            .unzip::<_, _, Vec<_>, Vec<_>>();
        selectable_links_in_document(
            document,
            document,
            value,
            0..content.text.len(),
            layout,
            text,
            ranges,
            move |link, window, cx| {
                if let Some(url) = urls.get(link) {
                    window.dispatch_action(
                        Box::new(OpenBrowserUrl {
                            url: url.clone(),
                            window: window.window_handle(),
                        }),
                        cx,
                    );
                }
            },
        )
        .into_any_element()
    }

    fn render_native_markdown_block(
        colors: ThemeColors,
        block: &markdown::Block,
        index: usize,
    ) -> gpui::AnyElement {
        match block {
            markdown::Block::Heading { level, content } => div()
                .id(("native-heading", index))
                .text_size(px(match level {
                    1 => 22.0,
                    2 => 19.0,
                    _ => 16.0,
                }))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(colors.text))
                .child(Self::render_native_inline(content, colors, index))
                .into_any_element(),
            markdown::Block::Paragraph(content) => div()
                .id(("native-paragraph", index))
                .text_size(px(14.0))
                .line_height(px(23.0))
                .text_color(rgb(colors.text))
                .child(Self::render_native_inline(content, colors, index))
                .into_any_element(),
            markdown::Block::Quote(content) => div()
                .id(("native-quote", index))
                .pl_3()
                .border_l_2()
                .border_color(rgb(colors.border))
                .text_size(px(14.0))
                .line_height(px(23.0))
                .text_color(rgb(colors.muted))
                .child(Self::render_native_inline(content, colors, index))
                .into_any_element(),
            markdown::Block::ListItem {
                number,
                depth,
                content,
            } => div()
                .id(("native-list-item", index))
                .pl(px(12.0 * f32::from(*depth)))
                .flex()
                .items_start()
                .gap_2()
                .text_size(px(14.0))
                .line_height(px(23.0))
                .text_color(rgb(colors.text))
                .child(number.map_or_else(|| "•".into(), |number| format!("{number}.")))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Self::render_native_inline(content, colors, index)),
                )
                .into_any_element(),
            markdown::Block::Rule => div()
                .id(("native-rule", index))
                .h(px(1.0))
                .w_full()
                .my_1()
                .bg(rgb(colors.border))
                .into_any_element(),
            markdown::Block::Code(code) => div()
                .id(("native-code", index))
                .w_full()
                .p_3()
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.surface))
                .font_family(MONO)
                .text_size(px(12.5))
                .line_height(px(19.0))
                .text_color(rgb(colors.text))
                .child(code.code.clone())
                .into_any_element(),
            markdown::Block::Table(table) => div()
                .id(("native-table", index))
                .w_full()
                .p_3()
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.surface))
                .font_family(MONO)
                .text_size(px(12.5))
                .line_height(px(19.0))
                .text_color(rgb(colors.text))
                .child(table.text.clone())
                .into_any_element(),
            markdown::Block::Analysis(blocks) => div()
                .id(("native-analysis", index))
                .w_full()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.surface))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(colors.muted))
                        .child("Analysis"),
                )
                .children(
                    blocks
                        .iter()
                        .enumerate()
                        .map(|(child_index, block)| {
                            let child_index =
                                scoped_element_id(&format!("native-analysis:{index}"), child_index)
                                    as usize;
                            Self::render_native_markdown_block(colors, block, child_index)
                        })
                        .collect::<Vec<_>>(),
                )
                .into_any_element(),
        }
    }

    fn render_native_activity(
        colors: ThemeColors,
        card: ActivityCard,
        index: usize,
        activity_id: i64,
        expanded: bool,
    ) -> gpui::AnyElement {
        let failed = card.kind == ActivityKind::Failure;
        let status_color = match card.kind {
            ActivityKind::Running => colors.working,
            ActivityKind::Success => colors.working,
            ActivityKind::Failure => colors.danger,
            ActivityKind::Finished => colors.muted,
        };
        let detail = card
            .patch
            .or_else(|| (!card.detail.is_empty()).then_some(card.detail));
        let item_rows = card
            .items
            .into_iter()
            .enumerate()
            .map(|(item_index, item)| {
                div()
                    .id(("native-activity-item", item_index))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl(px(20.0))
                    .font_family(MONO)
                    .text_size(px(12.0))
                    .line_height(px(20.0))
                    .text_color(rgb(colors.text))
                    .child(div().min_w_0().flex_1().child(item.name))
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(colors.muted))
                            .child(item.status),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .id(("native-activity", index))
            .w_full()
            .max_w(px(780.0))
            .px(px(14.0))
            .py(px(9.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(rgb(if failed { colors.danger } else { colors.border }))
            .bg(rgb(colors.surface))
            .child(
                div()
                    .id(("native-activity-toggle", index))
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(ToggleActivity { id: activity_id }), cx);
                    })
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(12.0))
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(rgb(colors.faint))
                            .child(icon(
                                if expanded {
                                    CHEVRON_DOWN_ICON
                                } else {
                                    CHEVRON_RIGHT_ICON
                                },
                                12.0,
                                colors.faint,
                            )),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors.text))
                            .child(card.title),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .font_family(MONO)
                            .text_size(px(12.0))
                            .text_color(rgb(colors.muted))
                            .child(card.name),
                    )
                    .child(if failed {
                        stamp(card.status, colors.danger)
                    } else {
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(rgb(status_color))
                            .child(card.status)
                            .into_any_element()
                    }),
            )
            .when_some(detail.filter(|_| expanded), |activity, detail| {
                activity.child(
                    div()
                        .id(("native-activity-detail", index))
                        .w_full()
                        .max_h(px(260.0))
                        .overflow_y_scroll()
                        .p_3()
                        .rounded(px(5.0))
                        .bg(rgb(colors.background))
                        .font_family(MONO)
                        .text_size(px(12.0))
                        .line_height(px(19.0))
                        .text_color(rgb(colors.muted))
                        .child(detail),
                )
            })
            .children(if expanded { item_rows } else { Vec::new() })
            .into_any_element()
    }

    /// Every agent still at work, above the composer: background agents keep
    /// running after the turn and the reader may have scrolled far away from
    /// their cards. A pill jumps to its card and opens it.
    fn render_agents_tray(
        colors: ThemeColors,
        agents: &[RunningAgent],
    ) -> Option<gpui::AnyElement> {
        if agents.is_empty() {
            return None;
        }
        const SHOWN: usize = 4;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let pills = agents
            .iter()
            .take(SHOWN)
            .enumerate()
            .map(|(index, agent)| {
                let reveal = RevealSubagent {
                    row: agent.row,
                    card_id: agent.card_id,
                };
                let elapsed = agent
                    .view
                    .started_at
                    .map(|started| activity::compact_duration(now.saturating_sub(started)));
                div()
                    .id(("agents-tray-pill", index))
                    .h(px(28.0))
                    .px(px(11.0))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.surface))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(colors.surface_high)))
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(reveal.clone()), cx);
                    })
                    .child(
                        div()
                            .size(px(6.0))
                            .flex_none()
                            .rounded_full()
                            .bg(rgb(colors.working))
                            .opacity(pulse_opacity()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(colors.text))
                            .child(agent.view.agent.clone()),
                    )
                    .child(
                        div()
                            .max_w(px(220.0))
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(rgb(colors.muted))
                            .child(if agent.view.activity.is_empty() {
                                agent.view.description.clone()
                            } else {
                                agent.view.activity.clone()
                            }),
                    )
                    .when_some(elapsed, |pill, elapsed| {
                        pill.child(
                            div()
                                .flex_none()
                                .text_size(px(11.0))
                                .text_color(rgb(colors.faint))
                                .child(elapsed),
                        )
                    })
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        Some(
            div()
                .w_full()
                .max_w(px(780.0))
                .mb_2()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(colors.faint))
                        .child(format!(
                            "{} agent{} working",
                            agents.len(),
                            if agents.len() == 1 { "" } else { "s" }
                        )),
                )
                .children(pills)
                .when(agents.len() > SHOWN, |tray| {
                    tray.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(colors.faint))
                            .child(format!("+{} more", agents.len() - SHOWN)),
                    )
                })
                .into_any_element(),
        )
    }

    /// One row of a work log. Rows draw their own slice of the shared frame, so
    /// a run of virtualized rows reads as a single card.
    #[allow(clippy::too_many_arguments)]
    fn render_work_log_row(
        colors: ThemeColors,
        card: ActivityCard,
        run: WorkRun,
        index: usize,
        row_key: i64,
        log_key: i64,
        log_expanded: bool,
        expanded: bool,
    ) -> gpui::AnyElement {
        let head = run.position == 0;
        let tail = run.position + 1 == run.len;
        let failed = card.kind == ActivityKind::Failure;
        let mark = match card.kind {
            ActivityKind::Running => colors.working,
            ActivityKind::Failure => colors.danger,
            _ => colors.faint,
        };
        let detail = card
            .patch
            .clone()
            .or_else(|| (!card.detail.is_empty()).then(|| card.detail.clone()));
        let header = head.then(|| {
            let hidden = run.hidden_count(log_expanded);
            let rows = run.start..run.start + run.len;
            div()
                .id(("work-log-header", index))
                .w_full()
                .px(px(14.0))
                .pt(px(10.0))
                .pb(px(4.0))
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(colors.faint))
                        .child(format!("Work log · {} steps", run.len)),
                )
                .when(run.len > WORK_LOG_VISIBLE, |header| {
                    header.child(
                        div()
                            .id(("work-log-toggle", index))
                            .cursor_pointer()
                            .text_color(rgb(colors.accent_ink))
                            .hover(|style| style.text_color(rgb(colors.text)))
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(
                                    Box::new(ToggleWorkLog {
                                        id: log_key,
                                        rows: rows.clone(),
                                    }),
                                    cx,
                                );
                            })
                            .child(if hidden > 0 {
                                format!("Show {hidden} earlier")
                            } else {
                                "Show fewer".to_owned()
                            }),
                    )
                })
        });
        let entry = (!head || run.head_shows_entry(log_expanded)).then(|| {
            div()
                .id(("work-log-entry", index))
                .w_full()
                .px(px(14.0))
                .py(px(2.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .when(detail.is_some() || !card.items.is_empty(), |row| {
                    row.cursor_pointer()
                        .hover(|style| style.bg(rgb(colors.surface_high)))
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(Box::new(ToggleActivity { id: row_key }), cx);
                        })
                })
                .child(
                    div()
                        .size(px(5.0))
                        .flex_none()
                        .rounded_full()
                        .bg(rgb(mark))
                        .when(card.kind == ActivityKind::Running, |dot| {
                            dot.opacity(pulse_opacity())
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .font_family(MONO)
                        .text_size(px(12.0))
                        .line_height(px(20.0))
                        .text_color(rgb(if failed { colors.danger } else { colors.muted }))
                        .child(card.name.clone()),
                )
                .when(card.kind == ActivityKind::Running || failed, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(rgb(mark))
                            .child(card.status.clone()),
                    )
                })
        });
        let opened = expanded && entry.is_some();
        div()
            .w_full()
            .max_w(px(780.0))
            .when(tail, |row| row.pb(px(8.0)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .bg(rgb(colors.surface))
                    .border_color(rgb(colors.border))
                    .border_l_1()
                    .border_r_1()
                    .when(head, |frame| frame.border_t_1().rounded_t(px(12.0)))
                    .when(tail, |frame| {
                        frame.border_b_1().rounded_b(px(12.0)).pb(px(8.0))
                    })
                    .children(header)
                    .children(entry)
                    .when_some(detail.filter(|_| opened), |frame, detail| {
                        frame.child(
                            div()
                                .id(("work-log-detail", index))
                                .mx_3()
                                .my_1()
                                .max_h(px(260.0))
                                .overflow_y_scroll()
                                .p_3()
                                .rounded(px(5.0))
                                .bg(rgb(colors.background))
                                .font_family(MONO)
                                .text_size(px(12.0))
                                .line_height(px(19.0))
                                .text_color(rgb(colors.muted))
                                .child(detail),
                        )
                    })
                    .when(opened, |frame| {
                        frame.children(card.items.into_iter().enumerate().map(
                            |(item_index, item)| {
                                div()
                                    .id(("work-log-item", item_index))
                                    .px_3()
                                    .pl(px(28.0))
                                    .flex()
                                    .gap_2()
                                    .font_family(MONO)
                                    .text_size(px(12.0))
                                    .line_height(px(20.0))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .text_color(rgb(colors.text))
                                            .child(item.name),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(rgb(colors.muted))
                                            .child(item.status),
                                    )
                            },
                        ))
                    }),
            )
            .into_any_element()
    }

    /// One delegated agent: who it is, what it is doing right now, and, once
    /// opened, its task, every step it took, and its report.
    fn render_native_subagent(
        colors: ThemeColors,
        view: SubagentView,
        pending: Option<&'static str>,
        index: usize,
        card_id: i64,
        expanded: bool,
    ) -> gpui::AnyElement {
        let running = view.status == SubagentState::Running;
        let signal = match view.status {
            SubagentState::Running => colors.working,
            SubagentState::Completed => colors.faint,
            SubagentState::Failed => colors.danger,
            SubagentState::Stopped => colors.muted,
        };
        let provider_icon = if view.agent == "Shell" {
            Some(TERMINAL_ICON)
        } else {
            match view.provider.to_ascii_lowercase().as_str() {
                "claude" => Some(CLAUDE_ICON),
                "codex" => Some(CODEX_ICON),
                _ => None,
            }
        };
        let stats = view.stats_at(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as u64)
                .unwrap_or(0),
        );
        let preview = if running {
            view.activity.clone()
        } else {
            first_line(if view.summary.is_empty() {
                &view.latest
            } else {
                &view.summary
            })
        };
        let section = |label: String| {
            div()
                .pt(px(6.0))
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(colors.faint))
                .child(label)
        };
        let step_row = |step_index: usize, tool: String, text: String| {
            div()
                .id(("native-subagent-step", step_index))
                .w_full()
                .flex()
                .gap_2()
                .font_family(MONO)
                .text_size(px(11.5))
                .line_height(px(18.0))
                .child(
                    div()
                        .w(px(52.0))
                        .flex_none()
                        .truncate()
                        .text_color(rgb(colors.faint))
                        .child(tool),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_color(rgb(colors.muted))
                        .child(text),
                )
                .into_any_element()
        };
        // While it works, the newest few steps show without opening the card.
        let recent_steps = if running && !expanded {
            let skip = view.steps.len().saturating_sub(3);
            view.steps
                .iter()
                .enumerate()
                .skip(skip)
                .map(|(step_index, step)| {
                    step_row(step_index, step.tool.clone(), step.text.clone())
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let header = div()
            .id(("native-subagent-toggle", index))
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(ToggleActivity { id: card_id }), cx);
            })
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .size(px(13.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(provider_icon, |slot, path| {
                        slot.child(icon(path, 13.0, colors.muted))
                    })
                    .when(provider_icon.is_none(), |slot| {
                        slot.child(div().size(px(7.0)).rounded_full().bg(rgb(signal)))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(colors.text))
                    .child(view.agent.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(13.0))
                    .text_color(rgb(colors.muted))
                    .child(view.description.clone()),
            )
            .when(view.background, |header| {
                header.child(stamp("Background", colors.faint))
            })
            .when_some(pending.filter(|_| running), |header, label| {
                header.child(
                    div()
                        .flex_none()
                        .text_size(px(12.0))
                        .text_color(rgb(colors.muted))
                        .child(format!("{label}…")),
                )
            })
            .when(pending.is_none() && view.can_stop(), |header| {
                let action =
                    |id: &'static str, label: &'static str, verb: &'static str, tone: u32| {
                        let key = view.key.clone();
                        div()
                            .id((id, index))
                            .flex_none()
                            .h(px(22.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(tone))
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                window.dispatch_action(
                                    Box::new(ControlSubagent {
                                        key: key.clone(),
                                        action: verb.to_owned(),
                                    }),
                                    cx,
                                );
                            })
                            .child(label)
                    };
                header
                    .when(view.can_background(), |header| {
                        header.child(action(
                            "native-subagent-background",
                            "Background",
                            "background",
                            colors.muted,
                        ))
                    })
                    .child(action(
                        "native-subagent-stop",
                        "Stop",
                        "stop",
                        colors.danger,
                    ))
            })
            .child(match view.status {
                SubagentState::Running => div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(12.0))
                    .text_color(rgb(colors.muted))
                    .child(
                        div()
                            .size(px(7.0))
                            .rounded_full()
                            .bg(rgb(colors.working))
                            .opacity(pulse_opacity()),
                    )
                    .child(view.status.label())
                    .into_any_element(),
                SubagentState::Failed => stamp(view.status.label(), colors.danger),
                _ => div()
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(rgb(colors.faint))
                    .child(view.status.label())
                    .into_any_element(),
            })
            .child(icon(
                if expanded {
                    CHEVRON_DOWN_ICON
                } else {
                    CHEVRON_RIGHT_ICON
                },
                12.0,
                colors.faint,
            ));
        let glance = (!preview.is_empty() || !stats.is_empty()).then(|| {
            div()
                .w_full()
                .pl(px(21.0))
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(rgb(colors.muted))
                        .child(preview),
                )
                .when(!view.model.is_empty(), |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(rgb(colors.faint))
                            .child(view.model.clone()),
                    )
                })
                .when(!stats.is_empty(), |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(rgb(colors.faint))
                            .child(stats),
                    )
                })
        });
        let mut details = Vec::new();
        if expanded {
            if !view.prompt.is_empty() {
                details.push(section("Task".into()).into_any_element());
                details.push(
                    div()
                        .id(("native-subagent-prompt", index))
                        .w_full()
                        .max_h(px(160.0))
                        .overflow_y_scroll()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(rgb(colors.muted))
                        .child(view.prompt.clone())
                        .into_any_element(),
                );
            }
            if !view.steps.is_empty() {
                let earlier = view.dropped_steps();
                details.push(
                    section(format!("Steps · {}", view.steps.len() as u64 + earlier))
                        .into_any_element(),
                );
                details.push(
                    div()
                        .id(("native-subagent-steps", index))
                        .w_full()
                        .max_h(px(240.0))
                        .overflow_y_scroll()
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .bg(rgb(colors.background))
                        .flex()
                        .flex_col()
                        .when(earlier > 0, |steps| {
                            steps.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(colors.faint))
                                    .child(format!("{earlier} earlier steps not kept")),
                            )
                        })
                        .children(view.steps.iter().enumerate().map(|(step_index, step)| {
                            step_row(step_index, step.tool.clone(), step.text.clone())
                        }))
                        .into_any_element(),
                );
            }
            let (report_label, report) = if !view.summary.is_empty() {
                ("Report", view.summary.as_str())
            } else {
                ("Latest message", view.latest.as_str())
            };
            if !report.is_empty() {
                let document = markdown::parse_assistant(report);
                let scope = format!("native-subagent-report:{card_id}");
                details.push(section(report_label.into()).into_any_element());
                details.push(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(
                            document
                                .blocks
                                .iter()
                                .enumerate()
                                .map(|(block_index, block)| {
                                    Self::render_native_markdown_block(
                                        colors,
                                        block,
                                        scoped_element_id(&scope, block_index) as usize,
                                    )
                                }),
                        )
                        .into_any_element(),
                );
            }
        }
        div()
            .id(("native-subagent", index))
            .w_full()
            .max_w(px(780.0))
            .flex()
            .rounded(px(12.0))
            .border_1()
            .border_color(rgb(if view.status == SubagentState::Failed {
                colors.danger
            } else {
                colors.border
            }))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .px(px(14.0))
                    .py(px(10.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(header)
                    .children(glance)
                    .when(!recent_steps.is_empty(), |card| {
                        card.child(
                            div()
                                .pl(px(21.0))
                                .pt(px(2.0))
                                .flex()
                                .flex_col()
                                .children(recent_steps),
                        )
                    })
                    .when(!details.is_empty(), |card| {
                        card.child(
                            div()
                                .pl(px(21.0))
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .children(details),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_native_message(
        colors: ThemeColors,
        message: &Message,
        activity: Option<ActivityCard>,
        subagent: Option<(SubagentView, Option<&'static str>)>,
        index: usize,
        expanded: bool,
    ) -> gpui::AnyElement {
        let role = message.role.as_str();
        if role == "duration" {
            let label = message
                .content
                .parse::<u64>()
                .map(|seconds| {
                    if seconds < 60 {
                        format!("Worked for {seconds}s")
                    } else {
                        format!("Worked for {}", activity::compact_duration(seconds * 1_000))
                    }
                })
                .unwrap_or_else(|_| "Finished".into());
            let rule = || div().h(px(1.0)).flex_1().bg(rgb(colors.border));
            return div()
                .id(("native-duration", index))
                .w_full()
                .max_w(px(780.0))
                .pt_1()
                .pb(px(14.0))
                .flex()
                .items_center()
                .gap_3()
                .child(rule())
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(rgb(colors.faint))
                        .child(label),
                )
                .child(rule())
                .into_any_element();
        }

        if let Some((subagent, pending)) = subagent {
            return div()
                .w_full()
                .max_w(px(780.0))
                .pb(px(8.0))
                .child(Self::render_native_subagent(
                    colors,
                    subagent,
                    pending,
                    index,
                    message.id.unwrap_or(-(index as i64) - 1),
                    expanded,
                ))
                .into_any_element();
        }

        if let Some(activity) = activity {
            return div()
                .w_full()
                .max_w(px(780.0))
                .pb(px(6.0))
                .child(Self::render_native_activity(
                    colors,
                    activity,
                    index,
                    message.id.unwrap_or(-(index as i64) - 1),
                    expanded,
                ))
                .into_any_element();
        }

        let user = role == "user";
        let error = role == "error";
        let markdown = message.markdown();
        let block_scope = format!("native-message:{}", message.id.unwrap_or(index as i64));
        let body = div().w_full().flex().flex_col().gap_2().children(
            markdown
                .blocks
                .iter()
                .enumerate()
                .map(|(block_index, block)| {
                    Self::render_native_markdown_block(
                        colors,
                        block,
                        scoped_element_id(&block_scope, block_index) as usize,
                    )
                })
                .collect::<Vec<_>>(),
        );
        // Prose sits on the page; only what the user wrote and host errors are
        // set apart, the way a conversation reads.
        let card = div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .when(user, |card| {
                card.max_w(px(600.0))
                    .px(px(14.0))
                    .py(px(9.0))
                    .rounded(px(18.0))
                    .bg(rgb(colors.surface_high))
            })
            .when(error, |card| {
                card.max_w(px(760.0))
                    .px_4()
                    .py_3()
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(rgb(colors.danger))
                    .bg(rgb(colors.danger_surface))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors.danger))
                            .child("Host"),
                    )
            })
            .when(!user && !error, |card| card.w_full().max_w(px(780.0)))
            .child(body);
        let copy = (!error && !message.content.trim().is_empty()).then(|| {
            let text = message.content.clone();
            div()
                .id(("native-copy-message", index))
                .invisible()
                .group_hover("native-message", |style| style.visible())
                .mt(px(4.0))
                .px(px(6.0))
                .h(px(20.0))
                .flex()
                .items_center()
                .rounded(px(6.0))
                .cursor_pointer()
                .text_size(px(11.0))
                .text_color(rgb(colors.faint))
                .hover(|style| {
                    style
                        .bg(rgb(colors.surface_high))
                        .text_color(rgb(colors.text))
                })
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                })
                .child("Copy")
        });
        div()
            .id(("native-message", index))
            .group("native-message")
            .w_full()
            .flex()
            .flex_col()
            .when(user, |row| row.items_end())
            .child(card)
            .children(copy)
            .child(div().h(px(if user { 10.0 } else { 6.0 })))
            .into_any_element()
    }

    fn control_subagent(&mut self, key: String, action: &str, cx: &mut Context<Self>) {
        let label = match action {
            "stop" => "Stopping",
            "background" => "Moving to background",
            _ => return,
        };
        if let (Some(chat), Some(host)) = (self.model.selected_chat.as_deref(), self.active_host())
        {
            match host.control_subagent(chat, &key, action) {
                Ok(()) => {
                    // The card reads this map when it draws, so it shows the
                    // pending action at once.
                    self.subagent_actions.insert(key, label);
                }
                Err(error) => self.model.connection_error = Some(error),
            }
        }
        cx.notify();
    }

    fn respond_interaction(&mut self, request_id: String, response: Value, cx: &mut Context<Self>) {
        if let (Some(chat), Some(host)) = (self.model.selected_chat.as_deref(), self.active_host())
        {
            if let Err(error) = host.respond(chat, &request_id, response) {
                self.model.connection_error = Some(error);
            }
        }
        cx.notify();
    }

    fn answer_interaction(
        &mut self,
        request_id: String,
        question_id: String,
        answer: String,
        cx: &mut Context<Self>,
    ) {
        if answer.trim().is_empty() {
            return;
        }
        self.interaction_answers
            .insert(format!("{request_id}/{question_id}"), answer);
        let Some(pending) = self
            .model
            .interactions
            .iter()
            .find(|pending| pending["request_id"].as_str() == Some(&request_id))
        else {
            return;
        };
        let mut answers = serde_json::Map::new();
        let questions = pending["params"]["questions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for question in questions {
            let Some(id) = question["id"].as_str() else {
                return;
            };
            let Some(answer) = self.interaction_answers.get(&format!("{request_id}/{id}")) else {
                self.question_answer.clear();
                self.question_input
                    .update(cx, |input, cx| input.set_text("", cx));
                cx.notify();
                return;
            };
            answers.insert(id.to_owned(), serde_json::json!({"answers":[answer]}));
        }
        self.respond_interaction(request_id, serde_json::json!({"answers":answers}), cx);
    }

    fn render_interaction(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let pending = self.model.interactions.first()?.clone();
        let request_id = pending["request_id"].as_str()?.to_owned();
        let method = pending["method"].as_str().unwrap_or_default();
        let params = &pending["params"];
        let mut card = div()
            .w_full()
            // Long patch lines scroll inside the preview; they must not
            // widen the card past the column.
            .min_w_0()
            .overflow_hidden()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .rounded(px(8.0))
            .border(px(1.5))
            .border_color(rgb(colors.waiting))
            .bg(rgb(colors.surface))
            .text_size(px(13.0))
            .text_color(rgb(colors.text));
        if method == "item/tool/requestUserInput" {
            let question = params["questions"]
                .as_array()?
                .iter()
                .find(|q| {
                    !self.interaction_answers.contains_key(&format!(
                        "{request_id}/{}",
                        q["id"].as_str().unwrap_or_default()
                    ))
                })?
                .clone();
            let question_id = question["id"].as_str()?.to_owned();
            card = card.child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(
                                question["question"]
                                    .as_str()
                                    .unwrap_or("Your input is needed")
                                    .to_owned(),
                            ),
                    )
                    .child(stamp("Waiting on you", colors.waiting)),
            );
            let buttons = question["options"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, option)| {
                    let label = option["label"].as_str().unwrap_or_default().to_owned();
                    let answer = label.clone();
                    let id = request_id.clone();
                    let question = question_id.clone();
                    ui_button(("interaction-answer", index), ButtonKind::Secondary, colors)
                        .justify_start()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.answer_interaction(
                                id.clone(),
                                question.clone(),
                                answer.clone(),
                                cx,
                            )
                        }))
                        .child(label)
                })
                .collect::<Vec<_>>();
            card = card
                .child(div().flex().flex_wrap().gap_2().children(buttons))
                .child(div().h(px(40.0)).child(if question["isSecret"] == true {
                    self.secret_question_input.clone()
                } else {
                    self.question_input.clone()
                }));
            card = card.child(
                div().flex().child(
                    ui_button("interaction-submit", ButtonKind::Primary, colors)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.answer_interaction(
                                request_id.clone(),
                                question_id.clone(),
                                this.question_answer.clone(),
                                cx,
                            )
                        }))
                        .child("Submit answer"),
                ),
            );
        } else {
            let command = params.get("command").and_then(Value::as_str);
            let reason = params
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.trim().is_empty() && Some(*reason) != command);
            let detail = command
                .or(reason)
                .unwrap_or("The agent requests permission to continue");
            let tool = params
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let title = approval_title(method, tool);
            let waiting = self.model.interactions.len();
            let patch = params
                .get("patch")
                .and_then(Value::as_str)
                .filter(|patch| !patch.trim().is_empty())
                .map(|patch| {
                    patch
                        .lines()
                        .take(400)
                        .enumerate()
                        .map(|(index, line)| {
                            let background = if line.starts_with("+++") || line.starts_with("---") {
                                None
                            } else if line.starts_with('+') {
                                Some(colors.diff_added)
                            } else if line.starts_with('-') {
                                Some(colors.diff_removed)
                            } else {
                                None
                            };
                            div()
                                .id(("interaction-patch-line", index))
                                .px_2()
                                .whitespace_nowrap()
                                .text_color(rgb(if line.starts_with("@@") {
                                    colors.accent_ink
                                } else if background.is_some() {
                                    colors.text
                                } else {
                                    colors.muted
                                }))
                                .when_some(background, |row, background| row.bg(rgb(background)))
                                .child(if line.is_empty() {
                                    " ".to_owned()
                                } else {
                                    line.to_owned()
                                })
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                });
            card = card
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .when(waiting > 1, |row| {
                            row.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(colors.faint))
                                    .child(format!("1 of {waiting}")),
                            )
                        })
                        .child(stamp("Waiting on you", colors.waiting)),
                )
                .when_some(reason.filter(|_| command.is_some()), |card, reason| {
                    card.child(
                        div()
                            .id("interaction-reason")
                            .max_h(px(220.0))
                            .overflow_y_scroll()
                            .text_size(px(12.5))
                            .line_height(px(19.0))
                            .text_color(rgb(colors.muted))
                            .child(reason.to_owned()),
                    )
                })
                .child(
                    div()
                        .id("interaction-detail")
                        .w_full()
                        .min_w_0()
                        .max_h(px(140.0))
                        .overflow_y_scroll()
                        .px_3()
                        .py_2()
                        .rounded(px(5.0))
                        .bg(rgb(colors.background))
                        .font_family(MONO)
                        .text_size(px(12.5))
                        .child(detail.to_owned()),
                )
                .when_some(patch, |card, lines| {
                    card.child(
                        div()
                            .id("interaction-patch")
                            .w_full()
                            .min_w_0()
                            .max_h(px(260.0))
                            .overflow_scroll()
                            .py_1()
                            .rounded(px(5.0))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.background))
                            .font_family(MONO)
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .children(lines),
                    )
                });
            if let Some(permissions) = params.get("permissions") {
                card = card.child(
                    div()
                        .font_family(MONO)
                        .text_size(px(11.5))
                        .text_color(rgb(colors.muted))
                        .child(permissions.to_string()),
                );
            }
            card = card.child(
                div().flex().gap_2().children(
                    [
                        ("Allow once", "accept"),
                        ("Allow for session", "acceptForSession"),
                        ("Deny", "decline"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, decision))| {
                        let id = request_id.clone();
                        let kind = match decision {
                            "accept" => ButtonKind::Primary,
                            "decline" => ButtonKind::Danger,
                            _ => ButtonKind::Secondary,
                        };
                        ui_button(("interaction-decision", index), kind, colors)
                            .when(decision == "decline", |button| button.ml_4())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond_interaction(
                                    id.clone(),
                                    serde_json::json!({"decision":decision}),
                                    cx,
                                )
                            }))
                            .child(label)
                    })
                    .collect::<Vec<_>>(),
                ),
            );
        }
        Some(card.into_any_element())
    }

    fn render_native_chat(
        &mut self,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if !self.transcript_scroll_handler_attached {
            self.transcript_scroll_handler_attached = true;
            let desktop = cx.entity();
            self.transcript
                .set_scroll_handler(move |event, _window, cx| {
                    let near_start = event.visible_range.start <= 8;
                    let near_end = event.visible_range.end.saturating_add(8) >= event.count;
                    let visible = event.visible_range.clone();
                    let _ = desktop.update(cx, |this, cx| {
                        if this.transcript_visible != visible {
                            this.transcript_visible = visible;
                            cx.notify();
                        }
                        if near_start && this.transcript_has_older {
                            this.request_older_messages();
                        } else if near_end {
                            this.request_newer_messages();
                        }
                    });
                });
        }
        let expanded_activities = self.expanded_activities.clone();
        let expanded_work_logs = self.expanded_work_logs.clone();
        let subagent_actions = self.subagent_actions.clone();
        let messages = self.transcript_snapshot.clone();
        let transcript_empty = messages.get(0).is_none();
        let transcript_loading = self.transcript_loading;
        let workflow_statuses = self.workflow_statuses.clone();
        let workflow_pending = self.workflow_pending.clone();
        let column = self.chat_column_width;
        let transcript = if transcript_loading {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.0))
                .text_color(rgb(colors.muted))
                .child("Loading transcript…")
                .into_any_element()
        } else if transcript_empty {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.0))
                .text_color(rgb(colors.muted))
                .child(if self.transcript_loaded {
                    "Start this session with a message."
                } else {
                    "Conversation did not load. Select the session to retry."
                })
                .into_any_element()
        } else {
            list(self.transcript.clone(), move |index, _window, _cx| {
                // The list's count follows the snapshot; should they ever
                // disagree for a frame, draw nothing rather than crash.
                let Some(message) = messages.get(index) else {
                    return div().into_any_element();
                };
                let (subagent, activity) = match message.tool_view() {
                    Some(ToolView::Subagent(view)) => (Some(view.clone()), None),
                    Some(ToolView::Activity(card)) => (
                        None,
                        Some(card.clone().with_workflow_status(
                            workflow_statuses.get(&message.content),
                            workflow_pending.contains(&message.content),
                        )),
                    ),
                    None => (None, None),
                };
                if let (Some(run), Some(card)) = (messages.work_run(index), activity.clone()) {
                    let log_key = messages.row_key(run.start);
                    let log_expanded = expanded_work_logs.contains(&log_key);
                    if run.hides(log_expanded) {
                        return div().w_full().into_any_element();
                    }
                    return div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(div().w(px(column)).child(Self::render_work_log_row(
                            colors,
                            card,
                            run,
                            index,
                            messages.row_key(index),
                            log_key,
                            log_expanded,
                            expanded_activities.contains(&messages.row_key(index)),
                        )))
                        .into_any_element();
                }
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(div().w(px(column)).child(Self::render_native_message(
                        colors,
                        message,
                        activity,
                        subagent.map(|view| {
                            let pending = subagent_actions.get(view.key.as_str()).copied();
                            (view, pending)
                        }),
                        index,
                        expanded_activities.contains(&message.id.unwrap_or(-(index as i64) - 1)),
                    )))
                    .into_any_element()
            })
            .size_full()
            .into_any_element()
        };
        let queued = self
            .model
            .queue
            .clone()
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                div()
                    .id(("native-queued", index))
                    .w_full()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.surface))
                    .child(
                        div()
                            .flex_none()
                            .w(px(10.0))
                            .h(px(5.0))
                            .rounded(px(1.0))
                            .border_1()
                            .border_color(rgb(colors.working)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors.muted))
                            .child(format!("Queued {}", index + 1)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_sm()
                            .text_color(rgb(colors.text))
                            .child(text),
                    )
                    .child(
                        div()
                            .id(("native-drop-queued", index))
                            .size(px(26.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_sm()
                            .text_color(rgb(colors.muted))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.background)))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.drop_queued(index, cx)),
                            )
                            .child(icon(CLOSE_ICON, 13.0, colors.muted)),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        // How much of the model's context the conversation fills, as the agent
        // last reported it.
        let context_meter = (self.model.context_window > 0).then(|| {
            let used = self.model.context_used.min(self.model.context_window);
            let share = used as f32 / self.model.context_window as f32;
            let tone = if share >= 0.9 {
                colors.danger
            } else if share >= 0.75 {
                colors.waiting
            } else {
                colors.faint
            };
            div()
                .id("native-context-meter")
                .flex_none()
                .mr_2()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .w(px(36.0))
                        .h(px(4.0))
                        .rounded(px(2.0))
                        .bg(rgb(colors.border))
                        .child(
                            div()
                                .h_full()
                                .w(px((36.0 * share).max(2.0)))
                                .rounded(px(2.0))
                                .bg(rgb(tone)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(tone))
                        .child(format!(
                            "{} / {}",
                            activity::compact_count(used),
                            activity::compact_count(self.model.context_window)
                        )),
                )
        });
        let composer_focus = self.composer_input.read(cx).focus_handle(cx);
        let can_send = self.model.connected
            && !self.sending
            && (!self.composer.trim().is_empty() || !self.model.draft_attachments.is_empty());
        let can_queue = self.model.connected
            && !self.composer.trim().is_empty()
            && self.model.draft_attachments.is_empty();
        let working = self.model.working;
        let agent = AgentCli::from_backend(&self.model.backend);

        let attachments = self
            .model
            .draft_attachments
            .clone()
            .into_iter()
            .enumerate()
            .map(|(index, attachment)| {
                let name = attachment.name.clone();
                div()
                    .id(("draft-image", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        gpui::img(attachment.preview)
                            .w(px(96.0))
                            .h(px(72.0))
                            .object_fit(gpui::ObjectFit::Contain),
                    )
                    .child(div().text_xs().text_color(rgb(colors.muted)).child(name))
                    .child(
                        div()
                            .id(("remove-draft-image", index))
                            .text_xs()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if index < this.model.draft_attachments.len() {
                                    this.model.draft_attachments.remove(index);
                                    this.attachments_dirty = true;
                                    this.attachment_generation =
                                        this.attachment_generation.saturating_add(1);
                                    this.schedule_draft_sync(cx);
                                }
                                cx.notify();
                            }))
                            .child("Remove"),
                    )
            })
            .collect::<Vec<_>>();
        let interaction = self.render_interaction(colors, cx);
        let agents_tray =
            Self::render_agents_tray(colors, &self.transcript_snapshot.running_agents);
        let options = [
            (
                "model",
                self.model
                    .model
                    .clone()
                    .unwrap_or_else(|| "Default model".to_owned()),
            ),
            ("effort", format!("{} effort", self.model.effort)),
            ("access", self.model.access.clone()),
            (
                "plan",
                if self.model.plan { "Plan" } else { "Build" }.to_owned(),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (option, label))| {
            ui_button(("native-option", index), ButtonKind::Ghost, colors)
                .h(px(28.0))
                .px(px(8.0))
                .gap(px(5.0))
                .text_size(px(12.0))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.native_option_open = Some(option);
                    this.focus_minimal_popup(this.minimal_popup_focus.clone(), window, cx);
                    cx.notify();
                }))
                .when(option == "model", |chip| {
                    chip.children(agent.map(|agent| icon(agent_icon(agent), 13.0, colors.muted)))
                })
                .child(label)
                .child(icon(CHEVRON_DOWN_ICON, 11.0, colors.faint))
        })
        .collect::<Vec<_>>();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .px(px(24.0))
            .bg(rgb(colors.background))
            .child(
                div()
                    .id("native-transcript")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .pt_4()
                    .child(transcript)
                    .when(
                        !self.transcript_pinned() && !transcript_empty && !transcript_loading,
                        |pane| {
                            pane.child(
                                div()
                                    .absolute()
                                    .bottom(px(12.0))
                                    .left_0()
                                    .right_0()
                                    .flex()
                                    .justify_center()
                                    .child(
                                        ui_button(
                                            "native-jump-latest",
                                            ButtonKind::Secondary,
                                            colors,
                                        )
                                        .h(px(28.0))
                                        .rounded(px(14.0))
                                        .text_size(px(12.0))
                                        .shadow_md()
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.jump_to_latest(cx)),
                                        )
                                        .when(working, |button| {
                                            button.child(
                                                div()
                                                    .size(px(6.0))
                                                    .rounded_full()
                                                    .bg(rgb(colors.working)),
                                            )
                                        })
                                        .child(if working {
                                            "New output below"
                                        } else {
                                            "Jump to latest"
                                        })
                                        .child(icon(
                                            CHEVRON_DOWN_ICON,
                                            12.0,
                                            colors.muted,
                                        )),
                                    ),
                            )
                        },
                    ),
            )
            .when(!attachments.is_empty(), |chat| {
                chat.child(
                    div()
                        .w_full()
                        .max_w(px(780.0))
                        .mb_2()
                        .flex()
                        .gap_3()
                        .children(attachments),
                )
            })
            .when_some(interaction, |chat, card| {
                chat.child(div().w_full().max_w(px(780.0)).mb_3().child(card))
            })
            .children(agents_tray)
            .when_some(
                self.open_question
                    .clone()
                    .filter(|_| self.model.interactions.is_empty()),
                |chat, question| {
                    let buttons = question
                        .options
                        .into_iter()
                        .enumerate()
                        .map(|(index, answer)| {
                            let label = answer.clone();
                            ui_button(("tagged-question", index), ButtonKind::Secondary, colors)
                                .justify_start()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.answer_question(answer.clone(), cx)
                                }))
                                .child(label)
                        })
                        .collect::<Vec<_>>();
                    chat.child(
                        div()
                            .w_full()
                            .max_w(px(780.0))
                            .mb_3()
                            .px_4()
                            .py_3()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .rounded(px(8.0))
                            .border(px(1.5))
                            .border_color(rgb(colors.waiting))
                            .bg(rgb(colors.surface))
                            .text_size(px(13.0))
                            .text_color(rgb(colors.text))
                            .child(
                                div()
                                    .flex()
                                    .items_start()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex_1()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(question.question),
                                    )
                                    .child(stamp("Waiting on you", colors.waiting)),
                            )
                            .child(div().flex().flex_wrap().gap_2().children(buttons))
                            .when(question.accepts_input, |card| {
                                card.child(div().h(px(40.0)).child(self.question_input.clone()))
                                    .child(
                                        div().flex().child(
                                            ui_button(
                                                "tagged-question-submit",
                                                ButtonKind::Primary,
                                                colors,
                                            )
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.send_question_input(cx)
                                            }))
                                            .child("Submit answer"),
                                        ),
                                    )
                            }),
                    )
                },
            )
            .when(working, |chat| {
                chat.child(
                    div()
                        .w_full()
                        .max_w(px(780.0))
                        .mb_2()
                        .px(px(2.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(12.5))
                        .text_color(rgb(colors.muted))
                        .child(
                            div()
                                .size(px(7.0))
                                .rounded_full()
                                .bg(rgb(colors.working))
                                .opacity(pulse_opacity()),
                        )
                        .child("Working")
                        .child(working_dots(0, colors.muted)),
                )
            })
            .when(!queued.is_empty(), |chat| {
                chat.child(
                    div()
                        .id("native-queue")
                        .max_h(px(150.0))
                        .overflow_y_scroll()
                        .w_full()
                        .max_w(px(780.0))
                        .mb_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .children(queued),
                )
            })
            .child(
                div()
                    .id("native-composer-shell")
                    .w_full()
                    .max_w(px(780.0))
                    .mb(px(16.0))
                    .pt(px(10.0))
                    .pb(px(8.0))
                    .px(px(8.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .rounded(px(20.0))
                    .border_1()
                    .border_color(rgb(if composer_focus.is_focused(window) {
                        colors.selected_border
                    } else {
                        colors.border
                    }))
                    .bg(rgb(colors.surface))
                    .shadow_md()
                    .on_click(cx.listener(|this, _, window, cx| {
                        let focus = this.composer_input.read(cx).focus_handle(cx);
                        window.focus(&focus);
                    }))
                    .child(
                        div()
                            .id("native-composer")
                            .min_h(px(48.0))
                            .max_h(px(200.0))
                            .px(px(8.0))
                            .py_1()
                            .text_size(px(14.0))
                            .track_focus(&composer_focus)
                            .child(self.composer_input.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .children(options)
                            .child(div().flex_1())
                            .when_some(context_meter, |controls, meter| controls.child(meter))
                            .when(can_queue, |controls| {
                                controls.child(
                                    ui_button("native-queue-message", ButtonKind::Ghost, colors)
                                        .h(px(28.0))
                                        .px(px(8.0))
                                        .text_size(px(12.0))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_queue {
                                                this.queue_composer(cx)
                                            }
                                        }))
                                        .child("Queue"),
                                )
                            })
                            .when(working, |controls| {
                                controls.child(
                                    div()
                                        .id("native-cancel-turn")
                                        .size(px(32.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(rgb(colors.surface_high))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(colors.danger_surface)))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cancel_turn(cx)),
                                        )
                                        .child(icon(STOP_ICON, 13.0, colors.text)),
                                )
                            })
                            .when(!working || can_send, |controls| {
                                controls.child(
                                    div()
                                        .id("native-send-message")
                                        .size(px(32.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(rgb(if can_send {
                                            colors.accent
                                        } else {
                                            colors.surface_high
                                        }))
                                        .when(can_send, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(colors.accent_hover)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_send {
                                                this.send_composer(cx)
                                            }
                                        }))
                                        .child(icon(
                                            SEND_ICON,
                                            16.0,
                                            if can_send {
                                                colors.accent_text
                                            } else {
                                                colors.faint
                                            },
                                        )),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn show_native_pane(&mut self, pane: NativePane, cx: &mut Context<Self>) {
        self.native_pane = pane;
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        match pane {
            NativePane::Files | NativePane::Diff => {
                self.diff_generation = self.diff_generation.saturating_add(1);
                self.diff_panel = Some(DiffPanel {
                    files_mode: pane == NativePane::Files,
                    loading: true,
                    ..Default::default()
                });
                if let Some(host) = self.active_host() {
                    let result = if pane == NativePane::Files {
                        host.file_browse_list(&chat_id, "", self.diff_generation)
                    } else {
                        host.diff_read(&chat_id, "working-all", None, None, self.diff_generation)
                    };
                    if let Err(error) = result {
                        self.model.connection_error = Some(error);
                    }
                }
                self.refresh_git_status();
            }
            NativePane::Terminal => {
                let mut panel = Self::new_terminal_panel(chat_id.clone());
                panel.workdir = self.model.workdir.clone();
                self.terminal_panel = Some(panel);
                self.refresh_terminal_sessions(cx);
            }
            NativePane::Chat => {
                self.stash_terminal_panel();
            }
        }
        cx.notify();
    }

    fn set_native_option(&mut self, option: &'static str, value: String, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(host) = self.active_host().cloned() else {
            return;
        };
        let result = match option {
            "model" => host.set_model(&chat_id, &value),
            "effort" => host.set_effort(&chat_id, &value),
            "access" => host.set_access(&chat_id, &value),
            _ => host.set_plan(&chat_id, value == "true"),
        };
        if let Err(error) = result {
            self.model.connection_error = Some(error);
        } else {
            self.native_option_open = None;
        }
        cx.notify();
    }

    /// A native session is the chat column plus, when the window has room,
    /// one docked pane beside it: changes, files, or a shell.
    fn render_native_workspace(
        &mut self,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let dock_pane = self.native_pane;
        let available = f32::from(window.viewport_size().width)
            - if self.browser_compacts_sidebar(window) {
                0.0
            } else {
                CATALOG_WIDTH
            }
            - if self.browser_is_open() {
                browser_pane_width(
                    self.settings.browser_width,
                    f32::from(window.viewport_size().width),
                ) + BROWSER_DIVIDER_WIDTH
            } else {
                0.0
            };
        let dock_width = native_dock_width(self.settings.diff_width, available);
        let side_by_side = available - dock_width >= 460.0;
        let pane_content = match dock_pane {
            NativePane::Chat => None,
            NativePane::Terminal => Some(self.render_minimal_terminal(colors, window, cx)),
            NativePane::Files => Some({
                let panel = self.diff_panel.clone().unwrap_or_default();
                let mut rows = Vec::new();
                if !panel.browse_path.is_empty() {
                    rows.push(
                        div()
                            .id("native-files-up")
                            .h(px(28.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(4.0))
                            .text_size(px(12.5))
                            .text_color(rgb(colors.muted))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface_high)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let (Some(chat), Some(panel)) =
                                    (this.model.selected_chat.clone(), &mut this.diff_panel)
                                {
                                    panel.browse_path = Path::new(&panel.browse_path)
                                        .parent()
                                        .map(|p| p.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    panel.loading = true;
                                    panel.file_preview = None;
                                    let path = panel.browse_path.clone();
                                    if let Some(host) = this.active_host() {
                                        let _ = host.file_browse_list(
                                            &chat,
                                            &path,
                                            this.diff_generation,
                                        );
                                    }
                                }
                                cx.notify();
                            }))
                            .child("← Parent directory")
                            .into_any_element(),
                    );
                }
                for (index, entry) in panel.browse_entries.iter().enumerate() {
                    let path = if panel.browse_path.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}/{}", panel.browse_path, entry.name)
                    };
                    let directory = entry.directory;
                    let label = entry.name.clone();
                    let open = panel
                        .file_preview
                        .as_ref()
                        .is_some_and(|preview| preview.path == path);
                    rows.push(
                        div()
                            .id(("native-file", index))
                            .h(px(28.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(4.0))
                            .font_family(MONO)
                            .text_size(px(12.0))
                            .text_color(rgb(if open { colors.text } else { colors.muted }))
                            .when(open, |row| row.bg(rgb(colors.selected_surface)))
                            .cursor_pointer()
                            .hover(|style| {
                                style
                                    .bg(rgb(colors.surface_high))
                                    .text_color(rgb(colors.text))
                            })
                            .child(
                                svg()
                                    .path(if directory { FOLDER_ICON } else { FILE_ICON })
                                    .size(px(13.0))
                                    .flex_none()
                                    .text_color(rgb(colors.faint)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(chat) = this.model.selected_chat.clone() {
                                    if let Some(panel) = &mut this.diff_panel {
                                        panel.file_preview = None;
                                        if directory {
                                            panel.browse_path = path.clone();
                                            panel.loading = true;
                                        } else {
                                            panel.file_loading = true;
                                        }
                                    }
                                    if let Some(host) = this.active_host() {
                                        let result = if directory {
                                            host.file_browse_list(
                                                &chat,
                                                &path,
                                                this.diff_generation,
                                            )
                                        } else {
                                            host.file_browse_read(
                                                &chat,
                                                &path,
                                                this.diff_generation,
                                            )
                                        };
                                        if let Err(error) = result {
                                            this.model.connection_error = Some(error);
                                        }
                                    }
                                }
                                cx.notify();
                            }))
                            .child(label)
                            .into_any_element(),
                    );
                }
                div()
                    .size_full()
                    .flex()
                    .min_h_0()
                    .text_color(rgb(colors.text))
                    .child(
                        div()
                            .id("native-file-list")
                            .w(px(220.0))
                            .flex_none()
                            .h_full()
                            .overflow_y_scroll()
                            .p(px(6.0))
                            .border_r_1()
                            .border_color(rgb(colors.border))
                            .children(rows),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .when(panel.loading || panel.file_loading, |pane| {
                                pane.child(
                                    div()
                                        .p_3()
                                        .text_size(px(12.5))
                                        .text_color(rgb(colors.muted))
                                        .child("Loading…"),
                                )
                            })
                            .when_some(panel.error, |pane, error| {
                                pane.child(
                                    div()
                                        .p_3()
                                        .text_size(px(12.5))
                                        .text_color(rgb(colors.danger))
                                        .child(error),
                                )
                            })
                            .when_some(panel.file_preview, |pane, preview| {
                                pane.child(
                                    div()
                                        .h(px(36.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .px_3()
                                        .border_b_1()
                                        .border_color(rgb(colors.border))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .truncate()
                                                .font_family(MONO)
                                                .text_size(px(12.0))
                                                .text_color(rgb(colors.muted))
                                                .child(preview.path),
                                        )
                                        .child(
                                            ui_button(
                                                "native-save-file",
                                                ButtonKind::Secondary,
                                                colors,
                                            )
                                            .h(px(26.0))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.save_browse_file(cx)
                                            }))
                                            .child(
                                                if preview.saving { "Saving…" } else { "Save" },
                                            ),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .p_2()
                                        .text_size(px(12.5))
                                        .child(self.file_editor.clone()),
                                )
                            }),
                    )
                    .into_any_element()
            }),
            NativePane::Diff => Some({
                let panel = self.diff_panel.clone().unwrap_or_default();
                let lines = if panel.patch.is_empty() {
                    Vec::new()
                } else {
                    panel
                        .patch
                        .lines()
                        .take(4_000)
                        .enumerate()
                        .map(|(index, line)| {
                            let (fg, bg) = if line.starts_with("+++") || line.starts_with("---") {
                                (colors.text, None)
                            } else if line.starts_with('+') {
                                (colors.text, Some(colors.diff_added))
                            } else if line.starts_with('-') {
                                (colors.text, Some(colors.diff_removed))
                            } else if line.starts_with("@@") {
                                (colors.accent_ink, None)
                            } else if line.starts_with("diff ") {
                                (colors.text, Some(colors.surface_high))
                            } else {
                                (colors.muted, None)
                            };
                            div()
                                .id(("native-diff-line", index))
                                .px_3()
                                .h(px(19.0))
                                .whitespace_nowrap()
                                .text_color(rgb(fg))
                                .when_some(bg, |row, bg| row.bg(rgb(bg)))
                                .when(line.starts_with("diff "), |row| {
                                    row.mt_2().font_weight(FontWeight::SEMIBOLD)
                                })
                                .child(if line.is_empty() {
                                    " ".to_owned()
                                } else {
                                    line.to_owned()
                                })
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .text_color(rgb(colors.text))
                    .child(
                        div()
                            .h(px(36.0))
                            .flex_none()
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_b_1()
                            .border_color(rgb(colors.border))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.5))
                                    .text_color(rgb(colors.muted))
                                    .child("Working tree"),
                            )
                            .child(
                                ui_button("native-refresh-diff", ButtonKind::Ghost, colors)
                                    .h(px(26.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_native_pane(NativePane::Diff, cx)
                                    }))
                                    .child("Refresh"),
                            ),
                    )
                    .when(panel.loading, |pane| {
                        pane.child(
                            div()
                                .p_3()
                                .text_size(px(12.5))
                                .text_color(rgb(colors.muted))
                                .child("Loading…"),
                        )
                    })
                    .when_some(panel.error, |pane, error| {
                        pane.child(
                            div()
                                .p_3()
                                .text_size(px(12.5))
                                .text_color(rgb(colors.danger))
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .id("native-diff-output")
                            .flex_1()
                            .min_h_0()
                            .overflow_scroll()
                            .py_2()
                            .font_family(MONO)
                            .text_size(px(12.0))
                            .children(lines)
                            .when(panel.patch.is_empty() && !panel.loading, |output| {
                                output.child(
                                    div()
                                        .px_3()
                                        .font_family(UI_FONT)
                                        .text_size(px(12.5))
                                        .text_color(rgb(colors.muted))
                                        .child("No changes."),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .p_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_t_1()
                            .border_color(rgb(colors.border))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(32.0))
                                    .px_2()
                                    .rounded(px(5.0))
                                    .border_1()
                                    .border_color(rgb(colors.border))
                                    .bg(rgb(colors.background))
                                    .child(self.git_commit_input.clone()),
                            )
                            .child(
                                ui_button("native-commit", ButtonKind::Primary, colors)
                                    .on_click(cx.listener(|this, _, _, cx| this.commit_changes(cx)))
                                    .child("Commit all"),
                            ),
                    )
                    .into_any_element()
            }),
        };
        let chat_width = if dock_pane != NativePane::Chat && side_by_side {
            available - dock_width - 1.0
        } else {
            available
        };
        self.chat_column_width = (chat_width - 48.0).clamp(240.0, 780.0);
        let chat = (dock_pane == NativePane::Chat || side_by_side)
            .then(|| self.render_native_chat(colors, window, cx));
        let dragging = self.dock_drag.is_some();
        div()
            .size_full()
            .min_h_0()
            .flex()
            .when_some(chat, |row, chat| {
                row.child(div().flex_1().min_w_0().h_full().child(chat))
            })
            .when_some(pane_content, |row, pane| {
                row.when(side_by_side, |row| {
                    row.child(
                        div()
                            .id("native-dock-divider")
                            .relative()
                            .w(px(1.0))
                            .h_full()
                            .flex_none()
                            .bg(rgb(if dragging {
                                colors.selected_border
                            } else {
                                colors.border
                            }))
                            .child(
                                div()
                                    .id("native-dock-resize")
                                    .absolute()
                                    .left(px(-4.0))
                                    .w(px(9.0))
                                    .h_full()
                                    .cursor(CursorStyle::ResizeLeftRight)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.dock_drag = Some((
                                                f32::from(event.position.x),
                                                dock_width as u16,
                                            ));
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    )
                })
                .child(
                    div()
                        .id("native-dock")
                        .when(side_by_side, |dock| dock.w(px(dock_width)).flex_none())
                        .when(!side_by_side, |dock| dock.flex_1().min_w_0())
                        .h_full()
                        .min_h_0()
                        .bg(rgb(colors.sidebar))
                        .when(dock_pane == NativePane::Terminal, |dock| dock.p_2())
                        .child(pane),
                )
            })
            .into_any_element()
    }

    fn render_minimal_cli(
        &mut self,
        colors: ThemeColors,
        _project_id: String,
        _chat_id: String,
        _agent: AgentCli,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let toolbar = self.render_minimal_context_toolbar(colors, cx);
        let native = self.native_session();
        let content = if native {
            self.render_native_workspace(colors, window, cx)
        } else {
            self.render_minimal_terminal(colors, window, cx)
        };

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
                    .min_w_0()
                    .min_h_0()
                    .when(!native, |content| content.p_3())
                    .bg(rgb(colors.background))
                    .child(content),
            )
            .into_any_element()
    }

    fn render_minimal_standalone_terminal(
        &mut self,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let terminal = self.render_minimal_terminal(colors, window, cx);
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(colors.background))
            .child(
                div()
                    .h(px(52.0))
                    .flex_none()
                    .px_5()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .text_size(px(15.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(colors.text))
                    .child("Terminal"),
            )
            .child(div().flex_1().min_h_0().p_3().child(terminal))
            .into_any_element()
    }

    /// Hand the current palette to every text input, so carets, selections,
    /// and placeholders follow the theme instead of a fixed dark scheme.
    fn sync_input_colors(&mut self, colors: ThemeColors, cx: &mut Context<Self>) {
        for editor in [
            self.composer_input.clone(),
            self.queue_edit_input.clone(),
            self.file_editor.clone(),
            self.tab_editor.clone(),
        ] {
            editor.update(cx, |editor, cx| editor.set_colors(colors, cx));
        }
        for input in [
            self.catalog_find_input.clone(),
            self.sidebar_edit_input.clone(),
            self.workspace_create_input.clone(),
            self.workspace_repo_input.clone(),
            self.workspace_clone_input.clone(),
            self.chat_create_input.clone(),
            self.workspace_context_input.clone(),
            self.workspace_workdir_input.clone(),
            self.workspace_repo_default_input.clone(),
            self.git_commit_input.clone(),
            self.auth_input.clone(),
            self.secret_name_input.clone(),
            self.secret_value_input.clone(),
            self.device_name_input.clone(),
            self.remote_pairing_input.clone(),
            self.share_host_input.clone(),
            self.question_input.clone(),
            self.secret_question_input.clone(),
            self.source_build_input.clone(),
        ] {
            input.update(cx, |input, cx| input.set_colors(colors, cx));
        }
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
        self.sync_input_colors(colors, cx);
        if self.browser_restore_focus {
            self.browser_restore_focus = false;
            if self.terminal_panel.is_some() {
                let focus = self.terminal_input.read(cx).focus_handle(cx);
                window.focus(&focus);
            } else {
                window.blur();
            }
        }
        let browser_pane = self.sync_browser_pane(window, cx);
        let browser_width = browser_pane_width(
            self.settings.browser_width,
            f32::from(window.viewport_size().width),
        );
        let route = self.minimal_route.clone();
        let active_chat = match &route {
            MinimalRoute::Cli { chat_id, .. } => Some(chat_id.clone()),
            _ => None,
        };
        let catalog = (!self.browser_compacts_sidebar(window))
            .then(|| self.render_catalog(colors, active_chat.as_deref(), cx));
        let content = match route {
            MinimalRoute::Projects { .. } | MinimalRoute::Sessions { .. } => {
                self.render_minimal_home(colors, window, cx)
            }
            MinimalRoute::Terminal => self.render_minimal_standalone_terminal(colors, window, cx),
            MinimalRoute::Cli {
                project_id,
                chat_id,
                agent,
            } => self.render_minimal_cli(colors, project_id, chat_id, agent, window, cx),
        };
        let titlebar = self.render_minimal_titlebar(colors, custom_titlebar, cx);
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
                        .rounded(px(7.0))
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
                                .text_color(rgb(colors.danger))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(colors.danger_surface)))
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
            let menu_row = |id: &'static str, label: &'static str| {
                div()
                    .id(id)
                    .w_full()
                    .h(px(32.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(5.0))
                    .text_size(px(13.0))
                    .text_color(rgb(colors.text))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(colors.surface_high)))
                    .child(label)
            };
            let group_label = |label: &'static str| {
                div()
                    .px_3()
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(colors.faint))
                    .child(label)
            };
            let rows = ThemePreset::ALL
                .into_iter()
                .enumerate()
                .map(|(index, theme)| {
                    let selected = self.settings.theme == theme;
                    let swatch = theme.colors();
                    div()
                        .id(("minimal-theme-choice", index))
                        .flex_1()
                        .h(px(34.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(6.0))
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
                        .text_size(px(13.0))
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(rgb(colors.text))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(colors.surface_high)))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.choose_minimal_theme(theme, cx)),
                        )
                        .child(
                            div()
                                .w(px(20.0))
                                .h(px(14.0))
                                .rounded(px(3.0))
                                .border_1()
                                .border_color(rgb(swatch.border))
                                .bg(rgb(swatch.surface))
                                .child(
                                    div()
                                        .mt(px(4.0))
                                        .mx(px(3.0))
                                        .h(px(1.0))
                                        .bg(rgb(swatch.rule)),
                                ),
                        )
                        .child(theme.label())
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
                        .id("workbench-settings")
                        .bottom(px(56.0))
                        .left(px(10.0))
                        .w(px(300.0))
                        .max_h(px(f32::from(window.viewport_size().height) - 24.0))
                        .overflow_y_scroll()
                        .p(px(6.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(group_label("Appearance"))
                        .child(div().px(px(6.0)).pb(px(4.0)).flex().gap_2().children(rows))
                        .when(self.active_endpoint == ChatEndpoint::Local, |card| {
                            card.child(group_label("Devices"))
                                .child(menu_row("pair-device", "Pair a device").on_click(cx.listener(|this, _, _, cx| {
                                    this.minimal_theme_open = false;
                                    this.share_panel = Some(SharePanel { loading: true, ..Default::default() });
                                    if let Some(host) = &this.host { let _ = host.peer_pairing(); }
                                    cx.notify();
                                })))
                                .child(menu_row("paired-devices", "Paired devices").on_click(cx.listener(|this, _, _, cx| {
                                    this.minimal_theme_open = false;
                                    this.devices_panel = Some(DevicesPanel { loading: true, ..Default::default() });
                                    this.request_devices();
                                    cx.notify();
                                })))
                        })
                        .child(group_label("Updates"))
                        .child(div().px_3().py_1().text_size(px(12.0)).text_color(rgb(colors.muted))
                            .child(self.self_update_panel.as_ref().map(|panel| {
                                if let Some(error) = &panel.error { return error.clone(); }
                                match panel.status.as_ref() {
                                    Some(status) if status.state == "installed" => "Installed. Restart xd to finish updating.".into(),
                                    Some(status) if status.state == "installing" => "Installing update…".into(),
                                    Some(status) if status.state == "checking" => "Checking for updates…".into(),
                                    Some(status) if !status.supported => "This installation requires a manual update.".into(),
                                    Some(status) if status.available => format!("Update available: {}", status.latest.as_deref().unwrap_or("new version")),
                                    Some(status) if status.latest.is_none() => format!("xd {} · Check for updates", status.version),
                                    Some(status) => format!("xd {} · Up to date", status.version),
                                    None => "Connecting to the local update service…".into(),
                                }
                            }).unwrap_or_default()))
                        .when(!self.self_update_panel.as_ref().is_some_and(|panel| panel.busy || panel.status.as_ref().is_some_and(|status| status.state == "installed")), |card| {
                            card.child(menu_row("check-app-update", "Check for updates").on_click(cx.listener(|this, _, _, cx| {
                                this.request_app_update("check", cx);
                            })))
                        })
                        .when(self.self_update_panel.as_ref().is_some_and(|panel| !panel.busy && panel.status.as_ref().is_some_and(|status| status.supported && status.available && status.state != "installed")), |card| {
                            card.child(menu_row("install-app-update", "Install update").on_click(cx.listener(|this, _, _, cx| {
                                this.request_app_update("install", cx);
                            })))
                        })
                        .when(self.self_update_panel.as_ref().is_some_and(|panel| !panel.busy && panel.status.as_ref().is_some_and(|status| status.state == "installed")), |card| {
                            card.child(menu_row("restart-app-update", "Restart to update").on_click(cx.listener(|this, _, _, cx| {
                                if this.flush_draft_before_navigation() {
                                    this.request_app_update("restart", cx);
                                }
                            })))
                        })
                        .child(group_label("Assistants"))
                        .child(menu_row("assistant-accounts", "Assistant accounts").on_click(cx.listener(|this, _, window, cx| {
                            this.minimal_theme_open = false; this.auth_open = true;
                            if let Some(host) = this.active_host() { let _ = host.agent_auth(); let _ = host.agent_clis(); }
                            this.focus_minimal_popup(this.minimal_popup_focus.clone(), window, cx);
                            cx.notify();
                        })))
                        .child(div().mx_2().my(px(6.0)).h(px(1.0)).bg(rgb(colors.border)))
                        .child(
                            div()
                                .id("minimal-all-permissions")
                                .w_full()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_3()
                                .rounded(px(5.0))
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
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(colors.text))
                                                .child("All permissions"),
                                        )
                                        .child(
                                            div()
                                                .mt_1()
                                                .text_size(px(12.0))
                                                .line_height(px(17.0))
                                                .text_color(rgb(colors.muted))
                                                .child("New agent tabs skip approvals and sandboxing when the CLI supports it."),
                                        ),
                                )
                                .child(settings_switch(colors, allow_all_permissions)),
                        ),
                )
                .into_any_element()
        });
        let share_overlay = self.share_panel.clone().map(|panel| {
            let invite = xd_transport::PairingInvite { host: panel.host.clone(), port: panel.port.unwrap_or(0), fingerprint: panel.fingerprint.clone(), code: panel.code.clone() };
            let can_copy = !panel.loading && xd_transport::validate_endpoint(&invite.host, invite.port, &invite.fingerprint).is_ok();
            div().occlude().absolute().inset_0().flex().items_center().justify_center().bg(rgba(colors.backdrop)).child(
                div().w(px(480.0)).p_5().flex().flex_col().gap_3().rounded_xl().bg(rgb(colors.surface)).text_color(rgb(colors.text))
                .child(div().text_size(px(16.0)).child("Pair a device"))
                .child(div().text_sm().text_color(rgb(colors.muted)).child("Choose a reachable LAN or Tailscale address. Copy this code to the other device within five minutes. Each code can be used once."))
                .child(div().h(px(36.0)).px_3().flex().items_center().bg(rgb(colors.background)).child(self.share_host_input.clone()))
                .child(if panel.loading { "Starting listener…".to_owned() } else { format!("Port {} · {}", panel.port.unwrap_or(0), panel.code) })
                .when_some(panel.error, |card, error| card.child(div().text_sm().text_color(rgb(colors.danger)).child(error)))
                .child(div().flex().gap_3()
                    .child(ui_button("copy-pairing-code", if can_copy { ButtonKind::Primary } else { ButtonKind::Secondary }, colors).on_click(cx.listener(move |_, _, _, cx| { if can_copy { cx.write_to_clipboard(ClipboardItem::new_string(invite.encode())); } })).child("Copy pairing code"))
                    .child(ui_button("refresh-pairing-code", ButtonKind::Secondary, colors).on_click(cx.listener(|this, _, _, cx| { if let Some(panel) = &mut this.share_panel { panel.loading = true; } if let Some(host) = &this.host { let _ = host.peer_pairing(); } cx.notify(); })).child("New code"))
                    .child(div().flex_1())
                    .child(ui_button("close-pairing", ButtonKind::Ghost, colors).on_click(cx.listener(|this, _, _, cx| { this.share_panel = None; cx.notify(); })).child("Close")))
            ).into_any_element()
        });
        let devices_overlay = self.devices_panel.clone().map(|panel| {
            let rows = panel
                .devices
                .iter()
                .enumerate()
                .map(|(index, device)| {
                    let id = device
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let name = device
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("Device")
                        .to_owned();
                    let confirmed = panel.revoke_confirmation.as_deref() == Some(id.as_str());
                    div()
                        .id(("paired-device", index))
                        .flex()
                        .items_center()
                        .gap_3()
                        .py_2()
                        .child(div().flex_1().child(name))
                        .child(
                            div()
                                .id(("revoke-device", index))
                                .px_3()
                                .py_2()
                                .text_color(rgb(colors.danger))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if confirmed {
                                        if let Some(host) = &this.host {
                                            let _ = host.revoke_device(&id);
                                        }
                                        if let Some(panel) = &mut this.devices_panel {
                                            panel.mutating = Some(id.clone());
                                        }
                                    } else if let Some(panel) = &mut this.devices_panel {
                                        panel.revoke_confirmation = Some(id.clone());
                                    }
                                    cx.notify();
                                }))
                                .child(if confirmed {
                                    "Confirm revoke"
                                } else {
                                    "Revoke"
                                }),
                        )
                })
                .collect::<Vec<_>>();
            div()
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(colors.backdrop))
                .child(
                    div()
                        .w(px(480.0))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(rgb(colors.surface))
                        .text_color(rgb(colors.text))
                        .child(div().text_size(px(16.0)).child("Paired devices"))
                        .when(panel.loading, |card| card.child("Loading…"))
                        .when(!panel.loading && rows.is_empty(), |card| {
                            card.child("No paired devices.")
                        })
                        .children(rows)
                        .when_some(panel.error, |card, error| {
                            card.child(div().text_color(rgb(colors.danger)).child(error))
                        })
                        .child(
                            div()
                                .id("close-devices")
                                .px_3()
                                .py_2()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.devices_panel = None;
                                    cx.notify();
                                }))
                                .child("Close"),
                        ),
                )
                .into_any_element()
        });
        let native_option_overlay = self.native_option_open.map(|option| {
            let catalog = self
                .model
                .agent_backends
                .iter()
                .find(|backend| backend.id == self.model.backend);
            let choices = match option {
                "model" => catalog
                    .map(|backend| {
                        backend
                            .models
                            .iter()
                            .map(|model| (model.id.clone(), model.name.clone()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                "effort" => catalog
                    .map(|backend| {
                        backend
                            .efforts
                            .iter()
                            .map(|effort| (effort.clone(), effort.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
                "access" => vec![
                    ("read-only".into(), "Read only".into()),
                    ("edit".into(), "Edit workspace".into()),
                    ("full".into(), "Full access".into()),
                ],
                _ => vec![
                    ("false".into(), "Act".into()),
                    ("true".into(), "Plan".into()),
                ],
            };
            let rows = choices
                .into_iter()
                .enumerate()
                .map(|(index, (value, label))| {
                    div()
                        .id(("native-setting-choice", index))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(colors.surface_high)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_native_option(option, value.clone(), cx)
                        }))
                        .child(label)
                })
                .collect::<Vec<_>>();
            div()
                .occlude()
                .absolute()
                .inset_0()
                .track_focus(&minimal_popup_focus)
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(colors.backdrop))
                .child(
                    div()
                        .w(px(420.0))
                        .max_h(px(500.0))
                        .flex()
                        .flex_col()
                        .p_4()
                        .gap_2()
                        .rounded_xl()
                        .bg(rgb(colors.surface))
                        .text_color(rgb(colors.text))
                        .child(div().text_size(px(16.0)).child(format!("Choose {option}")))
                        .child(
                            div()
                                .id("native-setting-choices")
                                .overflow_y_scroll()
                                .children(rows),
                        )
                        .child(
                            div()
                                .id("close-native-setting")
                                .py_2()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.native_option_open = None;
                                    cx.notify();
                                }))
                                .child("Cancel"),
                        ),
                )
                .into_any_element()
        });
        let auth_overlay = self.auth_open.then(|| {
            let rows = self
                .auth_providers
                .clone()
                .into_iter()
                .enumerate()
                .map(|(index, provider)| {
                    let id = provider.provider.clone();
                    let signing_in = provider.state == "signing-in";
                    div()
                        .id(("assistant-account", index))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .py_2()
                        .child(format!("{} · {}", provider.provider, provider.state))
                        .when_some(provider.detail, |row, detail| {
                            row.child(div().text_xs().text_color(rgb(colors.muted)).child(detail))
                        })
                        .when_some(provider.device_code, |row, code| {
                            row.child(div().font_family(MONO).child(code))
                        })
                        .when_some(provider.login_url, |row, url| {
                            row.child(
                                div()
                                    .id(("assistant-login-url", index))
                                    .py_2()
                                    .text_color(rgb(colors.accent_ink))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.auth_open = false;
                                        this.open_browser_url(&url, window, cx);
                                    }))
                                    .child("Open sign-in page"),
                            )
                        })
                        .child(
                            div()
                                .id(("assistant-login", index))
                                .py_2()
                                .text_color(rgb(colors.accent_ink))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(host) = this.active_host() {
                                        if let Err(error) = host.agent_auth_action(
                                            if signing_in {
                                                "agent-auth-cancel"
                                            } else {
                                                "agent-auth-start"
                                            },
                                            &id,
                                            None,
                                        ) {
                                            this.model.connection_error = Some(error);
                                        }
                                    }
                                    cx.notify();
                                }))
                                .child(if signing_in {
                                    "Cancel sign-in"
                                } else {
                                    "Sign in"
                                }),
                        )
                })
                .collect::<Vec<_>>();
            div()
                .occlude()
                .absolute()
                .inset_0()
                .track_focus(&minimal_popup_focus)
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(colors.backdrop))
                .child(
                    div()
                        .w(px(480.0))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(rgb(colors.surface))
                        .text_color(rgb(colors.text))
                        .child(div().text_size(px(16.0)).child("Assistant accounts"))
                        .children(rows)
                        .when(
                            self.auth_providers
                                .iter()
                                .any(|p| p.needs_input && p.state == "signing-in"),
                            |card| {
                                card.child(div().h(px(36.0)).child(self.auth_input.clone()))
                                    .child(
                                        div()
                                            .id("submit-assistant-auth")
                                            .py_2()
                                            .cursor_pointer()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.submit_auth_input(cx)
                                            }))
                                            .child("Submit sign-in code"),
                                    )
                            },
                        )
                        .child(
                            div()
                                .id("close-assistant-accounts")
                                .py_2()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auth_open = false;
                                    cx.notify();
                                }))
                                .child("Close"),
                        ),
                )
                .into_any_element()
        });
        let remote_overlay = self.remote_panel.clone().map(|panel| {
            let can_connect =
                !panel.submitting && xd_transport::PairingInvite::parse(panel.command.trim()).is_ok();
            div()
                .occlude()
                .absolute()
                .inset_0()
                .track_focus(&minimal_popup_focus)
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(colors.backdrop))
                .child(
                    div()
                        .w(px(470.0))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .shadow_lg()
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(colors.text))
                                .child("Pair with a host"),
                        )
                        .child(
                            div()
                                .mb_2()
                                .text_sm()
                                .text_color(rgb(colors.muted))
                                .child(
                                    "On the host, open Settings → Pair a device and copy its pairing code. Paste it here to connect directly.",
                                ),
                        )
                        .child(
                            div()
                                .h(px(36.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.background))
                                .child(self.remote_pairing_input.clone()),
                        )
                        .when_some(panel.error.clone(), |card, error| {
                            card.child(div().text_sm().text_color(rgb(colors.danger)).child(error))
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
                                        .h(px(30.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.0))
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
                                        .h(px(30.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.0))
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
                .bg(rgba(colors.backdrop))
                .child(
                    div()
                        .w(px(500.0))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .shadow_lg()
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(colors.text))
                                .child("New project"),
                        )
                        .child(
                            div()
                                .h(px(36.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.background))
                                .child(self.workspace_create_input.clone()),
                        )
                        .child(
                            div()
                                .h(px(36.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.background))
                                .child(self.workspace_repo_input.clone()),
                        )
                        .child(
                            div()
                                .h(px(36.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(rgb(colors.border))
                                .bg(rgb(colors.background))
                                .child(self.workspace_clone_input.clone()),
                        )
                        .child(div().text_xs().text_color(rgb(colors.muted)).child(
                            if cfg!(target_os = "windows")
                                && self.active_endpoint == ChatEndpoint::Local
                            {
                                "Use a WSL path (/home/… or /mnt/c/…) or a clone URL."
                            } else {
                                "Use either an existing repository path or a clone URL."
                            },
                        ))
                        .child(
                            div()
                                .mt_2()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("minimal-cancel-project")
                                        .h(px(30.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.0))
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
                                        .h(px(30.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.0))
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
                    .bg(rgba(colors.backdrop))
                    .child(
                        div()
                            .w(px(430.0))
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.surface))
                            .shadow_lg()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.text))
                                    .child(heading),
                            )
                            .child(
                                div()
                                    .h(px(36.0))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.0))
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
                                            .h(px(30.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .rounded(px(6.0))
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
                                            .h(px(30.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .rounded(px(6.0))
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
                    .bg(rgba(colors.backdrop))
                    .child(
                        div()
                            .w(px(450.0))
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.surface))
                            .shadow_lg()
                            .child(
                                div()
                                    .text_size(px(16.0))
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
                                            .h(px(30.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .rounded(px(6.0))
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
                                            .h(px(30.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .rounded(px(6.0))
                                            .bg(rgb(if submitting {
                                                colors.surface_high
                                            } else {
                                                colors.danger
                                            }))
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if submitting {
                                                colors.muted
                                            } else {
                                                colors.surface
                                            }))
                                            .when(!submitting, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(rgb(colors.danger)))
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
                    .bg(rgba(colors.backdrop))
                    .child(
                        div()
                            .w(px(450.0))
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.surface))
                            .shadow_lg()
                            .child(
                                div()
                                    .text_size(px(16.0))
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
                                            .h(px(30.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .rounded(px(6.0))
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
                                            .h(px(30.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .rounded(px(6.0))
                                            .bg(rgb(if submitting {
                                                colors.surface_high
                                            } else {
                                                colors.danger
                                            }))
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if submitting {
                                                colors.muted
                                            } else {
                                                colors.surface
                                            }))
                                            .when(!submitting, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(|style| style.bg(rgb(colors.danger)))
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
                .bg(rgba(colors.backdrop))
                .child(
                    div()
                        .w(px(520.0))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .shadow_lg()
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(colors.text))
                                .child("New agent session"),
                        )
                        .child(
                            div()
                                .h(px(36.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .rounded(px(6.0))
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
                                        .rounded(px(6.0))
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
                                .child("Start from"),
                        )
                        .child(
                            div()
                                .id("minimal-session-worktrees")
                                .max_h(px(340.0))
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
                                            .rounded(px(6.0))
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
                                                    .rounded(px(6.0))
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
                                })
                                .when(self.chat_create_sources_loading, |list| {
                                    list.child(
                                        div()
                                            .mt_1()
                                            .px_3()
                                            .py_2()
                                            .text_sm()
                                            .text_color(rgb(colors.muted))
                                            .child("Loading branches and pull requests…"),
                                    )
                                })
                                .when(!self.chat_create_branches.is_empty(), |list| {
                                    list.child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(colors.muted))
                                            .child("Branches"),
                                    )
                                    .children(self.chat_create_branches.iter().enumerate().map(
                                        |(index, branch)| {
                                            let reference = branch.reference.clone();
                                            let oid = branch.oid.clone();
                                            let selected = matches!(
                                                &selected_worktree,
                                                Some(NewSessionWorktree::Branch {
                                                    reference: selected_reference,
                                                    oid: selected_oid,
                                                }) if selected_reference == &reference && selected_oid == &oid
                                            );
                                            div()
                                                .id(("minimal-session-branch", index))
                                                .px_3()
                                                .py_2()
                                                .rounded(px(6.0))
                                                .border_1()
                                                .border_color(rgb(if selected { colors.accent } else { colors.border }))
                                                .bg(rgb(if selected { colors.surface_high } else { colors.background }))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.chat_create_worktree = Some(NewSessionWorktree::Branch {
                                                        reference: reference.clone(),
                                                        oid: oid.clone(),
                                                    });
                                                    cx.notify();
                                                }))
                                                .child(div().text_sm().text_color(rgb(colors.text)).child(branch.name.clone()))
                                        },
                                    ))
                                })
                                .when(!self.chat_create_pull_requests.is_empty(), |list| {
                                    list.child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(colors.muted))
                                            .child("Pull requests"),
                                    )
                                    .children(self.chat_create_pull_requests.iter().enumerate().map(
                                        |(index, pull_request)| {
                                            let number = pull_request.number;
                                            let oid = pull_request.oid.clone();
                                            let selected = matches!(
                                                &selected_worktree,
                                                Some(NewSessionWorktree::PullRequest {
                                                    number: selected_number,
                                                    oid: selected_oid,
                                                }) if *selected_number == number && selected_oid == &oid
                                            );
                                            div()
                                                .id(("minimal-session-pull-request", index))
                                                .px_3()
                                                .py_2()
                                                .rounded(px(6.0))
                                                .border_1()
                                                .border_color(rgb(if selected { colors.accent } else { colors.border }))
                                                .bg(rgb(if selected { colors.surface_high } else { colors.background }))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(colors.surface_high)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.chat_create_worktree = Some(NewSessionWorktree::PullRequest {
                                                        number,
                                                        oid: oid.clone(),
                                                    });
                                                    cx.notify();
                                                }))
                                                .child(div().text_sm().text_color(rgb(colors.text)).child(format!("#{} {}", pull_request.number, pull_request.title)))
                                        },
                                    ))
                                })
                                .when_some(self.chat_create_sources_error.clone(), |list, error| {
                                    list.child(div().px_3().py_2().text_xs().text_color(rgb(colors.muted)).child(error))
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
                                        .h(px(30.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.0))
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
                                        .h(px(30.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.0))
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
                .rounded(px(8.0))
                .border_1()
                .border_color(rgb(colors.danger))
                .bg(rgb(colors.danger_surface))
                .shadow_lg()
                .text_size(px(13.0))
                .text_color(rgb(colors.danger))
                .child(error)
                .into_any_element()
        });

        div()
            .id("desktop-root")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .key_context("XdDesktop")
            .bg(rgb(colors.background))
            .font_family(UI_FONT)
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    if let Some((start_x, start_width)) = this.dock_drag {
                        if event.dragging() {
                            this.settings.diff_width =
                                (f32::from(start_width) + start_x - f32::from(event.position.x))
                                    .round()
                                    .clamp(320.0, 900.0) as u16;
                            cx.notify();
                        } else {
                            this.finish_dock_resize(cx);
                        }
                    }
                    if let Some((start_x, start_width)) = this.browser_drag {
                        if event.dragging() {
                            let requested =
                                (f32::from(start_width) + start_x - f32::from(event.position.x))
                                    .round()
                                    .clamp(280.0, 1000.0) as u16;
                            this.settings.browser_width = browser_pane_width(
                                requested,
                                f32::from(window.viewport_size().width),
                            ) as u16;
                            cx.notify();
                        } else {
                            this.finish_browser_resize(cx);
                        }
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.finish_browser_resize(cx);
                    this.finish_dock_resize(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.finish_browser_resize(cx);
                    this.finish_dock_resize(cx);
                }),
            )
            .on_action(cx.listener(|this, action: &ToggleActivity, _, cx| {
                this.freeze_transcript();
                if !this.expanded_activities.insert(action.id) {
                    this.expanded_activities.remove(&action.id);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, action: &RevealSubagent, _, cx| {
                this.expanded_activities.insert(action.card_id);
                if action.row < this.transcript.item_count() {
                    // The card's top at the top of the view: its height is not
                    // known until it is drawn open, so "just visible" would
                    // land below the fold.
                    this.transcript.splice(action.row..action.row + 1, 1);
                    this.transcript.scroll_to(ListOffset {
                        item_ix: action.row,
                        offset_in_item: px(0.0),
                    });
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, action: &ControlSubagent, _, cx| {
                this.control_subagent(action.key.clone(), &action.action, cx);
            }))
            .on_action(cx.listener(|this, action: &ToggleWorkLog, _, cx| {
                if !this.expanded_work_logs.insert(action.id) {
                    this.expanded_work_logs.remove(&action.id);
                }
                // Hidden rows change height, so the list measures them again.
                this.freeze_transcript();
                let rows = action.rows.clone();
                if !rows.is_empty() {
                    this.remeasure_rows(rows.clone(), rows.len());
                }
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &CopyRenderedSelection, _, cx| {
                if let Some(text) = TextSelection::selected(cx) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }))
            .on_action(cx.listener(|this, _: &CloseSearch, window, cx| {
                if !this.catalog_query.is_empty() {
                    this.catalog_query.clear();
                    this.catalog_find_input
                        .update(cx, |input, cx| input.set_text("", cx));
                    cx.notify();
                    return;
                }
                if this.auth_open {
                    this.auth_open = false;
                    cx.notify();
                    return;
                }
                if this.native_option_open.take().is_some() {
                    cx.notify();
                    return;
                }
                if this.share_panel.take().is_some() || this.devices_panel.take().is_some() {
                    cx.notify();
                    return;
                }
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
            .child(titlebar)
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when_some(catalog, |row, catalog| row.child(catalog))
                    .child(div().flex_1().min_w_0().min_h_0().h_full().child(content))
                    .when_some(browser_pane, |row, pane| {
                        row.child(
                            div()
                                .id("browser-divider")
                                .relative()
                                .w(px(BROWSER_DIVIDER_WIDTH))
                                .h_full()
                                .flex_none()
                                .bg(rgb(if self.browser_drag.is_some() {
                                    colors.accent
                                } else {
                                    colors.border
                                }))
                                .child(
                                    div()
                                        .id("browser-resize")
                                        .absolute()
                                        .left(px(-4.0))
                                        .w(px(9.0))
                                        .h_full()
                                        .cursor(CursorStyle::ResizeLeftRight)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                move |this, event: &MouseDownEvent, _, cx| {
                                                    cx.stop_propagation();
                                                    this.browser_drag = Some((
                                                        f32::from(event.position.x),
                                                        browser_width as u16,
                                                    ));
                                                    for pane in this.browser_panes.values() {
                                                        pane.update(cx, |pane, _| {
                                                            pane.set_visible(false)
                                                        });
                                                    }
                                                    cx.notify();
                                                },
                                            ),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("integrated-browser")
                                .w(px(browser_width))
                                .h_full()
                                .min_h_0()
                                .flex_none()
                                .overflow_hidden()
                                .child(pane),
                        )
                    }),
            )
            .when_some(session_context_overlay, |root, overlay| root.child(overlay))
            .when_some(theme_overlay, |root, overlay| root.child(overlay))
            .when_some(remote_overlay, |root, overlay| root.child(overlay))
            .when_some(share_overlay, |root, overlay| root.child(overlay))
            .when_some(devices_overlay, |root, overlay| root.child(overlay))
            .when_some(native_option_overlay, |root, overlay| root.child(overlay))
            .when_some(auth_overlay, |root, overlay| root.child(overlay))
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
            CHEVRON_RIGHT_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/chevron-right.svg").as_slice(),
            )),
            CHEVRON_DOWN_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/chevron-down.svg").as_slice(),
            )),
            CLOSE_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/close.svg").as_slice(),
            )),
            TERMINAL_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/terminal.svg").as_slice(),
            )),
            SEARCH_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/search.svg").as_slice(),
            )),
            MINIMIZE_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/minimize.svg").as_slice(),
            )),
            MAXIMIZE_ICON => Some(Cow::Borrowed(
                include_bytes!("../assets/icons/maximize.svg").as_slice(),
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
            CHEVRON_RIGHT_ICON.into(),
            CHEVRON_DOWN_ICON.into(),
            CLOSE_ICON.into(),
            TERMINAL_ICON.into(),
            SEARCH_ICON.into(),
            MINIMIZE_ICON.into(),
            MAXIMIZE_ICON.into(),
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChatReorder {
    chat_id: String,
    anchor_id: String,
    after: bool,
}

fn chat_reorder_for_drop(
    drag: &SessionDragPayload,
    target_project_id: &str,
    target_index: usize,
    target_chat_id: &str,
) -> Option<ChatReorder> {
    if drag.project_id != target_project_id || drag.chat_id == target_chat_id {
        return None;
    }
    Some(ChatReorder {
        chat_id: drag.chat_id.clone(),
        anchor_id: target_chat_id.to_owned(),
        after: drag.source_index < target_index,
    })
}

fn apply_optimistic_chat_reorder(chats: &mut Vec<ChatSummary>, reorder: &ChatReorder) -> bool {
    let Some(source_index) = chats.iter().position(|chat| chat.id == reorder.chat_id) else {
        return false;
    };
    if !chats.iter().any(|chat| chat.id == reorder.anchor_id) {
        return false;
    }
    let chat = chats.remove(source_index);
    let anchor_index = chats
        .iter()
        .position(|candidate| candidate.id == reorder.anchor_id)
        .expect("the checked reorder anchor remains after removing another chat");
    let insert_at = if reorder.after {
        anchor_index + 1
    } else {
        anchor_index
    };
    chats.insert(insert_at, chat);
    true
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

    #[test]
    fn live_approvals_reach_the_model_without_waiting_for_a_reload() {
        let source = include_str!("main.rs");
        let handler = source
            .split_once("fn handle_event(")
            .expect("event handler")
            .1
            .split_once("fn request_tree(")
            .expect("end of event handler")
            .0;
        assert!(handler.contains(
            "\"interaction\" | \"interaction-resolved\" | \"interactions-cleared\" => {"
        ));
    }

    #[test]
    fn records_replace_their_snapshot_row_without_changing_the_count() {
        let mut model = AppModel::default();
        model.messages = vec![
            Message::new(Some(1), "user", "go", None),
            Message::new(
                Some(2),
                "tool",
                "subagent_run\n{\"status\":\"running\"}",
                None,
            ),
        ];
        model.live_items = vec![Message::new(Some(5), "tool", "subagent_run\n{}", None)];
        let mut snapshot = TranscriptSnapshot::default();
        snapshot.sync_messages(&model);
        snapshot.sync_live_items(&model);
        assert_eq!(snapshot.len(), 3);
        assert_eq!(snapshot.running_agents.len(), 2);

        // The model may already have moved on (a reply cleared its live rows);
        // the snapshot keeps what the list draws until it is synced.
        model.live_items.clear();
        assert_eq!(
            snapshot.replace_record(5, "subagent_run\n{\"status\":\"completed\"}"),
            Some(2)
        );
        assert_eq!(
            snapshot.replace_record(2, "subagent_run\n{\"status\":\"stopped\"}"),
            Some(1)
        );
        assert_eq!(snapshot.len(), 3);
        assert!(snapshot.running_agents.is_empty());
        assert_eq!(snapshot.replace_record(99, "x"), None);
    }

    #[test]
    fn previews_read_as_plain_text() {
        assert_eq!(
            first_line("\n## Found **two** files in `src/`\n- more"),
            "Found two files in src/"
        );
        assert_eq!(first_line("```rust\nfn main() {}"), "fn main() {}");
        assert_eq!(first_line(""), "");
    }
    use xd_desktop::model::{ChatSummary, Folder};

    #[gpui::test]
    fn bundled_ui_font_can_be_registered(cx: &mut gpui::TestAppContext) {
        assert_eq!(UI_FONT, "Inter");
        for font in EMBEDDED_UI_FONTS {
            assert!(font.starts_with(&[0x00, 0x01, 0x00, 0x00]));
        }
        install_embedded_fonts(cx.text_system()).expect("register bundled UI font");
    }

    #[gpui::test]
    fn settings_popup_responds_to_clicks(cx: &mut gpui::TestAppContext) {
        install_embedded_fonts(cx.text_system()).expect("register bundled UI font");
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        cx.run_until_parked();

        // The catalog footer holds Terminal and Settings.
        let footer_y = cx.update(|window, _| window.viewport_size().height - px(26.0));
        cx.simulate_click(
            point(px(CATALOG_WIDTH - 50.0), footer_y),
            gpui::Modifiers::default(),
        );
        cx.update(|_, cx| assert!(desktop.read(cx).minimal_theme_open));

        cx.simulate_click(point(px(700.0), px(300.0)), gpui::Modifiers::default());
        cx.update(|_, cx| assert!(!desktop.read(cx).minimal_theme_open));
    }

    #[gpui::test]
    fn clicking_a_native_markdown_link_dispatches_the_integrated_browser_action(
        cx: &mut gpui::TestAppContext,
    ) {
        struct MarkdownLink {
            content: markdown::InlineText,
            opened_url: Option<String>,
        }

        impl Render for MarkdownLink {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .text_size(px(14.0))
                    .line_height(px(22.0))
                    .child(XdDesktop::render_native_inline(
                        &self.content,
                        ThemePreset::default().colors(),
                        0,
                    ))
            }
        }

        let document = markdown::parse("[Preview](http://localhost:3000)");
        let markdown::Block::Paragraph(content) = document.blocks.into_iter().next().unwrap()
        else {
            panic!("the preview link must parse as a paragraph");
        };
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = cx.weak_entity();
            let target = window.window_handle();
            let app: &mut App = cx;
            app.on_action(move |action: &OpenBrowserUrl, cx| {
                if action.window != target {
                    cx.propagate();
                    return;
                }
                let _ = view.update(cx, |view: &mut MarkdownLink, cx| {
                    view.opened_url = Some(action.url.clone());
                    cx.notify();
                });
            });
            MarkdownLink {
                content,
                opened_url: None,
            }
        });
        cx.run_until_parked();
        cx.simulate_click(point(px(10.0), px(10.0)), gpui::Modifiers::default());

        let opened_url = cx.update(|_, cx| view.read(cx).opened_url.clone());
        assert_eq!(opened_url.as_deref(), Some("http://localhost:3000"));
        assert!(cx.opened_url().is_none());
    }

    #[test]
    fn working_dots_cycle_from_dim_to_three_lit() {
        assert_eq!(working_dot_alphas(0), [0x4d, 0x4d, 0x4d]);
        assert_eq!(working_dot_alphas(1), [0xff, 0x4d, 0x4d]);
        assert_eq!(working_dot_alphas(2), [0xff, 0xff, 0x4d]);
        assert_eq!(working_dot_alphas(3), [0xff, 0xff, 0xff]);
        assert_eq!(working_dot_alphas(4), [0x4d, 0x4d, 0x4d]);
    }

    #[gpui::test]
    fn native_message_list_reserves_the_height_of_wrapped_text(cx: &mut gpui::TestAppContext) {
        struct MessageList {
            state: ListState,
            message: Message,
            colors: ThemeColors,
        }

        impl Render for MessageList {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let message = self.message.clone();
                let colors = self.colors;
                list(self.state.clone(), move |index, _, _| {
                    XdDesktop::render_native_message(colors, &message, None, None, index, false)
                })
                .size_full()
            }
        }

        let cx = cx.add_empty_window();
        let state = ListState::new(1, ListAlignment::Top, px(0.0));
        let message = Message::new(
            None,
            "user",
            "This message must wrap inside a narrow chat bubble instead of painting its second and third lines outside the measured message card. ".repeat(3),
            None,
        );
        cx.draw(
            point(px(0.0), px(0.0)),
            size(px(400.0), px(400.0)),
            |_, cx| {
                cx.new(|_| MessageList {
                    state: state.clone(),
                    message,
                    colors: ThemePreset::default().colors(),
                })
            },
        );

        let bounds = state
            .bounds_for_item(0)
            .expect("the rendered message must have measured list bounds");
        assert!(
            bounds.size.height >= px(120.0),
            "wrapped text was painted into a card only {}px tall",
            f32::from(bounds.size.height)
        );
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
    fn working_dots_follow_the_shared_pulse_instead_of_animating_every_frame() {
        assert!(
            working_dots(0, 0xa8a8ad)
                .downcast_mut::<gpui::AnimationElement<gpui::Div>>()
                .is_none()
        );
        assert_eq!(working_dot_alphas(0), [0x4d, 0x4d, 0x4d]);
        assert_eq!(working_dot_alphas(3), [0xff, 0xff, 0xff]);
        let source = include_str!("main.rs");
        let production = source.split_once("#[cfg(test)]").expect("production").0;
        assert!(
            !production.contains(".repeat()"),
            "no per-frame repeating animations"
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
    fn global_terminal_panels_survive_chat_cache_pruning() {
        let live_chats = HashSet::new();
        assert!(terminal_panel_cache_entry_is_live(
            "global:remote:server",
            &live_chats
        ));
        assert!(!terminal_panel_cache_entry_is_live(
            "deleted-chat",
            &live_chats
        ));
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
    fn native_chat_mounts_structured_transcript_and_host_backed_composer_controls() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let chat = production
            .split_once("fn render_native_message(")
            .expect("native message renderer")
            .1
            .split_once("fn render_minimal_cli(")
            .expect("end of native chat renderer")
            .0;
        for behavior in [
            "self.transcript_snapshot.clone()",
            "list(self.transcript.clone()",
            "message.tool_view()",
            "message.markdown()",
            "render_native_message",
            "self.composer_input.clone()",
            "this.send_composer(cx)",
            "this.queue_composer(cx)",
            "this.cancel_turn(cx)",
            "this.drop_queued(index, cx)",
        ] {
            assert!(chat.contains(behavior), "missing {behavior}");
        }

        let actions = production
            .split_once("fn queue_composer(")
            .expect("native queue action")
            .1
            .split_once("fn clear_question(")
            .expect("end of native composer actions")
            .0;
        assert!(actions.contains("host.queue_message(&chat_id, &text)"));
        assert!(actions.contains("host.drop_queue(&chat_id, index)"));
        assert!(actions.contains("host.cancel(&chat_id)"));
    }

    #[test]
    fn native_transcript_pages_through_the_host_message_protocol() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let chat = production
            .split_once("fn render_native_chat(")
            .expect("native chat renderer")
            .1
            .split_once("fn render_minimal_cli(")
            .expect("end of native chat renderer")
            .0;
        assert!(chat.contains("this.request_older_messages()"));
        assert!(chat.contains("this.request_newer_messages()"));
        assert!(chat.contains("near_start && this.transcript_has_older"));

        let requests = production
            .split_once("fn request_messages(")
            .expect("message refresh requests")
            .1
            .split_once("fn request_workflow_statuses(")
            .expect("end of message requests")
            .0;
        assert!(requests.contains("MessageCursor::Before"));
        assert!(requests.contains("MessageCursor::After"));
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
    fn remote_setup_uses_a_pinned_pairing_invitation() {
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

        assert!(overlay.contains("Pair with a host"));
        assert!(overlay.contains("self.remote_pairing_input.clone()"));
        assert!(!overlay.contains("Pairing code"));
        assert!(!overlay.contains("remote_port_input"));
        assert!(production.contains("settings.remote_credentials = Some"));
    }

    #[gpui::test]
    fn opening_links_preserves_the_chats_open_tabs(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.active_endpoint = ChatEndpoint::Local;
            desktop.set_browser_open(false);
            desktop.settings.browser_sessions.clear();
            desktop.model.folders = vec![Folder {
                id: "link-project".into(),
                name: "Links".into(),
                parent: None,
            }];
            desktop.minimal_route = MinimalRoute::Cli {
                project_id: "link-project".into(),
                chat_id: "link-chat".into(),
                agent: AgentCli::Codex,
            };
            cx.notify();
        });
        cx.run_until_parked();
        let target = cx.update(|window, _| window.window_handle());
        cx.dispatch_action(OpenBrowserUrl {
            url: "http://localhost:3000/first".into(),
            window: target,
        });
        let first = desktop.update(cx, |desktop, cx| {
            assert!(desktop.browser_is_open());
            let pane = desktop.browser_panes["local/chat/link-chat"].clone();
            assert_eq!(
                pane.read(cx).current_url().as_deref(),
                Some("http://localhost:3000/first")
            );
            pane
        });
        assert!(cx.opened_url().is_none());

        cx.dispatch_action(OpenBrowserUrl {
            url: "http://localhost:3000/second".into(),
            window: target,
        });
        desktop.update(cx, |desktop, cx| {
            let second = &desktop.browser_panes["local/chat/link-chat"];
            assert_eq!(first.entity_id(), second.entity_id());
            assert_eq!(
                second.read(cx).current_url().as_deref(),
                Some("http://localhost:3000/second")
            );
            assert_eq!(
                second.read(cx).session().tabs,
                vec![
                    Some("http://localhost:3000/first".into()),
                    Some("http://localhost:3000/second".into()),
                ]
            );
        });
        assert!(cx.opened_url().is_none());
    }

    #[test]
    fn native_dock_keeps_the_chat_readable() {
        for requested in [0, 320, 460, u16::MAX] {
            let width = native_dock_width(requested, 900.0);
            assert!(width >= 320.0);
            assert!(900.0 - width >= 440.0);
        }
        assert_eq!(native_dock_width(460, 1400.0), 460.0);
        assert_eq!(native_dock_width(u16::MAX, 3000.0), 900.0);
    }

    #[test]
    fn session_signals_put_waiting_ahead_of_work() {
        assert_eq!(SessionSignal::of(true, true, true), SessionSignal::Waiting);
        assert_eq!(SessionSignal::of(true, false, true), SessionSignal::Working);
        assert_eq!(SessionSignal::of(false, false, true), SessionSignal::Done);
        assert_eq!(SessionSignal::of(false, false, false), SessionSignal::Idle);
        let colors = ThemePreset::Dark.colors();
        assert!(signal_tab(SessionSignal::Idle, colors).is_none());
        assert_eq!(SessionSignal::Waiting.color(colors), colors.waiting);
    }

    #[test]
    fn browser_width_keeps_room_for_the_session_at_minimum_window_size() {
        for requested in [0, 280, 460, u16::MAX] {
            let width = browser_pane_width(requested, 760.0);
            assert!(width >= 280.0);
            // The catalog compacts beside a wide browser, so the session keeps
            // the rest of the window.
            assert!(760.0 - width - BROWSER_DIVIDER_WIDTH >= 460.0);
        }
        assert_eq!(browser_pane_width(460, 1180.0), 460.0);
        assert_eq!(browser_pane_width(u16::MAX, 3000.0), 1000.0);
    }

    #[gpui::test]
    fn browser_panes_preserve_each_chats_tabs_toggle_and_settings_visibility(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        cx.update_window_entity(&desktop, |desktop, window, cx| {
            desktop.active_endpoint = ChatEndpoint::Local;
            desktop.model.folders = vec![
                Folder {
                    id: "a".into(),
                    name: "A".into(),
                    parent: None,
                },
                Folder {
                    id: "b".into(),
                    name: "B".into(),
                    parent: None,
                },
            ];
            desktop.minimal_route = MinimalRoute::Cli {
                project_id: "a".into(),
                chat_id: "chat-a".into(),
                agent: AgentCli::Codex,
            };
            desktop.settings.browser_sessions.clear();
            desktop.set_browser_open(true);
            let a_key = desktop.browser_scope_key();
            assert_eq!(a_key, "local/chat/chat-a");
            let saved = BrowserSession {
                tabs: vec![
                    Some("http://localhost:3000/preview".into()),
                    Some("https://example.com/docs".into()),
                ],
                active_tab: 1,
            };
            desktop.settings.browser_sessions.insert(
                a_key,
                BrowserState {
                    open: true,
                    session: saved.clone(),
                },
            );
            let a = desktop.sync_browser_pane(window, cx).unwrap();
            assert!(a.read(cx).is_visible());
            assert_eq!(a.read(cx).session(), saved);
            assert_eq!(
                a.read(cx).current_url().as_deref(),
                Some("https://example.com/docs")
            );

            desktop.minimal_route = MinimalRoute::Cli {
                project_id: "a".into(),
                chat_id: "chat-b".into(),
                agent: AgentCli::Codex,
            };
            assert!(!desktop.browser_is_open());
            assert!(desktop.sync_browser_pane(window, cx).is_none());
            assert!(!a.read(cx).is_visible());
            desktop.toggle_browser(cx);
            let b = desktop.sync_browser_pane(window, cx).unwrap();
            assert_ne!(a.entity_id(), b.entity_id());
            assert!(!a.read(cx).is_visible());
            assert!(b.read(cx).is_visible());
            assert_eq!(b.read(cx).current_url(), None);
            desktop.close_browser(cx);
            assert!(!desktop.browser_is_open());
            assert!(desktop.sync_browser_pane(window, cx).is_none());
            assert!(!b.read(cx).is_visible());

            desktop.minimal_route = MinimalRoute::Cli {
                project_id: "a".into(),
                chat_id: "chat-a".into(),
                agent: AgentCli::Codex,
            };
            assert!(desktop.browser_is_open());
            let restored = desktop.sync_browser_pane(window, cx).unwrap();
            assert_eq!(a.entity_id(), restored.entity_id());
            assert_eq!(restored.read(cx).session(), saved);
            assert!(!b.read(cx).is_visible());

            desktop.minimal_theme_open = true;
            desktop.sync_browser_pane(window, cx);
            assert!(a.read(cx).is_visible());
            desktop.minimal_theme_open = false;
            desktop.creating_workspace = true;
            desktop.sync_browser_pane(window, cx);
            assert!(!a.read(cx).is_visible());
            desktop.creating_workspace = false;
            desktop.browser_drag = Some((100.0, 460));
            desktop.sync_browser_pane(window, cx);
            assert!(!a.read(cx).is_visible());
            desktop.browser_drag = None;

            desktop.settings.remote_ssh_command = Some("ssh remote.example".into());
            desktop.active_endpoint = ChatEndpoint::Remote;
            assert!(!desktop.browser_is_open());
            assert!(desktop.sync_browser_pane(window, cx).is_none());
            desktop.toggle_browser(cx);
            let remote = desktop.sync_browser_pane(window, cx).unwrap();
            assert_ne!(a.entity_id(), remote.entity_id());
            assert!(!a.read(cx).is_visible());
            assert!(remote.read(cx).is_visible());

            desktop.close_browser(cx);
            assert!(desktop.sync_browser_pane(window, cx).is_none());
            assert!(!remote.read(cx).is_visible());
            desktop.active_endpoint = ChatEndpoint::Local;
            assert!(desktop.browser_is_open());
            assert_eq!(
                desktop.sync_browser_pane(window, cx).unwrap().entity_id(),
                a.entity_id()
            );
            assert_eq!(a.read(cx).session(), saved);
        });
    }

    #[gpui::test]
    fn removed_chats_release_their_browser_tabs_without_affecting_other_connections(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        cx.update_window_entity(&desktop, |desktop, window, cx| {
            desktop.settings.browser_sessions.clear();
            desktop.active_endpoint = ChatEndpoint::Local;
            desktop.model.chats = vec![ChatSummary {
                id: "kept".into(),
                folder: "project".into(),
                title: None,
                backend: "codex".into(),
                branch: None,
                working: false,
                terminal_working: false,
            }];
            for chat_id in ["kept", "removed"] {
                desktop.minimal_route = MinimalRoute::Cli {
                    project_id: "project".into(),
                    chat_id: chat_id.into(),
                    agent: AgentCli::Codex,
                };
                desktop.set_browser_open(true);
                desktop.sync_browser_pane(window, cx).unwrap();
            }
            desktop.settings.remote_ssh_command = Some("ssh remote.example".into());
            desktop.active_endpoint = ChatEndpoint::Remote;
            desktop.set_browser_open(true);
            let remote_key = desktop.browser_scope_key();
            desktop.sync_browser_pane(window, cx).unwrap();
            desktop.active_endpoint = ChatEndpoint::Local;
            let removed = desktop.browser_panes["local/chat/removed"].clone();

            desktop.prune_browser_panes(ChatEndpoint::Local, cx);

            assert!(!removed.read(cx).is_visible());
            assert!(!desktop.browser_panes.contains_key("local/chat/removed"));
            assert!(
                !desktop
                    .settings
                    .browser_sessions
                    .contains_key("local/chat/removed")
            );
            assert!(desktop.browser_panes.contains_key("local/chat/kept"));
            assert!(
                desktop
                    .settings
                    .browser_sessions
                    .contains_key("local/chat/kept")
            );
            assert!(desktop.browser_panes.contains_key(&remote_key));
            assert!(desktop.settings.browser_sessions.contains_key(&remote_key));
        });
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
            assert!(panel.loading); // Cached output remains visible while the daemon refreshes it.
            assert_eq!(panel.selected.as_deref(), Some("terminal-a"));
            assert_eq!(panel.sessions[0].sequence, Some(7));
            assert_eq!(panel.sessions[0].screen.rendered().text, "cached output");
            desktop.terminal_panel = None;
            desktop.terminal_panel_cache.clear();
        });
    }

    #[gpui::test]
    fn failed_draft_persistence_keeps_the_native_chat_selected(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.selected_chat = Some("chat-a".into());
            desktop.composer = "unsaved".into();
            desktop.draft_dirty = true;
            desktop.draft_generation = 7;

            desktop.show_minimal_projects(cx);

            assert_eq!(desktop.model.selected_chat.as_deref(), Some("chat-a"));
            assert_eq!(desktop.composer, "unsaved");
            assert!(desktop.draft_dirty);
            assert_eq!(desktop.draft_generation, 8);
        });
    }

    #[gpui::test]
    #[cfg(unix)]
    fn authoritative_tree_reconciliation_stops_removed_chat_processes(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, _| {
            desktop
                .terminal_runtime
                .open(
                    "chat-removed",
                    "terminal-reconciled",
                    "Terminal",
                    None,
                    xd_desktop::session_host::ProcessSpec::new("/bin/sh", ["-c", "sleep 30"]),
                    None,
                    80,
                    24,
                )
                .unwrap();
            desktop.model.chats.clear();

            desktop.prime_terminal_cache(ChatEndpoint::Local);

            assert!(
                desktop
                    .terminal_runtime
                    .input("terminal-reconciled", b"x")
                    .is_err()
            );
        });
    }

    #[gpui::test]
    #[cfg(unix)]
    fn deleting_a_chat_stops_only_that_endpoints_terminal_processes(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, _| {
            for (chat_id, terminal_id) in [
                ("chat-removed", "terminal-removed"),
                ("chat-other", "terminal-other"),
            ] {
                desktop
                    .terminal_runtime
                    .open(
                        chat_id,
                        terminal_id,
                        "Terminal",
                        None,
                        xd_desktop::session_host::ProcessSpec::new("/bin/sh", ["-c", "sleep 30"]),
                        None,
                        80,
                        24,
                    )
                    .unwrap();
            }
            desktop.terminal_panel_cache.insert(
                (ChatEndpoint::Local, "chat-removed".into()),
                TerminalPanel {
                    chat_id: "chat-removed".into(),
                    sessions: vec![TerminalTab {
                        id: "terminal-removed".into(),
                        title: "Terminal".into(),
                        agent: None,
                        sequence: None,
                        screen: TerminalScreen::new(80, 24),
                    }],
                    ..XdDesktop::new_agent_terminal_panel("chat-removed".into(), AgentCli::Codex)
                },
            );

            desktop.stop_terminal_sessions_for_chats(
                ChatEndpoint::Local,
                &HashSet::from(["chat-removed".into()]),
            );

            assert!(
                desktop
                    .terminal_runtime
                    .input("terminal-removed", b"x")
                    .is_err()
            );
            assert!(
                desktop
                    .terminal_runtime
                    .input("terminal-other", b"x")
                    .is_ok()
            );
            desktop.terminal_runtime.kill("terminal-other").unwrap();
        });
    }

    #[gpui::test]
    fn session_click_focuses_the_agent_terminal(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        cx.update_window_entity(&desktop, |desktop, window, cx| {
            desktop.model.chats = vec![ChatSummary {
                id: "chat-a".into(),
                folder: "project".into(),
                title: Some("A".into()),
                backend: "codex".into(),
                branch: None,
                working: false,
                terminal_working: false,
            }];

            desktop.open_minimal_session(
                "project".into(),
                "chat-a".into(),
                AgentCli::Codex,
                window,
                cx,
            );

            let composer_focus = desktop.composer_input.read(cx).focus_handle(cx);
            let terminal_focus = desktop.terminal_input.read(cx).focus_handle(cx);
            assert!(!composer_focus.is_focused(window));
            assert!(terminal_focus.is_focused(window));
            desktop.terminal_panel = None;
            desktop.terminal_panel_cache.clear();
        });
    }

    #[gpui::test]
    fn plain_terminal_events_update_the_active_panel(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            let mut panel = XdDesktop::new_agent_terminal_panel("chat-a".into(), AgentCli::Codex);
            panel.loading = false;
            panel.auto_open = false;
            desktop.terminal_panel = Some(panel);

            desktop.handle_terminal_screen_event(
                desktop.active_endpoint,
                "terminal-opened",
                &serde_json::json!({
                    "chat": "chat-a",
                    "terminal": "terminal-a",
                    "sequence": 1,
                    "columns": 80,
                    "rows": 24
                }),
                cx,
            );

            let panel = desktop.terminal_panel.as_ref().expect("active panel");
            assert!(panel.sessions.iter().any(|tab| tab.id == "terminal-a"));
            assert_eq!(
                panel
                    .sessions
                    .iter()
                    .find(|tab| tab.id == "terminal-a")
                    .unwrap()
                    .agent,
                None
            );
            desktop.terminal_panel = None;
        });
    }

    #[gpui::test]
    fn app_update_status_is_local_while_a_remote_chat_is_open(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            desktop.active_endpoint = ChatEndpoint::Remote;
            desktop.model.selected_chat = Some("remote-chat".into());
            desktop.handle_host_update(
                HostUpdate::Reply {
                    kind: RequestKind::HostUpdate {
                        action: "status".into(),
                    },
                    body: serde_json::json!({"ok":true, "state":"installed", "supported":true,
                    "version":"0.1.0", "latest":"0.2.0", "available":true})
                    .as_object()
                    .unwrap()
                    .clone(),
                    attachments: None,
                },
                desktop.connection_generation,
                cx,
            );
            let panel = desktop.self_update_panel.as_ref().unwrap();
            assert_eq!(panel.status.as_ref().unwrap().state, "installed");
            assert!(!panel.busy);
            assert_eq!(desktop.model.selected_chat.as_deref(), Some("remote-chat"));
            desktop.handle_host_update(
                HostUpdate::Event {
                    name: "host-update".into(),
                    body: serde_json::json!({"state":"failed", "error":"Download failed"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    attachments: None,
                },
                desktop.connection_generation,
                cx,
            );
            assert_eq!(
                desktop.self_update_panel.as_ref().unwrap().error.as_deref(),
                Some("Download failed")
            );
            desktop.self_update_panel.as_mut().unwrap().busy = true;
            desktop.handle_host_update(
                HostUpdate::Disconnected {
                    message: "Lost connection".into(),
                },
                desktop.connection_generation,
                cx,
            );
            let panel = desktop.self_update_panel.as_ref().unwrap();
            assert!(!panel.busy);
            assert!(panel.error.as_deref().unwrap().contains("Reconnecting"));
        });
    }

    #[gpui::test]
    fn new_chat_waits_for_tree_session_mode(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(XdDesktop::new);
        desktop.update(cx, |desktop, cx| {
            for mode in ["chat", "terminal"] {
                desktop.model = AppModel::default();
                desktop.minimal_route = MinimalRoute::default();
                desktop.handle_reply(
                    RequestKind::NewChat {
                        folder_id: "project".into(),
                        title: "New Chat".into(),
                        workdir: None,
                    },
                    serde_json::json!({"ok": true, "id": "chat", "backend": "codex"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    None,
                    cx,
                );
                assert_eq!(desktop.model.selected_chat, None);
                assert_eq!(
                    desktop.pending_minimal_session,
                    Some(("project".into(), "chat".into()))
                );

                desktop.handle_reply(
                    RequestKind::Tree,
                    serde_json::json!({
                        "ok": true,
                        "folders": [{"id": "project", "name": "Project"}],
                        "chats": [{"id": "chat", "folder": "project",
                            "backend": "codex", "session_mode": mode}]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    None,
                    cx,
                );
                assert_eq!(desktop.pending_minimal_session, None);
                assert_eq!(desktop.model.selected_chat.as_deref(), Some("chat"));
                assert_eq!(desktop.native_session(), mode == "chat");
                assert_eq!(desktop.terminal_panel.is_none(), mode == "chat");
                if mode == "chat" {
                    desktop.handle_reply(
                        RequestKind::Messages {
                            chat_id: "chat".into(),
                            cursor: MessageCursor::Tail,
                        },
                        serde_json::json!({"ok": true, "messages": []})
                            .as_object()
                            .unwrap()
                            .clone(),
                        None,
                        cx,
                    );
                    assert!(desktop.transcript_loaded);
                }
                desktop.terminal_panel = None;
                desktop.terminal_panel_cache.clear();
            }
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
    fn terminal_runtime_events_follow_their_origin_endpoint_after_switching() {
        let (endpoint, name, body) = terminal_runtime_event(SessionEvent {
            endpoint: SessionEndpoint::Remote,
            kind: SessionEventKind::Closed {
                chat_id: "chat-removed-from-both-trees".into(),
                terminal_id: "terminal".into(),
            },
        });

        assert_eq!(endpoint, ChatEndpoint::Remote);
        assert_eq!(name, "terminal-closed");
        assert_eq!(body["chat"], "chat-removed-from-both-trees");
    }

    #[gpui::test]
    fn switching_remote_and_back_keeps_the_local_runtime_subscribed(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.settings.remote_ssh_command = Some("ssh dev.example".into());
            desktop.settings.remote_credentials = Some(xd_transport::RemoteCredentials {
                host: "dev.example".into(),
                port: 4001,
                fingerprint: "ab".repeat(32),
                token: "test-token".into(),
            });
            desktop.remote_state = RemoteState::Connected;
            desktop.model.connected = true;
            let connection_generation = desktop.connection_generation;

            desktop.activate_remote_runtime(cx);

            assert_eq!(desktop.active_endpoint, ChatEndpoint::Remote);
            assert!(desktop.inactive_model.connected);
            assert_eq!(desktop.connection_generation, connection_generation);

            desktop.disconnect_remote_runtime(cx);

            assert_eq!(desktop.active_endpoint, ChatEndpoint::Local);
            assert!(desktop.model.connected);
            assert_eq!(desktop.connection_generation, connection_generation);
        });
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
    fn product_mark_is_embedded_as_a_folder_terminal_not_an_x() {
        let bytes = EmbeddedIcons
            .load(XD_MARK_ICON)
            .expect("embedded mark loads")
            .expect("the product mark exists");
        let mark = String::from_utf8_lossy(&bytes);
        assert!(mark.contains("M3 7V6"));
        assert!(mark.contains("m7 12 2 2-2 2"));
        assert!(!mark.contains("rotate"));
    }

    #[test]
    fn dragging_a_chat_chooses_before_or_after_from_its_relative_position() {
        let drag = SessionDragPayload {
            chat_id: "chat-b".into(),
            project_id: "workspace".into(),
            source_index: 1,
            title: "Second".into(),
        };

        assert_eq!(
            chat_reorder_for_drop(&drag, "workspace", 0, "chat-a"),
            Some(ChatReorder {
                chat_id: "chat-b".into(),
                anchor_id: "chat-a".into(),
                after: false,
            })
        );
        assert_eq!(
            chat_reorder_for_drop(&drag, "workspace", 2, "chat-c"),
            Some(ChatReorder {
                chat_id: "chat-b".into(),
                anchor_id: "chat-c".into(),
                after: true,
            })
        );
        assert_eq!(
            chat_reorder_for_drop(&drag, "other-workspace", 0, "chat-z"),
            None
        );
        assert_eq!(chat_reorder_for_drop(&drag, "workspace", 1, "chat-b"), None);
    }

    #[test]
    fn optimistic_chat_reorder_preserves_other_workspace_positions() {
        let chat = |id: &str, folder: &str| ChatSummary {
            id: id.into(),
            folder: folder.into(),
            title: Some(id.into()),
            backend: "codex".into(),
            branch: None,
            working: false,
            terminal_working: false,
        };
        let mut chats = vec![
            chat("a", "workspace"),
            chat("x", "other"),
            chat("b", "workspace"),
            chat("c", "workspace"),
        ];

        assert!(apply_optimistic_chat_reorder(
            &mut chats,
            &ChatReorder {
                chat_id: "a".into(),
                anchor_id: "c".into(),
                after: true,
            }
        ));
        assert_eq!(
            chats
                .iter()
                .map(|chat| chat.id.as_str())
                .collect::<Vec<_>>(),
            vec!["x", "b", "c", "a"]
        );
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
    fn terminal_activity_is_aggregated_across_tabs_in_the_same_chat() {
        let mut activity = HashMap::new();
        let chat = "chat-1";

        assert!(update_terminal_activity_state(
            &mut activity,
            ChatEndpoint::Local,
            chat,
            "claude",
            Some(true),
        ));
        assert!(update_terminal_activity_state(
            &mut activity,
            ChatEndpoint::Local,
            chat,
            "jcode",
            Some(false),
        ));
        assert!(!update_terminal_activity_state(
            &mut activity,
            ChatEndpoint::Local,
            chat,
            "claude",
            Some(false),
        ));
    }

    #[gpui::test]
    fn closing_one_terminal_preserves_another_terminal_working_state(
        cx: &mut gpui::TestAppContext,
    ) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.model.chats = vec![ChatSummary {
                id: "chat-1".into(),
                folder: "workspace".into(),
                title: Some("Chat".into()),
                backend: "claude".into(),
                branch: Some("main".into()),
                working: false,
                terminal_working: false,
            }];

            desktop.handle_event(
                "terminal-activity",
                serde_json::json!({
                    "chat": "chat-1",
                    "terminal": "claude",
                    "working": true,
                    "terminal_working": true,
                }),
                None,
                cx,
            );
            desktop.handle_event(
                "terminal-activity",
                serde_json::json!({
                    "chat": "chat-1",
                    "terminal": "jcode",
                    "working": false,
                    "terminal_working": true,
                }),
                None,
                cx,
            );
            desktop.handle_event(
                "terminal-closed",
                serde_json::json!({
                    "chat": "chat-1",
                    "terminal": "jcode",
                }),
                None,
                cx,
            );

            assert!(desktop.model.chats[0].terminal_working);
        });
    }

    #[gpui::test]
    fn inactive_endpoint_preserves_other_terminal_activity_on_close(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| XdDesktop::new(window, cx));
        desktop.update(cx, |desktop, cx| {
            desktop.inactive_model.chats = vec![ChatSummary {
                id: "remote-chat".into(),
                folder: "workspace".into(),
                title: Some("Remote chat".into()),
                backend: "claude".into(),
                branch: Some("main".into()),
                working: false,
                terminal_working: false,
            }];
            let generation = desktop.remote_generation;
            for (terminal, working, aggregate) in [("claude", true, true), ("jcode", false, true)] {
                desktop.handle_remote_update(
                    HostUpdate::Event {
                        name: "terminal-activity".into(),
                        body: serde_json::json!({
                            "chat": "remote-chat",
                            "terminal": terminal,
                            "working": working,
                            "terminal_working": aggregate,
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
                        attachments: None,
                    },
                    generation,
                    cx,
                );
            }
            desktop.handle_remote_update(
                HostUpdate::Event {
                    name: "terminal-closed".into(),
                    body: serde_json::json!({
                        "chat": "remote-chat",
                        "terminal": "jcode",
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    attachments: None,
                },
                generation,
                cx,
            );

            assert!(desktop.inactive_model.chats[0].terminal_working);
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
            workdir: None,
            agent: None,
            allow_agent_tabs: false,
            sessions: vec![first, second],
            selected: Some("terminal-one".into()),
            viewport: None,
            auto_open: false,
            opening: false,
            opening_agent: None,
            opening_generation: 0,
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
    fn terminal_open_timeout_only_releases_the_attempt_it_watched() {
        let mut panel = XdDesktop::new_agent_terminal_panel("chat".into(), AgentCli::Codex);
        let first = panel.begin_opening(Some(AgentCli::Codex));
        panel.finish_opening();
        let second = panel.begin_opening(Some(AgentCli::Codex));

        assert!(!panel.expire_opening(first));
        assert!(panel.opening, "an old timeout must not cancel a retry");
        assert!(panel.expire_opening(second));
        assert!(
            !panel.opening,
            "a stuck current attempt must become retryable"
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
            ("minimal-new-claude-tab", "Claude Code"),
            ("minimal-new-jcode-tab", "JCode"),
            ("minimal-new-copilot-tab", "GitHub Copilot"),
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
        assert!(terminal.contains("Box::new(OpenBrowserUrl"));
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
    fn the_catalog_is_the_one_browsing_surface() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let board = production
            .split_once("fn session_signal(&self")
            .expect("catalog renderer")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of catalog renderer")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("project front renderer")
            .1
            .split_once("fn render_native_inline(")
            .expect("end of project front renderer")
            .0;

        for behavior in [
            "minimal-session-board",
            "minimal-session-group",
            "minimal-session-card",
            "minimal-project-list",
            "minimal-new-project",
            ".child(session.branch)",
            "self.toggle_folder_collapsed(project_id, cx)",
            "this.begin_chat_create",
            "signal_tab(signal, colors)",
            "drawer_tabs(&signals, colors)",
        ] {
            assert!(board.contains(behavior), "missing {behavior}");
        }
        assert!(home.contains("project_cards(&self.model.folders, &self.model.chats)"));
        assert!(home.contains("minimal-project-content"));
        assert!(home.contains("minimal-new-session"));
        assert!(!home.contains("self.render_catalog("));

        let root = production
            .split_once("fn render_minimal(")
            .expect("minimal root renderer")
            .1;
        assert!(root.contains("self.render_catalog(colors, active_chat.as_deref(), cx)"));
        let cli = production
            .split_once("fn render_minimal_cli(")
            .expect("minimal CLI renderer")
            .1
            .split_once("fn render_minimal_standalone_terminal(")
            .expect("end of minimal CLI renderer")
            .0;
        assert!(!cli.contains("self.render_catalog("));
    }

    #[test]
    fn completed_unread_sessions_render_done_until_they_are_viewed() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let board = production
            .split_once("fn session_signal(&self")
            .expect("catalog renderer")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of catalog renderer")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("project front renderer")
            .1
            .split_once("fn render_native_inline(")
            .expect("end of project front renderer")
            .0;

        assert!(board.contains("self.model.unread_chats.contains(&session.id)"));
        assert!(board.contains("signal_tab(signal, colors)"));
        assert!(home.contains("self.session_signal(session)"));
        assert!(production.contains("Self::Done => \"Done\""));
        assert!(production.contains("Self::Done => colors.done"));
    }

    #[test]
    fn session_board_truncates_branch_names_inside_cards() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let board = production
            .split_once("fn session_signal(&self")
            .expect("catalog renderer")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of catalog renderer")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("project front renderer")
            .1
            .split_once("fn render_native_inline(")
            .expect("end of project front renderer")
            .0;

        let compact = board.split_whitespace().collect::<String>();
        assert!(
            compact.contains("div().max_w(px(96.0)).flex_none().truncate()"),
            "branch text needs a bounded, truncating child"
        );
        let _ = home;
    }

    #[test]
    fn session_board_only_emphasizes_the_selected_chat() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let board = production
            .split_once("fn session_signal(&self")
            .expect("catalog renderer")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of catalog renderer")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("project front renderer")
            .1
            .split_once("fn render_native_inline(")
            .expect("end of project front renderer")
            .0;

        assert!(board.contains("let emphasized = selected;"));
        assert!(!board.contains("let emphasized = selected || session.working;"));
        let _ = home;
    }

    #[test]
    fn chat_cards_offer_rename_and_delete_from_a_right_click_menu() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;
        let board = production
            .split_once("fn session_signal(&self")
            .expect("catalog renderer")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of catalog renderer")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("project front renderer")
            .1
            .split_once("fn render_native_inline(")
            .expect("end of project front renderer")
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
        let board = production
            .split_once("fn session_signal(&self")
            .expect("catalog renderer")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of catalog renderer")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("project front renderer")
            .1
            .split_once("fn render_native_inline(")
            .expect("end of project front renderer")
            .0;

        for behavior in [
            "minimal-delete-project",
            "this.begin_sidebar_delete(",
            "SidebarTarget::Folder",
            ".path(TRASH_ICON)",
            ".text_color(rgb(colors.danger))",
        ] {
            assert!(home.contains(behavior), "missing {behavior}");
        }
        let _ = board;

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
            .split_once("fn session_signal(&self")
            .expect("catalog renderer")
            .1
            .split_once("fn render_minimal_home(")
            .expect("end of catalog renderer")
            .0;
        let home = production
            .split_once("fn render_minimal_home(")
            .expect("project front renderer")
            .1
            .split_once("fn render_native_inline(")
            .expect("end of project front renderer")
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
    fn new_session_flow_chooses_a_worktree_branch_or_pull_request_before_creation() {
        let source = include_str!("main.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("desktop production source")
            .0;

        for behavior in [
            "chat_create_worktrees_loading",
            "NewSessionWorktree::New",
            "NewSessionWorktree::Existing",
            "NewSessionWorktree::Branch",
            "NewSessionWorktree::PullRequest",
            "Create new worktree",
            "Existing worktrees",
            "Branches",
            "Pull requests",
            "host.folder_settings(&folder_id)",
            "host.new_chat_sources(&folder_id)",
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
        .add_fonts(EMBEDDED_UI_FONTS.into_iter().map(Cow::Borrowed).collect())
        .map_err(|error| error.to_string())
}

fn main() {
    #[cfg(target_os = "windows")]
    windows::configure_environment();
    browser::configure_platform();
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
