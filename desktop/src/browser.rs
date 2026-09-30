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
    Style, Timer, Window, div, prelude::*, px, relative, rgb,
};
#[cfg(target_os = "linux")]
use raw_window_handle::XlibWindowHandle;
use raw_window_handle::{HasWindowHandle, RawWindowHandle, WindowHandle};
use url::Url;
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};
use xd_desktop::theme::ThemeColors;

use crate::input::{ComposerEvent, ComposerInput};

#[derive(Clone, Debug)]
pub enum BrowserEvent {
    Navigated(String),
    Close,
}

enum NativeEvent {
    Started(String),
    Finished(String),
    Title(String),
    Open(String),
    Error(String),
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

pub struct BrowserPane {
    native: Option<Rc<NativeBrowser>>,
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
    visible: bool,
    native_visible: bool,
    colors: ThemeColors,
    events: Sender<NativeEvent>,
    bounds: Rc<Cell<Option<NativeAllocation>>>,
}

impl EventEmitter<BrowserEvent> for BrowserPane {}

impl BrowserPane {
    pub fn new(
        initial_url: Option<String>,
        colors: ThemeColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address = cx.new(|cx| ComposerInput::new(cx, "URL or localhost:3000"));
        let (events, receiver) = async_channel::unbounded();
        let mut pane = Self {
            native: None,
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
            error: None,
            visible: true,
            native_visible: false,
            colors,
            events: events.clone(),
            bounds: Rc::new(Cell::new(None)),
        };
        // GPUI's test window deliberately has no native window handle.
        if !cfg!(test) {
            match create_native(window, events, cx) {
                Ok((native, context)) => {
                    pane.native = Some(Rc::new(NativeBrowser {
                        view: RefCell::new(Some(native)),
                        _context: context,
                    }));
                }
                Err(error) => pane.error = Some(error),
            }
        }
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
            cx.notify();
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if let Some(native) = &self.native {
            let shown = visible && self.url.is_some() && self.error.is_none();
            if self.native_visible != shown {
                // GTK needs a fresh allocation after remapping a foreign
                // child window, even when GPUI's pane bounds are unchanged.
                self.bounds.set(None);
                self.native_visible = shown;
            }
            if let Some(Err(error)) = native.with_view(|view| view.set_visible(shown)) {
                let _ = self.events.try_send(NativeEvent::Error(error.to_string()));
            }
        }
    }

    /// Release the actual native child before GPUI destroys its parent window.
    /// Render frames can retain NativeBrowser's Rc without retaining a live
    /// foreign X11/NSView child after this call.
    pub fn shutdown(&mut self) {
        self.visible = false;
        self.native_visible = false;
        if let Some(native) = self.native.take() {
            let view = native.view.borrow_mut().take();
            #[cfg(target_os = "linux")]
            if let Some(view) = &view {
                use gtk::prelude::*;
                use wry::WebViewExtUnix;

                if let Some(window) = view
                    .webview()
                    .toplevel()
                    .and_then(|widget| widget.downcast::<gtk::Window>().ok())
                {
                    if let Some(gdk_window) = window.window() {
                        let mut owner = std::ptr::null_mut();
                        // SAFETY: read the opaque user-data pointer from a live
                        // GDK window on GTK's main thread without dereferencing it.
                        unsafe {
                            gtk::gdk::ffi::gdk_window_get_user_data(
                                gdk_window.as_ptr(),
                                &mut owner,
                            );
                        }
                        if owner.is_null() {
                            // Wry replaces GtkWindow's own GDK window with a
                            // foreign one without registering it. GTK requires
                            // this owner when unrealize unregisters the window.
                            window.register_window(&gdk_window);
                        }
                    }
                    // Wry destroys the X11 child before closing its GTK
                    // wrapper. Release GTK's native resources while that
                    // child exists; closing an unrealized wrapper is a no-op.
                    window.unrealize();
                }
            }
            drop(view);
        }
    }

    pub fn current_url(&self) -> Option<String> {
        self.url.clone()
    }

    #[cfg(test)]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn navigate(&mut self, address: &str, cx: &mut Context<Self>) {
        let url = match normalize_url(address) {
            Ok(url) => url,
            Err(error) => {
                self.error = Some(error);
                self.set_visible(self.visible);
                cx.notify();
                return;
            }
        };
        self.draft = url.clone();
        self.address_dirty = false;
        self.address
            .update(cx, |input, cx| input.set_text(url.clone(), cx));
        let Some(native) = self.native.as_ref() else {
            if cfg!(test) {
                self.url = Some(url.clone());
                cx.emit(BrowserEvent::Navigated(url));
            }
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
            Some(Err(error)) => self.error = Some(format!("Could not open page: {error}")),
            None => return,
        }
        self.set_visible(self.visible);
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
        cx.emit(BrowserEvent::Navigated(url));
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
                    pane.set_visible(pane.visible);
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
            NativeEvent::Title(title) => self.title = title,
            NativeEvent::Open(url) => self.navigate(&url, cx),
            NativeEvent::Error(error) => {
                self.loading = false;
                self.load_timed_out = false;
                self.error = Some(error);
            }
        }
        self.set_visible(self.visible);
        cx.notify();
    }

    fn refresh_navigation(&mut self, cx: &mut Context<Self>) {
        let Some(native) = self.native.as_ref() else {
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
        let result = match &self.native {
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
        self.set_visible(self.visible);
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(native) = &self.native {
            match native.with_view(WebView::reload) {
                Some(Ok(())) => {
                    self.start_loading(cx);
                    self.error = None;
                }
                Some(Err(error)) => self.error = Some(error.to_string()),
                None => return,
            }
        }
        self.set_visible(self.visible);
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
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .text_size(px(15.))
            .text_color(rgb(if enabled {
                self.colors.text
            } else {
                self.colors.muted
            }))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(self.colors.surface_high)))
            })
            .child(label)
    }
}

impl Drop for BrowserPane {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Render for BrowserPane {
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
                        .h(px(38.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(10.))
                        .border_b_1()
                        .border_color(rgb(self.colors.border))
                        .child(div().text_size(px(12.)).child("Browser"))
                        .child(
                            self.button("browser-close", "×", true)
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.set_visible(false);
                                    cx.emit(BrowserEvent::Close);
                                })),
                        ),
                )
                .child(
                    div()
                        .h(px(42.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .px(px(6.))
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
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .px(px(8.))
                                .bg(rgb(self.colors.surface))
                                .border_1()
                                .border_color(rgb(self.colors.border))
                                .rounded(px(4.))
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
                        )),
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
            if let Some(native) = &self.native {
                pane = pane.child(NativeSurface {
                    native: native.clone(),
                    bounds: self.bounds.clone(),
                    events: self.events.clone(),
                    visible: self.visible,
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

struct NativeBrowser {
    view: RefCell<Option<WebView>>,
    // Keep the context alive even if a previously painted layout element is
    // still holding the native view after the owning entity is released.
    _context: WebContext,
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
    events: Sender<NativeEvent>,
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
                let result = view.set_bounds(rect);
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
                    let _ = self.events.try_send(NativeEvent::Error(error.to_string()));
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
        #[cfg(not(target_os = "linux"))]
        let native = self.native.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, _| {
            if phase == DispatchPhase::Capture && !bounds.contains(&event.position) {
                #[cfg(target_os = "linux")]
                _window.activate_window();
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
    events: Sender<NativeEvent>,
    cx: &mut App,
) -> Result<(WebView, WebContext), String> {
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
    #[cfg(target_os = "linux")]
    let mut context = WebContext::new(None);
    #[cfg(not(target_os = "linux"))]
    let mut context = WebContext::new(Some(browser_data_directory()));
    let load_events = events.clone();
    let title_events = events.clone();
    let popup_events = events.clone();
    let builder = WebViewBuilder::new_with_web_context(&mut context);
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
                let _ = load_events.try_send(event);
            }
        })
        .with_document_title_changed_handler(move |title| {
            let _ = title_events.try_send(NativeEvent::Title(title));
        })
        .with_new_window_req_handler(move |url, _| {
            if is_browser_url(&url) {
                let _ = popup_events.try_send(NativeEvent::Open(url));
            }
            NewWindowResponse::Deny
        })
        .build_as_child(&parent)
        .map_err(|error| format!("Could not create the browser: {error}"))?;
    #[cfg(target_os = "linux")]
    {
        use webkit2gtk::WebViewExt;
        use wry::WebViewExtUnix;
        let webview = native.webview();
        webview.connect_load_failed(move |_, _, url, error| {
            if !error.matches(webkit2gtk::NetworkError::Cancelled) {
                let _ =
                    events.try_send(NativeEvent::Error(format!("Could not load {url}: {error}")));
            }
            false
        });
    }
    Ok((native, context))
}

#[cfg(target_os = "linux")]
fn linux_related_view() -> Result<webkit2gtk::WebView, String> {
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
    Ok(webkit2gtk::WebView::with_context(&context))
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
    thread_local! { static RUNNING: Cell<bool> = const { Cell::new(false) }; }
    if RUNNING.replace(true) {
        return;
    }
    cx.spawn(async move |cx| {
        loop {
            Timer::after(Duration::from_millis(16)).await;
            if cx
                .update(|_| {
                    // Bound work per tick so a busy page cannot starve GPUI input.
                    for _ in 0..64 {
                        if !gtk::events_pending() {
                            break;
                        }
                        gtk::main_iteration_do(false);
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
            BrowserPane::new(
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
            BrowserPane::new(
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
