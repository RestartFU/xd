//! A system WebKit view occupying an ordinary GPUI layout slot.
//!
//! The native child is above GPUI's drawing surface. Owners must hide it when
//! changing workspaces or removing the pane from the tree, and cover it while a
//! popup is open: a covered page is replaced by a still picture of itself, so
//! the popup can draw over it.
//!
//! On Linux, GTK and WebKit's UI side run on their own thread (see
//! [`gtk_thread`]) so GPUI frames cannot delay page painting or scrolling, and
//! page work cannot delay GPUI. Every native call goes through
//! [`NativeBrowser::with_view`], and results return as [`NativeEvent`]s.

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
    RenderImage, ScrollHandle, Style, Timer, Window, div, prelude::*, px, relative, rgb, rgba,
};
#[cfg(target_os = "linux")]
use raw_window_handle::XlibWindowHandle;
use raw_window_handle::{HasWindowHandle, RawWindowHandle, WindowHandle};
use url::Url;
#[cfg(target_os = "linux")]
use wry::WebViewExtUnix;
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};
use xd_desktop::theme::ThemeColors;

use crate::input::{ComposerEvent, ComposerInput};

/// The pane runs the window's full height, so its two header rows sit level with
/// the session's: the tabs beside the session header, and the address bar, or a
/// side tool's own controls, beside the chat tabs. One rule closes both columns.
pub const SESSION_HEADER_HEIGHT: f32 = 48.0;
pub const TAB_BAR_HEIGHT: f32 = 43.0;
/// Page tabs keep one width so a loading title does not move its neighbours.
const BROWSER_TAB_WIDTH: f32 = 148.0;

#[derive(Clone, Debug)]
pub enum BrowserEvent {
    Close,
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
    Open(String),
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
    Open(String),
    Error(String),
    Navigation {
        url: Option<String>,
        can_back: bool,
        can_forward: bool,
    },
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

thread_local! {
    static SHARED_BROWSER_CONTEXT: RefCell<Option<Rc<RefCell<WebContext>>>> = const { RefCell::new(None) };
}

#[cfg(target_os = "linux")]
thread_local! {
    static SHARED_GTK_CONTEXT: RefCell<Option<webkit2gtk::WebContext>> = const { RefCell::new(None) };
    static GTK_VIEWS: RefCell<std::collections::HashMap<u64, NativeEntry>> = RefCell::default();
}

type NativeEntry = (WebView, Rc<RefCell<WebContext>>);

/// The one thread that touches GTK, GDK, and WebKit's UI side on Linux.
///
/// WebKit's frames and input wait on this thread, so sharing GPUI's would make
/// a page stutter whenever the window redraws, such as during a streaming
/// reply. GDK opens its own X11 connection, independent of GPUI's, and nothing
/// else in the process uses GLib's default main context.
#[cfg(target_os = "linux")]
mod gtk_thread {
    use std::{sync::OnceLock, thread};

    use async_channel::Sender;
    use gtk::glib::MainContext;

    type Job = Box<dyn FnOnce() + Send>;

    fn start() -> Option<Sender<Job>> {
        let (sender, receiver) = async_channel::unbounded::<Job>();
        thread::Builder::new()
            .name("xd-gtk".into())
            .spawn(move || {
                let context = MainContext::default();
                // Own the context for the thread's lifetime: GLib only wakes a
                // blocked iteration for other threads' sources if it has an owner.
                let Ok(_owner) = context.acquire() else {
                    return;
                };
                context.spawn_local(async move {
                    while let Ok(job) = receiver.recv().await {
                        job();
                    }
                });
                loop {
                    context.iteration(true);
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

/// Configure both native toolkits before any threads or windows are created.
/// Wry's child-webview embedding uses X11 on Linux; XWayland supplies it on a
/// Wayland desktop. Saved values are restored in host/terminal subprocesses.
pub fn configure_platform() {
    #[cfg(target_os = "linux")]
    if env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
        // SAFETY: called at the very beginning of main, before thread creation.
        unsafe {
            for (name, saved) in [
                ("WAYLAND_DISPLAY", "XD_HOST_WAYLAND_DISPLAY"),
                ("GDK_BACKEND", "XD_HOST_GDK_BACKEND"),
            ] {
                if env::var_os(saved).is_none() {
                    env::set_var(saved, env::var_os(name).unwrap_or_default());
                }
            }
            env::remove_var("WAYLAND_DISPLAY");
            env::set_var("GDK_BACKEND", "x11");
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
        let entity = cx.new(|cx| BrowserTab::new(url, self.colors, window, cx));
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
                TabEvent::Open(url) => pane.add_tab(Some(url.clone()), window, cx),
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
        self.select_auxiliary(None, cx);
        self.insert_tab(url, window, cx);
        self.session.active_tab = self.tabs.len() - 1;
        self.tab_scroll.scroll_to_item(self.active_tab_index());
        self.update_visibility();
        self.focus_active_tab(window, cx);
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

    fn focus_active_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible || self.covered || self.auxiliary.is_some() {
            return;
        }
        window.blur();
        let tab = self.tabs[self.session.active_tab].entity.read(cx);
        if tab.url.is_none() {
            tab.address.read(cx).focus_handle(cx).focus(window);
        } else if tab.display.native_visible.get()
            && let Some(native) = &tab.display.native
        {
            native.with_view(|view| {
                #[cfg(target_os = "linux")]
                focus_gtk_webview(&view.webview());
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
        let auxiliary_tabs = self
            .auxiliary_tabs
            .iter()
            .copied()
            .enumerate()
            .map(|(index, tab)| {
                tab_pill(
                    ("side-tool-tab", index),
                    self.auxiliary == Some(tab),
                    colors,
                )
                .on_click(cx.listener(move |pane, _, _, cx| pane.select_auxiliary(Some(tab), cx)))
                .child(crate::icon(tab.icon(), 13., colors.muted))
                .child(tab.label())
                .child(
                    tab_close(("close-side-tool", index), colors).on_click(cx.listener(
                        move |pane, _, _, cx| {
                            cx.stop_propagation();
                            pane.close_auxiliary(tab, cx);
                        },
                    )),
                )
            })
            .collect::<Vec<_>>();
        let tools = self.tools_enabled.then(|| {
            [SideTab::Files, SideTab::Changes]
                .into_iter()
                .enumerate()
                // An open tool already has its tab; only offer the others.
                .filter(|(_, tab)| !self.auxiliary_tabs.contains(tab))
                .map(|(index, tab)| {
                    crate::pane_icon_button(
                        ("side-add-tool", index),
                        tab.label(),
                        tab.icon(),
                        false,
                        colors,
                    )
                    .on_click(
                        cx.listener(move |pane, _, _, cx| pane.select_auxiliary(Some(tab), cx)),
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
            .bg(rgb(colors.background))
            .child(
                div()
                    .id("browser-tabs")
                    .h(px(SESSION_HEADER_HEIGHT))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_1()
                    .bg(rgb(colors.chrome))
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
                                tab_pill(("browser-tab", slot.id), active, colors)
                                    .w(px(BROWSER_TAB_WIDTH))
                                    .on_click(cx.listener(move |pane, _, window, cx| {
                                        pane.select_tab(index, window, cx)
                                    }))
                                    .child(crate::icon(crate::BROWSER_ICON, 13., colors.muted))
                                    .child(div().flex_1().min_w_0().text_ellipsis().child(label))
                                    .child(
                                        tab_close(("browser-close-tab", slot.id), colors).on_click(
                                            cx.listener(move |pane, _, window, cx| {
                                                cx.stop_propagation();
                                                pane.close_tab(index, window, cx);
                                            }),
                                        ),
                                    )
                            }))
                            .children(auxiliary_tabs),
                    )
                    .when_some(tools, |bar, tools| bar.children(tools))
                    .child(
                        crate::ui_button("browser-new-tab", crate::ButtonKind::Ghost, colors)
                            .size(px(28.))
                            .px_0()
                            .on_click(
                                cx.listener(|pane, _, window, cx| pane.add_tab(None, window, cx)),
                            )
                            .child(crate::plus_icon(colors.muted)),
                    ),
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

/// The side pane's tab, in the shape of the chat tabs beside it.
fn tab_pill(
    id: impl Into<ElementId>,
    selected: bool,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(30.))
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .pl_2()
        .pr_1()
        .rounded(px(7.))
        .border_1()
        .border_color(rgba(if selected {
            (colors.border << 8) | 0xff
        } else {
            0
        }))
        .when(selected, |tab| tab.bg(rgb(colors.selected_surface)))
        .text_size(px(12.))
        .text_color(rgb(if selected { colors.text } else { colors.muted }))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(colors.surface_high)))
}

fn tab_close(id: impl Into<ElementId>, colors: ThemeColors) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .hover(|style| style.bg(rgb(colors.surface_high)))
        .child(crate::icon(crate::CLOSE_ICON, 11., colors.muted))
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
    colors: ThemeColors,
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
            match NativeBrowser::create(window, events.clone()) {
                Ok(native) => (Some(Rc::new(native)), None),
                Err(error) => (None, Some(error)),
            }
        };
        let mut pane = Self {
            display: Rc::new(TabDisplay {
                native,
                events,
                bounds: Rc::new(Cell::new(None)),
                visible: Cell::new(false),
                covered: Cell::new(false),
                snapshot: RefCell::new(None),
                generation: Cell::new(0),
                awaiting_picture: Cell::new(None),
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
            colors,
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
        #[cfg(target_os = "linux")]
        cx.observe_window_activation(window, |tab, window, _| {
            if !window.is_window_active()
                && tab.display.native_visible.get()
                && let Some(native) = &tab.display.native
            {
                native.with_view(|view| {
                    // GPUI treats FocusOut(Inferior) as deactivation too.
                    // Keep page focus when X moved into this native child.
                    let webview = view.webview();
                    if !gtk_browser_has_keyboard_focus(&webview)
                        && let Some(window) = gtk_browser_toplevel(&webview)
                    {
                        set_gtk_window_focus(&window, false);
                    }
                });
            }
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
        // which WebKit does not necessarily emit a page-load event.
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
        let Some(native) = self.display.native.as_ref() else {
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
            NativeEvent::Open(url) => cx.emit(TabEvent::Open(url)),
            NativeEvent::Error(error) => {
                self.loading = false;
                self.load_timed_out = false;
                self.error = Some(error);
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
        let Some(native) = self.display.native.as_ref() else {
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
        if let Some(native) = &self.display.native {
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
        if let Some(native) = &self.display.native {
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
    fn retry(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = self.url.clone() {
            self.draft = url.clone();
            self.address_dirty = false;
            self.address.update(cx, |input, cx| input.set_text(url, cx));
        }
        self.error = None;
        self.reload(cx);
    }

    fn button(
        &self,
        id: &'static str,
        path: &'static str,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = self.colors;
        div()
            .id(id)
            .w(px(28.))
            .h(px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .opacity(if enabled { 1. } else { 0.4 })
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(colors.surface_high)))
            })
            .child(crate::icon(path, 15., colors.muted))
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
        let mut pane = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(colors.background))
            .text_color(rgb(colors.text))
            .child(
                div()
                    .id("browser-toolbar")
                    .h(px(TAB_BAR_HEIGHT))
                    .bg(rgb(colors.chrome))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        self.button("browser-back", crate::ARROW_LEFT_ICON, back_enabled)
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                if back_enabled {
                                    pane.history(true, cx);
                                }
                            })),
                    )
                    .child(
                        self.button("browser-forward", crate::ARROW_RIGHT_ICON, forward_enabled)
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                if forward_enabled {
                                    pane.history(false, cx);
                                }
                            })),
                    )
                    .child(
                        self.button("browser-reload", crate::ROTATE_CW_ICON, has_page)
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                if has_page {
                                    pane.reload(cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("browser-address")
                            .flex_1()
                            .min_w_0()
                            .h(px(28.))
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
                            .rounded(px(6.))
                            .cursor_text()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|pane, _, window, cx| {
                                    pane.address.read(cx).focus_handle(cx).focus(window);
                                }),
                            )
                            .child(div().flex_1().min_w_0().child(self.address.clone()))
                            .when(self.loading, |field| {
                                field.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors.faint))
                                        .child("Loading…"),
                                )
                            }),
                    )
                    .child(
                        self.button("browser-external", crate::EXTERNAL_LINK_ICON, has_page)
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                if let Some(url) = &pane.url {
                                    cx.open_url(url);
                                }
                            })),
                    ),
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
                            .text_size(px(13.))
                            .text_color(rgb(colors.muted))
                            .child(error.clone()),
                    )
                    .when(has_page, |state| {
                        state.child(
                            crate::ui_button("browser-retry", crate::ButtonKind::Secondary, colors)
                                .h(px(28.))
                                .on_click(cx.listener(|pane, _, _, cx| pane.retry(cx)))
                                .child("Reload"),
                        )
                    }),
            );
        } else if has_page {
            if let Some(native) = &self.display.native {
                let surface = NativeSurface {
                    native: native.clone(),
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
                            .text_size(px(16.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Preview your app"),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
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
                                .h(px(28.))
                                .font_family(crate::MONO)
                                .text_size(px(12.))
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
    native: Option<Rc<NativeBrowser>>,
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
    native_visible: Cell<bool>,
    has_page: Cell<bool>,
    healthy: Cell<bool>,
}

impl TabDisplay {
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
        let Some(native) = &self.native else {
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
        }
        let display = self.clone();
        cx.spawn(async move |_, _| {
            Timer::after(PICTURE_FALLBACK_DELAY).await;
            display.finish_handoff(generation);
        })
        .detach();
        true
    }

    /// The tab drew the picture in a frame; hide the view once it is shown.
    fn picture_drawn(self: &Rc<Self>, window: &mut Window) {
        if let Some(generation) = self.awaiting_picture.get()
            && self.snapshot.borrow().is_some()
        {
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
        self.set_visible(false);
        if let Some(native) = &self.native {
            native.dispose();
        }
    }
}

/// Hides a page awaiting its picture even if no frame comes to draw it.
const PICTURE_FALLBACK_DELAY: Duration = Duration::from_millis(250);

fn set_native_visible(view: &WebView, visible: bool, events: &NativeEvents) {
    #[cfg(target_os = "linux")]
    if !visible {
        blur_gtk_webview(&view.webview());
    }
    if let Err(error) = view.set_visible(visible) {
        events.send(NativeEvent::Error(error.to_string()));
    }
}

/// The view's current pixels in GPUI's BGRA order, where the platform can
/// read them back. Elsewhere a covered page leaves its slot blank.
fn capture_view(view: &WebView) -> Option<image::RgbaImage> {
    #[cfg(target_os = "linux")]
    return capture_gtk_webview(&view.webview());
    #[cfg(not(target_os = "linux"))]
    {
        let _ = view;
        None
    }
}

/// A handle to a native view living on the thread its toolkit requires: GPUI's
/// on macOS and Windows, [`gtk_thread`] on Linux.
struct NativeBrowser {
    #[cfg(target_os = "linux")]
    id: u64,
    // Keep the context alive even if a previously painted layout element is
    // still holding the native view after the owning entity is released.
    #[cfg(not(target_os = "linux"))]
    entry: RefCell<Option<NativeEntry>>,
    disposed: Cell<bool>,
}

impl NativeBrowser {
    fn is_disposed(&self) -> bool {
        self.disposed.get()
    }

    /// Apply a call to the view, in submission order. On Linux this returns
    /// before the call runs, so outcomes must be reported as a [`NativeEvent`].
    /// `Send` holds on every platform so that shared callers stay portable.
    fn with_view(&self, apply: impl FnOnce(&WebView) + Send + 'static) {
        if self.is_disposed() {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let id = self.id;
            gtk_thread::post(move || {
                GTK_VIEWS.with(|views| {
                    if let Some((view, _)) = views.borrow().get(&id) {
                        apply(view);
                    }
                });
            });
        }
        #[cfg(not(target_os = "linux"))]
        if let Some((view, _)) = self.entry.borrow().as_ref() {
            apply(view);
        }
    }

    #[cfg(target_os = "linux")]
    fn create(window: &Window, events: NativeEvents) -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let _ = window;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (reply, created) = std::sync::mpsc::channel();
        gtk_thread::post(move || {
            let result = create_linux_native(events).map(|entry| {
                GTK_VIEWS.with(|views| views.borrow_mut().insert(id, entry));
            });
            let _ = reply.send(result);
        });
        created
            .recv_timeout(NATIVE_REPLY_TIMEOUT)
            .map_err(|_| "The browser did not start. Restart xd to try again.".to_string())??;
        Ok(Self {
            id,
            disposed: Cell::new(false),
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn create(window: &Window, events: NativeEvents) -> Result<Self, String> {
        let parent = NativeParent(
            HasWindowHandle::window_handle(window)
                .map_err(|error| error.to_string())?
                .as_raw(),
        );
        Ok(Self {
            entry: RefCell::new(Some(create_native(parent, events)?)),
            disposed: Cell::new(false),
        })
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
            gtk_thread::post(move || {
                let entry = GTK_VIEWS.with(|views| views.borrow_mut().remove(&id));
                if let Some((view, context)) = entry {
                    use gtk::prelude::*;
                    if let Some(window) = register_gtk_browser_window(&view) {
                        // Release GTK's resources before Wry destroys its X11 child.
                        window.unrealize();
                    }
                    drop(view);
                    drop(context);
                }
                let _ = reply.send(());
            });
            let _ = disposed.recv_timeout(NATIVE_REPLY_TIMEOUT);
        }
        #[cfg(not(target_os = "linux"))]
        drop(self.entry.borrow_mut().take());
    }
}

/// How long GPUI waits for the two calls it cannot leave pending. The GTK
/// thread never waits on GPUI, so reaching this means WebKit itself is stuck.
#[cfg(target_os = "linux")]
const NATIVE_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

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
                    native.with_view(|view| blur_gtk_webview(&view.webview()));
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

/// A borrowed OS parent handle; Wry obtains the X11 connection from GTK.
struct NativeParent(RawWindowHandle);

impl HasWindowHandle for NativeParent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: the GPUI window owns this handle and outlives its child view.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

/// Runs on [`gtk_thread`], which is therefore the thread GTK treats as its main.
#[cfg(target_os = "linux")]
fn create_linux_native(events: NativeEvents) -> Result<NativeEntry, String> {
    if env::var_os("DISPLAY").is_none_or(|display| display.is_empty()) {
        return Err(
            "The integrated browser needs X11 or XWayland. Enable XWayland and restart xd.".into(),
        );
    }
    gtk::init().map_err(|error| format!("Could not initialize the browser: {error}"))?;
    let parent = NativeParent(RawWindowHandle::Xlib(XlibWindowHandle::new(
        find_x11_parent()? as _,
    )));
    create_native(parent, events)
}

fn create_native(parent: NativeParent, events: NativeEvents) -> Result<NativeEntry, String> {
    let context = SHARED_BROWSER_CONTEXT.with(|shared| {
        shared
            .borrow_mut()
            .get_or_insert_with(|| {
                #[cfg(target_os = "linux")]
                let context = WebContext::new(None);
                #[cfg(not(target_os = "linux"))]
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
    #[cfg(target_os = "linux")]
    let builder = {
        use wry::WebViewBuilderExtUnix;
        // Wry 0.57 does not expose its WebContext's native GTK context. Its
        // public related-view API lets us supply a context configured before
        // view creation, which can start WebKit subprocesses immediately.
        builder.with_related_view(linux_related_view()?)
    };
    let native = builder
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
                popup_events.send(NativeEvent::Open(url));
            }
            NewWindowResponse::Deny
        })
        .build_as_child(&parent)
        .map_err(|error| format!("Could not create the browser: {error}"))?;
    // Wry's X11 builder shows the GTK wrapper before attaching its X11 data,
    // so its initial with_visible(false) only hides the inner WebKit widget.
    // Hide the fully constructed child before trusting our visibility cache;
    // otherwise a blank/background tab leaves a mapped 200px surface at (0, 0).
    native
        .set_visible(false)
        .map_err(|error| format!("Could not hide the browser: {error}"))?;
    #[cfg(target_os = "linux")]
    {
        register_gtk_browser_window(&native);
        use gtk::prelude::WidgetExt;
        use webkit2gtk::WebViewExt;
        use wry::WebViewExtUnix;
        let webview = native.webview();
        webview.connect_button_press_event(|webview, _| {
            focus_gtk_webview(webview);
            gtk::glib::Propagation::Proceed
        });
        webview.connect_load_failed(move |_, _, url, error| {
            if !error.matches(webkit2gtk::NetworkError::Cancelled) {
                events.send(NativeEvent::Error(format!("Could not load {url}: {error}")));
            }
            false
        });
    }
    drop(context_ref);
    Ok((native, context))
}

/// The page's current pixels, read back from its X11 window, in the BGRA
/// order GPUI's renderer takes.
#[cfg(target_os = "linux")]
fn capture_gtk_webview(webview: &webkit2gtk::WebView) -> Option<image::RgbaImage> {
    use gtk::prelude::*;

    let window = gtk_browser_toplevel(webview)?.window()?;
    if !window.is_viewable() {
        return None;
    }
    let pixbuf = window.pixbuf(0, 0, window.width(), window.height())?;
    let (width, height) = (pixbuf.width() as usize, pixbuf.height() as usize);
    let stride = pixbuf.rowstride() as usize;
    let channels = pixbuf.n_channels() as usize;
    if width == 0 || height == 0 || channels < 3 {
        return None;
    }
    let pixels = pixbuf.read_pixel_bytes();
    let mut frame = Vec::with_capacity(width * height * 4);
    for row in pixels.chunks(stride).take(height) {
        for pixel in row[..width * channels].chunks_exact(channels) {
            frame.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0xff]);
        }
    }
    image::RgbaImage::from_raw(width as u32, height as u32, frame)
}

#[cfg(target_os = "linux")]
fn register_gtk_browser_window(view: &WebView) -> Option<gtk::Window> {
    use gtk::prelude::*;
    use wry::WebViewExtUnix;

    let window = view.webview().toplevel()?.downcast::<gtk::Window>().ok()?;
    if let Some(gdk_window) = window.window() {
        let mut owner = std::ptr::null_mut();
        // SAFETY: inspect the live GDK window's opaque owner on GTK's thread;
        // the pointer is never dereferenced.
        unsafe {
            gtk::gdk::ffi::gdk_window_get_user_data(gdk_window.as_ptr(), &mut owner);
        }
        if owner.is_null() {
            // Wry replaces GtkWindow's registered GDK window with a foreign
            // child. GTK needs its owner to route focus events to the toplevel,
            // which lets WebKit display the caret in a focused page input.
            window.register_window(&gdk_window);
        }
        gdk_window.set_events(
            gdk_window.events()
                | gtk::gdk::EventMask::FOCUS_CHANGE_MASK
                | gtk::gdk::EventMask::KEY_PRESS_MASK
                | gtk::gdk::EventMask::KEY_RELEASE_MASK,
        );
    }
    Some(window)
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn gdk_x11_window_get_xid(window: *mut gtk::gdk::ffi::GdkWindow) -> std::ffi::c_ulong;
}

#[cfg(target_os = "linux")]
fn gtk_browser_toplevel(webview: &webkit2gtk::WebView) -> Option<gtk::Window> {
    use gtk::prelude::*;
    webview.toplevel()?.downcast::<gtk::Window>().ok()
}

#[cfg(target_os = "linux")]
fn set_gtk_window_focus(window: &gtk::Window, focused: bool) {
    use gtk::prelude::*;
    if window.has_toplevel_focus() == focused && window.is_active() == focused {
        return;
    }
    let Some(gdk_window) = window.window() else {
        return;
    };
    // GDK does not translate foreign-window focus into GtkWindow state.
    // Deliver the standard GTK focus event on interaction, rather than
    // leaving WebKit's document/caret unfocused while keyboard input works.
    // SAFETY: this event is allocated and freed here on GTK's thread.
    // It owns one reference to the live GDK window; GTK handles it synchronously.
    unsafe {
        let event = gtk::gdk::ffi::gdk_event_new(gtk::gdk::ffi::GDK_FOCUS_CHANGE);
        let focus = event.cast::<gtk::gdk::ffi::GdkEventFocus>();
        (*focus).window = gtk::glib::gobject_ffi::g_object_ref(gdk_window.as_ptr().cast()).cast();
        (*focus).send_event = 1;
        (*focus).in_ = i16::from(focused);
        gtk::ffi::gtk_widget_event(window.as_ptr().cast(), event);
        gtk::gdk::ffi::gdk_event_free(event);
    }
}

#[cfg(target_os = "linux")]
fn focus_gtk_webview(webview: &webkit2gtk::WebView) {
    use gtk::prelude::*;
    use x11rb::protocol::xproto::{ConnectionExt, InputFocus};
    if let Some(window) = gtk_browser_toplevel(webview) {
        // Complete GTK's pending map before focusing its foreign X11 child.
        // GdkWindow::focus routes through the WM on EWMH desktops, where an
        // unmanaged child may be ignored. Set the keyboard target directly.
        window.display().sync();
        let Some((connection, child, focused)) = gtk_browser_keyboard_focus(webview) else {
            return;
        };
        if !focused {
            let Ok(request) = connection.set_input_focus(InputFocus::PARENT, child, 0u32) else {
                return;
            };
            if request.check().is_err() {
                return;
            }
        }
        set_gtk_window_focus(&window, true);
        webview.grab_focus();
    }
}

#[cfg(target_os = "linux")]
fn gtk_browser_keyboard_focus(
    webview: &webkit2gtk::WebView,
) -> Option<(x11rb::rust_connection::RustConnection, u32, bool)> {
    use gtk::prelude::*;
    use x11rb::protocol::xproto::ConnectionExt;
    let window = gtk_browser_toplevel(webview)?.window()?;
    // SAFETY: borrow the live X11 GDK window on its owning thread.
    let child = u32::try_from(unsafe { gdk_x11_window_get_xid(window.as_ptr()) }).ok()?;
    let (connection, _) = x11rb::connect(None).ok()?;
    let mut focus = connection.get_input_focus().ok()?.reply().ok()?.focus;
    for _ in 0..8 {
        if focus == child {
            return Some((connection, child, true));
        }
        if focus <= 1 {
            break;
        }
        focus = connection.query_tree(focus).ok()?.reply().ok()?.parent;
    }
    Some((connection, child, false))
}

#[cfg(target_os = "linux")]
fn gtk_browser_has_keyboard_focus(webview: &webkit2gtk::WebView) -> bool {
    gtk_browser_keyboard_focus(webview).is_some_and(|(_, _, focused)| focused)
}

#[cfg(target_os = "linux")]
fn blur_gtk_webview(webview: &webkit2gtk::WebView) {
    use x11rb::{
        connection::Connection,
        protocol::xproto::{ConnectionExt, InputFocus},
    };
    if let Some(window) = gtk_browser_toplevel(webview) {
        set_gtk_window_focus(&window, false);
    }
    // Restore GPUI's keyboard target only while this page owns it. Hiding an
    // inactive page must not take focus from another application.
    if let Some((connection, child, true)) = gtk_browser_keyboard_focus(webview)
        && let Ok(reply) = connection.query_tree(child)
        && let Ok(tree) = reply.reply()
    {
        let _ = connection.set_input_focus(InputFocus::PARENT, tree.parent, 0u32);
        let _ = connection.flush();
    }
}

#[cfg(target_os = "linux")]
fn linux_related_view() -> Result<webkit2gtk::WebView, String> {
    let context = SHARED_GTK_CONTEXT.with(|shared| {
        if let Some(context) = shared.borrow().as_ref() {
            return Ok(context.clone());
        }
        let context = create_linux_browser_context()?;
        *shared.borrow_mut() = Some(context.clone());
        Ok::<_, String>(context)
    })?;
    // A related view inherits its seed's web process. Use a fresh seed per tab
    // while sharing the context/data manager, so a busy page cannot stall
    // every chat's browser through one common renderer.
    Ok(webkit2gtk::WebView::with_context(&context))
}

#[cfg(target_os = "linux")]
fn create_linux_browser_context() -> Result<webkit2gtk::WebContext, String> {
    use webkit2gtk::{CookieManagerExt, WebContextExt, WebsiteDataManagerExt};

    let directory = browser_data_directory();
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create browser data directory: {error}"))?;
    let data = webkit2gtk::WebsiteDataManager::builder()
        .base_cache_directory(directory.to_string_lossy())
        .base_data_directory(directory.to_string_lossy())
        .build();
    let context = webkit2gtk::WebContext::builder()
        .website_data_manager(&data)
        .build();
    // Must precede even constructing the first WebView: WebKit rejects
    // sandbox changes once either a network or a web process has spawned.
    for name in ["XD_BROWSER_BUNDLE_ROOT", "XD_BROWSER_RUNTIME_ROOT"] {
        if let Some(path) = env::var_os(name) {
            context.add_path_to_sandbox(PathBuf::from(path), true);
        }
    }
    if let Some(cookies) = data.cookie_manager() {
        cookies.set_persistent_storage(
            &directory.join("cookies").to_string_lossy(),
            webkit2gtk::CookiePersistentStorage::Text,
        );
    }
    Ok(context)
}

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
fn find_x11_parent() -> Result<u32, String> {
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
                display.finish_handoff(cover);
                assert_eq!(display.awaiting_picture.get(), None);

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
        cx.update(|_, cx| {
            tab.update(cx, |tab, cx| {
                tab.navigate("ftp://example.invalid/", cx);
                assert!(tab.error.is_some());
                tab.retry(cx);
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
                    tab.native_event(NativeEvent::Open("https://example.com/popup".into()), cx);
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
    fn gtk_jobs_run_in_order_on_one_dedicated_thread() {
        let (reply, results) = std::sync::mpsc::channel();
        for index in 0..1_000 {
            let reply = reply.clone();
            gtk_thread::post(move || {
                let _ = reply.send((index, std::thread::current().id()));
            });
        }
        let received = (0..1_000)
            .map(|_| results.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect::<Vec<_>>();
        let gtk = received[0].1;
        assert_ne!(gtk, std::thread::current().id());
        for (expected, (index, thread)) in received.into_iter().enumerate() {
            assert_eq!(index, expected);
            assert_eq!(thread, gtk);
        }
    }

    #[test]
    fn native_events_can_cross_to_the_gtk_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<NativeEvents>();
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
        events.send(NativeEvent::Open("https://example.com/popup".into()));
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
        assert!(matches!(receiver.try_recv().unwrap(), NativeEvent::Open(_)));
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
