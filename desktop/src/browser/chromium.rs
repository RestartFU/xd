//! Chromium (through CEF) as the Linux browser engine.
//!
//! [`ChromiumView`] mirrors the parts of `wry::WebView` that `browser.rs` uses.
//! CEF runs its UI on its own thread, so these calls are queued by CEF and page
//! painting and scrolling never wait for GPUI. The browser is created
//! asynchronously, so calls that arrive early are remembered in [`Shared`] and
//! applied once it exists.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicUsize, Ordering},
        mpsc,
    },
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
    // Chromium will not sandbox itself as root, and exits the whole process
    // rather than failing to start.
    // SAFETY: this call cannot fail and reads no memory.
    if unsafe { libc::geteuid() } == 0 {
        return Err("The browser cannot run as root. Run xd as your own user to use it.".into());
    }
    // xd ships no SUID sandbox helper, so user namespaces are the only way in.
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let namespaces_ready = std::process::Command::new(&executable)
        .env(USER_NAMESPACE_PROBE, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if namespaces_ready {
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

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(ChromiumProcessHandler::new())
        }
    }
}

wrap_browser_process_handler! {
    struct ChromiumProcessHandler;

    impl BrowserProcessHandler {
        /// Another process asked for this profile and was refused; see
        /// [`profile_holder`]. Left to CEF, its request would open a Chromium
        /// window of its own, outside xd. There is nothing to open: xd's panes
        /// make their own browsers.
        fn on_already_running_app_relaunch(
            &self,
            _command_line: Option<&mut CommandLine>,
            _current_directory: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }
    }
}

/// Chromium lets one process at a time use a profile. Another that tries is
/// refused, and CEF passes its launch to the one holding the profile. The
/// profile's `SingletonLock` symlink names the holder as `host-pid`, and its
/// `SingletonSocket` leads to the socket the holder listens on. A lock left by
/// a process that died stays behind, and its PID may since belong to another
/// process, so the holder counts only while its socket still accepts.
fn profile_holder(directory: &Path) -> Option<u32> {
    let target = std::fs::read_link(directory.join("SingletonLock")).ok()?;
    let (_, pid) = target.to_str()?.rsplit_once('-')?;
    let pid = pid.parse::<u32>().ok()?;
    // The socket lives in a short temporary directory; the link to it may be
    // too long for a socket address.
    let socket = std::fs::read_link(directory.join("SingletonSocket")).ok()?;
    (pid != std::process::id() && listening(&directory.join(socket))).then_some(pid)
}

/// Whether a process listens on the Unix socket at `path`, asked without
/// waiting: a holder too busy to accept would otherwise stall the browser
/// thread. A full queue still means someone listens. The holder takes the
/// connection, which sends nothing, as a bad message and drops it.
fn listening(path: &Path) -> bool {
    use std::os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    };

    // SAFETY: an all-zero `sockaddr_un` is a valid value.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= address.sun_path.len() {
        return false;
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    // SAFETY: plain system call; the result is checked before use.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return false;
    }
    // SAFETY: `fd` is a new descriptor nothing else owns.
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: `address` is a valid `sockaddr_un` that outlives the call.
    let connected = unsafe {
        libc::connect(
            socket.as_raw_fd(),
            std::ptr::from_ref(&address).cast(),
            std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
        )
    } == 0;
    connected
        || matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EAGAIN | libc::EINPROGRESS)
        )
}

fn profile_in_use(pid: u32) -> String {
    format!(
        "The browser is already open in another xd (process {pid}). \
         It opens here once that xd quits."
    )
}

/// CEF allows one initialization per process, so after a refused one only a
/// restart helps.
fn profile_taken(pid: u32) -> String {
    format!(
        "The browser is already open in another xd (process {pid}). \
         Quit that xd, then restart this one."
    )
}

/// Whether CEF started, decided by the first browser.
static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();
/// The last start found the profile held by another process. CEF was left
/// uninitialized, so a later start can still succeed.
static PROFILE_HELD: AtomicBool = AtomicBool::new(false);

/// Whether starting the browser failed only because another process held its
/// profile, so trying again once that process lets go can succeed.
pub(super) fn profile_held_elsewhere() -> bool {
    PROFILE_HELD.load(Ordering::SeqCst)
        && INITIALIZED.get().is_none()
        && !SHUT_DOWN.load(Ordering::SeqCst)
}
/// Set once CEF has been shut down, after which no browser can be created.
static SHUT_DOWN: AtomicBool = AtomicBool::new(false);
/// Browsers CEF has accepted, whether or not created yet, and not finished
/// closing. One that is never created keeps CEF from shutting down, which is
/// the safe way round.
static LIVE_BROWSERS: AtomicUsize = AtomicUsize::new(0);

/// CEF allows one initialization per process, so the first browser's scale
/// applies to all of them. GPUI's X11 scale is also per process.
fn ensure_initialized(scale: f32) -> Result<(), String> {
    if SHUT_DOWN.load(Ordering::SeqCst) {
        return Err("The browser has shut down.".into());
    }
    if let Some(result) = INITIALIZED.get() {
        return result.clone();
    }
    let directory = browser_data_directory().join("chromium");
    // Initializing against a profile another process holds fails, and CEF
    // allows no second try. Leaving it uninitialized lets a later pane start
    // once the other process has gone.
    let holder = profile_holder(&directory);
    PROFILE_HELD.store(holder.is_some(), Ordering::SeqCst);
    if let Some(pid) = holder {
        return Err(profile_in_use(pid));
    }
    INITIALIZED
        .get_or_init(|| {
            check_sandbox()?;
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
                // Another process may have taken the profile since the check.
                return Err(profile_holder(&directory)
                    .map_or_else(|| "Could not initialize the browser.".into(), profile_taken));
            }
            Ok(())
        })
        .clone()
}

/// Shut CEF down so it writes out the profile, such as recent sign-ins, before
/// the process exits. Call on the thread that created the browsers, after
/// closing them. A browser CEF has not finished closing would make shutting
/// down unsafe, so then CEF is left running and the process exits around it.
pub fn shutdown() {
    if SHUT_DOWN.swap(true, Ordering::SeqCst)
        || !matches!(INITIALIZED.get(), Some(Ok(())))
        || LIVE_BROWSERS.load(Ordering::SeqCst) != 0
    {
        return;
    }
    cef::shutdown();
}

/// CEF's browser object is reference counted atomically, and these calls are
/// documented as usable from any browser-process thread.
struct SharedBrowser(Browser);
// SAFETY: see above; the wrapper only exposes CEF's thread-safe calls.
unsafe impl Send for SharedBrowser {}

/// Keeps the DevTools observer registered; dropping it removes the observer.
/// Only created and dropped on CEF's UI thread or with the browser closing.
struct SharedRegistration(#[allow(dead_code)] Registration);
// SAFETY: CEF's registration is reference counted atomically and has no calls.
unsafe impl Send for SharedRegistration {}

/// DevTools protocol calls sent and not yet answered, by message id.
#[derive(Default)]
struct DevToolsCalls {
    next: AtomicI32,
    waiting: Mutex<HashMap<i32, mpsc::Sender<Result<serde_json::Value, String>>>>,
}

impl DevToolsCalls {
    fn take(&self, id: i32) -> Option<mpsc::Sender<Result<serde_json::Value, String>>> {
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
    }

    /// Fails every waiting call, as when the page has gone.
    fn fail_all(&self) {
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

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
    /// Agent commands running on this page. While any is, a page off screen
    /// still paints, so a screenshot or a page that waits for a frame works.
    awake: usize,
    /// When the last agent command here ended. A popup it caused can be
    /// asked for a moment after.
    agent_finished: Option<std::time::Instant>,
    /// Tabs this page has asked for, so a command can tell it opened one.
    popups: u64,
    devtools: Arc<DevToolsCalls>,
    devtools_observer: Option<SharedRegistration>,
    /// Held by the agent command running on this page; see `agent::take_turn`.
    turns: Arc<Mutex<()>>,
}

/// How long after an agent command a popup still counts as the agent's.
const AGENT_POPUP_GRACE: Duration = Duration::from_secs(1);

impl Shared {
    /// Whether an agent's command is running on the page, or just ended.
    fn agent_active(&self) -> bool {
        self.awake > 0
            || self
                .agent_finished
                .is_some_and(|finished| finished.elapsed() < AGENT_POPUP_GRACE)
    }
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
        if shared.visible || shared.awake > 0 {
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
    let created = connection
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
        .map_err(|e| error(e.to_string()))
        .and_then(|cookie| cookie.check().map_err(|e| error(e.to_string())));
    if let Err(error) = created {
        let _ = connection.free_colormap(colormap);
        let _ = connection.flush();
        return Err(error);
    }
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
                // Decided now, while the command that made the page ask may
                // still run, rather than when GPUI gets to the event.
                let by_agent = {
                    let mut shared = lock(&self.shared);
                    shared.popups += 1;
                    shared.agent_active()
                };
                self.events.send(NativeEvent::Open { url, by_agent });
            }
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            LIVE_BROWSERS.fetch_sub(1, Ordering::SeqCst);
            let mut shared = lock(&self.shared);
            shared.browser = None;
            shared.browser_gone = true;
            shared.devtools_observer = None;
            shared.devtools.fail_all();
            match shared.closed.take() {
                Some(closed) => {
                    let _ = closed.send(());
                }
                // xd did not ask for this: the page closed itself, with
                // `window.close()`. Close its tab, as a browser would.
                None => self.events.send(NativeEvent::Closed),
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
                    // CEF's Alloy embedder disables Chromium's native history
                    // overscroll controller. Recognize unconsumed horizontal
                    // wheel gestures in the page and route them through the
                    // same history controls as the toolbar.
                    frame.execute_java_script(
                        Some(&CefString::from(include_str!("history_gestures.js"))),
                        Some(&CefString::from("xd-browser-history.js")),
                        1,
                    );
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

wrap_download_handler! {
    struct ViewDownloads;

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }

        // Chromium's runtime here has no download UI and cancels unless told
        // where to save, so save like WebKit did: into Downloads, never over
        // an existing file.
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else {
                return 0;
            };
            let name = suggested_name.map(ToString::to_string).unwrap_or_default();
            let directory = downloads_directory();
            let _ = std::fs::create_dir_all(&directory);
            // Two downloads of the same name could both find it free before
            // either creates its file, so names stay reserved until done.
            let mut reserved = RESERVED_DOWNLOADS
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let path = unused_path(&directory, &name, |path| {
                path.exists() || reserved.iter().any(|(_, taken)| taken == path)
            });
            if let Some(item) = download_item {
                reserved.push((item.id(), path.clone()));
            }
            drop(reserved);
            callback.cont(Some(&CefString::from(path.to_string_lossy().as_ref())), 0);
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            if let Some(item) = download_item
                && (item.is_complete() != 0 || item.is_canceled() != 0)
            {
                let id = item.id();
                RESERVED_DOWNLOADS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|(reserved, _)| *reserved != id);
            }
        }
    }
}

/// Destinations of downloads still in progress, by CEF's download id.
static RESERVED_DOWNLOADS: Mutex<Vec<(u32, PathBuf)>> = Mutex::new(Vec::new());

/// The user's Downloads directory, as the XDG user-dirs configuration names it.
fn downloads_directory() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|dirs| xdg_download_dir(&dirs, &home))
        .unwrap_or_else(|| home.join("Downloads"))
}

/// Read `XDG_DOWNLOAD_DIR="$HOME/..."` from a user-dirs file. A value of just
/// `$HOME` disables the directory, which is when the default is used.
fn xdg_download_dir(dirs: &str, home: &std::path::Path) -> Option<PathBuf> {
    let value = dirs.lines().find_map(|line| {
        line.trim()
            .strip_prefix("XDG_DOWNLOAD_DIR=")
            .map(|value| value.trim().trim_matches('"'))
    })?;
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    (path.is_absolute() && path != home).then_some(path)
}

/// `directory/name`, or `name (1)`, `name (2)`, ... before its extension if
/// that is taken. Only the name's last component is kept, so a page cannot
/// save outside the directory.
fn unused_path(
    directory: &std::path::Path,
    name: &str,
    taken: impl Fn(&std::path::Path) -> bool,
) -> PathBuf {
    let name = std::path::Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("download");
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };
    (0..)
        .map(|index| {
            let stem = if index == 0 {
                stem.to_string()
            } else {
                format!("{stem} ({index})")
            };
            directory.join(match extension {
                Some(extension) => format!("{stem}.{extension}"),
                None => stem,
            })
        })
        .find(|path| !taken(path))
        .unwrap_or_else(|| directory.join(name))
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
            if main_frame && let Some(back) = history_gesture(&url) {
                self.events.send(NativeEvent::HistoryGesture(back));
                return 1;
            }
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

fn history_gesture(url: &str) -> Option<bool> {
    match url {
        "xd-browser-history://back/" => Some(true),
        "xd-browser-history://forward/" => Some(false),
        _ => None,
    }
}

wrap_dev_tools_message_observer! {
    struct DevToolsAnswers {
        calls: Arc<DevToolsCalls>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            let Some(answer) = self.calls.take(message_id) else {
                return;
            };
            let value = result
                .and_then(|result| serde_json::from_slice::<serde_json::Value>(result).ok())
                .unwrap_or_default();
            let _ = answer.send(if success != 0 {
                Ok(value)
            } else {
                Err(value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("The page refused the command.")
                    .to_owned())
            });
        }
    }
}

/// Where a DevTools call is: waiting for CEF's UI thread, sent to the page,
/// or given up on by its caller before it was sent, so never to be sent.
const CALL_WAITING: u8 = 0;
const CALL_SENT: u8 = 1;
const CALL_ABANDONED: u8 = 2;

// Sends one DevTools message, which CEF accepts only on its UI thread.
wrap_task! {
    struct SendDevToolsMessage {
        shared: SharedState,
        id: i32,
        message: Vec<u8>,
        state: Arc<AtomicU8>,
        cancelled: Arc<AtomicBool>,
    }

    impl Task {
        fn execute(&self) {
            // A command stopped while this waited for the UI thread does no
            // more, even of what it had already asked for.
            if self.cancelled.load(Ordering::Acquire)
                && self
                    .state
                    .compare_exchange(
                        CALL_WAITING,
                        CALL_ABANDONED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
            {
                let calls = lock(&self.shared).devtools.clone();
                if let Some(answer) = calls.take(self.id) {
                    let _ = answer.send(Err("The command was stopped.".into()));
                }
                return;
            }
            // A caller that stopped waiting has reported the call as failed.
            if self
                .state
                .compare_exchange(CALL_WAITING, CALL_SENT, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }
            let (host, calls, observed) = {
                let shared = lock(&self.shared);
                (
                    shared.browser.as_ref().and_then(|browser| browser.0.host()),
                    shared.devtools.clone(),
                    shared.devtools_observer.is_some(),
                )
            };
            let Some(host) = host else {
                if let Some(answer) = calls.take(self.id) {
                    let _ = answer.send(Err("The page has closed.".into()));
                }
                return;
            };
            if !observed {
                let mut observer = DevToolsAnswers::new(calls.clone());
                let registration = host.add_dev_tools_message_observer(Some(&mut observer));
                let mut shared = lock(&self.shared);
                if shared.devtools_observer.is_none() && !shared.browser_gone {
                    shared.devtools_observer = registration.map(SharedRegistration);
                }
            }
            if host.send_dev_tools_message(Some(&self.message)) != 1
                && let Some(answer) = calls.take(self.id)
            {
                let _ = answer.send(Err("The page did not take the command.".into()));
            }
        }
    }
}

/// A DevTools protocol connection to one page, for agents driving it. Its
/// calls block, so make them off GPUI's thread and the browser thread.
#[derive(Clone)]
pub struct DevTools {
    shared: SharedState,
    /// Set once the command these calls are for is stopped.
    cancelled: Arc<AtomicBool>,
}

impl DevTools {
    /// The same page, for a command `cancelled` stops: calls still waiting
    /// for CEF's UI thread when it is set are never sent.
    pub fn stopped_by(self, cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled, ..self }
    }

    /// The same page, for calls nothing stops, such as letting go of a key.
    pub fn unstoppable(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            cancelled: Arc::default(),
        }
    }

    /// Calls `method` and waits up to `timeout` for its result.
    pub fn call(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let calls = {
            let shared = lock(&self.shared);
            if shared.browser.is_none() {
                return Err(if shared.browser_gone {
                    "The page has closed.".into()
                } else {
                    "The browser is still starting.".into()
                });
            }
            shared.devtools.clone()
        };
        // Ids only need to be unique among this page's waiting calls.
        let id = calls.next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let (answer, answered) = mpsc::channel();
        calls
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, answer);
        let message =
            serde_json::to_vec(&serde_json::json!({"id": id, "method": method, "params": params}))
                .map_err(|error| error.to_string())?;
        let state = Arc::new(AtomicU8::new(CALL_WAITING));
        let mut task = SendDevToolsMessage::new(
            self.shared.clone(),
            id,
            message,
            state.clone(),
            self.cancelled.clone(),
        );
        if post_task(ThreadId::UI, Some(&mut task)) != 1 {
            calls.take(id);
            return Err("The browser has shut down.".into());
        }
        // Waited for in short spells, so a stopped command lets the page go.
        let started = std::time::Instant::now();
        let result = loop {
            let left = timeout.saturating_sub(started.elapsed());
            match answered.recv_timeout(left.min(Duration::from_millis(100))) {
                Err(mpsc::RecvTimeoutError::Timeout)
                    if !left.is_zero() && !self.cancelled.load(Ordering::Acquire) => {}
                result => break result,
            }
        };
        calls.take(id);
        match result {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) if self.cancelled.load(Ordering::Acquire) => {
                let abandoned = state
                    .compare_exchange(
                        CALL_WAITING,
                        CALL_ABANDONED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok();
                Err(if abandoned {
                    "The command was stopped.".into()
                } else {
                    format!("The command was stopped; the page may still finish {method}.")
                })
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let abandoned = state
                    .compare_exchange(
                        CALL_WAITING,
                        CALL_ABANDONED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok();
                Err(if abandoned {
                    format!("The browser was too busy to run {method}, so it did not.")
                } else {
                    format!(
                        "The page took {method} but did not answer within {} seconds, so it \
                         may have happened.",
                        timeout.as_secs()
                    )
                })
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("The page has closed.".into()),
        }
    }

    /// How many tabs this page has asked for.
    pub fn popups(&self) -> u64 {
        lock(&self.shared).popups
    }

    /// What an agent command holds while it runs on this page.
    pub fn turns(&self) -> Arc<Mutex<()>> {
        lock(&self.shared).turns.clone()
    }

    /// Whether the page is loading, from the moment a navigation starts,
    /// before any of the new document arrives.
    pub fn is_loading(&self) -> bool {
        lock(&self.shared)
            .browser
            .as_ref()
            .is_some_and(|browser| browser.0.is_loading() != 0)
    }

    /// Waits for CEF to create the page, which a new tab asks for and gets
    /// later.
    pub fn wait_until_started(&self, timeout: Duration) -> Result<(), String> {
        let started = std::time::Instant::now();
        loop {
            {
                let shared = lock(&self.shared);
                if shared.browser.is_some() {
                    return Ok(());
                }
                if shared.browser_gone || shared.closed.is_some() {
                    return Err("The page has closed.".into());
                }
            }
            if self.cancelled.load(Ordering::Acquire) {
                return Err("The command was stopped.".into());
            }
            if started.elapsed() >= timeout {
                return Err("The browser did not start in time.".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Keeps the page painting while it is off screen, until [`Self::sleep`].
    /// A page never shown gets a size to lay itself out in.
    pub fn wake(&self) {
        let mut shared = lock(&self.shared);
        shared.awake += 1;
        if shared.bounds.is_none() {
            shared.bounds = Some((0, 0, UNSHOWN_WIDTH, UNSHOWN_HEIGHT));
        }
        apply_window_state(&shared);
    }

    pub fn sleep(&self) {
        let mut shared = lock(&self.shared);
        shared.awake = shared.awake.saturating_sub(1);
        shared.agent_finished = Some(std::time::Instant::now());
        apply_window_state(&shared);
    }
}

/// The size, in physical pixels, of a page an agent uses before the user has
/// ever shown it.
const UNSHOWN_WIDTH: u32 = 1280;
const UNSHOWN_HEIGHT: u32 = 800;

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

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(ViewDownloads::new())
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
        // Counted before asking, so a shutdown can never fall between CEF
        // accepting the browser and creating it.
        LIVE_BROWSERS.fetch_add(1, Ordering::SeqCst);
        let created = browser_host_create_browser(
            Some(&info),
            Some(&mut client),
            Some(&CefString::from("about:blank")),
            Some(&BrowserSettings::default()),
            None,
            None,
        );
        if created != 1 {
            LIVE_BROWSERS.fetch_sub(1, Ordering::SeqCst);
            release_windows(&mut lock(&shared));
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

    /// A DevTools connection to this page, for an agent's commands.
    pub fn devtools(&self) -> DevTools {
        DevTools {
            shared: self.shared.clone(),
            cancelled: Arc::default(),
        }
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
        // A page can close itself, with `window.close()`; then nothing is left
        // to wait for.
        if shared.browser_gone {
            release_windows(&mut shared);
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_download_directory_from_user_dirs() {
        let home = std::path::Path::new("/home/user");
        let dirs = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n";
        assert_eq!(
            xdg_download_dir(dirs, home),
            Some(PathBuf::from("/home/user/Téléchargements"))
        );
        assert_eq!(
            xdg_download_dir("XDG_DOWNLOAD_DIR=\"/data/dl\"", home),
            Some(PathBuf::from("/data/dl"))
        );
        // `$HOME` alone disables it, and a relative path is not allowed.
        assert_eq!(xdg_download_dir("XDG_DOWNLOAD_DIR=\"$HOME/\"", home), None);
        assert_eq!(xdg_download_dir("XDG_DOWNLOAD_DIR=\"dl\"", home), None);
        assert_eq!(
            xdg_download_dir("XDG_MUSIC_DIR=\"$HOME/Music\"", home),
            None
        );
    }

    #[test]
    fn a_download_never_replaces_a_file_or_leaves_its_directory() {
        let directory = std::env::temp_dir().join(format!("xd-downloads-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        assert_eq!(
            unused_path(&directory, "report.csv", |path| path.exists()),
            directory.join("report.csv")
        );
        std::fs::write(directory.join("report.csv"), "").unwrap();
        std::fs::write(directory.join("report (1).csv"), "").unwrap();
        assert_eq!(
            unused_path(&directory, "report.csv", |path| path.exists()),
            directory.join("report (2).csv")
        );
        assert_eq!(
            unused_path(&directory, "../../etc/passwd", |path| path.exists()),
            directory.join("passwd")
        );
        assert_eq!(
            unused_path(&directory, "", |path| path.exists()),
            directory.join("download")
        );
        assert_eq!(
            unused_path(&directory, ".bashrc", |path| path.exists()),
            directory.join(".bashrc")
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_reserved_download_name_is_not_handed_out_again() {
        let directory = std::path::Path::new("/nonexistent-xd-downloads");
        let reserved = [directory.join("a.zip"), directory.join("a (1).zip")];
        assert_eq!(
            unused_path(directory, "a.zip", |path| reserved
                .iter()
                .any(|taken| taken == path)),
            directory.join("a (2).zip")
        );
    }

    #[test]
    fn only_a_listening_holder_holds_the_profile() {
        let directory = std::env::temp_dir().join(format!("xd-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("socket");
        let link = |name: &str, target: &Path| {
            let path = directory.join(name);
            let _ = std::fs::remove_file(&path);
            std::os::unix::fs::symlink(target, path).unwrap();
        };
        assert_eq!(profile_holder(&directory), None, "no lock");

        link("SingletonLock", Path::new("some-host-1"));
        link("SingletonSocket", &socket);
        assert_eq!(profile_holder(&directory), None, "nothing listening");

        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert_eq!(profile_holder(&directory), Some(1));
        let own = format!("some-host-{}", std::process::id());
        link("SingletonLock", Path::new(&own));
        assert_eq!(profile_holder(&directory), None, "this process's own lock");

        link("SingletonLock", Path::new("some-host-1"));
        drop(listener);
        assert!(socket.exists());
        assert_eq!(
            profile_holder(&directory),
            None,
            "left by a holder that exited"
        );
        std::fs::remove_file(&socket).unwrap();
        assert_eq!(profile_holder(&directory), None, "socket removed");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_holder_with_a_full_queue_still_holds_the_profile() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::os::unix::ffi::OsStrExt;

        let path = std::env::temp_dir().join(format!("xd-queue-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        // Connect without accepting until the queue refuses more.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (slot, byte) in address.sun_path.iter_mut().zip(path.as_os_str().as_bytes()) {
            *slot = *byte as libc::c_char;
        }
        let mut waiting = Vec::new();
        let full = (0..100_000).any(|_| {
            let fd =
                unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0) };
            assert!(fd >= 0);
            let socket = unsafe { OwnedFd::from_raw_fd(fd) };
            let result = unsafe {
                libc::connect(
                    socket.as_raw_fd(),
                    std::ptr::from_ref(&address).cast(),
                    std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
                )
            };
            let refused =
                result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EAGAIN);
            waiting.push(socket);
            refused
        });
        assert!(full, "the queue never filled");
        let started = std::time::Instant::now();
        assert!(listening(&path));
        assert!(started.elapsed() < Duration::from_secs(1));
        std::fs::remove_file(&path).unwrap();
    }
}
