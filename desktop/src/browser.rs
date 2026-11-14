//! A native browser view occupying an ordinary GPUI layout slot: Chromium on
//! Linux, and the system web view on macOS and Windows.
//!
//! The native child is above GPUI's drawing surface. Owners must hide it when
//! changing workspaces or removing the pane from the tree, and cover it while a
//! popup is open: a covered page is replaced by a still picture of itself, so
//! the popup can draw over it.
//!
//! On Linux, calls into Chromium are queued on their own thread (see
//! [`browser_thread`]) and Chromium runs its UI on another, so GPUI frames cannot
//! delay page painting or scrolling, and page work cannot delay GPUI. Every
//! native call goes through [`NativeBrowser::with_view`], and results return as
//! [`NativeEvent`]s.

use crate::metrics::{
    CONTROL_HEIGHT, CONTROL_HEIGHT_COMPACT, HIT_MIN, RADIUS_CONTROL, RADIUS_TAB, TEXT_CAPTION,
    TEXT_CONTROL, TEXT_HEADING, TEXT_LABEL, px, text_px,
};
use std::{
    cell::{Cell, RefCell},
    env,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use async_channel::Sender;
use gpui::{
    App, Bounds, Context, DispatchPhase, ElementId, Entity, EventEmitter, Focusable, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, IntoElement, LayoutId, MouseDownEvent, Pixels, Render,
    RenderImage, ScrollHandle, Style, Timer, Window, div, prelude::*, relative, rgb, rgba,
};
#[cfg(not(target_os = "linux"))]
use raw_window_handle::{HasWindowHandle, RawWindowHandle, WindowHandle};
use url::Url;
#[cfg(not(target_os = "linux"))]
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};
use xd_desktop::theme::ThemeColors;

use crate::input::{ComposerEvent, ComposerInput};

/// The pane runs the window's full height, so its two header rows sit level with
/// the session's: the tabs beside the session header, and the address bar, or a
/// side tool's own controls, beside the chat tabs. One rule closes both columns.
pub const SESSION_HEADER_HEIGHT: f32 = 48.0;
pub const TAB_BAR_HEIGHT: f32 = 43.0;
/// Told to an agent whose command made the page open a tab.
#[cfg(target_os = "linux")]
const NEW_TAB_NOTE: &str = "The page opened a new tab, which is now the selected one, so later \
commands act on it. What follows is the page before; take a browser_snapshot to read the new tab.";
/// How long an answer waits for the pane to open a tab the page asked for.
#[cfg(target_os = "linux")]
const POPUP_WAIT: Duration = Duration::from_secs(2);
/// The same, after a command that failed.
#[cfg(target_os = "linux")]
const NEW_TAB_AFTER_ERROR: &str = "The page also opened a new tab, which is now the selected one, \
so later commands act on it; take a browser_snapshot to read it.";

/// Page tabs keep one width so a loading title does not move its neighbours.
const BROWSER_TAB_WIDTH: f32 = 148.0;

#[cfg(any(target_os = "linux", test))]
mod agent;
#[cfg(target_os = "linux")]
mod chromium;
#[cfg(target_os = "linux")]
pub use chromium::{run_smoke_test_if_requested, run_subprocess_if_requested};

#[derive(Clone, Debug)]
pub enum BrowserEvent {
    Close,
    /// One of the app's shortcuts, pressed in the page, as GPUI writes its
    /// keys. The page has given the keyboard back; the app acts on the keys.
    AppShortcut(&'static str),
    SessionChanged(BrowserSession),
    AuxiliarySelected(Option<SideTab>),
    AuxiliaryClosed(SideTab),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideTab {
    Files,
    Changes,
}

impl SideTab {
    fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Changes => "Changes",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Files => crate::FILE_ICON,
            Self::Changes => crate::CHANGES_ICON,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BrowserSession {
    pub tabs: Vec<Option<String>>,
    pub active_tab: usize,
}

impl Default for BrowserSession {
    fn default() -> Self {
        Self {
            tabs: vec![None],
            active_tab: 0,
        }
    }
}

#[derive(Clone, Debug)]
enum TabEvent {
    Navigated(String),
    Updated,
    /// A page asked for a new tab; see [`NativeEvent::Open`].
    Open {
        url: String,
        by_agent: bool,
    },
    /// The page closed itself, which closes its tab.
    #[cfg(target_os = "linux")]
    Closed,
    /// See [`NativeEvent::AppShortcut`].
    #[cfg(target_os = "linux")]
    AppShortcut(&'static str),
    /// See [`NativeEvent::TakeFocus`].
    #[cfg(target_os = "linux")]
    TakeFocus,
}

enum NativeEvent {
    Started(String),
    Finished(String),
    TitleReady,
    /// The page as it looked just before a covering popup hid it.
    Snapshot {
        generation: u64,
        frame: image::RgbaImage,
    },
    /// A shown view was placed in its slot, over any picture of it.
    Shown {
        generation: u64,
    },
    /// A page asked for a new tab, as an agent's command made it do when
    /// `by_agent`. Such a tab leaves the keyboard where it is.
    Open {
        url: String,
        by_agent: bool,
    },
    Error(String),
    /// Windows could not create the view it was asked for earlier. Unlike a
    /// page's error, trying again means creating it, not reloading.
    #[cfg(any(target_os = "windows", test))]
    EngineFailed(String),
    #[cfg(target_os = "linux")]
    HistoryGesture(bool),
    Navigation {
        url: Option<String>,
        can_back: bool,
        can_forward: bool,
    },
    /// A page asked for a file: Chromium has no dialog of its own on Linux.
    #[cfg(target_os = "linux")]
    FileDialog(chromium::FileDialog),
    /// The page closed itself, so there is no browser left in its tab.
    #[cfg(target_os = "linux")]
    Closed,
    /// One of the app's shortcuts was pressed while the page had the
    /// keyboard, as GPUI writes its keys: it goes to the app.
    #[cfg(target_os = "linux")]
    AppShortcut(&'static str),
    /// Tab went past the page's last control, or Shift+Tab before its first.
    #[cfg(target_os = "linux")]
    TakeFocus,
}

#[derive(Default)]
struct PendingTitle {
    latest: Option<String>,
    queued: bool,
}

/// Load/error/popup messages retain their order. High-frequency page titles
/// share one pending value so a page cannot build an ever-growing UI backlog.
#[derive(Clone)]
struct NativeEvents {
    sender: Sender<NativeEvent>,
    title: Arc<Mutex<PendingTitle>>,
}

impl NativeEvents {
    fn channel() -> (Self, async_channel::Receiver<NativeEvent>) {
        let (sender, receiver) = async_channel::unbounded();
        (
            Self {
                sender,
                title: Arc::default(),
            },
            receiver,
        )
    }

    fn send(&self, event: NativeEvent) {
        let _ = self.sender.try_send(event);
    }

    fn pending_title(&self) -> MutexGuard<'_, PendingTitle> {
        self.title.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn title_changed(&self, title: String) {
        let mut pending = self.pending_title();
        if pending.latest.as_ref() == Some(&title) {
            return;
        }
        pending.latest = Some(title);
        if pending.queued {
            return;
        }
        pending.queued = true;
        drop(pending);
        if self.sender.try_send(NativeEvent::TitleReady).is_err() {
            self.pending_title().queued = false;
        }
    }

    fn take_title(&self) -> Option<String> {
        let mut pending = self.pending_title();
        pending.queued = false;
        pending.latest.take()
    }
}

#[cfg(not(target_os = "linux"))]
thread_local! {
    static SHARED_BROWSER_CONTEXT: RefCell<Option<Rc<RefCell<WebContext>>>> = const { RefCell::new(None) };
}

#[cfg(target_os = "linux")]
thread_local! {
    static BROWSER_VIEWS: RefCell<std::collections::HashMap<u64, NativeEntry>> = RefCell::default();
}

#[cfg(not(target_os = "linux"))]
type NativeView = WebView;
#[cfg(target_os = "linux")]
type NativeView = chromium::ChromiumView;

#[cfg(not(target_os = "linux"))]
type NativeEntry = (WebView, Rc<RefCell<WebContext>>);
#[cfg(target_os = "linux")]
type NativeEntry = (NativeView, ());

/// Runs native browser calls in order on Linux, some of which wait on the X
/// server or on Chromium, so that they cannot delay a GPUI frame.
///
/// Chromium's UI thread pumps GLib's default main context, including its X11
/// events, so this queue must never touch that context.
#[cfg(target_os = "linux")]
mod browser_thread {
    use std::{sync::OnceLock, thread};

    use async_channel::Sender;

    type Job = Box<dyn FnOnce() + Send>;

    fn start() -> Option<Sender<Job>> {
        let (sender, receiver) = async_channel::unbounded::<Job>();
        thread::Builder::new()
            .name("xd-browser".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv_blocking() {
                    job();
                }
            })
            .ok()?;
        Some(sender)
    }

    /// Queue a job, preserving submission order. A job dropped because the
    /// thread is gone also drops whatever reply channel it captured.
    pub(super) fn post(job: impl FnOnce() + Send + 'static) {
        static JOBS: OnceLock<Option<Sender<Job>>> = OnceLock::new();
        if let Some(jobs) = JOBS.get_or_init(start) {
            let _ = jobs.try_send(Box::new(job));
        }
    }
}

/// Configure the window system before any threads or windows are created.
/// Chromium embeds as an X11 child on Linux; XWayland supplies it on a Wayland
/// desktop. Saved values are restored in host/terminal subprocesses.
pub fn configure_platform() {
    #[cfg(target_os = "linux")]
    if env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
        // SAFETY: called at the very beginning of main, before thread creation.
        unsafe {
            if env::var_os("XD_HOST_WAYLAND_DISPLAY").is_none() {
                env::set_var(
                    "XD_HOST_WAYLAND_DISPLAY",
                    env::var_os("WAYLAND_DISPLAY").unwrap_or_default(),
                );
            }
            env::remove_var("WAYLAND_DISPLAY");
        }
    }
}

struct TabSlot {
    id: usize,
    entity: Entity<BrowserTab>,
    display: Rc<TabDisplay>,
}

/// A chat's open tabs. Every native tab shares the application's website data.
pub struct BrowserPane {
    tabs: Vec<TabSlot>,
    session: BrowserSession,
    next_tab_id: usize,
    tab_scroll: ScrollHandle,
    reveal_after_layout: bool,
    visible: bool,
    covered: bool,
    colors: ThemeColors,
    auxiliary: Option<SideTab>,
    auxiliary_tabs: Vec<SideTab>,
    tools_enabled: bool,
    /// The window's Tab stops, which the pane's tabs and buttons join.
    keys: crate::KeyTargets,
}

impl EventEmitter<BrowserEvent> for BrowserPane {}

impl BrowserPane {
    #[cfg(test)]
    pub fn new(
        initial_url: Option<String>,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_session(
            BrowserSession {
                tabs: vec![initial_url],
                active_tab: 0,
            },
            colors,
            window,
            cx,
        )
    }

    pub fn with_session(
        session: BrowserSession,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            tabs: Vec::new(),
            session: BrowserSession {
                tabs: Vec::new(),
                active_tab: session.active_tab,
            },
            next_tab_id: 0,
            tab_scroll: ScrollHandle::new(),
            reveal_after_layout: true,
            visible: true,
            covered: false,
            colors,
            auxiliary: None,
            auxiliary_tabs: Vec::new(),
            tools_enabled: false,
            keys: crate::KeyTargets::default(),
        };
        for url in session.tabs {
            pane.insert_tab(url, window, cx);
        }
        if pane.tabs.is_empty() {
            pane.insert_tab(None, window, cx);
        }
        pane.session.active_tab = pane.session.active_tab.min(pane.tabs.len() - 1);
        pane.update_visibility();
        pane
    }

    fn active_tab_index(&self) -> usize {
        self.auxiliary
            .and_then(|tab| {
                self.auxiliary_tabs
                    .iter()
                    .position(|candidate| *candidate == tab)
            })
            .map(|index| self.tabs.len() + index)
            .unwrap_or(self.session.active_tab)
    }

    pub fn active_side_tab(&self) -> Option<SideTab> {
        self.auxiliary
    }

    pub fn configure_side_tabs(
        &mut self,
        selected: Option<SideTab>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.tools_enabled = enabled;
        let selected = if enabled { selected } else { None };
        if !enabled && !self.auxiliary_tabs.is_empty() {
            self.auxiliary_tabs.clear();
            cx.notify();
        }
        if let Some(tab) = selected
            && !self.auxiliary_tabs.contains(&tab)
        {
            self.auxiliary_tabs.push(tab);
        }
        if self.auxiliary != selected {
            self.auxiliary = selected;
            self.reveal_after_layout = true;
            self.update_visibility();
            cx.notify();
        }
    }

    fn select_auxiliary(&mut self, selected: Option<SideTab>, cx: &mut Context<Self>) {
        self.configure_side_tabs(selected, self.tools_enabled, cx);
        cx.emit(BrowserEvent::AuxiliarySelected(selected));
        cx.notify();
    }

    fn close_auxiliary(&mut self, tab: SideTab, cx: &mut Context<Self>) {
        self.auxiliary_tabs.retain(|candidate| *candidate != tab);
        cx.emit(BrowserEvent::AuxiliaryClosed(tab));
        if self.auxiliary == Some(tab) {
            self.select_auxiliary(None, cx);
        }
        cx.notify();
    }

    pub fn session(&self) -> BrowserSession {
        self.session.clone()
    }

    pub fn current_url(&self) -> Option<String> {
        self.session.tabs[self.session.active_tab].clone()
    }

    #[cfg(test)]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    #[cfg(test)]
    pub fn is_covered(&self) -> bool {
        self.covered
    }

    pub fn set_visible(&mut self, visible: bool) {
        if visible && !self.visible {
            self.tab_scroll.scroll_to_item(self.active_tab_index());
        }
        self.visible = visible;
        self.update_visibility();
    }

    /// Something GPUI draws, such as a popup, lies over the pane. The native
    /// page would paint over it, so the page is shown as a picture instead.
    pub fn set_covered(&mut self, covered: bool) {
        self.covered = covered;
        self.update_visibility();
    }

    /// Joins the pane's tabs and buttons, and its pages' toolbars, to the
    /// window's Tab stops.
    pub fn set_key_targets(&mut self, keys: &crate::KeyTargets, cx: &mut Context<Self>) {
        if Rc::ptr_eq(&self.keys.0, &keys.0) {
            return;
        }
        self.keys = keys.clone();
        for tab in &self.tabs {
            tab.entity.update(cx, |tab, _| tab.keys = keys.clone());
        }
    }

    pub fn set_colors(&mut self, colors: ThemeColors, cx: &mut Context<Self>) {
        if self.colors == colors {
            return;
        }
        self.colors = colors;
        for tab in &self.tabs {
            tab.entity.update(cx, |tab, cx| tab.set_colors(colors, cx));
        }
        cx.notify();
    }

    pub fn shutdown(&mut self) {
        self.visible = false;
        for tab in &self.tabs {
            tab.display.shutdown();
        }
    }

    /// Runs an agent's browser command on the selected page, and passes
    /// `reply` what happened. Commands wait on the page, so they run on a
    /// thread of their own. Opening a page shows it, in a new tab if asked.
    pub fn run_agent_command(
        &mut self,
        mut command: serde_json::Value,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
        reply: impl FnOnce(Result<serde_json::Value, String>) + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return reply(Err("The command was stopped.".into()));
        }
        if command["action"] == "navigate" {
            match command["url"].as_str().map(normalize_url) {
                Some(Ok(url)) => command["url"] = serde_json::Value::String(url),
                Some(Err(error)) => return reply(Err(error)),
                None => return reply(Err("Pass url, the address to open.".into())),
            }
            if command["new_tab"] == true {
                // The user may be typing elsewhere; an agent's tab leaves the
                // keyboard where it is.
                self.open_tab(None, false, window, cx);
            } else if self.auxiliary.is_some() {
                self.select_auxiliary(None, cx);
            }
        }
        let tab = &self.tabs[self.session.active_tab];
        let native = tab.display.native().filter(|native| !native.is_disposed());
        let Some(native) = native else {
            let error = tab.entity.read(cx).error.clone();
            return reply(Err(
                error.unwrap_or_else(|| "The browser has not started.".into())
            ));
        };
        #[cfg(target_os = "linux")]
        let deadline = std::time::Instant::now() + agent::COMMAND_TIME;
        // The worker hands its outcome back here, and the agent is answered
        // once the tab has opened every tab its page asked for meanwhile,
        // so the agent's next command finds the one now selected.
        #[cfg(target_os = "linux")]
        let (finished, outcome) =
            async_channel::bounded::<(Result<serde_json::Value, String>, u64)>(1);
        #[cfg(target_os = "linux")]
        {
            let tab = tab.entity.downgrade();
            cx.spawn(async move |_, cx| {
                let Ok((outcome, popups)) = outcome.recv().await else {
                    return;
                };
                let asked = std::time::Instant::now();
                while asked.elapsed() < POPUP_WAIT
                    && tab
                        .update(cx, |tab, _| tab.popups_handled < popups)
                        .unwrap_or(false)
                {
                    Timer::after(Duration::from_millis(20)).await;
                }
                reply(outcome);
            })
            .detach();
        }
        #[cfg(target_os = "linux")]
        native.with_view(move |view| {
            let page = view.devtools().stopped_by(cancelled.clone());
            let spawned = std::thread::Builder::new()
                .name("xd-browser-agent".into())
                .spawn(move || {
                    let turns = page.turns();
                    let outcome =
                        agent::take_turn(&turns, deadline, &cancelled).and_then(|_turn| {
                            let left =
                                deadline.saturating_duration_since(std::time::Instant::now());
                            page.wait_until_started(left.min(Duration::from_secs(10)))?;
                            let popups = page.popups();
                            page.wake();
                            let outcome = agent::run(&page, &command, deadline, &cancelled);
                            page.sleep();
                            // The pane selects the tab, and later commands
                            // act on it, not the page this one describes,
                            // even when the command then failed.
                            if page.popups() == popups {
                                return outcome;
                            }
                            match outcome {
                                Ok(mut outcome) => {
                                    outcome.text = format!("{NEW_TAB_NOTE}\n\n{}", outcome.text);
                                    Ok(outcome)
                                }
                                Err(error) => Err(format!("{error}\n\n{NEW_TAB_AFTER_ERROR}")),
                            }
                        });
                    let _ = finished
                        .send_blocking((outcome.map(agent::Outcome::into_value), page.popups()));
                });
            if let Err(error) = spawned {
                eprintln!("xd: could not run a browser command: {error}");
            }
        });
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (native, command, cancelled);
            reply(Err(
                "Agents can use xd's browser only on Linux, where it is Chromium.".into(),
            ));
        }
    }

    /// Links from a chat reuse an existing matching tab or the current blank tab.
    pub fn open_url(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        let url = match normalize_url(address) {
            Ok(url) => url,
            Err(_) => {
                self.navigate(address, cx);
                return;
            }
        };
        if let Some(index) = self
            .session
            .tabs
            .iter()
            .position(|tab| tab.as_ref() == Some(&url))
        {
            self.select_tab(index, window, cx);
        } else if self.current_url().is_none() {
            self.navigate(&url, cx);
        } else {
            self.add_tab(Some(url), window, cx);
        }
    }

    pub fn navigate(&mut self, address: &str, cx: &mut Context<Self>) {
        let index = self.session.active_tab;
        self.tabs[index]
            .entity
            .update(cx, |tab, cx| tab.navigate(address, cx));
        let url = self.tabs[index].entity.read(cx).current_url();
        if self.session.tabs[index] != url {
            self.session.tabs[index] = url;
            cx.emit(BrowserEvent::SessionChanged(self.session()));
        }
        cx.notify();
    }

    fn insert_tab(&mut self, url: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let entity = cx.new(|cx| {
            let mut tab = BrowserTab::new(url, self.colors, window, cx);
            tab.keys = self.keys.clone();
            tab
        });
        let display = entity.read(cx).display.clone();
        self.session.tabs.push(entity.read(cx).current_url());
        cx.subscribe_in(&entity, window, move |pane, _, event, window, cx| {
            let Some(index) = pane.tabs.iter().position(|tab| tab.id == id) else {
                return;
            };
            match event {
                TabEvent::Navigated(url) => {
                    if pane.session.tabs[index].as_ref() != Some(url) {
                        pane.session.tabs[index] = Some(url.clone());
                        cx.emit(BrowserEvent::SessionChanged(pane.session()));
                    }
                }
                TabEvent::Updated => {}
                TabEvent::Open { url, by_agent } => {
                    pane.open_tab(Some(url.clone()), !by_agent, window, cx);
                }
                #[cfg(target_os = "linux")]
                TabEvent::Closed => pane.close_tab(index, window, cx),
                // The keyboard leaves the page for the app, which acts on
                // the shortcut.
                #[cfg(target_os = "linux")]
                TabEvent::AppShortcut(keys) => {
                    if index == pane.session.active_tab {
                        pane.release_page_keyboard(cx);
                        cx.emit(BrowserEvent::AppShortcut(keys));
                    }
                    return;
                }
                // Past either end of the page, the keyboard goes to the
                // toolbar's address field, the control next to the page.
                #[cfg(target_os = "linux")]
                TabEvent::TakeFocus => {
                    if index == pane.session.active_tab {
                        pane.release_page_keyboard(cx);
                        let tab = pane.tabs[index].entity.read(cx);
                        tab.address.read(cx).focus_handle(cx).focus(window);
                    }
                }
            }
            cx.notify();
        })
        .detach();
        self.tabs.push(TabSlot {
            id,
            entity,
            display,
        });
    }

    fn add_tab(&mut self, url: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(url, true, window, cx);
    }

    /// Adds a tab and selects it, giving it the keyboard when `focus`.
    fn open_tab(
        &mut self,
        url: Option<String>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_auxiliary(None, cx);
        self.insert_tab(url, window, cx);
        self.session.active_tab = self.tabs.len() - 1;
        self.tab_scroll.scroll_to_item(self.active_tab_index());
        self.update_visibility();
        if focus {
            self.focus_active_tab(window, cx);
        }
        cx.emit(BrowserEvent::SessionChanged(self.session()));
        cx.notify();
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.select_auxiliary(None, cx);
        self.session.active_tab = index;
        self.tab_scroll.scroll_to_item(index);
        self.update_visibility();
        self.focus_active_tab(window, cx);
        cx.emit(BrowserEvent::SessionChanged(self.session()));
        cx.notify();
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let was_active = index == self.session.active_tab;
        let closes_pane = self.tabs.len() == 1 && self.auxiliary_tabs.is_empty();
        self.tabs[index].display.shutdown();
        self.tabs.remove(index);
        self.session.tabs.remove(index);
        if self.tabs.is_empty() {
            self.insert_tab(None, window, cx);
            self.session.active_tab = 0;
        } else if index < self.session.active_tab {
            self.session.active_tab -= 1;
        } else {
            self.session.active_tab = self.session.active_tab.min(self.tabs.len() - 1);
        }
        if closes_pane {
            self.set_visible(false);
        }
        self.tab_scroll.scroll_to_item(self.active_tab_index());
        self.update_visibility();
        if was_active {
            self.focus_active_tab(window, cx);
        }
        cx.emit(BrowserEvent::SessionChanged(self.session()));
        if closes_pane {
            cx.emit(BrowserEvent::Close);
        }
        cx.notify();
    }

    fn update_visibility(&self) {
        for (index, tab) in self.tabs.iter().enumerate() {
            tab.display.set_state(
                self.visible && self.auxiliary.is_none() && index == self.session.active_tab,
                self.covered,
            );
        }
    }

    /// Takes the keyboard back from the page on screen, to the app's window.
    #[cfg(target_os = "linux")]
    fn release_page_keyboard(&self, cx: &mut Context<Self>) {
        let tab = self.tabs[self.session.active_tab].entity.read(cx);
        if let Some(native) = tab.display.native() {
            native.with_view(blur_native_view);
        }
    }

    fn focus_active_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible || self.covered || self.auxiliary.is_some() {
            return;
        }
        window.blur();
        let tab = self.tabs[self.session.active_tab].entity.read(cx);
        if tab.url.is_none() {
            tab.address.read(cx).focus_handle(cx).focus(window);
        } else if tab.display.native_visible.get()
            && let Some(native) = tab.display.native()
        {
            native.with_view(|view| {
                let _ = view.focus();
            });
        }
    }
}

impl Drop for BrowserPane {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Render for BrowserPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The scroll viewport is initialized by the first prepaint. Retain
        // restored selection until its bounds exist, then reveal it once.
        if self.reveal_after_layout {
            if self.tab_scroll.bounds().size.width > px(0.) {
                self.tab_scroll.scroll_to_item(self.active_tab_index());
                self.reveal_after_layout = false;
            } else {
                window.request_animation_frame();
            }
        }
        let colors = self.colors;
        let keys = self.keys.clone();
        let pane_handle = cx.entity().downgrade();
        let stop = |control: gpui::Stateful<gpui::Div>,
                    id: String,
                    label: Option<&'static str>,
                    radius: f32,
                    run: PaneKeyRun| {
            let pane = pane_handle.clone();
            crate::key_stop_in_view(
                control,
                &keys,
                crate::KeyLayer::Page,
                id,
                label.map(gpui::SharedString::from),
                radius,
                colors,
                move |window, cx| {
                    let _ = pane.update(cx, |pane, cx| run(pane, window, cx));
                },
            )
        };
        let auxiliary_tabs = self
            .auxiliary_tabs
            .iter()
            .copied()
            .enumerate()
            .map(|(index, tab)| {
                let pill = tab_pill(
                    ("side-tool-tab", index),
                    self.auxiliary == Some(tab),
                    colors,
                )
                .on_click(cx.listener(move |pane, _, _, cx| pane.select_auxiliary(Some(tab), cx)))
                .child(crate::icon(tab.icon(), 13., colors.muted))
                .child(tab.label());
                let close = tab_close(("close-side-tool", index), "Close", colors).on_click(
                    cx.listener(move |pane, _, _, cx| {
                        cx.stop_propagation();
                        pane.close_auxiliary(tab, cx);
                    }),
                );
                stop(
                    pill,
                    format!("side-tool-tab-{index}"),
                    None,
                    TAB_RING_RADIUS,
                    Box::new(move |pane, _, cx| pane.select_auxiliary(Some(tab), cx)),
                )
                .child(stop(
                    close,
                    format!("close-side-tool-{index}"),
                    Some("Close"),
                    crate::FOCUS_RING_RADIUS,
                    Box::new(move |pane, _, cx| pane.close_auxiliary(tab, cx)),
                ))
            })
            .collect::<Vec<_>>();
        let tools = self.tools_enabled.then(|| {
            [SideTab::Files, SideTab::Changes]
                .into_iter()
                .enumerate()
                // An open tool already has its tab; only offer the others.
                .filter(|(_, tab)| !self.auxiliary_tabs.contains(tab))
                .map(|(index, tab)| {
                    stop(
                        crate::icon_button(
                            ("side-add-tool", index),
                            tab.label(),
                            tab.icon(),
                            colors,
                        )
                        .build()
                        .on_click(
                            cx.listener(move |pane, _, _, cx| pane.select_auxiliary(Some(tab), cx)),
                        ),
                        format!("side-add-tool-{index}"),
                        Some(tab.label()),
                        crate::FOCUS_RING_RADIUS,
                        Box::new(move |pane, _, cx| pane.select_auxiliary(Some(tab), cx)),
                    )
                })
                .collect::<Vec<_>>()
        });
        div()
            .relative()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(crate::glass::canvas())
            .child(
                div()
                    .id("browser-tabs")
                    .h(px(SESSION_HEADER_HEIGHT))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_1()
                    .bg(crate::glass::pane(colors))
                    .child(
                        div()
                            .id("browser-tab-list")
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .overflow_x_scroll()
                            .track_scroll(&self.tab_scroll)
                            .children(self.tabs.iter().enumerate().map(|(index, slot)| {
                                let tab = slot.entity.read(cx);
                                let label = if !tab.title.is_empty() {
                                    tab.title.clone()
                                } else if let Some(url) = &tab.url {
                                    Url::parse(url)
                                        .ok()
                                        .and_then(|url| url.host_str().map(str::to_owned))
                                        .unwrap_or_else(|| url.clone())
                                } else {
                                    "New tab".to_owned()
                                };
                                let active =
                                    self.auxiliary.is_none() && index == self.session.active_tab;
                                let pill = tab_pill(("browser-tab", slot.id), active, colors)
                                    .w(px(BROWSER_TAB_WIDTH))
                                    .on_click(cx.listener(move |pane, _, window, cx| {
                                        pane.select_tab(index, window, cx)
                                    }))
                                    .child(crate::icon(crate::BROWSER_ICON, 13., colors.muted))
                                    // Cut short with an ellipsis, the whole
                                    // title under the pointer.
                                    .child(crate::truncating_label(label).flex_1());
                                let close =
                                    tab_close(("browser-close-tab", slot.id), "Close tab", colors)
                                        .on_click(cx.listener(move |pane, _, window, cx| {
                                            cx.stop_propagation();
                                            pane.close_tab(index, window, cx);
                                        }));
                                stop(
                                    pill,
                                    format!("browser-tab-{}", slot.id),
                                    None,
                                    TAB_RING_RADIUS,
                                    Box::new(move |pane, window, cx| {
                                        pane.select_tab(index, window, cx)
                                    }),
                                )
                                .child(stop(
                                    close,
                                    format!("browser-close-tab-{}", slot.id),
                                    Some("Close tab"),
                                    crate::FOCUS_RING_RADIUS,
                                    Box::new(move |pane, window, cx| {
                                        pane.close_tab(index, window, cx)
                                    }),
                                ))
                            }))
                            .children(auxiliary_tabs),
                    )
                    .when_some(tools, |bar, tools| bar.children(tools))
                    .child(stop(
                        crate::icon_button(
                            "browser-new-tab",
                            "New browser tab",
                            crate::PLUS_ICON,
                            colors,
                        )
                        .drawn(crate::plus_icon)
                        .build()
                        .on_click(
                            cx.listener(|pane, _, window, cx| pane.add_tab(None, window, cx)),
                        ),
                        "browser-new-tab".into(),
                        Some("New browser tab"),
                        crate::FOCUS_RING_RADIUS,
                        Box::new(on_pane_new_tab),
                    )),
            )
            .when(self.auxiliary.is_none(), |pane| {
                pane.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.tabs[self.session.active_tab].entity.clone()),
                )
            })
    }
}

/// What Enter or Space on one of the pane's controls does to the pane.
type PaneKeyRun = Box<dyn Fn(&mut BrowserPane, &mut Window, &mut Context<BrowserPane>)>;

/// The corners of the focus ring around one of the pane's tabs.
const TAB_RING_RADIUS: f32 = RADIUS_TAB + crate::FOCUS_RING_GAP;

fn on_pane_new_tab(pane: &mut BrowserPane, window: &mut Window, cx: &mut Context<BrowserPane>) {
    pane.add_tab(None, window, cx);
}

/// The side pane's tab, in the shape of the chat tabs beside it.
fn tab_pill(
    id: impl Into<ElementId>,
    selected: bool,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(CONTROL_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .pl_2()
        .pr_1()
        .rounded(px(RADIUS_TAB))
        .border_1()
        .border_color(rgba(if selected {
            (colors.border << 8) | 0xff
        } else {
            0
        }))
        .when(selected, |tab| {
            tab.bg(rgb(colors.selected_surface))
                .child(crate::selection_marker(crate::MarkerEdge::Foot, colors))
        })
        .text_size(text_px(TEXT_LABEL))
        .text_color(rgb(if selected { colors.text } else { colors.muted }))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(colors.surface_high)))
}

/// A tab's close button. It is an icon alone, so `label` names it.
fn tab_close(
    id: impl Into<ElementId>,
    label: &'static str,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    crate::icon_button(id, label, crate::CLOSE_ICON, colors)
        .size(HIT_MIN)
        .glyph(11.)
        .build()
}

struct BrowserTab {
    display: Rc<TabDisplay>,
    address: Entity<ComposerInput>,
    draft: String,
    address_dirty: bool,
    url: Option<String>,
    title: String,
    loading: bool,
    loading_generation: u64,
    load_timed_out: bool,
    can_back: bool,
    can_forward: bool,
    error: Option<String>,
    /// The engine could not start for this tab, so it has no view yet.
    engine_failed: bool,
    /// A retry is scheduled for an engine another process kept from starting.
    waiting_for_engine: bool,
    colors: ThemeColors,
    #[cfg(target_os = "linux")]
    last_history_gesture: Option<std::time::Instant>,
    /// Tabs this tab's pages asked for that the pane has opened.
    popups_handled: u64,
    /// The window's Tab stops, which the toolbar joins.
    keys: crate::KeyTargets,
}

impl EventEmitter<TabEvent> for BrowserTab {}

impl BrowserTab {
    pub fn new(
        initial_url: Option<String>,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address = cx.new(|cx| {
            let mut input = ComposerInput::new(cx, "URL or localhost:3000");
            input.set_compact(true);
            input.set_colors(colors, cx);
            input
        });
        let (events, receiver) = NativeEvents::channel();
        let (native, error) = if cfg!(test) {
            (None, None)
        } else {
            match NativeBrowser::create(window, events.clone(), cx) {
                Ok(native) => (Some(native), None),
                Err(error) => (None, Some(error)),
            }
        };
        let engine_failed = error.is_some();
        let waiting_for_engine = engine_failed && engine_may_start_later();
        if waiting_for_engine {
            Self::wait_for_engine(window, cx);
        }
        let mut pane = Self {
            display: Rc::new(TabDisplay {
                native: RefCell::new(native),
                shut_down: Cell::new(false),
                events,
                bounds: Rc::new(Cell::new(None)),
                visible: Cell::new(false),
                covered: Cell::new(false),
                snapshot: RefCell::new(None),
                generation: Cell::new(0),
                awaiting_picture: Cell::new(None),
                picture_generation: Cell::new(None),
                native_visible: Cell::new(false),
                has_page: Cell::new(false),
                healthy: Cell::new(error.is_none()),
            }),
            address: address.clone(),
            draft: String::new(),
            address_dirty: false,
            url: None,
            title: String::new(),
            loading: false,
            loading_generation: 0,
            load_timed_out: false,
            can_back: false,
            can_forward: false,
            error,
            engine_failed,
            waiting_for_engine,
            colors,
            #[cfg(target_os = "linux")]
            last_history_gesture: None,
            popups_handled: 0,
            keys: crate::KeyTargets::default(),
        };
        cx.subscribe_in(&address, window, |pane, _, event, _, cx| match event {
            ComposerEvent::Changed(text) => {
                pane.draft = text.clone();
                pane.address_dirty = true;
            }
            ComposerEvent::Submit => {
                let url = pane.draft.clone();
                pane.navigate(&url, cx);
            }
            _ => {}
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = receiver.recv().await {
                if this
                    .update(cx, |pane, cx| pane.native_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        // Includes same-document navigation (e.g. an SPA's pushState), for
        // which the engine does not necessarily emit a page-load event.
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(250)).await;
                if this
                    .update(cx, |pane, _| pane.refresh_navigation())
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        if let Some(url) = initial_url {
            pane.navigate(&url, cx);
        }
        pane
    }

    pub fn set_colors(&mut self, colors: ThemeColors, cx: &mut Context<Self>) {
        if self.colors != colors {
            self.colors = colors;
            self.address
                .update(cx, |input, cx| input.set_colors(colors, cx));
            cx.notify();
        }
    }

    fn refresh_visibility(&self) {
        self.display.has_page.set(self.url.is_some());
        self.display.healthy.set(self.error.is_none());
        self.display.set_visible(self.display.visible.get());
    }

    fn shutdown(&self) {
        self.display.shutdown();
    }

    pub fn current_url(&self) -> Option<String> {
        self.url.clone()
    }

    pub fn navigate(&mut self, address: &str, cx: &mut Context<Self>) {
        let url = match normalize_url(address) {
            Ok(url) => url,
            Err(error) => {
                self.error = Some(error);
                self.refresh_visibility();
                cx.notify();
                return;
            }
        };
        self.draft = url.clone();
        self.address_dirty = false;
        self.address
            .update(cx, |input, cx| input.set_text(url.clone(), cx));
        let Some(native) = self.display.native() else {
            self.record_url(url, cx);
            self.refresh_visibility();
            cx.notify();
            return;
        };
        if native.is_disposed() {
            return;
        }
        let events = self.display.events.clone();
        let target = url.clone();
        native.with_view(move |view| {
            if let Err(error) = view.load_url(&target) {
                events.send(NativeEvent::Error(format!("Could not open page: {error}")));
            }
        });
        self.error = None;
        self.start_loading(cx);
        self.title.clear();
        self.record_url(url, cx);
        self.refresh_visibility();
        cx.notify();
    }

    fn record_url(&mut self, url: String, cx: &mut Context<Self>) {
        if !is_browser_url(&url) || self.url.as_ref() == Some(&url) {
            return;
        }
        self.url = Some(url.clone());
        if !self.address_dirty {
            self.draft = url.clone();
            self.address
                .update(cx, |input, cx| input.set_text(url.clone(), cx));
        }
        cx.emit(TabEvent::Navigated(url));
    }

    fn start_loading(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.load_timed_out = false;
        self.loading_generation = self.loading_generation.wrapping_add(1);
        let generation = self.loading_generation;
        // WKWebView does not expose failed provisional loads through Wry.
        // This also gives a stalled renderer a usable retry path on Linux.
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(30)).await;
            let _ = this.update(cx, |pane, cx| {
                if pane.loading && pane.loading_generation == generation && pane.error.is_none() {
                    pane.loading = false;
                    pane.load_timed_out = true;
                    pane.error = Some("The page is taking too long to load. Check that the server is running, then reload.".into());
                    pane.refresh_visibility();
                    cx.notify();
                }
            });
        }).detach();
    }

    fn native_event(&mut self, event: NativeEvent, cx: &mut Context<Self>) {
        match event {
            NativeEvent::Started(url) => {
                self.start_loading(cx);
                self.error = None;
                self.record_url(url, cx);
            }
            NativeEvent::Finished(url) => {
                self.loading = false;
                if self.load_timed_out {
                    self.error = None;
                    self.load_timed_out = false;
                }
                self.record_url(url, cx);
                self.refresh_navigation();
            }
            NativeEvent::TitleReady => {
                let Some(title) = self.display.events.take_title() else {
                    return;
                };
                if self.title == title {
                    return;
                }
                self.title = title;
                cx.emit(TabEvent::Updated);
            }
            NativeEvent::Snapshot { generation, frame } => {
                if !self.display.clone().take_snapshot(generation, frame, cx) {
                    return;
                }
            }
            // Sent with every placement of a shown view, not only the first.
            NativeEvent::Shown { generation } => {
                if self.display.generation.get() != generation
                    || self.display.snapshot.take().is_none()
                {
                    return;
                }
            }
            NativeEvent::Open { url, by_agent } => {
                self.popups_handled += 1;
                cx.emit(TabEvent::Open { url, by_agent });
            }
            #[cfg(target_os = "linux")]
            NativeEvent::HistoryGesture(back) => {
                // Keep momentum from continuing through a second document,
                // and ignore requests from a hidden or covered tab.
                if !self.display.native_visible.get()
                    || self.display.covered.get()
                    || self
                        .last_history_gesture
                        .is_some_and(|last| last.elapsed() < Duration::from_millis(800))
                {
                    return;
                }
                self.last_history_gesture = Some(std::time::Instant::now());
                self.history(back, cx);
            }
            NativeEvent::Error(error) => {
                self.loading = false;
                self.load_timed_out = false;
                self.error = Some(error);
            }
            #[cfg(any(target_os = "windows", test))]
            NativeEvent::EngineFailed(error) => {
                self.loading = false;
                self.load_timed_out = false;
                // The view that failed is of no use; another try makes its own.
                self.display.native.borrow_mut().take();
                self.engine_failed = true;
                self.error = Some(error);
            }
            #[cfg(target_os = "linux")]
            NativeEvent::FileDialog(dialog) => {
                dialog.show(cx);
                return;
            }
            #[cfg(target_os = "linux")]
            NativeEvent::Closed => {
                cx.emit(TabEvent::Closed);
                return;
            }
            // Only from the page on screen, which alone has the keyboard.
            #[cfg(target_os = "linux")]
            NativeEvent::AppShortcut(keys) => {
                if self.display.native_visible.get() {
                    cx.emit(TabEvent::AppShortcut(keys));
                }
                return;
            }
            #[cfg(target_os = "linux")]
            NativeEvent::TakeFocus => {
                if self.display.native_visible.get() {
                    cx.emit(TabEvent::TakeFocus);
                }
                return;
            }
            // This arrives several times a second, so it must only redraw when
            // something changed; it returns before the unconditional notify.
            NativeEvent::Navigation {
                url,
                can_back,
                can_forward,
            } => {
                self.apply_navigation(url, can_back, can_forward, cx);
                return;
            }
        }
        self.refresh_visibility();
        cx.notify();
    }

    fn refresh_navigation(&mut self) {
        if !self.display.visible.get() {
            return;
        }
        let Some(native) = self.display.native() else {
            return;
        };
        let events = self.display.events.clone();
        native.with_view(move |view| {
            events.send(NativeEvent::Navigation {
                url: view.url().ok(),
                can_back: view.can_go_back().unwrap_or(false),
                can_forward: view.can_go_forward().unwrap_or(false),
            });
        });
    }

    fn apply_navigation(
        &mut self,
        url: Option<String>,
        can_back: bool,
        can_forward: bool,
        cx: &mut Context<Self>,
    ) {
        if self.can_back != can_back || self.can_forward != can_forward {
            self.can_back = can_back;
            self.can_forward = can_forward;
            cx.notify();
        }
        if !self.loading
            && !self.load_timed_out
            && self.error.is_none()
            && let Some(url) = url
            && is_browser_url(&url)
            && self.url.as_ref() != Some(&url)
        {
            self.record_url(url, cx);
            cx.notify();
        }
    }

    fn history(&mut self, back: bool, cx: &mut Context<Self>) {
        if let Some(native) = self.display.native() {
            let events = self.display.events.clone();
            native.with_view(move |view| {
                let result = if back {
                    view.go_back()
                } else {
                    view.go_forward()
                };
                if let Err(error) = result {
                    events.send(NativeEvent::Error(error.to_string()));
                }
            });
        }
        self.address_dirty = false;
        self.error = None;
        self.loading = false;
        self.load_timed_out = false;
        self.loading_generation = self.loading_generation.wrapping_add(1);
        // Same-document history may have no load events. Keep URL polling
        // active; a document load's Started event will arm its timeout.
        self.refresh_visibility();
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(native) = self.display.native() {
            if native.is_disposed() {
                return;
            }
            let events = self.display.events.clone();
            native.with_view(move |view| {
                if let Err(error) = view.reload() {
                    events.send(NativeEvent::Error(error.to_string()));
                }
            });
            self.start_loading(cx);
            self.error = None;
        }
        self.refresh_visibility();
        cx.notify();
    }

    /// Back to the page behind an error, whose address may be one that failed.
    /// A tab whose engine never started tries to start it again instead.
    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.engine_failed {
            if self.start_native(window, cx) && !self.waiting_for_engine {
                self.waiting_for_engine = true;
                Self::wait_for_engine(window, cx);
            }
            return;
        }
        if let Some(url) = self.url.clone() {
            self.draft = url.clone();
            self.address_dirty = false;
            self.address.update(cx, |input, cx| input.set_text(url, cx));
        }
        self.error = None;
        self.reload(cx);
    }

    /// Start the engine for a tab that could not start it, and open the tab's
    /// page in it. Returns whether a later try could still succeed.
    fn start_native(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.engine_failed || self.display.shut_down.get() {
            return false;
        }
        match NativeBrowser::create(window, self.display.events.clone(), cx) {
            Ok(native) => {
                *self.display.native.borrow_mut() = Some(native.clone());
                self.engine_failed = false;
                self.error = None;
                // Load the tab's page without touching the address field, which
                // the user may be editing.
                if let Some(url) = self.url.clone() {
                    let events = self.display.events.clone();
                    native.with_view(move |view| {
                        if let Err(error) = view.load_url(&url) {
                            events
                                .send(NativeEvent::Error(format!("Could not open page: {error}")));
                        }
                    });
                    self.start_loading(cx);
                }
                self.refresh_visibility();
                cx.notify();
                false
            }
            Err(error) => {
                self.error = Some(error);
                self.refresh_visibility();
                cx.notify();
                engine_may_start_later()
            }
        }
    }

    /// What to offer when Windows could not create the browser at all: its
    /// runtime to install if that is what is missing, and another try.
    #[cfg(target_os = "windows")]
    fn engine_recovery(&self, error: &str, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.engine_failed {
            return None;
        }
        let colors = self.colors;
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .when(error == WEBVIEW2_MISSING, |row| {
                    row.child(
                        crate::ui_button(
                            "browser-get-webview2",
                            crate::ButtonKind::Primary,
                            colors,
                        )
                        .h(px(28.))
                        .on_click(|_, _, cx| cx.open_url(WEBVIEW2_INSTALLER))
                        .child("Get WebView2"),
                    )
                })
                .child(
                    crate::ui_button("browser-retry", crate::ButtonKind::Secondary, colors)
                        .h(px(28.))
                        .on_click(cx.listener(|pane, _, window, cx| pane.retry(window, cx)))
                        .child("Try again"),
                )
                .into_any_element(),
        )
    }

    #[cfg(not(target_os = "windows"))]
    fn engine_recovery(&self, _: &str, _: &mut Context<Self>) -> Option<gpui::AnyElement> {
        None
    }

    /// Another xd holding the browser profile is often one still exiting,
    /// such as the one this xd restarted from. Start once it lets go.
    fn wait_for_engine(window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                Timer::after(ENGINE_RETRY_INTERVAL).await;
                let again = this.update_in(cx, |pane, window, cx| {
                    let again = pane.start_native(window, cx);
                    pane.waiting_for_engine = again;
                    again
                });
                if !matches!(again, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// A toolbar button. It is an icon alone, so `label` names it.
    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        path: &'static str,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        crate::icon_button(id, label, path, self.colors)
            .glyph(15.)
            .enabled(enabled)
            .build()
    }
}

impl Drop for BrowserTab {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Render for BrowserTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_page = self.current_url().is_some();
        let back_enabled = self.can_back;
        let forward_enabled = self.can_forward;
        let colors = self.colors;
        let address_focused = self.address.read(cx).focus_handle(cx).is_focused(window);
        let keys = self.keys.clone();
        let tab_handle = cx.entity().downgrade();
        // A toolbar button the window's Tab stops on while it can be used:
        // Enter or Space does what a click does.
        let stop = |button: gpui::Stateful<gpui::Div>,
                    id: &'static str,
                    label: &'static str,
                    enabled: bool,
                    run: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            if !enabled {
                return button;
            }
            let tab = tab_handle.clone();
            crate::key_stop_in_view(
                button,
                &keys,
                crate::KeyLayer::Page,
                id,
                Some(label.into()),
                crate::FOCUS_RING_RADIUS,
                colors,
                move |window, cx| {
                    let _ = tab.update(cx, |tab, cx| run(tab, window, cx));
                },
            )
        };
        let mut pane = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(crate::glass::canvas())
            .text_color(rgb(colors.text))
            .child(
                div()
                    .id("browser-toolbar")
                    .h(px(TAB_BAR_HEIGHT))
                    .bg(crate::glass::pane(colors))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(stop(
                        self.button("browser-back", "Back", crate::ARROW_LEFT_ICON, back_enabled)
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                if back_enabled {
                                    pane.history(true, cx);
                                }
                            })),
                        "browser-back",
                        "Back",
                        back_enabled,
                        |tab, _, cx| tab.history(true, cx),
                    ))
                    .child(stop(
                        self.button(
                            "browser-forward",
                            "Forward",
                            crate::ARROW_RIGHT_ICON,
                            forward_enabled,
                        )
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            if forward_enabled {
                                pane.history(false, cx);
                            }
                        })),
                        "browser-forward",
                        "Forward",
                        forward_enabled,
                        |tab, _, cx| tab.history(false, cx),
                    ))
                    .child(stop(
                        self.button(
                            "browser-reload",
                            "Reload",
                            crate::ROTATE_CW_ICON,
                            has_page || self.engine_failed,
                        )
                        .on_click(cx.listener(
                            move |pane, _, window, cx| {
                                if pane.engine_failed {
                                    pane.retry(window, cx);
                                } else if has_page {
                                    pane.reload(cx);
                                }
                            },
                        )),
                        "browser-reload",
                        "Reload",
                        has_page || self.engine_failed,
                        |tab, window, cx| {
                            if tab.engine_failed {
                                tab.retry(window, cx);
                            } else if tab.url.is_some() {
                                tab.reload(cx);
                            }
                        },
                    ))
                    .child(
                        div()
                            .id("browser-address")
                            .flex_1()
                            .min_w_0()
                            .h(px(CONTROL_HEIGHT_COMPACT))
                            .mx(px(4.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px(px(10.))
                            .bg(rgb(colors.background))
                            .border_1()
                            .border_color(rgb(if address_focused {
                                colors.selected_border
                            } else {
                                colors.border
                            }))
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_text()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|pane, _, window, cx| {
                                    pane.address.read(cx).focus_handle(cx).focus(window);
                                }),
                            )
                            .child(div().flex_1().min_w_0().child(self.address.clone()))
                            .child(keys.mark(
                                crate::KeyLayer::Page,
                                crate::KeyStop::Field(self.address.read(cx).focus_handle(cx)),
                            ))
                            .when(self.loading, |field| {
                                field.child(
                                    div()
                                        .flex_none()
                                        .text_size(text_px(TEXT_CAPTION))
                                        .text_color(rgb(colors.faint))
                                        .child("Loading…"),
                                )
                            }),
                    )
                    .child(stop(
                        self.button(
                            "browser-external",
                            "Open in your browser",
                            crate::EXTERNAL_LINK_ICON,
                            has_page,
                        )
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            if let Some(url) = &pane.url {
                                cx.open_url(url);
                            }
                        })),
                        "browser-external",
                        "Open in your browser",
                        has_page,
                        |tab, _, cx| {
                            if let Some(url) = &tab.url {
                                cx.open_url(url);
                            }
                        },
                    )),
            );
        if let Some(error) = &self.error {
            pane = pane.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .p(px(24.))
                    .child(crate::icon(crate::ALERT_ICON, 18., colors.muted))
                    .child(
                        div()
                            .max_w(px(320.))
                            .text_center()
                            .text_size(text_px(TEXT_CONTROL))
                            .text_color(rgb(colors.muted))
                            .child(error.clone()),
                    )
                    .map(|state| match self.engine_recovery(error, cx) {
                        Some(recovery) => state.child(recovery),
                        None if has_page => state.child(
                            crate::ui_button("browser-retry", crate::ButtonKind::Secondary, colors)
                                .h(px(CONTROL_HEIGHT_COMPACT))
                                .on_click(cx.listener(|pane, _, window, cx| pane.retry(window, cx)))
                                .child("Reload"),
                        ),
                        None => state,
                    }),
            );
        } else if has_page {
            if let Some(native) = self.display.native() {
                let surface = NativeSurface {
                    native,
                    bounds: self.display.bounds.clone(),
                    events: self.display.events.clone(),
                    visible: self.display.native_visible.get(),
                    generation: self.display.generation.get(),
                };
                self.display.picture_drawn(window);
                pane = match self.display.snapshot.borrow().clone() {
                    // Behind the native slot, where the page left it.
                    Some(snapshot) => pane.child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                div().absolute().inset_0().child(
                                    gpui::img(snapshot)
                                        .size_full()
                                        .object_fit(gpui::ObjectFit::Cover),
                                ),
                            )
                            .child(surface),
                    ),
                    None => pane.child(surface),
                };
            } else {
                pane = pane.child(div().flex_1());
            }
        } else {
            pane = pane.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .p(px(24.))
                    .text_center()
                    .child(
                        div()
                            .text_size(text_px(TEXT_HEADING))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Preview your app"),
                    )
                    .child(
                        div()
                            .text_size(text_px(TEXT_LABEL))
                            .text_color(rgb(colors.muted))
                            .child("Enter a URL above, or open a local development server."),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .flex()
                            .flex_wrap()
                            .justify_center()
                            .gap(px(8.))
                            .children([3000_u32, 5173, 8080].map(|port| {
                                crate::ui_button(
                                    ("browser-localhost", port),
                                    crate::ButtonKind::Secondary,
                                    colors,
                                )
                                .h(px(CONTROL_HEIGHT_COMPACT))
                                .font_family(crate::MONO)
                                .text_size(text_px(TEXT_LABEL))
                                .font_weight(FontWeight::NORMAL)
                                .child(format!("localhost:{port}"))
                                .on_click(cx.listener(
                                    move |pane, _, _, cx| {
                                        pane.navigate(&format!("http://localhost:{port}"), cx);
                                    },
                                ))
                            })),
                    ),
            );
        }
        pane
    }
}

/// Shared native visibility permits pane changes without mutating child entities.
struct TabDisplay {
    /// Empty when the engine could not start; see [`BrowserTab::start_native`].
    native: RefCell<Option<Rc<NativeBrowser>>>,
    /// Set once the tab is shut down, after which no view may be created.
    shut_down: Cell<bool>,
    events: NativeEvents,
    bounds: Rc<Cell<Option<NativeAllocation>>>,
    visible: Cell<bool>,
    covered: Cell<bool>,
    /// The page as it looked when it was covered, drawn in its place.
    snapshot: RefCell<Option<Arc<RenderImage>>>,
    /// Counts native visibility requests, so a late reply to an earlier one
    /// cannot change the picture.
    generation: Cell<u64>,
    /// The capture request whose view is still on screen above any popup.
    awaiting_picture: Cell<Option<u64>>,
    /// The capture request the current picture answers.
    picture_generation: Cell<Option<u64>>,
    native_visible: Cell<bool>,
    has_page: Cell<bool>,
    healthy: Cell<bool>,
}

impl TabDisplay {
    fn native(&self) -> Option<Rc<NativeBrowser>> {
        self.native.borrow().clone()
    }

    fn set_visible(&self, visible: bool) {
        self.set_state(visible, self.covered.get());
    }

    fn set_state(&self, visible: bool, covered: bool) {
        self.visible.set(visible);
        self.covered.set(covered);
        let capture = visible && self.keeps_picture();
        let shown = visible && !covered && self.has_page.get() && self.healthy.get();
        // A covered page keeps its picture even while its tab is out of view,
        // since a hidden view cannot be captured again. A page being shown
        // again keeps it until it is back on screen, so the slot does not
        // flash empty in between.
        if !self.keeps_picture() && !shown {
            self.snapshot.take();
        }
        if self.native_visible.replace(shown) == shown {
            // A view left on screen for its picture must still go at once
            // when the pane does, rather than when its picture is drawn.
            if !capture && self.awaiting_picture.get().is_some() {
                self.request(false, false);
            }
            return;
        }
        self.bounds.set(None);
        self.request(shown, capture);
    }

    /// Ask the native view to show or hide. Asking it to capture leaves it on
    /// screen and sends a picture back; GPUI hides it once it draws that.
    fn request(&self, shown: bool, capture: bool) {
        // Replies to earlier requests no longer describe the view.
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.awaiting_picture.set(capture.then_some(generation));
        let Some(native) = self.native() else {
            return;
        };
        let events = self.events.clone();
        native.with_view(move |view| {
            // Capture while still on screen: an unmapped window has no pixels.
            if capture && let Some(frame) = capture_view(view) {
                events.send(NativeEvent::Snapshot { generation, frame });
                return;
            }
            set_native_visible(view, shown, &events);
        });
    }

    fn keeps_picture(&self) -> bool {
        self.covered.get() && self.has_page.get() && self.healthy.get()
    }

    /// Keep the picture captured for request `generation`, unless a later
    /// request has taken over. Its view is hidden once a frame shows the
    /// picture (see [`Self::picture_drawn`]), or after a fallback delay.
    fn take_snapshot(
        self: &Rc<Self>,
        generation: u64,
        frame: image::RgbaImage,
        cx: &mut Context<BrowserTab>,
    ) -> bool {
        if generation != self.generation.get() {
            return false;
        }
        if self.keeps_picture() {
            *self.snapshot.borrow_mut() =
                Some(Arc::new(RenderImage::new(vec![image::Frame::new(frame)])));
            self.picture_generation.set(Some(generation));
        }
        let display = self.clone();
        cx.spawn(async move |_, _| {
            Timer::after(PICTURE_FALLBACK_DELAY).await;
            display.finish_handoff(generation);
        })
        .detach();
        true
    }

    /// The capture request whose own picture is ready to replace its view.
    /// An older picture, still shown until the new one arrives, does not count.
    fn handoff_ready(&self) -> Option<u64> {
        self.awaiting_picture
            .get()
            .filter(|generation| self.picture_generation.get() == Some(*generation))
            .filter(|_| self.snapshot.borrow().is_some())
    }

    /// The tab drew the picture in a frame; hide the view once it is shown.
    fn picture_drawn(self: &Rc<Self>, window: &mut Window) {
        if let Some(generation) = self.handoff_ready() {
            let display = self.clone();
            // Runs before the next frame, so after this one is presented.
            window.on_next_frame(move |_, _| display.finish_handoff(generation));
        }
    }

    fn finish_handoff(&self, generation: u64) {
        if self.awaiting_picture.get() == Some(generation) && self.generation.get() == generation {
            self.request(false, false);
        }
    }

    fn shutdown(&self) {
        self.shut_down.set(true);
        self.set_visible(false);
        if let Some(native) = self.native() {
            native.dispose();
        }
    }
}

/// Hides a page awaiting its picture even if no frame comes to draw it.
const PICTURE_FALLBACK_DELAY: Duration = Duration::from_millis(250);

fn set_native_visible(view: &NativeView, visible: bool, events: &NativeEvents) {
    if !visible {
        blur_native_view(view);
    }
    if let Err(error) = view.set_visible(visible) {
        events.send(NativeEvent::Error(error.to_string()));
    }
}

/// The view's current pixels in GPUI's BGRA order, where the platform can
/// read them back. Elsewhere a covered page leaves its slot blank.
fn capture_view(view: &NativeView) -> Option<image::RgbaImage> {
    #[cfg(target_os = "linux")]
    return view.capture();
    #[cfg(not(target_os = "linux"))]
    {
        let _ = view;
        None
    }
}

/// A handle to a native view living on the thread its toolkit requires: GPUI's
/// on macOS and Windows, [`browser_thread`] on Linux.
struct NativeBrowser {
    #[cfg(target_os = "linux")]
    id: u64,
    // Keep the context alive even if a previously painted layout element is
    // still holding the native view after the owning entity is released.
    #[cfg(not(target_os = "linux"))]
    entry: RefCell<Option<NativeEntry>>,
    /// Calls made while Windows is still creating the view, applied in order
    /// once it exists. `None` once the view is there or has failed.
    #[cfg(target_os = "windows")]
    pending: RefCell<Option<Vec<PendingCall>>>,
    disposed: Cell<bool>,
}

#[cfg(target_os = "windows")]
type PendingCall = Box<dyn FnOnce(&NativeView)>;

impl NativeBrowser {
    fn is_disposed(&self) -> bool {
        self.disposed.get()
    }

    /// Apply a call to the view, in submission order. On Linux this returns
    /// before the call runs, so outcomes must be reported as a [`NativeEvent`].
    /// `Send` holds on every platform so that shared callers stay portable.
    fn with_view(&self, apply: impl FnOnce(&NativeView) + Send + 'static) {
        if self.is_disposed() {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let id = self.id;
            browser_thread::post(move || {
                BROWSER_VIEWS.with(|views| {
                    if let Some((view, _)) = views.borrow().get(&id) {
                        apply(view);
                    }
                });
            });
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        if let Some((view, _)) = self.entry.borrow().as_ref() {
            apply(view);
        }
        #[cfg(target_os = "windows")]
        match self.entry.borrow().as_ref() {
            Some((view, _)) => apply(view),
            // Still being created: kept for when it exists.
            None => {
                if let Some(pending) = self.pending.borrow_mut().as_mut() {
                    pending.push(Box::new(apply));
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn create(window: &Window, events: NativeEvents, _: &mut App) -> Result<Rc<Self>, String> {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let scale = window.scale_factor();
        let _ = window;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (reply, created) = std::sync::mpsc::channel();
        browser_thread::post(move || {
            let created_entry =
                chromium::ChromiumView::create(events, scale).map(|view| (view, ()));
            let result = created_entry.map(|entry| {
                BROWSER_VIEWS.with(|views| views.borrow_mut().insert(id, entry));
            });
            let _ = reply.send(result);
        });
        let Ok(result) = created.recv_timeout(NATIVE_REPLY_TIMEOUT) else {
            // Jobs run in order, so this runs once the slow creation is done.
            // A view it made anyway has no owner left to close it.
            browser_thread::post(move || {
                let entry = BROWSER_VIEWS.with(|views| views.borrow_mut().remove(&id));
                if let Some((view, ())) = entry {
                    view.close(CEF_CLOSE_TIMEOUT);
                }
            });
            return Err("The browser did not start. Restart xd to try again.".into());
        };
        result?;
        Ok(Rc::new(Self {
            id,
            disposed: Cell::new(false),
        }))
    }

    #[cfg(target_os = "macos")]
    fn create(window: &Window, events: NativeEvents, _: &mut App) -> Result<Rc<Self>, String> {
        let parent = NativeParent(
            HasWindowHandle::window_handle(window)
                .map_err(|error| error.to_string())?
                .as_raw(),
        );
        Ok(Rc::new(Self {
            entry: RefCell::new(Some(create_native(parent, events)?)),
            disposed: Cell::new(false),
        }))
    }

    /// Returns at once, with the view still to come.
    ///
    /// WebView2 hands over a view through the window's message queue, and wry
    /// waits for it by running that queue. This is called while GPUI draws,
    /// with the whole app borrowed, and the queue also carries GPUI's own
    /// timers and tasks: the first to touch the app while it was borrowed
    /// panicked, and a panic inside a window procedure ends the process. The
    /// pane was already saved as open by then, so the next launch created the
    /// view the same way and died the same way.
    ///
    /// So the view is created later, from GPUI's message loop, where nothing
    /// is borrowed and the queue can run freely.
    #[cfg(target_os = "windows")]
    fn create(window: &Window, events: NativeEvents, cx: &mut App) -> Result<Rc<Self>, String> {
        let parent = NativeParent(
            HasWindowHandle::window_handle(window)
                .map_err(|error| error.to_string())?
                .as_raw(),
        );
        let native = Rc::new(Self {
            entry: RefCell::new(None),
            pending: RefCell::new(Some(Vec::new())),
            disposed: Cell::new(false),
        });
        let created = Rc::downgrade(&native);
        deferred::run(
            move || {
                // A tab closed before its turn came needs no view.
                if created.upgrade().is_none_or(|native| native.is_disposed()) {
                    return;
                }
                let entry = create_native(parent, events.clone());
                let Some(native) = created.upgrade() else {
                    return;
                };
                match entry {
                    Ok(entry) => native.attach(entry),
                    Err(error) => {
                        native.pending.borrow_mut().take();
                        events.send(NativeEvent::EngineFailed(error));
                    }
                }
            },
            cx,
        );
        Ok(native)
    }

    /// Take the created view and apply what was asked of it meanwhile.
    #[cfg(target_os = "windows")]
    fn attach(&self, entry: NativeEntry) {
        let pending = self.pending.borrow_mut().take().unwrap_or_default();
        // Closed while it was being created: dropping the entry closes it.
        if self.is_disposed() {
            return;
        }
        *self.entry.borrow_mut() = Some(entry);
        for call in pending {
            if let Some((view, _)) = self.entry.borrow().as_ref() {
                call(view);
            }
        }
    }

    /// Destroy the view before returning: callers may be about to destroy the
    /// parent window, and the X11 child must not outlive it.
    fn dispose(&self) {
        if self.disposed.replace(true) {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let id = self.id;
            let (reply, disposed) = std::sync::mpsc::channel();
            browser_thread::post(move || {
                let entry = BROWSER_VIEWS.with(|views| views.borrow_mut().remove(&id));
                #[cfg(target_os = "linux")]
                if let Some((view, ())) = entry {
                    view.close(CEF_CLOSE_TIMEOUT);
                }
                let _ = reply.send(());
            });
            let _ = disposed.recv_timeout(NATIVE_REPLY_TIMEOUT);
        }
        #[cfg(target_os = "windows")]
        self.pending.borrow_mut().take();
        #[cfg(not(target_os = "linux"))]
        drop(self.entry.borrow_mut().take());
    }
}

/// Work that has to run from GPUI's message loop on Windows, outside any
/// update of the app, one piece at a time.
#[cfg(target_os = "windows")]
mod deferred {
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
    };

    use gpui::App;

    thread_local! {
        static QUEUE: RefCell<VecDeque<Box<dyn FnOnce()>>> = const { RefCell::new(VecDeque::new()) };
        static DRAINING: Cell<bool> = const { Cell::new(false) };
    }

    /// Run `job` after every job queued before it has finished.
    ///
    /// A job may run the message queue itself, and another job queued from
    /// inside that must not start until the first is done: creating two
    /// WebView2 views at once would borrow their shared context twice.
    pub(super) fn run(job: impl FnOnce() + 'static, cx: &mut App) {
        QUEUE.with(|queue| queue.borrow_mut().push_back(Box::new(job)));
        if DRAINING.replace(true) {
            return;
        }
        // A spawned task is first polled from the message loop, never here.
        cx.spawn(async move |_| {
            while let Some(job) = QUEUE.with(|queue| queue.borrow_mut().pop_front()) {
                job();
            }
            DRAINING.set(false);
        })
        .detach();
    }
}

/// How long GPUI waits for the two calls it cannot leave pending. The browser
/// thread never waits on GPUI, so reaching this means Chromium itself is stuck.
#[cfg(target_os = "linux")]
const NATIVE_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Shut the browser engine down before the process exits, once every pane has
/// been shut down, so Chromium writes out website data such as sign-ins.
#[cfg(target_os = "linux")]
pub fn shutdown_engine() {
    let (reply, done) = std::sync::mpsc::channel();
    browser_thread::post(move || {
        chromium::shutdown();
        let _ = reply.send(());
    });
    let _ = done.recv_timeout(NATIVE_REPLY_TIMEOUT);
}

/// The system web views on macOS and Windows need no engine shutdown.
#[cfg(not(target_os = "linux"))]
pub fn shutdown_engine() {}

/// Whether an engine that failed to start may start on a later try, because
/// another process held its profile.
#[cfg(target_os = "linux")]
fn engine_may_start_later() -> bool {
    chromium::profile_held_elsewhere()
}

#[cfg(not(target_os = "linux"))]
fn engine_may_start_later() -> bool {
    false
}

/// How often a tab waiting for another process's browser checks again.
const ENGINE_RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// How long closing a view waits for CEF. Shorter than GPUI's own wait, so a
/// stuck close still answers GPUI before it gives up on the browser thread.
#[cfg(target_os = "linux")]
const CEF_CLOSE_TIMEOUT: Duration = Duration::from_secs(4);

type NativeAllocation = (Bounds<Pixels>, f32, gpui::Point<Pixels>);

struct NativeSurface {
    native: Rc<NativeBrowser>,
    bounds: Rc<Cell<Option<NativeAllocation>>>,
    events: NativeEvents,
    visible: bool,
    /// The visibility request this frame describes, echoed once the view is
    /// placed so its picture is dropped only when the page is in position.
    generation: u64,
}

impl IntoElement for NativeSurface {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for NativeSurface {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.flex_grow = 1.;
        style.flex_shrink = 1.;
        style.min_size.height = px(0.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Hitbox {
        let allocation = (bounds, window.scale_factor(), window.bounds().origin);
        if self.visible && self.bounds.get() != Some(allocation) {
            let geometry_changed = self
                .bounds
                .get()
                .is_none_or(|previous| previous.0 != bounds || previous.1 != allocation.1);
            #[cfg(target_os = "linux")]
            let rect = chromium::PhysicalBounds::new(
                f64::from(bounds.origin.x),
                f64::from(bounds.origin.y),
                f64::from(bounds.size.width.max(px(1.))),
                f64::from(bounds.size.height.max(px(1.))),
                f64::from(allocation.1),
            );
            #[cfg(not(target_os = "linux"))]
            let rect = Rect {
                position: wry::dpi::LogicalPosition::new(
                    f64::from(bounds.origin.x),
                    f64::from(bounds.origin.y),
                )
                .into(),
                size: wry::dpi::LogicalSize::new(
                    f64::from(bounds.size.width.max(px(1.))),
                    f64::from(bounds.size.height.max(px(1.))),
                )
                .into(),
            };
            let events = self.events.clone();
            let generation = self.generation;
            self.native.with_view(move |view| {
                if geometry_changed && let Err(error) = view.set_bounds(rect) {
                    events.send(NativeEvent::Error(error.to_string()));
                }
                #[cfg(target_os = "windows")]
                {
                    use wry::WebViewExtWindows;
                    // Child embedding leaves parent move notifications to us.
                    unsafe {
                        let _ = view.controller().NotifyParentWindowPositionChanged();
                    }
                }
                events.send(NativeEvent::Shown { generation });
            });
            self.bounds.set(Some(allocation));
        }
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut Hitbox,
        window: &mut Window,
        _: &mut App,
    ) {
        let native = self.native.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, _| {
            if phase == DispatchPhase::Capture && !bounds.contains(&event.position) {
                #[cfg(target_os = "linux")]
                {
                    native.with_view(blur_native_view);
                    _window.activate_window();
                }
                #[cfg(not(target_os = "linux"))]
                native.with_view(|view| {
                    let _ = view.focus_parent();
                });
            }
        });
    }
}

/// A borrowed OS parent handle.
#[cfg(not(target_os = "linux"))]
struct NativeParent(RawWindowHandle);

#[cfg(not(target_os = "linux"))]
impl HasWindowHandle for NativeParent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: the GPUI window owns this handle and outlives its child view.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

#[cfg(not(target_os = "linux"))]
fn create_native(parent: NativeParent, events: NativeEvents) -> Result<NativeEntry, String> {
    let context = SHARED_BROWSER_CONTEXT.with(|shared| {
        shared
            .borrow_mut()
            .get_or_insert_with(|| {
                let context = WebContext::new(Some(browser_data_directory()));
                Rc::new(RefCell::new(context))
            })
            .clone()
    });
    let mut context_ref = context.borrow_mut();
    let load_events = events.clone();
    let title_events = events.clone();
    let popup_events = events.clone();
    let builder = WebViewBuilder::new_with_web_context(&mut context_ref);
    let native = builder
        .with_back_forward_navigation_gestures(true)
        .with_visible(false)
        .with_focused(false)
        .with_bounds(Rect {
            position: wry::dpi::LogicalPosition::new(0., 0.).into(),
            size: wry::dpi::LogicalSize::new(1., 1.).into(),
        })
        .with_navigation_handler(|url| is_browser_url(&url) || url == "about:blank")
        .with_on_page_load_handler(move |event, url| {
            if is_browser_url(&url) {
                let event = match event {
                    PageLoadEvent::Started => NativeEvent::Started(url),
                    PageLoadEvent::Finished => NativeEvent::Finished(url),
                };
                load_events.send(event);
            }
        })
        .with_document_title_changed_handler(move |title| {
            title_events.title_changed(title);
        })
        .with_new_window_req_handler(move |url, _| {
            if is_browser_url(&url) {
                popup_events.send(NativeEvent::Open {
                    url,
                    by_agent: false,
                });
            }
            NewWindowResponse::Deny
        })
        .build_as_child(&parent)
        .map_err(creation_error)?;
    // Hide the fully constructed child before trusting our visibility cache.
    native
        .set_visible(false)
        .map_err(|error| format!("Could not hide the browser: {error}"))?;
    drop(context_ref);
    Ok((native, context))
}

#[cfg(target_os = "macos")]
fn creation_error(error: wry::Error) -> String {
    format!("Could not create the browser: {error}")
}

/// What a tab shows when Windows has no WebView2 Runtime to draw pages with.
#[cfg(any(target_os = "windows", test))]
const WEBVIEW2_MISSING: &str = "The browser needs Microsoft's WebView2 Runtime, which this PC does not have. Install it, then try again.";

/// Microsoft's own installer for the WebView2 Runtime.
#[cfg(target_os = "windows")]
const WEBVIEW2_INSTALLER: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

#[cfg(target_os = "windows")]
fn creation_error(error: wry::Error) -> String {
    // wry waits for the view by running the message queue, and reports a quit
    // it finds there as a cancelled task. It took the quit off the queue to
    // do so: put it back, or the window closes and the process stays.
    if matches!(&error, wry::Error::WebView2Error(inner) if format!("{inner:?}") == "TaskCanceled")
    {
        crate::windows::repost_quit();
    }
    if wry::webview_version().is_err() {
        WEBVIEW2_MISSING.into()
    } else {
        format!("Could not create the browser: {error}")
    }
}

#[cfg(target_os = "linux")]
fn blur_native_view(view: &NativeView) {
    view.blur();
}

#[cfg(not(target_os = "linux"))]
fn blur_native_view(_: &NativeView) {}

fn browser_data_directory() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
        .join(xd_desktop::channel::data_name())
        .join("browser")
}

/// GPUI 0.2.2's Linux HasWindowHandle implementation is unimplemented. xd
/// owns one desktop window, whose PID and WM_CLASS GPUI publishes on X11.
/// Resolve that identity through the public X11 protocol without assuming
/// anything about GPUI's internal Rust struct layout.
#[cfg(target_os = "linux")]
pub(crate) fn find_x11_parent() -> Result<u32, String> {
    use x11rb::{
        connection::Connection,
        protocol::xproto::{AtomEnum, ConnectionExt},
    };

    fn lookup() -> Result<u32, Box<dyn std::error::Error>> {
        let (connection, _) = x11rb::connect(None)?;
        let pid_atom = connection.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
        let app_id = xd_desktop::channel::app_id();
        let mut candidates = Vec::new();
        let mut pending = connection
            .setup()
            .roots
            .iter()
            .map(|screen| (screen.root, 0))
            .collect::<std::collections::VecDeque<_>>();
        let mut visited = 0;
        while let Some((window, depth)) = pending.pop_front() {
            visited += 1;
            if visited > 4096 {
                return Err("Too many X11 windows to identify the xd browser parent".into());
            }
            // A window can disappear between the tree query and properties.
            let matches = (|| -> Result<bool, Box<dyn std::error::Error>> {
                let pid = connection
                    .get_property(false, window, pid_atom, AtomEnum::CARDINAL, 0, 1)?
                    .reply()?;
                if pid.value32().and_then(|mut values| values.next()) != Some(std::process::id()) {
                    return Ok(false);
                }
                let class = connection
                    .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)?
                    .reply()?;
                Ok(class
                    .value
                    .split(|byte| *byte == 0)
                    .any(|part| part == app_id.as_bytes()))
            })()
            .unwrap_or(false);
            if matches {
                candidates.push(window);
            } else if depth < 4
                && let Ok(tree) = connection.query_tree(window)?.reply()
            {
                pending.extend(tree.children.into_iter().map(|window| (window, depth + 1)));
            }
        }
        match candidates.as_slice() {
            [window] => Ok(*window),
            [] => Err("Could not locate xd's X11 window".into()),
            _ => Err("The browser cannot select a parent from multiple xd windows".into()),
        }
    }

    lookup().map_err(|error| format!("Could not embed the browser: {error}"))
}

fn is_browser_url(address: &str) -> bool {
    Url::parse(address)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
}

fn normalize_url(address: &str) -> Result<String, String> {
    let address = address.trim();
    if address.is_empty() {
        return Err("Enter a URL to open a page.".into());
    }
    if address.chars().any(char::is_whitespace) {
        return Err("Enter a URL without spaces.".into());
    }
    let url = if address.contains("://") {
        Url::parse(address)
    } else {
        // A colon can identify either a URL scheme or a host's port number.
        if let Some((prefix, suffix)) = address.split_once(':')
            && !prefix.contains('.')
            && prefix != "localhost"
            && !prefix.starts_with('[')
            && suffix
                .split('/')
                .next()
                .is_none_or(|port| port.parse::<u16>().is_err())
        {
            return Err("This browser supports HTTP and HTTPS URLs.".into());
        }
        let candidate = Url::parse(&format!("https://{address}"))
            .map_err(|_| "Enter a valid URL.".to_string())?;
        let local = match candidate.host() {
            Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
            Some(url::Host::Ipv4(host)) => host.is_loopback(),
            Some(url::Host::Ipv6(host)) => host.is_loopback(),
            None => false,
        };
        Url::parse(&format!(
            "{}://{address}",
            if local { "http" } else { "https" }
        ))
    }
    .map_err(|_| "Enter a valid URL.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err("This browser supports HTTP and HTTPS URLs.".into());
    }
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn disabling_tools_restores_the_browser_and_removes_tool_tabs(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                None,
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.update(|_, cx| {
            pane.update(cx, |pane, cx| {
                pane.configure_side_tabs(Some(SideTab::Files), true, cx);
                assert_eq!(pane.active_side_tab(), Some(SideTab::Files));
                pane.configure_side_tabs(Some(SideTab::Files), false, cx);
                assert!(pane.active_side_tab().is_none());
                assert!(pane.auxiliary_tabs.is_empty());
                assert!(pane.tabs[0].display.visible.get());
            })
        });
    }

    #[gpui::test]
    fn side_tools_hide_webviews_and_keep_page_drafts(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                None,
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.tabs[0]
                    .entity
                    .update(cx, |tab, _| tab.draft = "unfinished.localhost".into());
                pane.configure_side_tabs(Some(SideTab::Files), true, cx);
                assert!(!pane.tabs[0].display.visible.get());
                pane.select_tab(0, window, cx);
                assert!(pane.auxiliary.is_none());
                assert!(pane.tabs[0].display.visible.get());
                assert_eq!(pane.tabs[0].entity.read(cx).draft, "unfinished.localhost");
                pane.select_auxiliary(Some(SideTab::Changes), cx);
                pane.add_tab(None, window, cx);
                assert!(pane.auxiliary.is_none());
                assert_eq!(pane.session.active_tab, 1);
                assert!(pane.tabs[1].display.visible.get());
                assert_eq!(pane.auxiliary_tabs, vec![SideTab::Files, SideTab::Changes]);
            })
        });
    }

    #[gpui::test]
    fn a_covered_page_shows_only_its_latest_picture(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                Some("http://localhost:3000/".into()),
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.update(|_, cx| {
            pane.update(cx, |pane, cx| {
                let tab = pane.tabs[0].entity.clone();
                let display = pane.tabs[0].display.clone();
                let mut send = |event| tab.update(cx, |tab, cx| tab.native_event(event, cx));
                let snapshot = |generation| NativeEvent::Snapshot {
                    generation,
                    frame: image::RgbaImage::new(1, 1),
                };
                let has_picture = || display.snapshot.borrow().is_some();
                assert!(display.native_visible.get());
                let shown = display.generation.get();

                // Covering asks for a picture and leaves the view up for it.
                pane.set_covered(true);
                assert!(display.visible.get());
                assert!(!display.native_visible.get());
                let cover = display.generation.get();
                assert_eq!(display.awaiting_picture.get(), Some(cover));
                send(snapshot(shown));
                assert!(!has_picture());
                send(snapshot(cover));
                assert!(has_picture());
                assert_eq!(display.handoff_ready(), Some(cover));
                display.finish_handoff(cover);
                assert_eq!(display.awaiting_picture.get(), None);

                // Covered again before the live page returned, the old
                // picture stays as a backdrop but cannot stand in for the
                // new capture.
                pane.set_covered(false);
                assert!(has_picture());
                pane.set_covered(true);
                let recover = display.generation.get();
                assert_eq!(display.awaiting_picture.get(), Some(recover));
                assert!(has_picture());
                assert_eq!(display.handoff_ready(), None);
                send(snapshot(recover));
                assert_eq!(display.handoff_ready(), Some(recover));
                display.finish_handoff(recover);

                // Uncovered, the picture waits for the live page to return,
                // and replies to earlier requests do not count.
                pane.set_covered(false);
                let reveal = display.generation.get();
                assert!(has_picture());
                send(NativeEvent::Shown { generation: cover });
                assert!(has_picture());
                send(snapshot(cover));
                send(NativeEvent::Shown { generation: reveal });
                assert!(!has_picture());

                // Leaving the chat while the view waits for its picture hides
                // it at once, and its late picture is not kept.
                pane.set_covered(true);
                assert!(display.awaiting_picture.get().is_some());
                let cover = display.generation.get();
                pane.set_visible(false);
                assert_eq!(display.awaiting_picture.get(), None);
                assert_ne!(display.generation.get(), cover);
                send(snapshot(cover));
                assert!(!has_picture());

                // A hidden view cannot be captured again, so a picture
                // outlasts its tab leaving view while the popup stays open.
                pane.set_visible(true);
                pane.set_covered(false);
                pane.set_covered(true);
                send(snapshot(display.generation.get()));
                assert!(has_picture());
                pane.set_visible(false);
                pane.set_visible(true);
                assert!(has_picture());
                // Closing the popup while the tab is away drops it.
                pane.set_visible(false);
                pane.set_covered(false);
                assert!(!has_picture());
            })
        });
    }

    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn history_gestures_ignore_hidden_tabs_and_cross_document_momentum(
        cx: &mut gpui::TestAppContext,
    ) {
        let (tab, cx) = cx.add_window_view(|window, cx| {
            BrowserTab::new(
                Some("https://example.com/".into()),
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.update(|_, cx| {
            tab.update(cx, |tab, cx| {
                tab.native_event(NativeEvent::HistoryGesture(true), cx);
                assert!(tab.last_history_gesture.is_none());
                tab.display.native_visible.set(true);
                tab.display.covered.set(true);
                tab.native_event(NativeEvent::HistoryGesture(true), cx);
                assert!(tab.last_history_gesture.is_none());
                tab.display.covered.set(false);
                tab.native_event(NativeEvent::HistoryGesture(true), cx);
                let generation = tab.loading_generation;
                assert!(tab.last_history_gesture.is_some());
                tab.native_event(
                    NativeEvent::Started("https://example.com/previous".into()),
                    cx,
                );
                let started = tab.loading_generation;
                assert!(started > generation);
                tab.display.native_visible.set(true);
                tab.native_event(NativeEvent::HistoryGesture(true), cx);
                assert_eq!(tab.loading_generation, started);
            });
        });
    }

    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn the_page_on_screen_hands_the_keyboard_back_for_the_apps_keys_and_past_its_ends(
        cx: &mut gpui::TestAppContext,
    ) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                Some("example.com".into()),
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        let shortcuts = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let shortcuts = shortcuts.clone();
            cx.subscribe(&pane, move |_, event: &BrowserEvent, _| {
                if let BrowserEvent::AppShortcut(keys) = event {
                    shortcuts.borrow_mut().push(*keys);
                }
            })
            .detach();
        });
        let tab = pane.read_with(cx, |pane, _| pane.tabs[0].entity.clone());
        let send = |cx: &mut gpui::VisualTestContext, event: NativeEvent| {
            cx.update(|_, cx| tab.update(cx, |tab, cx| tab.native_event(event, cx)));
            cx.run_until_parked();
        };
        // A page that is not on screen has no keyboard to give back.
        tab.read_with(cx, |tab, _| tab.display.native_visible.set(false));
        send(cx, NativeEvent::AppShortcut("ctrl-l"));
        assert!(shortcuts.borrow().is_empty());
        tab.read_with(cx, |tab, _| tab.display.native_visible.set(true));
        send(cx, NativeEvent::AppShortcut("ctrl-l"));
        send(cx, NativeEvent::AppShortcut("shift-escape"));
        assert_eq!(*shortcuts.borrow(), ["ctrl-l", "shift-escape"]);
        // Tab past the page's last control goes to the address field.
        send(cx, NativeEvent::TakeFocus);
        cx.update(|window, cx| {
            let tab = tab.read(cx);
            assert!(tab.address.read(cx).focus_handle(cx).is_focused(window));
        });
    }

    #[gpui::test]
    fn retrying_after_a_bad_address_returns_to_the_open_page(cx: &mut gpui::TestAppContext) {
        let (tab, cx) = cx.add_window_view(|window, cx| {
            BrowserTab::new(
                Some("http://localhost:3000/".into()),
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| {
            tab.update(cx, |tab, cx| {
                tab.navigate("ftp://example.invalid/", cx);
                assert!(tab.error.is_some());
                tab.retry(window, cx);
                assert!(tab.error.is_none());
                assert_eq!(tab.draft, "http://localhost:3000/");
                assert_eq!(tab.current_url().as_deref(), Some("http://localhost:3000/"));
            })
        });
    }

    #[gpui::test]
    fn clicking_the_address_field_focuses_it_and_accepts_text(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserTab::new(
                None,
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        cx.simulate_click(gpui::point(px(150.), px(21.)), gpui::Modifiers::default());
        cx.simulate_input("localhost:3000");
        cx.run_until_parked();

        cx.update(|window, cx| {
            let pane = pane.read(cx);
            assert!(pane.address.read(cx).focus_handle(cx).is_focused(window));
            assert_eq!(pane.draft, "localhost:3000");
        });
    }

    #[gpui::test]
    fn a_stopped_agent_command_opens_no_tab(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                Some("https://example.com/".into()),
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let (answer, answered) = std::sync::mpsc::channel();
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.run_agent_command(
                    serde_json::json!({"action": "navigate", "url": "localhost:3000",
                        "new_tab": true}),
                    Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    move |outcome| answer.send(outcome).unwrap(),
                    window,
                    cx,
                );
            });
        });
        assert_eq!(
            answered.try_recv().unwrap(),
            Err("The command was stopped.".into())
        );
        cx.update(|_, cx| assert_eq!(pane.read(cx).session().tabs.len(), 1));
    }

    #[gpui::test]
    fn an_agents_new_tab_leaves_the_keyboard_where_it_is(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                Some("https://example.com/".into()),
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let (answer, answered) = std::sync::mpsc::channel();
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.run_agent_command(
                    serde_json::json!({"action": "navigate", "url": "localhost:3000",
                        "new_tab": true}),
                    Arc::default(),
                    move |outcome| answer.send(outcome).unwrap(),
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        // A test window has no engine to run the command in.
        assert!(answered.try_recv().unwrap().is_err());
        cx.update(|window, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.session().active_tab, 1);
            let tab = pane.tabs[1].entity.read(cx);
            assert!(!tab.address.read(cx).focus_handle(cx).is_focused(window));
        });
    }

    #[gpui::test]
    fn a_tab_a_page_opens_for_an_agent_leaves_the_keyboard(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                None,
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let open_popup = |by_agent: bool, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                // Where the user is typing.
                let typing = pane.read(cx).tabs[0].entity.read(cx).address.clone();
                typing.read(cx).focus_handle(cx).focus(window);
                let first = pane.read(cx).tabs[0].entity.clone();
                first.update(cx, |_, cx| {
                    cx.emit(TabEvent::Open {
                        url: "https://example.com/popup".into(),
                        by_agent,
                    })
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let typing = pane.read(cx).tabs[0].entity.read(cx).address.clone();
                typing.read(cx).focus_handle(cx).is_focused(window)
            })
        };
        // As when an agent's click or script opens a popup.
        assert!(open_popup(true, cx));
        cx.update(|_, cx| assert_eq!(pane.read(cx).session().active_tab, 1));
        // The user's own popup takes the keyboard to its tab, as before.
        assert!(!open_popup(false, cx));
        cx.update(|_, cx| assert_eq!(pane.read(cx).session().active_tab, 2));
    }

    #[gpui::test]
    fn tabs_preserve_address_drafts_and_close_independently(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                None,
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        cx.simulate_click(gpui::point(px(150.), px(69.)), gpui::Modifiers::default());
        cx.simulate_input("first.localhost:3000");
        // With no side tools to offer, the add button opens a page at once.
        let plus_x = cx.update(|window, _| window.viewport_size().width - px(26.));
        cx.simulate_click(gpui::point(plus_x, px(24.)), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.simulate_input("second.localhost:5173");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.session().active_tab, 1);
            assert_eq!(pane.tabs[0].entity.read(cx).draft, "first.localhost:3000");
            assert_eq!(pane.tabs[1].entity.read(cx).draft, "second.localhost:5173");
            assert!(!pane.tabs[0].display.visible.get());
            assert!(pane.tabs[1].display.visible.get());
        });

        cx.simulate_click(gpui::point(px(50.), px(24.)), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.session().active_tab, 0);
            let tab = pane.tabs[0].entity.read(cx);
            assert_eq!(tab.draft, "first.localhost:3000");
            assert!(tab.address.read(cx).focus_handle(cx).is_focused(window));
            assert!(pane.tabs[0].display.visible.get());
            assert!(!pane.tabs[1].display.visible.get());
        });

        // Close the first tab using its button; the second tab remains intact.
        cx.simulate_click(gpui::point(px(144.), px(24.)), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.tabs[0].entity.read(cx).draft, "second.localhost:5173");
        });
        cx.simulate_click(gpui::point(px(144.), px(24.)), gpui::Modifiers::default());
        cx.run_until_parked();
        // Closing the only tab collapses the pane and leaves a blank tab behind.
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.session(), BrowserSession::default());
            assert!(!pane.is_visible());
            assert!(pane.tabs[0].entity.read(cx).draft.is_empty());
        });
    }

    #[gpui::test]
    fn chat_links_reuse_matching_tabs_and_popups_open_new_tabs(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                None,
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.open_url("localhost:3000", window, cx);
                pane.open_url("example.com/docs", window, cx);
                pane.open_url("localhost:3000", window, cx);
                assert_eq!(
                    pane.session(),
                    BrowserSession {
                        tabs: vec![
                            Some("http://localhost:3000/".into()),
                            Some("https://example.com/docs".into())
                        ],
                        active_tab: 0,
                    }
                );
                pane.tabs[0].entity.update(cx, |tab, cx| {
                    tab.native_event(
                        NativeEvent::Open {
                            url: "https://example.com/popup".into(),
                            by_agent: false,
                        },
                        cx,
                    );
                });
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.session().active_tab, 2);
            assert_eq!(
                pane.current_url().as_deref(),
                Some("https://example.com/popup")
            );
            assert_eq!(
                pane.session().tabs[0].as_deref(),
                Some("http://localhost:3000/")
            );
            assert_eq!(pane.tabs.len(), 3);
        });
    }

    #[gpui::test]
    fn restored_tab_sets_are_valid_and_keep_the_selected_page(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::with_session(
                BrowserSession {
                    tabs: vec![
                        Some("localhost:3000".into()),
                        None,
                        Some("example.com".into()),
                    ],
                    active_tab: usize::MAX,
                },
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.session().active_tab, 2);
            assert_eq!(pane.current_url().as_deref(), Some("https://example.com/"));
            assert_eq!(pane.session().tabs[1], None);
            assert!(!pane.tabs[0].display.visible.get());
            assert!(pane.tabs[2].display.visible.get());
        });
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.close_tab(0, window, cx);
                assert_eq!(pane.session().active_tab, 1);
                assert_eq!(pane.current_url().as_deref(), Some("https://example.com/"));
            });
        });
    }

    #[gpui::test]
    fn closing_the_only_tab_closes_the_pane_and_leaves_a_blank_tab(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::new(
                Some("example.com".into()),
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        let closed = Rc::new(Cell::new(0));
        cx.update(|_, cx| {
            let closed = closed.clone();
            cx.subscribe(&pane, move |_, event: &BrowserEvent, _| {
                if matches!(event, BrowserEvent::Close) {
                    closed.set(closed.get() + 1);
                }
            })
            .detach();
        });

        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.add_tab(None, window, cx);
                pane.close_tab(1, window, cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(closed.get(), 0, "another page is still open");

        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.configure_side_tabs(Some(SideTab::Files), true, cx);
                pane.close_tab(0, window, cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(closed.get(), 0, "the Files tab is still open");
        cx.update(|_, cx| assert!(pane.read(cx).is_visible()));

        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.close_auxiliary(SideTab::Files, cx);
                pane.navigate("example.com", cx);
                pane.close_tab(0, window, cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(closed.get(), 1);
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            assert!(!pane.is_visible());
            assert_eq!(pane.session(), BrowserSession::default());
            assert!(!pane.tabs[0].display.visible.get());
        });
    }

    #[gpui::test]
    fn selected_overflow_tabs_scroll_into_view(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserPane::with_session(
                BrowserSession {
                    tabs: vec![None; 32],
                    active_tab: 31,
                },
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        fn draw_frames(cx: &mut gpui::VisualTestContext) {
            // The test platform supplies no animation-frame callbacks.
            for _ in 0..2 {
                cx.update(|window, cx| {
                    window.refresh();
                    window.draw(cx).clear();
                });
            }
        }
        fn assert_selected_visible(pane: &BrowserPane) {
            let tab = pane
                .tab_scroll
                .bounds_for_item(pane.session.active_tab)
                .unwrap();
            let viewport = pane.tab_scroll.bounds();
            let offset = pane.tab_scroll.offset().x;
            assert!(tab.left() + offset >= viewport.left());
            assert!(tab.right() + offset <= viewport.right());
        }
        cx.run_until_parked();
        draw_frames(cx);
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            assert!(pane.tab_scroll.offset().x < px(0.));
            assert_selected_visible(pane);
        });
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| pane.select_tab(0, window, cx));
        });
        cx.run_until_parked();
        draw_frames(cx);
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.tab_scroll.offset().x, px(0.));
            assert_selected_visible(pane);
        });
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| pane.add_tab(None, window, cx));
        });
        cx.run_until_parked();
        draw_frames(cx);
        cx.update(|_, cx| assert_selected_visible(pane.read(cx)));
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| pane.close_tab(32, window, cx));
        });
        cx.run_until_parked();
        draw_frames(cx);
        cx.update(|_, cx| assert_selected_visible(pane.read(cx)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn browser_jobs_run_in_order_on_one_dedicated_thread() {
        let (reply, results) = std::sync::mpsc::channel();
        for index in 0..1_000 {
            let reply = reply.clone();
            browser_thread::post(move || {
                let _ = reply.send((index, std::thread::current().id()));
            });
        }
        let received = (0..1_000)
            .map(|_| results.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect::<Vec<_>>();
        let browser = received[0].1;
        assert_ne!(browser, std::thread::current().id());
        for (expected, (index, thread)) in received.into_iter().enumerate() {
            assert_eq!(index, expected);
            assert_eq!(thread, browser);
        }
    }

    #[test]
    fn native_events_can_cross_to_the_browser_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<NativeEvents>();
    }

    #[gpui::test]
    fn a_view_windows_could_not_create_leaves_the_tab_able_to_try_again(
        cx: &mut gpui::TestAppContext,
    ) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserTab::new(
                Some("http://localhost:3000".into()),
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        pane.update(cx, |pane, cx| {
            assert!(!pane.engine_failed);
            // Creation is reported later on Windows, not when the tab is made.
            pane.native_event(NativeEvent::EngineFailed(WEBVIEW2_MISSING.into()), cx);
            assert!(pane.engine_failed, "another try must create the view");
            assert_eq!(pane.error.as_deref(), Some(WEBVIEW2_MISSING));
            assert!(pane.display.native().is_none());
            assert!(!pane.loading);
            // The page it was opened for is still the one to load.
            assert_eq!(
                pane.current_url().as_deref(),
                Some("http://localhost:3000/")
            );
        });
    }

    #[gpui::test]
    fn navigation_polls_update_state_without_overriding_a_pending_load(
        cx: &mut gpui::TestAppContext,
    ) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserTab::new(
                None,
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        let poll = |url: &str| NativeEvent::Navigation {
            url: Some(url.into()),
            can_back: true,
            can_forward: false,
        };
        pane.update(cx, |pane, cx| {
            pane.native_event(poll("http://localhost:3000/spa"), cx);
            assert_eq!(
                pane.current_url().as_deref(),
                Some("http://localhost:3000/spa")
            );
            assert!(pane.can_back);
            assert!(!pane.can_forward);

            pane.navigate("http://localhost:3000/next", cx);
            // Queued before the load began, so it describes the old document.
            pane.loading = true;
            pane.native_event(poll("http://localhost:3000/old"), cx);
            assert_eq!(
                pane.current_url().as_deref(),
                Some("http://localhost:3000/next")
            );

            pane.loading = false;
            pane.native_event(
                NativeEvent::Navigation {
                    url: Some("about:blank".into()),
                    can_back: false,
                    can_forward: false,
                },
                cx,
            );
            assert_eq!(
                pane.current_url().as_deref(),
                Some("http://localhost:3000/next")
            );
            assert!(!pane.can_back);
        });
    }

    #[test]
    fn page_title_flood_has_bounded_work_and_preserves_navigation_order() {
        let (events, receiver) = NativeEvents::channel();
        events.send(NativeEvent::Started("https://example.com/".into()));
        for index in 0..10_000 {
            events.title_changed(format!("Page update {index}"));
        }
        events.send(NativeEvent::Finished("https://example.com/".into()));
        events.send(NativeEvent::Open {
            url: "https://example.com/popup".into(),
            by_agent: false,
        });
        events.send(NativeEvent::Error("Connection closed".into()));
        assert_eq!(receiver.len(), 5);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::Started(_)
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::TitleReady
        ));
        assert_eq!(events.take_title().as_deref(), Some("Page update 9999"));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::Finished(_)
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::Open { .. }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::Error(_)
        ));
        assert!(receiver.is_empty());

        // A second burst still produces only one wake. The tab suppresses
        // an unchanged display title after consuming that pending value.
        for _ in 0..10_000 {
            events.title_changed("Page update 9999".into());
        }
        assert_eq!(receiver.len(), 1);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::TitleReady
        ));
        assert_eq!(events.take_title().as_deref(), Some("Page update 9999"));
        events.title_changed("Final title".into());
        assert_eq!(receiver.len(), 1);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::TitleReady
        ));
        assert_eq!(events.take_title().as_deref(), Some("Final title"));
    }

    #[test]
    fn a_new_document_can_deliver_the_same_title_again() {
        let (events, receiver) = NativeEvents::channel();
        events.title_changed("Local app".into());
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::TitleReady
        ));
        assert_eq!(events.take_title().as_deref(), Some("Local app"));

        // Loading another document clears the displayed title. Its matching
        // title must still reach the tab, even after the previous wake drained.
        events.send(NativeEvent::Started("http://localhost:3000/another".into()));
        events.title_changed("Local app".into());
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::Started(_)
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            NativeEvent::TitleReady
        ));
        assert_eq!(events.take_title().as_deref(), Some("Local app"));
        assert!(receiver.is_empty());
    }

    #[test]
    fn addresses_select_http_for_loopback_and_https_for_websites() {
        for (input, expected) in [
            ("localhost:3000", "http://localhost:3000/"),
            ("127.0.0.1:5173/app", "http://127.0.0.1:5173/app"),
            ("[::1]:8080", "http://[::1]:8080/"),
            ("example.com/docs", "https://example.com/docs"),
            ("  https://example.com  ", "https://example.com/"),
        ] {
            assert_eq!(normalize_url(input).unwrap(), expected);
        }
    }

    #[test]
    fn addresses_reject_executable_schemes_and_invalid_input() {
        for input in [
            "javascript:alert(1)",
            "data:text/html,hello",
            "file:///etc/passwd",
            "ftp://example.com",
            "",
            "http://",
            "not a url",
        ] {
            assert!(normalize_url(input).is_err(), "{input}");
        }
    }

    #[gpui::test]
    fn load_completion_clears_a_timeout_but_preserves_a_network_failure(
        cx: &mut gpui::TestAppContext,
    ) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserTab::new(
                None,
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        pane.update(cx, |pane, cx| {
            pane.error = Some("Timed out".into());
            pane.load_timed_out = true;
            pane.native_event(NativeEvent::Finished("http://localhost:3000/".into()), cx);
            assert!(pane.error.is_none());
            assert_eq!(
                pane.current_url().as_deref(),
                Some("http://localhost:3000/")
            );

            pane.native_event(NativeEvent::Error("Connection refused".into()), cx);
            pane.native_event(NativeEvent::Finished("http://localhost:3000/".into()), cx);
            assert_eq!(pane.error.as_deref(), Some("Connection refused"));
            assert!(!pane.loading);
        });
    }

    #[gpui::test]
    fn same_document_history_does_not_arm_a_load_timeout(cx: &mut gpui::TestAppContext) {
        let (pane, cx) = cx.add_window_view(|window, cx| {
            BrowserTab::new(
                None,
                xd_desktop::theme::ThemePreset::Dark.colors(),
                window,
                cx,
            )
        });
        pane.update(cx, |pane, cx| {
            pane.navigate("http://localhost:3000/route-two", cx);
            pane.start_loading(cx);
            let old_generation = pane.loading_generation;
            pane.history(true, cx);
            assert!(!pane.loading);
            assert!(!pane.load_timed_out);
            assert_ne!(pane.loading_generation, old_generation);

            // An SPA's previous route needs no Finished event to become the
            // address; native URL polling is allowed to record it immediately.
            pane.record_url("http://localhost:3000/route-one".into(), cx);
            assert_eq!(pane.draft, "http://localhost:3000/route-one");

            // A subsequent real document navigation still gets load tracking.
            pane.native_event(NativeEvent::Started("https://example.com/".into()), cx);
            assert!(pane.loading);
        });
    }
}
