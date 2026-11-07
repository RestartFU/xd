//! Chromium (through CEF) as the Linux browser engine.
//!
//! [`ChromiumView`] mirrors the parts of `wry::WebView` that `browser.rs` uses.
//! CEF runs its UI on its own thread, so these calls are queued by CEF and page
//! painting and scrolling never wait for GPUI. The browser is created
//! asynchronously, so calls that arrive early are remembered in [`Shared`] and
//! applied once it exists.

use std::{
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, mpsc},
    time::Duration,
};

use cef::*;
use x11rb::{
    connection::Connection,
    protocol::xproto::{
        AtomEnum, ColormapAlloc, ConfigureWindowAux, ConnectionExt, CreateWindowAux, ImageFormat,
        ImageOrder, InputFocus, PropMode, Window as XWindow, WindowClass,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use super::{NativeEvent, NativeEvents, browser_data_directory, find_x11_parent, is_browser_url};

/// Handle a Chromium helper-process launch. Call this first in `main`: helpers
/// re-execute this binary and must exit here without starting the application.
pub fn run_subprocess_if_requested() {
    if std::env::var_os(USER_NAMESPACE_PROBE).is_some() {
        std::process::exit(i32::from(!namespaces_usable()));
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let arguments = cef::args::Args::new();
    let code = execute_process(Some(arguments.as_main_args()), None, std::ptr::null_mut());
    if code >= 0 {
        std::process::exit(code);
    }
}

const USER_NAMESPACE_PROBE: &str = "XD_CHROMIUM_USER_NAMESPACE_PROBE";

/// Do what Chromium's namespace sandbox does. Creating the user namespace is
/// not enough: Ubuntu's AppArmor restriction allows that, then denies the
/// capabilities inside it, which is why this goes on to create the others.
///
/// This must run before any other thread starts, and in a process of its own.
fn namespaces_usable() -> bool {
    // SAFETY: these two calls cannot fail and read no memory.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    // SAFETY: the process is single-threaded here and exits straight after.
    if unsafe { libc::unshare(libc::CLONE_NEWUSER) } != 0 {
        return false;
    }
    let mapped = std::fs::write("/proc/self/setgroups", "deny").is_ok()
        && std::fs::write("/proc/self/uid_map", format!("0 {uid} 1")).is_ok()
        && std::fs::write("/proc/self/gid_map", format!("0 {gid} 1")).is_ok();
    // SAFETY: as above.
    mapped && unsafe { libc::unshare(libc::CLONE_NEWPID | libc::CLONE_NEWNET) } == 0
}

const SANDBOX_UNAVAILABLE: &str = "The browser needs its security sandbox, but this system does \
     not allow unprivileged user namespaces. Enable them to use the browser.";

/// Chromium aborts the whole process when it has no usable sandbox, so find out
/// first: opening a browser pane must not take down the application. The probe
/// runs in a child because `unshare` needs a single-threaded caller.
fn check_sandbox() -> Result<(), String> {
    if std::env::var_os("XD_CHROMIUM_NO_SANDBOX").is_some() {
        return Ok(());
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let helper_ready = executable
        .with_file_name("chrome-sandbox")
        .metadata()
        .is_ok_and(|helper| helper.uid() == 0 && helper.mode() & 0o4000 != 0);
    let namespaces_ready = std::process::Command::new(&executable)
        .env(USER_NAMESPACE_PROBE, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if helper_ready || namespaces_ready {
        return Ok(());
    }
    Err(SANDBOX_UNAVAILABLE.into())
}

wrap_app! {
    struct ChromiumApp {
        scale: String,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(command_line) = command_line else {
                return;
            };
            // `configure_platform` makes X11 the toolkit; the child embedding needs it.
            command_line.append_switch_with_value(
                Some(&CefString::from("ozone-platform")),
                Some(&CefString::from("x11")),
            );
            // Match GPUI's scale rather than depending on what the desktop publishes.
            command_line.append_switch_with_value(
                Some(&CefString::from("force-device-scale-factor")),
                Some(&CefString::from(self.scale.as_str())),
            );
        }
    }
}

/// CEF allows one initialization per process, so the first browser's scale
/// applies to all of them. GPUI's X11 scale is also per process.
fn ensure_initialized(scale: f32) -> Result<(), String> {
    static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            check_sandbox()?;
            let directory = browser_data_directory().join("chromium");
            std::fs::create_dir_all(&directory)
                .map_err(|error| format!("Could not create browser data directory: {error}"))?;
            let settings = Settings {
                no_sandbox: i32::from(std::env::var_os("XD_CHROMIUM_NO_SANDBOX").is_some()),
                multi_threaded_message_loop: 1,
                persist_session_cookies: 1,
                // Without a cache path CEF keeps the profile in memory, and every
                // restart would sign the user out of every site.
                cache_path: CefString::from(directory.to_string_lossy().as_ref()),
                root_cache_path: CefString::from(directory.to_string_lossy().as_ref()),
                ..Default::default()
            };
            let arguments = cef::args::Args::new();
            let mut app = ChromiumApp::new(scale.to_string());
            if initialize(
                Some(arguments.as_main_args()),
                Some(&settings),
                Some(&mut app),
                std::ptr::null_mut(),
            ) != 1
            {
                return Err("Could not initialize the browser.".to_string());
            }
            Ok(())
        })
        .clone()
}

/// CEF's browser object is reference counted atomically, and these calls are
/// documented as usable from any browser-process thread.
struct SharedBrowser(Browser);
// SAFETY: see above; the wrapper only exposes CEF's thread-safe calls.
unsafe impl Send for SharedBrowser {}

#[derive(Default)]
struct Shared {
    /// GPUI's window, which takes the keyboard back from the page.
    parent: XWindow,
    /// The root of GPUI's screen, where a host waiting on CEF is kept.
    root: XWindow,
    /// Sits between GPUI's window and Chromium's; see [`create_host_window`].
    host: XWindow,
    colormap: u32,
    browser: Option<SharedBrowser>,
    pending_url: Option<String>,
    bounds: Option<(i32, i32, u32, u32)>,
    visible: bool,
    /// Set once the view is being closed, which can happen before the browser
    /// it asked for exists. That browser then closes as soon as it arrives.
    closed: Option<mpsc::Sender<()>>,
    /// CEF has finished with the browser, so its windows may go.
    browser_gone: bool,
    /// A close gave up waiting for CEF; release the windows once it finishes.
    release_pending: bool,
    released: bool,
}

type SharedState = Arc<Mutex<Shared>>;

fn lock(shared: &SharedState) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

fn x11() -> Option<&'static RustConnection> {
    static CONNECTION: OnceLock<Option<RustConnection>> = OnceLock::new();
    CONNECTION
        .get_or_init(|| x11rb::connect(None).ok().map(|(connection, _)| connection))
        .as_ref()
}

fn browser_window(browser: &Browser) -> Option<XWindow> {
    u32::try_from(browser.host()?.window_handle()).ok()
}

/// Bring the X11 window in line with what GPUI last asked for.
fn apply_window_state(shared: &Shared) {
    let Some(connection) = x11() else {
        return;
    };
    if let Some((x, y, width, height)) = shared.bounds {
        let _ = connection.configure_window(
            shared.host,
            &ConfigureWindowAux::new()
                .x(x)
                .y(y)
                .width(width.max(1))
                .height(height.max(1)),
        );
    }
    if shared.visible {
        let _ = connection.map_window(shared.host);
    } else {
        let _ = connection.unmap_window(shared.host);
    }
    let window = shared
        .browser
        .as_ref()
        .and_then(|browser| browser_window(&browser.0));
    let Some(window) = window else {
        let _ = connection.flush();
        return;
    };
    if let Some((_, _, width, height)) = shared.bounds {
        let _ = connection.configure_window(
            window,
            &ConfigureWindowAux::new()
                .x(0)
                .y(0)
                .width(width.max(1))
                .height(height.max(1)),
        );
    }
    // CEF treats `_NET_WM_STATE_HIDDEN` on its window as the page being hidden,
    // which is what lets the renderer stop producing frames while unmapped.
    if let (Ok(state), Ok(hidden)) = (
        intern(connection, b"_NET_WM_STATE"),
        intern(connection, b"_NET_WM_STATE_HIDDEN"),
    ) {
        if shared.visible {
            let _ = connection.delete_property(window, state);
        } else {
            let _ = connection.change_property32(
                PropMode::REPLACE,
                window,
                state,
                AtomEnum::ATOM,
                &[hidden],
            );
        }
    }
    let _ = connection.flush();
}

/// Move a host whose browser CEF has not finished with out from under GPUI's
/// window, hidden, so destroying GPUI's window cannot destroy Chromium's with
/// it. The round trip makes sure the server has done so before GPUI goes on.
fn detach_windows(shared: &Shared) {
    if let Some(connection) = x11() {
        let _ = connection.unmap_window(shared.host);
        let _ = connection.reparent_window(shared.host, shared.root, 0, 0);
        let _ = connection.get_input_focus().map(|cookie| cookie.reply());
    }
}

/// Destroy the host window and its colormap, once, after Chromium is done.
fn release_windows(shared: &mut Shared) {
    if std::mem::replace(&mut shared.released, true) {
        return;
    }
    if let Some(connection) = x11() {
        let _ = connection.destroy_window(shared.host);
        let _ = connection.free_colormap(shared.colormap);
        let _ = connection.flush();
    }
}

/// GPUI's window has a 32-bit visual. Chromium creates its window with the
/// screen's default visual, which X11 rejects as a direct child of a window of
/// another depth unless a colormap and border are given, so parent it here.
/// Chromium inherits the colormap, so it must outlive Chromium's window.
fn create_host_window(parent: XWindow) -> Result<(XWindow, u32, XWindow), String> {
    let error = |error: String| format!("Could not embed the browser: {error}");
    let connection = x11().ok_or_else(|| error("no X11 connection".into()))?;
    // The visual and colormap must come from the screen GPUI's window is on,
    // which need not be the first on a server with several.
    let root = connection
        .get_geometry(parent)
        .map_err(|e| error(e.to_string()))?
        .reply()
        .map_err(|e| error(e.to_string()))?
        .root;
    let screen = connection
        .setup()
        .roots
        .iter()
        .find(|screen| screen.root == root)
        .ok_or_else(|| error("no X11 screen".into()))?;
    let host = connection.generate_id().map_err(|e| error(e.to_string()))?;
    let colormap = connection.generate_id().map_err(|e| error(e.to_string()))?;
    connection
        .create_colormap(
            ColormapAlloc::NONE,
            colormap,
            screen.root,
            screen.root_visual,
        )
        .map_err(|e| error(e.to_string()))?
        .check()
        .map_err(|e| error(e.to_string()))?;
    connection
        .create_window(
            screen.root_depth,
            host,
            parent,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .background_pixel(screen.black_pixel)
                .border_pixel(screen.black_pixel)
                .colormap(colormap),
        )
        .map_err(|e| error(e.to_string()))?
        .check()
        .map_err(|e| error(e.to_string()))?;
    let _ = connection.flush();
    Ok((host, colormap, root))
}

fn intern(connection: &RustConnection, name: &[u8]) -> Result<u32, ()> {
    connection
        .intern_atom(false, name)
        .map_err(|_| ())?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|_| ())
}

fn text(value: &CefStringUserfree) -> String {
    CefString::from(value).to_string()
}

wrap_life_span_handler! {
    struct ViewLifeSpan {
        shared: SharedState,
        events: NativeEvents,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else {
                return;
            };
            let mut shared = lock(&self.shared);
            if shared.closed.is_some() {
                let host = browser.host();
                shared.browser = Some(SharedBrowser(browser.clone()));
                drop(shared);
                if let Some(host) = host {
                    host.close_browser(1);
                }
                return;
            }
            shared.browser = Some(SharedBrowser(browser.clone()));
            apply_window_state(&shared);
            if let (Some(url), Some(frame)) = (shared.pending_url.take(), browser.main_frame()) {
                frame.load_url(Some(&CefString::from(url.as_str())));
            }
        }

        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let Some(url) = target_url.map(ToString::to_string)
                && is_browser_url(&url)
            {
                self.events.send(NativeEvent::Open(url));
            }
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            let mut shared = lock(&self.shared);
            shared.browser = None;
            shared.browser_gone = true;
            if let Some(closed) = shared.closed.take() {
                let _ = closed.send(());
            }
            if shared.release_pending {
                release_windows(&mut shared);
            }
        }
    }
}

/// A page's request for files, carried to GPUI's thread, which owns the system's
/// file chooser. Chromium keeps the page waiting until it is answered.
pub struct FileDialog {
    callback: SharedCallback,
    mode: FileDialogMode,
    title: String,
    default_path: Option<PathBuf>,
}

struct SharedCallback(FileDialogCallback);
// SAFETY: CEF documents this callback as usable from any browser-process thread.
unsafe impl Send for SharedCallback {}

impl FileDialog {
    pub(super) fn show<T: 'static>(self, cx: &mut gpui::Context<T>) {
        if self.mode == FileDialogMode::SAVE {
            let path = self.default_path.clone().unwrap_or_default();
            let directory = path.parent().map(PathBuf::from).unwrap_or_default();
            let chosen =
                cx.prompt_for_new_path(&directory, path.file_name().and_then(|n| n.to_str()));
            cx.spawn(async move |_, _| match chosen.await {
                Ok(Ok(Some(path))) => self.answer(&[path]),
                outcome => self.decline(outcome.map(|result| result.map(|_| ()))),
            })
            .detach();
            return;
        }
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: self.mode != FileDialogMode::OPEN_FOLDER,
            directories: self.mode == FileDialogMode::OPEN_FOLDER,
            multiple: self.mode == FileDialogMode::OPEN_MULTIPLE,
            prompt: (!self.title.is_empty()).then(|| self.title.clone().into()),
        });
        cx.spawn(async move |_, _| match chosen.await {
            Ok(Ok(Some(paths))) => self.answer(&paths),
            outcome => self.decline(outcome.map(|result| result.map(|_| ()))),
        })
        .detach();
    }

    fn answer(&self, paths: &[PathBuf]) {
        let mut files = CefStringList::new();
        for path in paths {
            files.append(&path.to_string_lossy());
        }
        self.callback.0.cont(Some(&mut files));
    }

    /// A closed chooser is an answer too. If the desktop has no chooser at all
    /// the page sees the same thing, so say why somewhere it can be found.
    fn decline<E: std::fmt::Display, F: std::fmt::Display>(
        &self,
        outcome: Result<Result<(), E>, F>,
    ) {
        match outcome {
            Ok(Err(error)) => eprintln!("Could not open the file chooser: {error}"),
            Err(error) => eprintln!("Could not open the file chooser: {error}"),
            Ok(Ok(())) => {}
        }
        self.callback.0.cancel();
    }
}

wrap_dialog_handler! {
    struct ViewDialogs {
        events: NativeEvents,
    }

    impl DialogHandler {
        fn on_file_dialog(
            &self,
            _browser: Option<&mut Browser>,
            mode: FileDialogMode,
            title: Option<&CefString>,
            default_file_path: Option<&CefString>,
            _accept_filters: Option<&mut CefStringList>,
            _accept_extensions: Option<&mut CefStringList>,
            _accept_descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else {
                return 0;
            };
            self.events.send(NativeEvent::FileDialog(FileDialog {
                callback: SharedCallback(callback.clone()),
                mode,
                title: title.map(ToString::to_string).unwrap_or_default(),
                default_path: default_file_path
                    .map(ToString::to_string)
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from),
            }));
            1
        }
    }
}

wrap_load_handler! {
    struct ViewLoad {
        events: NativeEvents,
    }

    impl LoadHandler {
        fn on_load_start(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _transition_type: TransitionType,
        ) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() != 0) {
                let url = text(&frame.url());
                if is_browser_url(&url) {
                    self.events.send(NativeEvent::Started(url));
                }
            }
        }

        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _http_status_code: ::std::os::raw::c_int,
        ) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() != 0) {
                let url = text(&frame.url());
                if is_browser_url(&url) {
                    self.events.send(NativeEvent::Finished(url));
                }
            }
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            if error_code == Errorcode::ABORTED || frame.is_none_or(|frame| frame.is_main() == 0) {
                return;
            }
            let url = failed_url.map(ToString::to_string).unwrap_or_default();
            let reason = error_text.map(ToString::to_string).unwrap_or_default();
            self.events
                .send(NativeEvent::Error(format!("Could not load {url}: {reason}")));
        }
    }
}

wrap_display_handler! {
    struct ViewDisplay {
        events: NativeEvents,
    }

    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            self.events
                .title_changed(title.map(ToString::to_string).unwrap_or_default());
        }
    }
}

wrap_request_handler! {
    struct ViewRequests {
        events: NativeEvents,
    }

    impl RequestHandler {
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: ::std::os::raw::c_int,
            _is_redirect: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let main_frame = frame.is_some_and(|frame| frame.is_main() != 0);
            let url = request.map(|request| text(&request.url())).unwrap_or_default();
            i32::from(main_frame && !is_browser_url(&url) && url != "about:blank")
        }

        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            _status: TerminationStatus,
            _error_code: ::std::os::raw::c_int,
            _error_string: Option<&CefString>,
        ) {
            self.events.send(NativeEvent::Error(
                "The page stopped responding and was closed. Reload to try again.".into(),
            ));
        }
    }
}

wrap_client! {
    struct ViewClient {
        shared: SharedState,
        events: NativeEvents,
    }

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(ViewLifeSpan::new(self.shared.clone(), self.events.clone()))
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(ViewLoad::new(self.events.clone()))
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(ViewDisplay::new(self.events.clone()))
        }

        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(ViewDialogs::new(self.events.clone()))
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(ViewRequests::new(self.events.clone()))
        }
    }
}

pub struct ChromiumView {
    shared: SharedState,
}

/// X11 geometry is physical, unlike GPUI's logical layout.
#[derive(Clone, Copy)]
pub struct PhysicalBounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl PhysicalBounds {
    pub fn new(x: f64, y: f64, width: f64, height: f64, scale: f64) -> Self {
        Self {
            x: (x * scale).round() as i32,
            y: (y * scale).round() as i32,
            width: (width * scale).round() as u32,
            height: (height * scale).round() as u32,
        }
    }
}

impl ChromiumView {
    pub(super) fn create(events: NativeEvents, scale: f32) -> Result<Self, String> {
        if std::env::var_os("DISPLAY").is_none_or(|display| display.is_empty()) {
            return Err(
                "The integrated browser needs X11 or XWayland. Enable XWayland and restart xd."
                    .into(),
            );
        }
        ensure_initialized(scale)?;
        Self::create_in(find_x11_parent()?, events)
    }

    fn create_in(parent: XWindow, events: NativeEvents) -> Result<Self, String> {
        let (host, colormap, root) = create_host_window(parent)?;
        let shared = SharedState::new(Mutex::new(Shared {
            parent,
            root,
            host,
            colormap,
            ..Default::default()
        }));
        let info = WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            ..Default::default()
        }
        .set_as_child(
            host as _,
            &cef::Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        );
        let mut client = ViewClient::new(shared.clone(), events);
        let created = browser_host_create_browser(
            Some(&info),
            Some(&mut client),
            Some(&CefString::from("about:blank")),
            Some(&BrowserSettings::default()),
            None,
            None,
        );
        if created != 1 {
            return Err("Could not create the browser.".into());
        }
        Ok(Self { shared })
    }

    fn browser(&self) -> Result<Browser, String> {
        lock(&self.shared)
            .browser
            .as_ref()
            .map(|browser| browser.0.clone())
            .ok_or_else(|| "The browser is still starting.".to_string())
    }

    pub fn load_url(&self, url: &str) -> Result<(), String> {
        let mut shared = lock(&self.shared);
        let Some(browser) = &shared.browser else {
            shared.pending_url = Some(url.to_string());
            return Ok(());
        };
        let frame = browser
            .0
            .main_frame()
            .ok_or_else(|| "The page is not available.".to_string())?;
        frame.load_url(Some(&CefString::from(url)));
        Ok(())
    }

    pub fn url(&self) -> Result<String, String> {
        let frame = self
            .browser()?
            .main_frame()
            .ok_or_else(|| "The page is not available.".to_string())?;
        Ok(text(&frame.url()))
    }

    pub fn can_go_back(&self) -> Result<bool, String> {
        Ok(self.browser()?.can_go_back() != 0)
    }

    pub fn can_go_forward(&self) -> Result<bool, String> {
        Ok(self.browser()?.can_go_forward() != 0)
    }

    pub fn go_back(&self) -> Result<(), String> {
        self.browser()?.go_back();
        Ok(())
    }

    pub fn go_forward(&self) -> Result<(), String> {
        self.browser()?.go_forward();
        Ok(())
    }

    pub fn reload(&self) -> Result<(), String> {
        self.browser()?.reload();
        Ok(())
    }

    pub fn set_visible(&self, visible: bool) -> Result<(), String> {
        let mut shared = lock(&self.shared);
        shared.visible = visible;
        apply_window_state(&shared);
        Ok(())
    }

    pub fn set_bounds(&self, bounds: PhysicalBounds) -> Result<(), String> {
        let mut shared = lock(&self.shared);
        shared.bounds = Some((bounds.x, bounds.y, bounds.width, bounds.height));
        apply_window_state(&shared);
        Ok(())
    }

    pub fn focus(&self) -> Result<(), String> {
        let browser = self.browser()?;
        if let Some(host) = browser.host() {
            host.set_focus(1);
        }
        Ok(())
    }

    /// Give the keyboard back to GPUI's window, but only if this page has it:
    /// hiding an inactive page must not take focus from another application.
    pub fn blur(&self) {
        let Ok(browser) = self.browser() else {
            return;
        };
        if let Some(host) = browser.host() {
            host.set_focus(0);
        }
        let Some(connection) = x11() else {
            return;
        };
        // Chromium's window is a grandchild of GPUI's, so focus goes back to
        // GPUI itself, not to the host window in between.
        let (host, parent) = {
            let shared = lock(&self.shared);
            (shared.host, shared.parent)
        };
        if has_keyboard_focus(connection, host) {
            let _ = connection.set_input_focus(InputFocus::PARENT, parent, 0u32);
            let _ = connection.flush();
        }
    }

    /// The page's current pixels in GPUI's BGRA order, read from the X server
    /// while the page is on screen. An unmapped window has none to give.
    pub fn capture(&self) -> Option<image::RgbaImage> {
        let connection = x11()?;
        let (host, (_, _, width, height)) = {
            let shared = lock(&self.shared);
            if !shared.visible {
                return None;
            }
            (shared.host, shared.bounds?)
        };
        let (width, height) = (u16::try_from(width).ok()?, u16::try_from(height).ok()?);
        if width == 0 || height == 0 {
            return None;
        }
        let setup = connection.setup();
        let reply = connection
            .get_image(ImageFormat::Z_PIXMAP, host, 0, 0, width, height, u32::MAX)
            .ok()?
            .reply()
            .ok()?;
        // A 24-bit window is stored as 32-bit pixels; least significant byte
        // first, that is already blue, green, red, and an unused byte.
        let format = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == reply.depth)?;
        if format.bits_per_pixel != 32 || setup.image_byte_order != ImageOrder::LSB_FIRST {
            return None;
        }
        let row = usize::from(width) * 4;
        let pad = usize::from(format.scanline_pad) / 8;
        let stride = row.div_ceil(pad) * pad;
        let mut frame = Vec::with_capacity(row * usize::from(height));
        for line in reply.data.chunks(stride).take(usize::from(height)) {
            for pixel in line.get(..row)?.chunks_exact(4) {
                frame.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 0xff]);
            }
        }
        image::RgbaImage::from_raw(u32::from(width), u32::from(height), frame)
    }

    /// Destroy the browser before returning: the parent X11 window may be next.
    /// If CEF does not confirm in time, its windows stay until it does, since
    /// Chromium may still be drawing into them or about to create one there.
    pub fn close(&self, timeout: Duration) {
        let (closed, done) = mpsc::channel();
        let mut shared = lock(&self.shared);
        shared.closed = Some(closed);
        let host = shared.browser.as_ref().and_then(|browser| browser.0.host());
        drop(shared);
        if let Some(host) = host {
            host.close_browser(1);
        }
        let confirmed = done.recv_timeout(timeout).is_ok();
        let mut shared = lock(&self.shared);
        if confirmed || shared.browser_gone {
            release_windows(&mut shared);
        } else {
            shared.release_pending = true;
            detach_windows(&shared);
        }
    }
}

fn has_keyboard_focus(connection: &RustConnection, window: XWindow) -> bool {
    let Ok(Ok(reply)) = connection.get_input_focus().map(|cookie| cookie.reply()) else {
        return false;
    };
    let mut focus = reply.focus;
    for _ in 0..8 {
        if focus == window {
            return true;
        }
        if focus <= 1 {
            return false;
        }
        match connection.query_tree(focus).map(|cookie| cookie.reply()) {
            Ok(Ok(tree)) => focus = tree.parent,
            _ => return false,
        }
    }
    false
}

/// Load one page in a real browser and exit, so that a packaged build can prove
/// its helpers, data files, and sandbox work. Exits 0 once the page has loaded,
/// and 3 if the sandbox check stopped Chromium from starting.
pub fn run_smoke_test_if_requested() {
    const TIMEOUT: Duration = Duration::from_secs(60);

    let Some(url) = std::env::var_os("XD_BROWSER_SMOKE_TEST_URL") else {
        return;
    };
    let url = url.to_string_lossy().into_owned();
    if let Err(error) = ensure_initialized(1.) {
        eprintln!("browser smoke: {error}");
        std::process::exit(if error == SANDBOX_UNAVAILABLE { 3 } else { 1 });
    }
    std::thread::spawn(|| {
        std::thread::sleep(TIMEOUT);
        eprintln!("browser smoke: the page did not load");
        std::process::exit(1);
    });
    let root =
        x11().and_then(|connection| connection.setup().roots.first().map(|screen| screen.root));
    let Some(root) = root else {
        eprintln!("browser smoke: no X11 screen");
        std::process::exit(1);
    };
    let (events, received) = NativeEvents::channel();
    let view = match ChromiumView::create_in(root, events) {
        Ok(view) => view,
        Err(error) => {
            eprintln!("browser smoke: {error}");
            std::process::exit(1);
        }
    };
    let _ = view.set_bounds(PhysicalBounds::new(0., 0., 400., 300., 1.));
    let _ = view.set_visible(true);
    let _ = view.load_url(&url);
    while let Ok(event) = received.recv_blocking() {
        match event {
            NativeEvent::Finished(_) => {
                println!("browser smoke: page loaded");
                std::process::exit(0);
            }
            NativeEvent::Error(error) => {
                eprintln!("browser smoke: {error}");
                std::process::exit(1);
            }
            _ => {}
        }
    }
    std::process::exit(1);
}
