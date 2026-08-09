use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use serde_json::{Value, json};
#[cfg(any(windows, test))]
use std::ffi::OsString;
#[cfg(any(windows, test))]
use uuid::Uuid;

use crate::{EventBus, private_fs::executable_file};

const NIGHTLY_RELEASE_URL: &str = "https://api.github.com/repos/RestartFU/xd/releases/tags/nightly";
const STABLE_RELEASE_URL: &str = "https://api.github.com/repos/RestartFU/xd/releases/latest";
const MAX_RELEASE_BYTES: u64 = 256 * 1024;
const INSTALL_OUTPUT_LIMIT: usize = 16 * 1024;

#[derive(Clone)]
pub(crate) struct SelfUpdate {
    inner: Arc<SelfUpdateInner>,
}

struct SelfUpdateInner {
    state: Mutex<UpdateState>,
    events: Arc<EventBus>,
    install: Option<InstallLocation>,
    channel: UpdateChannel,
    current: String,
    #[cfg(windows)]
    staged: Mutex<Option<StagedUpdate>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateChannel {
    Nightly,
    Release,
}

impl UpdateChannel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Nightly => "nightly",
            Self::Release => "release",
        }
    }

    fn feed_name(self) -> &'static str {
        match self {
            Self::Nightly => "nightly release feed",
            Self::Release => "release feed",
        }
    }

    fn release_url(self) -> &'static str {
        match self {
            Self::Nightly => NIGHTLY_RELEASE_URL,
            Self::Release => STABLE_RELEASE_URL,
        }
    }
}

#[derive(Clone)]
struct InstallLocation {
    installer: PathBuf,
    daemon: PathBuf,
    #[cfg(windows)]
    desktop: PathBuf,
    #[cfg(windows)]
    root: PathBuf,
}

#[cfg(windows)]
#[derive(Clone)]
struct StagedUpdate {
    directory: PathBuf,
    setup: PathBuf,
    checksum: PathBuf,
}

struct UpdateState {
    phase: &'static str,
    latest: Option<String>,
    error: Option<String>,
}

impl SelfUpdate {
    pub(crate) fn new(events: Arc<EventBus>) -> Self {
        let channel = update_channel();
        Self {
            inner: Arc::new(SelfUpdateInner {
                state: Mutex::new(UpdateState {
                    phase: "idle",
                    latest: None,
                    error: None,
                }),
                events,
                install: install_location(),
                channel,
                current: current_version(channel),
                #[cfg(windows)]
                staged: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn perform(&self, action: Option<&str>) -> Result<Value, String> {
        match action.unwrap_or("status") {
            "status" => {}
            "check" => self.check()?,
            "install" => self.install()?,
            "restart" => self.restart()?,
            _ => return Err("No such daemon-update action.".into()),
        }
        Ok(self.snapshot())
    }

    fn check(&self) -> Result<(), String> {
        if !self.begin("checking") {
            return Ok(());
        }
        let updater = self.clone();
        let channel = self.inner.channel;
        thread::Builder::new()
            .name("xd-update-check".into())
            .spawn(move || match latest_release(channel) {
                Ok(latest) => updater.finish("idle", Some(latest), None),
                Err(error) => updater.finish("failed", None, Some(error)),
            })
            .map(|_| ())
            .map_err(|error| {
                let message = format!("Cannot start the update check: {error}");
                self.finish("failed", None, Some(message.clone()));
                message
            })
    }

    fn install(&self) -> Result<(), String> {
        let location = self
            .inner
            .install
            .clone()
            .ok_or_else(|| "This daemon's installation cannot update itself.".to_owned())?;
        if !self.begin("installing") {
            return Ok(());
        }
        let updater = self.clone();
        let channel = self.inner.channel;
        thread::Builder::new()
            .name("xd-update-install".into())
            .spawn(move || match updater.install_update(&location, channel) {
                Ok(()) => updater.finish("installed", None, None),
                Err(error) => updater.finish("failed", None, Some(error)),
            })
            .map(|_| ())
            .map_err(|error| {
                let message = format!("Cannot start the installer: {error}");
                self.finish("failed", None, Some(message.clone()));
                message
            })
    }

    #[cfg(unix)]
    fn install_update(
        &self,
        location: &InstallLocation,
        channel: UpdateChannel,
    ) -> Result<(), String> {
        run_installer(&location.installer, channel)
    }

    #[cfg(windows)]
    fn install_update(
        &self,
        location: &InstallLocation,
        channel: UpdateChannel,
    ) -> Result<(), String> {
        let staged = stage_windows_update(&location.installer, channel)?;
        self.replace_staged(staged);
        Ok(())
    }

    #[cfg(windows)]
    fn replace_staged(&self, staged: StagedUpdate) {
        if let Ok(mut current) = self.inner.staged.lock() {
            if let Some(previous) = current.replace(staged) {
                let _ = fs::remove_dir_all(previous.directory);
            }
        }
    }

    fn restart(&self) -> Result<(), String> {
        let location = self
            .inner
            .install
            .clone()
            .ok_or_else(|| "This daemon's installation cannot restart itself.".to_owned())?;

        #[cfg(windows)]
        {
            let staged = self
                .inner
                .staged
                .lock()
                .ok()
                .and_then(|staged| staged.clone())
                .ok_or_else(|| "Install the update before restarting the daemon.".to_owned())?;
            spawn_windows_update_handoff(&location, &staged, self.inner.channel)?;
            if let Ok(mut current) = self.inner.staged.lock() {
                let _ = current.take();
            }
            self.publish();
            thread::spawn(|| {
                thread::sleep(Duration::from_millis(500));
                std::process::exit(0);
            });
            return Ok(());
        }

        #[cfg(unix)]
        {
            let arguments = env::args_os().skip(1).collect::<Vec<_>>();
            self.publish();
            thread::Builder::new()
                .name("xd-update-restart".into())
                .spawn(move || {
                    thread::sleep(Duration::from_millis(250));
                    use std::os::unix::process::CommandExt;
                    let error = Command::new(location.daemon).args(arguments).exec();
                    eprintln!("xd-daemon: cannot restart after update: {error}");
                })
                .map(|_| ())
                .map_err(|error| format!("Cannot schedule the daemon restart: {error}"))
        }
    }

    fn begin(&self, phase: &'static str) -> bool {
        let started = self.inner.state.lock().is_ok_and(|mut state| {
            if matches!(state.phase, "checking" | "installing") {
                return false;
            }
            state.phase = phase;
            state.error = None;
            true
        });
        if started {
            self.publish();
        }
        started
    }

    fn finish(&self, phase: &'static str, latest: Option<String>, error: Option<String>) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.phase = phase;
            if latest.is_some() {
                state.latest = latest;
            }
            state.error = error;
        }
        self.publish();
    }

    pub(crate) fn snapshot(&self) -> Value {
        let (phase, latest, error) = self
            .inner
            .state
            .lock()
            .map(|state| (state.phase, state.latest.clone(), state.error.clone()))
            .unwrap_or(("failed", None, Some("Update state is unavailable.".into())));
        snapshot_value(
            &self.inner.current,
            self.inner.channel,
            self.inner.install.is_some(),
            phase,
            latest.as_deref(),
            error.as_deref(),
        )
    }

    fn publish(&self) {
        let mut event = self.snapshot();
        event["event"] = Value::String("daemon-update".into());
        self.inner.events.publish(event);
    }
}

fn snapshot_value(
    current: &str,
    channel: UpdateChannel,
    supported: bool,
    state: &str,
    latest: Option<&str>,
    error: Option<&str>,
) -> Value {
    let mut value = json!({
        "ok": true,
        "version": current,
        "channel": channel.as_str(),
        "supported": supported,
        "state": state,
        "available": latest.is_some_and(|latest| latest != current),
    });
    if let Some(latest) = latest {
        value["latest"] = Value::String(latest.into());
    }
    if let Some(error) = error {
        value["error"] = Value::String(error.into());
    }
    value
}

fn update_channel() -> UpdateChannel {
    match env::var("XD_UPDATE_CHANNEL").as_deref() {
        Ok("release") => UpdateChannel::Release,
        _ => UpdateChannel::Nightly,
    }
}

fn current_version(channel: UpdateChannel) -> String {
    match channel {
        UpdateChannel::Nightly => option_env!("XD_COMMIT")
            .filter(|version| !version.is_empty())
            .unwrap_or(env!("CARGO_PKG_VERSION")),
        UpdateChannel::Release => env!("CARGO_PKG_VERSION"),
    }
    .to_owned()
}

fn latest_release(channel: UpdateChannel) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut response = agent
        .get(channel.release_url())
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "xd")
        .call()
        .map_err(|_| format!("Could not reach the {}.", channel.feed_name()))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_RELEASE_BYTES)
        .read_to_string()
        .map_err(|_| format!("The {} was unreadable.", channel.feed_name()))?;
    let release: Value = serde_json::from_str(&body)
        .map_err(|_| format!("The {} returned invalid data.", channel.feed_name()))?;
    release_identity(channel, &release)
}

fn release_identity(channel: UpdateChannel, release: &Value) -> Result<String, String> {
    let field = match channel {
        UpdateChannel::Nightly => "target_commitish",
        UpdateChannel::Release => "tag_name",
    };
    release
        .get(field)
        .and_then(Value::as_str)
        .filter(|target| !target.is_empty() && target.len() <= 128)
        .map(|identity| match channel {
            UpdateChannel::Nightly => identity.to_owned(),
            UpdateChannel::Release => identity.strip_prefix('v').unwrap_or(identity).to_owned(),
        })
        .ok_or_else(|| format!("The {} did not identify its build.", channel.feed_name()))
}

#[allow(dead_code)]
#[derive(Clone)]
enum InstallerMode {
    Install,
    #[cfg(any(windows, test))]
    Stage(PathBuf),
}

#[cfg(unix)]
fn run_installer(installer: &Path, channel: UpdateChannel) -> Result<(), String> {
    run_installer_process(
        installer_command(installer, channel, InstallerMode::Install),
        true,
    )
}

#[cfg(windows)]
fn stage_windows_update(installer: &Path, channel: UpdateChannel) -> Result<StagedUpdate, String> {
    let directory = env::temp_dir().join(format!("xd-update-{}", Uuid::new_v4()));
    if let Err(error) = fs::create_dir(&directory) {
        return Err(format!(
            "Cannot create the update staging directory: {error}"
        ));
    }
    let asset = if channel == UpdateChannel::Release {
        "xd-windows-x86_64-setup.exe"
    } else {
        "xd-nightly-windows-x86_64-setup.exe"
    };
    let setup = directory.join(asset);
    let checksum = directory.join(format!("{asset}.sha256"));
    let result = run_installer_process(
        installer_command(installer, channel, InstallerMode::Stage(directory.clone())),
        false,
    );
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&directory);
        return Err(error);
    }
    if !regular_file(&setup) || !regular_file(&checksum) {
        let _ = fs::remove_dir_all(&directory);
        return Err("The Windows installer did not produce a complete staged update.".into());
    }
    Ok(StagedUpdate {
        directory,
        setup,
        checksum,
    })
}

fn run_installer_process(mut command: Command, allow_running_install: bool) -> Result<(), String> {
    if allow_running_install {
        command.env("XD_ALLOW_RUNNING_INSTALL", "1");
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot run the xd installer: {error}"))?;
    let stderr = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let Ok(count) = stderr.read(&mut buffer) else {
                    break;
                };
                if count == 0 {
                    break;
                }
                let remaining = INSTALL_OUTPUT_LIMIT.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..count.min(remaining)]);
            }
            kept
        })
    });
    let status = child
        .wait()
        .map_err(|error| format!("Cannot wait for the xd installer: {error}"))?;
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&stderr);
    let detail = detail.trim();
    Err(if detail.is_empty() {
        "The xd installer failed.".into()
    } else {
        detail.into()
    })
}

#[cfg(windows)]
fn regular_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

#[cfg(unix)]
fn installer_command(installer: &Path, channel: UpdateChannel, _mode: InstallerMode) -> Command {
    let mut command = Command::new("sh");
    command.arg(installer);
    if channel == UpdateChannel::Release {
        command.arg("--release");
    }
    command
}

#[cfg(windows)]
fn installer_command(installer: &Path, channel: UpdateChannel, mode: InstallerMode) -> Command {
    use std::os::windows::process::CommandExt;

    let mut command = Command::new(windows_powershell_path());
    command.args(windows_installer_arguments(installer, channel, mode));
    command.creation_flags(0x0800_0000);
    command
}

#[cfg(any(windows, test))]
fn windows_powershell_path_from(system_root: Option<OsString>) -> PathBuf {
    system_root
        .map(PathBuf::from)
        .map(|root| root.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

#[cfg(windows)]
fn windows_powershell_path() -> PathBuf {
    windows_powershell_path_from(env::var_os("SystemRoot"))
}

#[cfg(any(windows, test))]
fn windows_installer_arguments(
    installer: &Path,
    channel: UpdateChannel,
    mode: InstallerMode,
) -> Vec<OsString> {
    let mut arguments = [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    arguments.push(installer.as_os_str().to_owned());
    match channel {
        UpdateChannel::Release => arguments.push("-Release".into()),
        UpdateChannel::Nightly => {}
    }
    match mode {
        InstallerMode::Install => {
            arguments.push("-Quiet".into());
            arguments.push("-InApp".into());
        }
        InstallerMode::Stage(directory) => {
            arguments.push("-StageDirectory".into());
            arguments.push(directory.into_os_string());
            arguments.push("-StageOnly".into());
        }
    }
    arguments
}

/// Everything the handoff names on the installer's command line.
///
/// Borrowed rather than taken from `InstallLocation` and `StagedUpdate`
/// directly, because those carry fields that only exist on Windows and this
/// has to stay checkable everywhere the tests run.
#[cfg(any(windows, test))]
struct WindowsHandoff<'a> {
    installer: &'a Path,
    setup: &'a Path,
    checksum: &'a Path,
    cleanup: &'a Path,
    root: &'a Path,
    desktop: &'a Path,
}

/// What the handoff tells the installer to do.
///
/// The channel goes with it. The installer defaults to nightly when nobody
/// says otherwise, and it is the same script that decides which product it is
/// installing over, which running processes belong to it, and what to call
/// what it installed. The staged package is the right one either way, so what
/// this prevents is an installer whose idea of the product disagrees with the
/// package it was handed.
#[cfg(any(windows, test))]
fn windows_handoff_arguments(
    handoff: &WindowsHandoff<'_>,
    channel: UpdateChannel,
) -> Vec<OsString> {
    let mut arguments = [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    arguments.push(handoff.installer.as_os_str().to_owned());
    if channel == UpdateChannel::Release {
        arguments.push("-Release".into());
    }
    for (flag, value) in [
        ("-SetupPath", handoff.setup),
        ("-SetupChecksumPath", handoff.checksum),
        ("-CleanupDirectory", handoff.cleanup),
        ("-InstallRoot", handoff.root),
    ] {
        arguments.push(flag.into());
        arguments.push(value.as_os_str().to_owned());
    }
    arguments.push("-Quiet".into());
    arguments.push("-InApp".into());
    arguments.push("-WaitForInstalledExit".into());
    arguments.push("-RelaunchPath".into());
    arguments.push(handoff.desktop.as_os_str().to_owned());
    arguments
}

#[cfg(windows)]
fn spawn_windows_update_handoff(
    location: &InstallLocation,
    staged: &StagedUpdate,
    channel: UpdateChannel,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let handoff = WindowsHandoff {
        installer: &location.installer,
        setup: &staged.setup,
        checksum: &staged.checksum,
        cleanup: &staged.directory,
        root: &location.root,
        desktop: &location.desktop,
    };
    let mut command = Command::new(windows_powershell_path());
    command
        .args(windows_handoff_arguments(&handoff, channel))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Cannot start the Windows update handoff: {error}"))
}

fn install_location() -> Option<InstallLocation> {
    let executable = fs::canonicalize(env::current_exe().ok()?).ok()?;
    #[cfg(windows)]
    let location = {
        let program_files = env::var_os("ProgramW6432")
            .or_else(|| env::var_os("ProgramFiles"))
            .map(PathBuf::from)?;
        windows_install_location_for_executable(&executable, &program_files)?
    };
    #[cfg(not(windows))]
    let location = {
        let home = PathBuf::from(env::var_os("HOME")?);
        install_location_for_executable(&executable, &home)?
    };
    #[cfg(windows)]
    let desktop = location.desktop;
    #[cfg(windows)]
    let root = location.root;
    let installer = location.installer;
    let daemon = location.daemon;
    if !executable_file(&installer) || !executable_file(&daemon) {
        return None;
    }
    #[cfg(windows)]
    if !executable_file(&desktop) {
        return None;
    }
    Some(InstallLocation {
        installer,
        daemon,
        #[cfg(windows)]
        desktop,
        #[cfg(windows)]
        root,
    })
}

#[cfg(not(windows))]
fn install_location_for_executable(executable: &Path, home: &Path) -> Option<InstallLocation> {
    if executable.file_name()?.to_str()? != "xd-daemon" {
        return None;
    }
    let libexec = executable.parent()?;
    if libexec.file_name()?.to_str()? != "libexec" {
        return None;
    }
    let parent = libexec.parent()?;
    let linux_parent = home.join(".local/opt");
    let linux_layout = matches!(parent.file_name()?.to_str()?, "xd" | "xd-nightly")
        && parent.parent() == Some(linux_parent.as_path());
    let applications = home.join("Applications");
    let macos_layout = parent.file_name()?.to_str()? == "Resources"
        && parent.parent()?.file_name()?.to_str()? == "Contents"
        && matches!(
            parent.parent()?.parent()?.file_name()?.to_str()?,
            "xd.app" | "xd-nightly.app"
        )
        && parent.parent()?.parent()?.parent() == Some(applications.as_path());
    let recognized = linux_layout || macos_layout;
    if !recognized {
        return None;
    }
    Some(InstallLocation {
        installer: libexec.join("install.sh"),
        daemon: libexec.join("xd-daemon"),
    })
}

#[cfg(any(windows, test))]
fn windows_install_location_for_executable(
    executable: &Path,
    program_files: &Path,
) -> Option<InstallLocation> {
    if !path_name_is(executable, "xd-daemon.exe") {
        return None;
    }
    let bin = executable.parent()?;
    if !path_name_is(bin, "bin") {
        return None;
    }
    let product = bin.parent()?;
    if !matches_path_name(product, &["xd", "xd-nightly"]) {
        return None;
    }
    let manufacturer = product.parent()?;
    if !path_name_is(manufacturer, "RestartFU")
        || !windows_path_eq(manufacturer.parent()?, program_files)
    {
        return None;
    }
    Some(InstallLocation {
        installer: bin.join("install.ps1"),
        daemon: executable.to_owned(),
        #[cfg(windows)]
        desktop: bin.join("xd.exe"),
        #[cfg(windows)]
        root: product.to_owned(),
    })
}

#[cfg(any(windows, test))]
fn path_name_is(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

#[cfg(any(windows, test))]
fn matches_path_name(path: &Path, expected: &[&str]) -> bool {
    expected.iter().any(|name| path_name_is(path, name))
}

#[cfg(any(windows, test))]
fn windows_path_eq(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        let mut value = path.to_string_lossy().replace('/', "\\");
        if let Some(stripped) = value.strip_prefix(r"\\?\") {
            value = stripped.to_owned();
        }
        while value.ends_with('\\') {
            value.pop();
        }
        value.to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_snapshots_compare_exact_build_identity() {
        assert_eq!(
            snapshot_value(
                "abc",
                UpdateChannel::Nightly,
                true,
                "idle",
                Some("abc"),
                None
            )["available"],
            false
        );
        let available = snapshot_value(
            "abc",
            UpdateChannel::Nightly,
            true,
            "idle",
            Some("def"),
            None,
        );
        assert_eq!(available["available"], true);
        assert_eq!(available["channel"], "nightly");
        assert_eq!(available["latest"], "def");
    }

    #[test]
    fn stable_releases_compare_semantic_versions() {
        let release = json!({"tag_name": "v1.2.3", "target_commitish": "master"});
        assert_eq!(
            release_identity(UpdateChannel::Release, &release).unwrap(),
            "1.2.3"
        );
        assert_eq!(
            release_identity(UpdateChannel::Nightly, &release).unwrap(),
            "master"
        );
        let snapshot = snapshot_value(
            "1.2.3",
            UpdateChannel::Release,
            true,
            "idle",
            Some("1.2.3"),
            None,
        );
        assert_eq!(snapshot["channel"], "release");
        assert_eq!(snapshot["available"], false);
    }

    #[test]
    fn stable_release_feed_errors_do_not_repeat_release() {
        assert_eq!(
            release_identity(UpdateChannel::Release, &json!({})).unwrap_err(),
            "The release feed did not identify its build."
        );
    }

    #[test]
    fn source_builds_cannot_replace_an_installation() {
        let updater = SelfUpdate::new(Arc::new(EventBus::default()));
        assert_eq!(updater.snapshot()["supported"], false);
        assert!(updater.perform(Some("install")).is_err());
        assert!(updater.perform(Some("unknown")).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn recognizes_linux_and_macos_release_layouts_only() {
        for executable in [
            "/home/person/.local/opt/xd/libexec/xd-daemon",
            "/home/person/.local/opt/xd-nightly/libexec/xd-daemon",
            "/Users/person/Applications/xd.app/Contents/Resources/libexec/xd-daemon",
            "/Users/person/Applications/xd-nightly.app/Contents/Resources/libexec/xd-daemon",
        ] {
            let home = if executable.starts_with("/Users/") {
                Path::new("/Users/person")
            } else {
                Path::new("/home/person")
            };
            let location = install_location_for_executable(Path::new(executable), home).unwrap();
            assert_eq!(location.daemon, Path::new(executable));
            assert_eq!(
                location.installer,
                Path::new(executable).parent().unwrap().join("install.sh")
            );
        }

        for executable in [
            "/tmp/xd-daemon",
            "/tmp/xd/libexec/not-the-daemon",
            "/tmp/source/xd/libexec/xd-daemon",
            "/Users/person/Applications/other.app/Contents/Resources/libexec/xd-daemon",
            "/Users/person/Applications/xd.app/Contents/MacOS/xd-daemon",
        ] {
            assert!(
                install_location_for_executable(Path::new(executable), Path::new("/tmp/home"))
                    .is_none()
            );
        }
    }

    #[test]
    fn recognizes_windows_release_layouts_only() {
        let program_files = Path::new("/Program Files");
        for executable in [
            "/Program Files/RestartFU/xd/bin/xd-daemon.exe",
            "/Program Files/RestartFU/xd-nightly/bin/xd-daemon.exe",
            "/Program Files/RestartFU/XD/bin/XD-DAEMON.EXE",
        ] {
            let location =
                windows_install_location_for_executable(Path::new(executable), program_files)
                    .unwrap();
            assert_eq!(location.daemon, Path::new(executable));
            assert_eq!(
                location.installer,
                Path::new(executable).parent().unwrap().join("install.ps1")
            );
        }

        for executable in [
            "/Program Files/RestartFU/other/bin/xd-daemon.exe",
            "/Program Files/Other/xd/bin/xd-daemon.exe",
            "/Program Files/RestartFU/xd/libexec/xd-daemon.exe",
            "/Program Files/RestartFU/xd/bin/xd-daemon",
            "/tmp/RestartFU/xd/bin/xd-daemon.exe",
        ] {
            assert!(
                windows_install_location_for_executable(Path::new(executable), program_files)
                    .is_none()
            );
        }
    }

    #[test]
    fn windows_installer_arguments_select_channel_and_mode() {
        let installer = Path::new(r"C:\Program Files\RestartFU\xd\bin\install.ps1");
        let stage = Path::new(r"C:\Users\person\AppData\Local\Temp\xd-update");
        let strings = |arguments: Vec<OsString>| {
            arguments
                .into_iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let expected = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };

        let mut stable_expected = expected(&[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ]);
        stable_expected.push(installer.to_string_lossy().into_owned());
        stable_expected.extend(expected(&["-Release", "-Quiet", "-InApp"]));
        assert_eq!(
            strings(windows_installer_arguments(
                installer,
                UpdateChannel::Release,
                InstallerMode::Install,
            )),
            stable_expected
        );

        let mut nightly_expected = expected(&[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ]);
        nightly_expected.push(installer.to_string_lossy().into_owned());
        nightly_expected.push("-StageDirectory".into());
        nightly_expected.push(stage.to_string_lossy().into_owned());
        nightly_expected.push("-StageOnly".into());
        assert_eq!(
            strings(windows_installer_arguments(
                installer,
                UpdateChannel::Nightly,
                InstallerMode::Stage(stage.to_owned()),
            )),
            nightly_expected
        );
    }

    #[test]
    fn the_windows_handoff_names_the_channel_it_is_installing() {
        let handoff = WindowsHandoff {
            installer: Path::new("C:/Program Files/RestartFU/xd/bin/install.ps1"),
            setup: Path::new("C:/Temp/xd-update/xd-windows-x86_64-setup.exe"),
            checksum: Path::new("C:/Temp/xd-update/xd-windows-x86_64-setup.exe.sha256"),
            cleanup: Path::new("C:/Temp/xd-update"),
            root: Path::new("C:/Program Files/RestartFU/xd"),
            desktop: Path::new("C:/Program Files/RestartFU/xd/bin/xd.exe"),
        };
        let strings = |arguments: Vec<OsString>| {
            arguments
                .into_iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        // Without this the installer takes its default, which is the nightly,
        // and a release would be updated as though it were one.
        let release = strings(windows_handoff_arguments(&handoff, UpdateChannel::Release));
        assert!(release.contains(&"-Release".to_owned()));
        assert_eq!(
            release.iter().position(|argument| argument == "-Release"),
            Some(7),
            "the channel belongs to the script, not to one of its parameters"
        );
        assert!(release.contains(&"C:/Program Files/RestartFU/xd".to_owned()));
        assert!(release.contains(&"C:/Program Files/RestartFU/xd/bin/xd.exe".to_owned()));
        assert!(release.contains(&"-SetupPath".to_owned()));
        assert!(release.contains(&"C:/Temp/xd-update/xd-windows-x86_64-setup.exe".to_owned()));
        assert!(!release.contains(&"-MsiPath".to_owned()));

        let nightly = strings(windows_handoff_arguments(&handoff, UpdateChannel::Nightly));
        assert!(!nightly.contains(&"-Release".to_owned()));
        assert_eq!(nightly.len() + 1, release.len());
    }

    #[test]
    fn windows_powershell_path_prefers_existing_system_path() {
        let root = env::temp_dir().join(format!("xd-powershell-test-{}", Uuid::new_v4()));
        let executable = root.join("System32/WindowsPowerShell/v1.0/powershell.exe");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"").unwrap();

        assert_eq!(
            windows_powershell_path_from(Some(root.as_os_str().to_owned())),
            executable
        );
        assert_eq!(
            windows_powershell_path_from(None),
            PathBuf::from("powershell.exe")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn windows_paths_normalize_prefixes_and_separators() {
        assert!(windows_path_eq(
            Path::new("\\\\?\\C:\\Program Files\\RestartFU\\xd\\"),
            Path::new("c:/Program Files/RestartFU/xd")
        ));
        assert!(!windows_path_eq(
            Path::new("C:/Program Files/RestartFU/xd"),
            Path::new("C:/Program Files/RestartFU/xd-nightly")
        ));
    }
}
