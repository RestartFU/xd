//! A system WebKit view occupying an ordinary GPUI layout slot.
//!
//! The native child is above GPUI's drawing surface. Owners must hide it when
//! showing a modal, changing workspaces, or removing the pane from the tree.

use std::{
    cell::{Cell, RefCell},
    env,
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use async_channel::Sender;
use gpui::{
    App, Bounds, Context, DispatchPhase, ElementId, Entity, EventEmitter, Focusable,
    GlobalElementId, Hitbox, HitboxBehavior, IntoElement, LayoutId, MouseDownEvent, Pixels, Render,
    ScrollHandle, Style, Timer, Window, div, prelude::*, px, relative, rgb,
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

#[derive(Clone, Debug)]
pub enum BrowserEvent {
    Close,
    SessionChanged(BrowserSession),
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
    ClosePane,
}

enum NativeEvent {
    Started(String),
    Finished(String),
    TitleReady,
    Open(String),
    Error(String),
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
    title: Rc<RefCell<PendingTitle>>,
}

impl NativeEvents {
    fn channel() -> (Self, async_channel::Receiver<NativeEvent>) {
        let (sender, receiver) = async_channel::unbounded();
        (
            Self {
                sender,
                title: Rc::default(),
            },
            receiver,
        )
    }

    fn send(&self, event: NativeEvent) {
        let _ = self.sender.try_send(event);
    }

    fn title_changed(&self, title: String) {
        let mut pending = self.title.borrow_mut();
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
            self.title.borrow_mut().queued = false;
        }
    }

    fn take_title(&self) -> Option<String> {
        let mut pending = self.title.borrow_mut();
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
    static VISIBLE_GTK_BROWSERS: Cell<usize> = const { Cell::new(0) };
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
    colors: ThemeColors,
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
            colors,
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

    pub fn set_visible(&mut self, visible: bool) {
        if visible && !self.visible {
            self.tab_scroll.scroll_to_item(self.session.active_tab);
        }
        self.visible = visible;
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
                TabEvent::ClosePane => {
                    pane.set_visible(false);
                    cx.emit(BrowserEvent::Close);
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
        self.insert_tab(url, window, cx);
        self.session.active_tab = self.tabs.len() - 1;
        self.tab_scroll.scroll_to_item(self.session.active_tab);
        self.update_visibility();
        self.focus_active_tab(window, cx);
        cx.emit(BrowserEvent::SessionChanged(self.session()));
        cx.notify();
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() || index == self.session.active_tab {
            return;
        }
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
        self.tab_scroll.scroll_to_item(self.session.active_tab);
        self.update_visibility();
        if was_active {
            self.focus_active_tab(window, cx);
        }
        cx.emit(BrowserEvent::SessionChanged(self.session()));
        cx.notify();
    }

    fn update_visibility(&self) {
        for (index, tab) in self.tabs.iter().enumerate() {
            tab.display
                .set_visible(self.visible && index == self.session.active_tab);
        }
    }

    fn focus_active_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        window.blur();
        let tab = self.tabs[self.session.active_tab].entity.read(cx);
        if tab.url.is_none() {
            tab.address.read(cx).focus_handle(cx).focus(window);
        } else if tab.display.native_visible.get()
            && let Some(native) = &tab.display.native
        {
            let _ = native.with_view(|view| {
                #[cfg(target_os = "linux")]
                focus_gtk_webview(&view.webview());
                view.focus()
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
                self.tab_scroll.scroll_to_item(self.session.active_tab);
                self.reveal_after_layout = false;
            } else {
                window.request_animation_frame();
            }
        }
        let colors = self.colors;
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(colors.background))
            .child(
                div()
                    .id("browser-tabs")
                    .h(px(32.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(rgb(colors.sidebar))
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .id("browser-tab-list")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
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
                                let active = index == self.session.active_tab;
                                div()
                                    .id(("browser-tab", slot.id))
                                    .h_full()
                                    .w(px(148.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .pl(px(10.))
                                    .pr(px(3.))
                                    .border_r_1()
                                    .border_color(rgb(colors.border))
                                    .text_size(px(11.))
                                    .text_color(rgb(if active {
                                        colors.text
                                    } else {
                                        colors.muted
                                    }))
                                    .when(active, |tab| {
                                        tab.bg(rgb(colors.background))
                                            .border_b_1()
                                            .border_color(rgb(colors.accent))
                                    })
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |pane, _, window, cx| {
                                        pane.select_tab(index, window, cx)
                                    }))
                                    .child(div().flex_1().min_w_0().text_ellipsis().child(label))
                                    .child(
                                        div()
                                            .id(("browser-close-tab", slot.id))
                                            .w(px(22.))
                                            .h(px(22.))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(3.))
                                            .hover(|style| style.bg(rgb(colors.surface_high)))
                                            .child("×")
                                            .on_click(cx.listener(move |pane, _, window, cx| {
                                                cx.stop_propagation();
                                                pane.close_tab(index, window, cx);
                                            })),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .id("browser-new-tab")
                            .w(px(32.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(colors.muted))
                            .cursor_pointer()
                            .hover(|style| {
                                style
                                    .bg(rgb(colors.surface_high))
                                    .text_color(rgb(colors.text))
                            })
                            .child("+")
                            .on_click(
                                cx.listener(|pane, _, window, cx| pane.add_tab(None, window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.tabs[self.session.active_tab].entity.clone()),
            )
    }
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
            input.set_colors(colors, cx);
            input
        });
        let (events, receiver) = NativeEvents::channel();
        let (native, error) = if cfg!(test) {
            (None, None)
        } else {
            match create_native(window, events.clone(), cx) {
                Ok((native, context)) => (
                    Some(Rc::new(NativeBrowser {
                        view: RefCell::new(Some(native)),
                        _context: context,
                    })),
                    None,
                ),
                Err(error) => (None, Some(error)),
            }
        };
        let mut pane = Self {
            display: Rc::new(TabDisplay {
                native,
                events,
                bounds: Rc::new(Cell::new(None)),
                visible: Cell::new(false),
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
                let _ = native.with_view(|view| {
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
                    .update(cx, |pane, cx| pane.refresh_navigation(cx))
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
        match native.with_view(|view| view.load_url(&url)) {
            Some(Ok(())) => {
                self.error = None;
                self.start_loading(cx);
                self.title.clear();
                self.record_url(url, cx);
            }
            Some(Err(error)) => {
                self.error = Some(format!("Could not open page: {error}"));
                self.record_url(url, cx);
            }
            None => return,
        }
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
                self.refresh_navigation(cx);
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
            NativeEvent::Open(url) => cx.emit(TabEvent::Open(url)),
            NativeEvent::Error(error) => {
                self.loading = false;
                self.load_timed_out = false;
                self.error = Some(error);
            }
        }
        self.refresh_visibility();
        cx.notify();
    }

    fn refresh_navigation(&mut self, cx: &mut Context<Self>) {
        if !self.display.visible.get() {
            return;
        }
        let Some(native) = self.display.native.as_ref() else {
            return;
        };
        let Some((url, can_back, can_forward)) = native.with_view(|view| {
            (
                view.url().ok(),
                view.can_go_back().unwrap_or(false),
                view.can_go_forward().unwrap_or(false),
            )
        }) else {
            return;
        };
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
        let result = match &self.display.native {
            Some(native) if back => native.with_view(WebView::go_back).unwrap_or(Ok(())),
            Some(native) => native.with_view(WebView::go_forward).unwrap_or(Ok(())),
            None => Ok(()),
        };
        if let Err(error) = result {
            self.error = Some(error.to_string());
        } else {
            self.address_dirty = false;
            self.error = None;
            self.loading = false;
            self.load_timed_out = false;
            self.loading_generation = self.loading_generation.wrapping_add(1);
            // Same-document history may have no load events. Keep URL polling
            // active; a document load's Started event will arm its timeout.
        }
        self.refresh_visibility();
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(native) = &self.display.native {
            match native.with_view(WebView::reload) {
                Some(Ok(())) => {
                    self.start_loading(cx);
                    self.error = None;
                }
                Some(Err(error)) => self.error = Some(error.to_string()),
                None => return,
            }
        }
        self.refresh_visibility();
        cx.notify();
    }

    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .w(px(28.))
            .h(px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .text_size(px(15.))
            .text_color(rgb(self.colors.muted))
            .opacity(if enabled { 1. } else { 0.4 })
            .when(enabled, |button| {
                button.cursor_pointer().hover(|style| {
                    style
                        .bg(rgb(self.colors.surface_high))
                        .text_color(rgb(self.colors.text))
                })
            })
            .child(label)
    }
}

impl Drop for BrowserTab {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Render for BrowserTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_page = self.current_url().is_some();
        let back_enabled = self.can_back;
        let forward_enabled = self.can_forward;
        let mut pane =
            div()
                .size_full()
                .min_w_0()
                .flex()
                .flex_col()
                .bg(rgb(self.colors.background))
                .text_color(rgb(self.colors.text))
                .child(
                    div()
                        .id("browser-toolbar")
                        .h(px(42.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .px(px(6.))
                        .bg(rgb(self.colors.sidebar))
                        .border_b_1()
                        .border_color(rgb(self.colors.border))
                        .child(self.button("browser-back", "←", back_enabled).on_click(
                            cx.listener(move |pane, _, _, cx| {
                                if back_enabled {
                                    pane.history(true, cx);
                                }
                            }),
                        ))
                        .child(
                            self.button("browser-forward", "→", forward_enabled)
                                .on_click(cx.listener(move |pane, _, _, cx| {
                                    if forward_enabled {
                                        pane.history(false, cx);
                                    }
                                })),
                        )
                        .child(
                            self.button("browser-reload", "↻", has_page)
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
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .px(px(8.))
                                .bg(rgb(self.colors.background))
                                .border_1()
                                .border_color(rgb(self.colors.border))
                                .rounded(px(5.))
                                .text_size(px(12.))
                                .cursor_text()
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|pane, _, window, cx| {
                                        pane.address.read(cx).focus_handle(cx).focus(window);
                                    }),
                                )
                                .child(self.address.clone()),
                        )
                        .child(self.button("browser-external", "↗", has_page).on_click(
                            cx.listener(move |pane, _, _, cx| {
                                if let Some(url) = &pane.url {
                                    cx.open_url(url);
                                }
                            }),
                        ))
                        .child(
                            self.button("browser-close", "×", true)
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.display.set_visible(false);
                                    cx.emit(TabEvent::ClosePane);
                                })),
                        ),
                );
        if let Some(error) = &self.error {
            pane = pane.child(
                div()
                    .flex_1()
                    .p(px(24.))
                    .text_size(px(13.))
                    .text_color(rgb(self.colors.muted))
                    .child(error.clone()),
            );
        } else if has_page {
            if let Some(native) = &self.display.native {
                pane = pane.child(NativeSurface {
                    native: native.clone(),
                    bounds: self.display.bounds.clone(),
                    events: self.display.events.clone(),
                    visible: self.display.visible.get(),
                });
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
                    .gap(px(12.))
                    .p(px(20.))
                    .child(div().text_size(px(16.)).child("Preview your app"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(self.colors.muted))
                            .child("Enter a URL above, or open a local development server."),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .children([3000_u32, 5173, 8080].map(|port| {
                                div()
                                    .id(("browser-localhost", port))
                                    .px(px(8.))
                                    .py(px(5.))
                                    .rounded(px(4.))
                                    .bg(rgb(self.colors.surface))
                                    .text_size(px(12.))
                                    .cursor_pointer()
                                    .child(format!(":{port}"))
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        pane.navigate(&format!("http://localhost:{port}"), cx);
                                    }))
                            })),
                    ),
            );
        }
        pane.child(
            div()
                .h(px(24.))
                .bg(rgb(self.colors.sidebar))
                .flex_shrink_0()
                .px(px(10.))
                .flex()
                .items_center()
                .border_t_1()
                .border_color(rgb(self.colors.border))
                .text_size(px(11.))
                .text_color(rgb(self.colors.muted))
                .overflow_hidden()
                .child(if self.loading {
                    "Loading…".to_string()
                } else {
                    self.title.clone()
                }),
        )
    }
}

/// Shared native visibility permits pane changes without mutating child entities.
struct TabDisplay {
    native: Option<Rc<NativeBrowser>>,
    events: NativeEvents,
    bounds: Rc<Cell<Option<NativeAllocation>>>,
    visible: Cell<bool>,
    native_visible: Cell<bool>,
    has_page: Cell<bool>,
    healthy: Cell<bool>,
}

impl TabDisplay {
    fn set_visible(&self, visible: bool) {
        self.visible.set(visible);
        let Some(native) = &self.native else {
            return;
        };
        let shown = visible && self.has_page.get() && self.healthy.get();
        if self.native_visible.replace(shown) == shown {
            return;
        }
        #[cfg(target_os = "linux")]
        if !shown {
            let _ = native.with_view(|view| blur_gtk_webview(&view.webview()));
        }
        self.bounds.set(None);
        #[cfg(target_os = "linux")]
        VISIBLE_GTK_BROWSERS.with(|count| {
            count.set(if shown {
                count.get() + 1
            } else {
                count.get().saturating_sub(1)
            });
        });
        if let Some(Err(error)) = native.with_view(|view| view.set_visible(shown)) {
            self.events.send(NativeEvent::Error(error.to_string()));
        }
    }

    fn shutdown(&self) {
        self.set_visible(false);
        if let Some(native) = &self.native {
            let view = native.view.borrow_mut().take();
            #[cfg(target_os = "linux")]
            if let Some(view) = &view {
                use gtk::prelude::*;
                if let Some(window) = register_gtk_browser_window(view) {
                    // Release GTK's resources before Wry destroys its X11 child.
                    window.unrealize();
                }
            }
            drop(view);
        }
    }
}

struct NativeBrowser {
    view: RefCell<Option<WebView>>,
    // Keep the context alive even if a previously painted layout element is
    // still holding the native view after the owning entity is released.
    _context: Rc<RefCell<WebContext>>,
}

impl NativeBrowser {
    fn with_view<T>(&self, apply: impl FnOnce(&WebView) -> T) -> Option<T> {
        self.view.borrow().as_ref().map(apply)
    }
}

type NativeAllocation = (Bounds<Pixels>, f32, gpui::Point<Pixels>);

struct NativeSurface {
    native: Rc<NativeBrowser>,
    bounds: Rc<Cell<Option<NativeAllocation>>>,
    events: NativeEvents,
    visible: bool,
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
            match self.native.with_view(|view| {
                let result = if geometry_changed {
                    view.set_bounds(rect)
                } else {
                    Ok(())
                };
                #[cfg(target_os = "windows")]
                {
                    use wry::WebViewExtWindows;
                    // Child embedding leaves parent move notifications to us.
                    unsafe {
                        let _ = view.controller().NotifyParentWindowPositionChanged();
                    }
                }
                result
            }) {
                Some(Ok(())) => self.bounds.set(Some(allocation)),
                Some(Err(error)) => {
                    self.events.send(NativeEvent::Error(error.to_string()));
                }
                None => {}
            }
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
                    let _ = native.with_view(|view| blur_gtk_webview(&view.webview()));
                    _window.activate_window();
                }
                #[cfg(not(target_os = "linux"))]
                let _ = native.with_view(WebView::focus_parent);
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

fn create_native(
    window: &Window,
    events: NativeEvents,
    cx: &mut App,
) -> Result<(WebView, Rc<RefCell<WebContext>>), String> {
    #[cfg(target_os = "linux")]
    {
        if env::var_os("DISPLAY").is_none_or(|display| display.is_empty()) {
            return Err(
                "The integrated browser needs X11 or XWayland. Enable XWayland and restart xd."
                    .into(),
            );
        }
        gtk::init().map_err(|error| format!("Could not initialize the browser: {error}"))?;
        ensure_gtk_pump(cx);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cx;
    #[cfg(target_os = "linux")]
    let parent = {
        let _ = window;
        NativeParent(RawWindowHandle::Xlib(XlibWindowHandle::new(
            find_x11_parent()? as _,
        )))
    };
    #[cfg(not(target_os = "linux"))]
    let parent = NativeParent(
        HasWindowHandle::window_handle(window)
            .map_err(|error| error.to_string())?
            .as_raw(),
    );
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

#[cfg(target_os = "linux")]
fn register_gtk_browser_window(view: &WebView) -> Option<gtk::Window> {
    use gtk::prelude::*;
    use wry::WebViewExtUnix;

    let window = view.webview().toplevel()?.downcast::<gtk::Window>().ok()?;
    if let Some(gdk_window) = window.window() {
        let mut owner = std::ptr::null_mut();
        // SAFETY: inspect the live GDK window's opaque owner on GTK's main
        // thread; the pointer is never dereferenced.
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
    // SAFETY: this event is allocated and freed here on GTK's main thread.
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
    // SAFETY: borrow the live X11 GDK window on its owning main thread.
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

#[cfg(target_os = "linux")]
fn ensure_gtk_pump(cx: &mut App) {
    use std::time::Instant;

    thread_local! { static RUNNING: Cell<bool> = const { Cell::new(false) }; }
    if RUNNING.replace(true) {
        return;
    }
    cx.spawn(async move |cx| {
        let mut pending_work = false;
        loop {
            // Use a short catch-up interval after a busy dispatch budget,
            // then return to normal visible/hidden cadence once GTK is idle.
            let delay = VISIBLE_GTK_BROWSERS.with(|count| {
                Duration::from_millis(if count.get() == 0 {
                    16
                } else if pending_work {
                    1
                } else {
                    4
                })
            });
            Timer::after(delay).await;
            match cx.update(|_| {
                // Bound elapsed time as well as iterations so page work
                // yields promptly to GPUI input and terminal rendering.
                let started = Instant::now();
                for _ in 0..64 {
                    if !gtk::events_pending() {
                        break;
                    }
                    gtk::main_iteration_do(false);
                    if started.elapsed() >= Duration::from_millis(2) {
                        break;
                    }
                }
                gtk::events_pending()
            }) {
                Ok(pending) => pending_work = pending,
                Err(_) => break,
            }
        }
    })
    .detach();
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
            && !suffix
                .split('/')
                .next()
                .is_some_and(|port| port.parse::<u16>().is_ok())
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
        cx.simulate_click(gpui::point(px(150.), px(53.)), gpui::Modifiers::default());
        cx.simulate_input("first.localhost:3000");
        let plus_x = cx.update(|window, _| window.viewport_size().width - px(16.));
        cx.simulate_click(gpui::point(plus_x, px(16.)), gpui::Modifiers::default());
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

        cx.simulate_click(gpui::point(px(50.), px(16.)), gpui::Modifiers::default());
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
        cx.simulate_click(gpui::point(px(134.), px(16.)), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|_, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.tabs[0].entity.read(cx).draft, "second.localhost:5173");
        });
        cx.simulate_click(gpui::point(px(134.), px(16.)), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let pane = pane.read(cx);
            assert_eq!(pane.session(), BrowserSession::default());
            let tab = pane.tabs[0].entity.read(cx);
            assert!(tab.draft.is_empty());
            assert!(tab.address.read(cx).focus_handle(cx).is_focused(window));
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
