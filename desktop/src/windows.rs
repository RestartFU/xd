//! What makes xd a Windows application rather than a program that happens to
//! run there: where it keeps its files, one window per user, a taskbar
//! identity, something to show for a crash, and a clean exit when Windows or an
//! installer asks for one.

use std::path::{Path, PathBuf};

/// Set the per-user directories before GPUI or host threads start. Existing
/// overrides remain available for custom installations and CI fixtures.
///
/// Windows is asked where the signed-in user's folders are. The environment
/// usually says the same, but it is whatever the starting program passed on:
/// the installer opens xd from a system service, whose own profile is not the
/// user's.
#[cfg(target_os = "windows")]
pub fn configure_environment() {
    use std::env;

    use ::windows::Win32::UI::Shell::{
        FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_RoamingAppData,
    };

    let folder = |known, variable: &str| {
        native::known_folder(known).or_else(|| {
            env::var_os(variable)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
    };
    let home = folder(&FOLDERID_Profile, "USERPROFILE");
    if let Some(home) = home {
        let roaming = folder(&FOLDERID_RoamingAppData, "APPDATA");
        let local = folder(&FOLDERID_LocalAppData, "LOCALAPPDATA");
        for (key, path) in profile_paths(&home, roaming.as_deref(), local.as_deref()) {
            set_default(key, path);
        }
        set_default("HOME", home);
    }
    // GPUI normally draws through DirectComposition, in a layer above every
    // child window of its own. The browser pane is such a child window, so its
    // pages loaded, titled their tabs and stayed invisible under the app. Drawn
    // straight to the window instead, the app is under its children as Windows
    // means it to be. The window is opaque, so nothing else is lost.
    set_default("GPUI_DISABLE_DIRECT_COMPOSITION", "1");
    set_default(
        "XD_UPDATE_CHANNEL",
        if xd_desktop::channel::nightly() {
            "nightly"
        } else {
            "release"
        },
    );
}

#[cfg(target_os = "windows")]
fn set_default(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    if std::env::var_os(key).is_none_or(|value| value.is_empty()) {
        // Called only by main, before any threads exist.
        unsafe { std::env::set_var(key, value) };
    }
}

fn profile_paths(
    home: &Path,
    roaming: Option<&Path>,
    local: Option<&Path>,
) -> [(&'static str, PathBuf); 3] {
    let roaming = roaming
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join("AppData/Roaming"));
    let local = local
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join("AppData/Local"));
    [
        ("XDG_CONFIG_HOME", roaming),
        ("XDG_DATA_HOME", local.clone()),
        ("XDG_CACHE_HOME", local.join("Cache")),
    ]
}

/// Where this build keeps what it must find again after a crash: the marker
/// of a run in progress and the crash log. Beside the browser profile, under
/// `%LOCALAPPDATA%`, because none of it should roam between machines.
#[cfg(target_os = "windows")]
fn state_directory() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(xd_desktop::channel::data_name())
}

/// The name Windows knows this build's one running instance by. Stable and
/// nightly keep separate data, so each may run beside the other.
fn instance_name(data_name: &str) -> String {
    let name = data_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    // `Local\` keeps the name to this sign-in session; a kernel object's name
    // may hold no other backslash.
    format!(r"Local\xd-desktop-{name}")
}

/// What the crash dialog says. It runs in a process of its own, so it knows
/// only the path it was started with.
fn crash_dialog_text(report: &Path) -> String {
    format!(
        "xd ran into a problem and had to close.\n\nA report was saved to:\n{}\n\nOpen xd again?",
        report.display()
    )
}

/// The toast Windows shows for `text`: one line, under the app's own name
/// and icon, which Windows takes from the Start menu shortcut.
fn toast_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            // XML has no way to write most control characters at all.
            character if character.is_control() => escaped.push(' '),
            character => escaped.push(character),
        }
    }
    format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{escaped}</text></binding></visual></toast>"#
    )
}

#[cfg(target_os = "windows")]
pub use native::*;

#[cfg(target_os = "windows")]
mod native {
    use std::{
        ffi::OsString,
        os::windows::process::CommandExt,
        path::{Path, PathBuf},
        process::Command,
        sync::atomic::{AtomicBool, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use windows::{
        Win32::{
            Foundation::{
                CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT,
                POINT, RECT, WPARAM,
            },
            Graphics::Gdi::{
                GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
            },
            System::{
                Com::CoTaskMemFree,
                Console::{ATTACH_PARENT_PROCESS, AttachConsole},
                Recovery::{REGISTER_APPLICATION_RESTART_FLAGS, RegisterApplicationRestart},
                Threading::{
                    CreateMutexW, GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32,
                    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
                },
            },
            UI::{
                Shell::{
                    DefSubclassProc, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
                    SetCurrentProcessExplicitAppUserModelID, SetWindowSubclass,
                },
                WindowsAndMessaging::{
                    ENDSESSION_CLOSEAPP, EnumWindows, GetWindowThreadProcessId, IDYES, IsIconic,
                    IsWindowVisible, MB_ICONERROR, MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
                    PostMessageW, PostQuitMessage, SW_RESTORE, SetForegroundWindow, ShowWindow,
                    WM_CLOSE, WM_ENDSESSION, WM_QUERYENDSESSION,
                },
            },
        },
        core::{BOOL, HSTRING, PCWSTR, PWSTR},
    };

    use super::{crash_dialog_text, instance_name, state_directory, toast_xml};
    use crate::recovery;

    /// The flag a crashing xd starts a second xd with, to say so.
    const CRASH_REPORT_FLAG: &str = "--crash-report";

    /// `CREATE_NO_WINDOW`: a helper started from here needs no console.
    const NO_CONSOLE: u32 = 0x0800_0000;

    fn wide(text: &str) -> HSTRING {
        HSTRING::from(text)
    }

    /// Let `xd --version` answer in the terminal it was typed into. A program
    /// built for the Windows desktop starts with no console of its own; one
    /// whose output is already redirected keeps that.
    pub fn attach_parent_console() {
        // Failing means there is no parent console, which is the usual case.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }

    /// One of the signed-in user's folders, as Windows itself resolves it.
    pub(super) fn known_folder(folder: &windows::core::GUID) -> Option<PathBuf> {
        let path = unsafe { SHGetKnownFolderPath(folder, KF_FLAG_DEFAULT, None) }.ok()?;
        // SAFETY: on success Windows returns a NUL-terminated string that this
        // caller owns and frees.
        let resolved = unsafe { path.to_string() }.ok().map(PathBuf::from);
        unsafe { CoTaskMemFree(Some(path.0 as _)) };
        resolved.filter(|path| !path.as_os_str().is_empty())
    }

    /// The primary monitor and the part of it the taskbar leaves free, each
    /// as left, top, right and bottom in physical pixels.
    pub fn primary_monitor() -> Option<([i32; 4], [i32; 4])> {
        let monitor = unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetMonitorInfoW(monitor, &mut info) }
            .as_bool()
            .then(|| {
                let edges = |area: RECT| [area.left, area.top, area.right, area.bottom];
                (edges(info.rcMonitor), edges(info.rcWork))
            })
    }

    /// wry takes a quit off the message queue while it waits for WebView2.
    pub fn repost_quit() {
        unsafe { PostQuitMessage(0) };
    }

    /// Group the window with its Start menu shortcut, which the installer
    /// gives the same identity, so pinning xd pins the app and not one window.
    pub fn set_app_identity() {
        let identity = wide(&xd_desktop::channel::app_id());
        // Without it the taskbar falls back to the executable's path.
        let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(&identity) };
        // After an update or a reboot that closed xd, Windows may start it
        // again; not after a crash or a hang, which the crash dialog covers.
        const NO_CRASH: u32 = 1;
        const NO_HANG: u32 = 2;
        let _ = unsafe {
            RegisterApplicationRestart(
                PCWSTR::null(),
                REGISTER_APPLICATION_RESTART_FLAGS(NO_CRASH | NO_HANG),
            )
        };
    }

    /// Show a Windows notification. Clicking it opens xd, which brings the
    /// running window forward.
    pub fn notify(text: String) {
        use windows::{
            Data::Xml::Dom::XmlDocument,
            UI::Notifications::{ToastNotification, ToastNotificationManager},
        };

        // Off the UI thread: the first notification starts a system service.
        let _ = std::thread::Builder::new()
            .name("xd-notification".into())
            .spawn(move || {
                let show = || -> windows::core::Result<()> {
                    let document = XmlDocument::new()?;
                    document.LoadXml(&wide(&toast_xml(&text)))?;
                    let toast = ToastNotification::CreateToastNotification(&document)?;
                    // The identity the window and its shortcut share.
                    let identity = wide(&xd_desktop::channel::app_id());
                    ToastNotificationManager::CreateToastNotifierWithId(&identity)?.Show(&toast)
                };
                // A notification that cannot be shown is not worth an error.
                let _ = show();
            });
    }

    /// Held for as long as this is the running xd.
    pub struct SingleInstance(HANDLE);

    impl Drop for SingleInstance {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    impl SingleInstance {
        /// Claim to be the running xd. `None` when another already is: its
        /// window has been brought forward, and this process should exit.
        ///
        /// Two would share one settings file and one browser profile, and a
        /// second click on the Start menu is asking for the window, not for
        /// another copy.
        pub fn acquire() -> Option<Self> {
            let name = wide(&instance_name(
                &xd_desktop::channel::data_name().to_string_lossy(),
            ));
            let handle = unsafe { CreateMutexW(None, false, &name) };
            let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            match handle {
                Ok(handle) if exists => {
                    let _ = unsafe { CloseHandle(handle) };
                    show_running_instance();
                    None
                }
                Ok(handle) => Some(Self(handle)),
                // Windows could not say. Starting is better than refusing to.
                Err(_) => Some(Self(HANDLE::default())),
            }
        }
    }

    fn executable_of(process: u32) -> Option<PathBuf> {
        use std::os::windows::ffi::OsStringExt;

        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process) }.ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let named = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        };
        let _ = unsafe { CloseHandle(handle) };
        named.ok()?;
        Some(PathBuf::from(OsString::from_wide(
            &buffer[..length as usize],
        )))
    }

    /// Bring the window of the xd that is already running to the front.
    fn show_running_instance() {
        struct Search {
            executable: PathBuf,
            own_process: u32,
            found: Option<HWND>,
        }

        unsafe extern "system" fn visit(window: HWND, search: LPARAM) -> BOOL {
            // SAFETY: `search` is the `Search` below, alive for the whole enumeration.
            let search = unsafe { &mut *(search.0 as *mut Search) };
            let mut process = 0u32;
            unsafe { GetWindowThreadProcessId(window, Some(&mut process)) };
            if process != 0
                && process != search.own_process
                && unsafe { IsWindowVisible(window) }.as_bool()
                && executable_of(process).is_some_and(|path| {
                    // Windows file names ignore case.
                    path.as_os_str()
                        .eq_ignore_ascii_case(search.executable.as_os_str())
                })
            {
                search.found = Some(window);
                return false.into();
            }
            true.into()
        }

        let Ok(executable) = std::env::current_exe() else {
            return;
        };
        let mut search = Search {
            executable,
            own_process: unsafe { GetCurrentProcessId() },
            found: None,
        };
        // Stopping the enumeration early reports as an error.
        let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
        if let Some(window) = search.found {
            unsafe {
                if IsIconic(window).as_bool() {
                    let _ = ShowWindow(window, SW_RESTORE);
                }
                let _ = SetForegroundWindow(window);
            }
        }
    }

    /// Run as the crash dialog if that is what this process was started for.
    /// Returns whether it was, in which case there is nothing else to do.
    pub fn run_crash_dialog(arguments: &[OsString]) -> bool {
        let [flag, report] = arguments else {
            return false;
        };
        if flag != CRASH_REPORT_FLAG {
            return false;
        }
        let text = wide(&crash_dialog_text(Path::new(report)));
        let answer = unsafe {
            MessageBoxW(
                None,
                &text,
                &wide("xd"),
                MB_YESNO | MB_ICONERROR | MB_SETFOREGROUND,
            )
        };
        if answer == IDYES
            && let Ok(executable) = std::env::current_exe()
        {
            let _ = Command::new(executable).spawn();
        }
        true
    }

    /// Write down every panic, and say so when one is about to end the app.
    ///
    /// A release build has no console, and a panic on the UI thread happens
    /// inside a window procedure, where it ends the process at once: the
    /// window used to vanish with nothing to show what happened or where.
    pub fn install_crash_handler(version: String) {
        static DIALOG_SHOWN: AtomicBool = AtomicBool::new(false);
        let ui_thread = std::thread::current().id();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            let message = panic
                .payload()
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
                .or_else(|| panic.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic without a message".into());
            let location = panic.location().map(ToString::to_string);
            let thread = std::thread::current();
            let backtrace = std::backtrace::Backtrace::force_capture().to_string();
            let report = recovery::CrashReport {
                version: &version,
                thread: thread.name().unwrap_or("unnamed"),
                message: &message,
                location: location.as_deref(),
                backtrace: &backtrace,
                time: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_secs()),
            }
            .append(&state_directory());
            previous(panic);
            // Only a panic on the UI thread takes the app down with it.
            if thread.id() != ui_thread || DIALOG_SHOWN.swap(true, Ordering::AcqRel) {
                return;
            }
            // A dialog shown from here would run the message loop of an app
            // that is half way through falling over. A fresh process is safe.
            if let (Ok(report), Ok(executable)) = (report, std::env::current_exe()) {
                let _ = Command::new(executable)
                    .arg(CRASH_REPORT_FLAG)
                    .arg(report)
                    .creation_flags(NO_CONSOLE)
                    .spawn();
            }
        }));
    }

    /// Mark the run as started, and report the last one if it never finished.
    pub fn begin_run() -> Option<recovery::FailedRun> {
        let directory = state_directory();
        let log = recovery::crash_log(&directory);
        let reported = log.metadata().and_then(|metadata| metadata.modified()).ok();
        if !recovery::begin(&directory) {
            return None;
        }
        // Point at the log only if it could be about the run that failed.
        let recent = reported.is_some_and(|modified| {
            modified
                .elapsed()
                .is_ok_and(|age| age < std::time::Duration::from_secs(7 * 24 * 60 * 60))
        });
        Some(recovery::FailedRun {
            crash_log: recent.then_some(log),
        })
    }

    /// Leave when Windows says the session is ending or an installer needs
    /// the files, and leave no sign of a crash behind when it does.
    ///
    /// GPUI answers neither message, so an update used to find xd unwilling to
    /// close and ask for a reboot instead.
    pub fn close_with_the_session(window: isize) {
        unsafe extern "system" fn procedure(
            window: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
            _: usize,
            _: usize,
        ) -> LRESULT {
            match message {
                WM_QUERYENDSESSION => return LRESULT(1),
                WM_ENDSESSION if wparam.0 != 0 => {
                    // Windows ends the process after this returns, with no
                    // other chance to say the run ended as it should.
                    recovery::finish();
                    if lparam.0 as u32 & ENDSESSION_CLOSEAPP != 0 {
                        // Only this app is being closed: do it the usual way,
                        // so drafts and settings are saved first.
                        let _ =
                            unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) };
                    }
                    return LRESULT(0);
                }
                _ => {}
            }
            unsafe { DefSubclassProc(window, message, wparam, lparam) }
        }

        let _ = unsafe { SetWindowSubclass(HWND(window as _), Some(procedure), 1, 0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_channel_is_its_own_instance_and_the_name_is_one_windows_accepts() {
        assert_eq!(instance_name("xd"), r"Local\xd-desktop-xd");
        assert_eq!(instance_name("xd-nightly"), r"Local\xd-desktop-xd-nightly");
        // A data name is a directory name; whatever else it holds, the kernel
        // object's name keeps exactly one backslash.
        assert_eq!(
            instance_name(r"team\xd dev"),
            r"Local\xd-desktop-team_xd_dev"
        );
    }

    #[test]
    fn a_notification_shows_a_chat_title_as_text_whatever_it_contains() {
        assert_eq!(
            toast_xml("Fix the login finished"),
            r#"<toast><visual><binding template="ToastGeneric"><text>Fix the login finished</text></binding></visual></toast>"#
        );
        // A title is whatever the user or a model wrote.
        let xml = toast_xml("</text><actions> \"a\" & 'b'\n\u{1} finished");
        assert!(xml.contains(
            "<text>&lt;/text&gt;&lt;actions&gt; &quot;a&quot; &amp; &apos;b&apos;   finished</text>"
        ));
    }

    #[test]
    fn the_crash_dialog_names_the_report_and_offers_to_reopen() {
        let text = crash_dialog_text(Path::new(r"C:\Users\me\AppData\Local\xd\crash.log"));
        assert!(text.starts_with("xd ran into a problem and had to close."));
        assert!(text.contains(r"C:\Users\me\AppData\Local\xd\crash.log"));
        assert!(text.ends_with("Open xd again?"));
    }

    #[test]
    fn portable_launch_uses_user_profile_directories_without_shell_initialization() {
        let home = Path::new("C:/Users/Example User");
        let fallback = profile_paths(home, None, None);
        assert_eq!(fallback[0].1, home.join("AppData/Roaming"));
        assert_eq!(fallback[1].1, home.join("AppData/Local"));
        let roaming = Path::new("D:/Roaming");
        let local = Path::new("D:/Local");
        let configured = profile_paths(home, Some(roaming), Some(local));
        assert_eq!(configured[0].1, roaming);
        assert_eq!(configured[1].1, local);
        assert_eq!(configured[2].1, local.join("Cache"));
    }
}
