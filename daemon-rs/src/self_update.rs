use std::{
    env, fs,
    io::Read,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::EventBus;

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
            .spawn(move || match run_installer(&location.installer, channel) {
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

    fn restart(&self) -> Result<(), String> {
        let location = self
            .inner
            .install
            .clone()
            .ok_or_else(|| "This daemon's installation cannot restart itself.".to_owned())?;
        let arguments = env::args_os().skip(1).collect::<Vec<_>>();
        self.publish();
        thread::Builder::new()
            .name("xd-update-restart".into())
            .spawn(move || {
                thread::sleep(Duration::from_millis(250));
                let error = Command::new(location.daemon).args(arguments).exec();
                eprintln!("xd-daemon: cannot restart after update: {error}");
            })
            .map(|_| ())
            .map_err(|error| format!("Cannot schedule the daemon restart: {error}"))
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
        .map_err(|_| format!("Could not reach the {} release feed.", channel.as_str()))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_RELEASE_BYTES)
        .read_to_string()
        .map_err(|_| format!("The {} release feed was unreadable.", channel.as_str()))?;
    let release: Value = serde_json::from_str(&body).map_err(|_| {
        format!(
            "The {} release feed returned invalid data.",
            channel.as_str()
        )
    })?;
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
        .ok_or_else(|| {
            format!(
                "The {} release feed did not identify its build.",
                channel.as_str()
            )
        })
}

fn run_installer(installer: &Path, channel: UpdateChannel) -> Result<(), String> {
    let mut command = Command::new("sh");
    command.arg(installer);
    if channel == UpdateChannel::Release {
        command.arg("--release");
    }
    let mut child = command
        .env("XD_ALLOW_RUNNING_INSTALL", "1")
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

fn install_location() -> Option<InstallLocation> {
    let executable = fs::canonicalize(env::current_exe().ok()?).ok()?;
    let root = executable.parent()?.parent()?;
    if !matches!(root.file_name()?.to_str()?, "xd" | "xd-nightly") {
        return None;
    }
    let installer = root.join("libexec/install.sh");
    let daemon = root.join("libexec/xd-daemon");
    if !executable_file(&installer) || !executable_file(&daemon) {
        return None;
    }
    Some(InstallLocation { installer, daemon })
}

fn executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
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
    fn source_builds_cannot_replace_an_installation() {
        let updater = SelfUpdate::new(Arc::new(EventBus::default()));
        assert_eq!(updater.snapshot()["supported"], false);
        assert!(updater.perform(Some("install")).is_err());
        assert!(updater.perform(Some("unknown")).is_err());
    }
}
