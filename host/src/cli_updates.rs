use std::{
    collections::HashMap,
    env,
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    EventBus,
    agent::{resolve_claude, resolve_codex},
    background_process::command as background_command,
    cli_versions::clean_output,
};

const REGISTRY_URL: &str = "https://registry.npmjs.org";
const MAX_REGISTRY_BYTES: u64 = 1024 * 1024;
const STARTUP_DELAY: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_secs(30 * 60);
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const OUTPUT_LIMIT: usize = 64 * 1024;
const DETAIL_LIMIT: usize = 400;

type LatestVersion = fn(&str) -> Result<String, String>;

/// Updates the assistant CLIs the user installed themselves.
///
/// xd never bundles these, so it does not know how each was installed. Every
/// CLI listed here ships its own `update` command that handles its install
/// method, and the npm package only answers which version is current.
#[derive(Clone)]
pub(crate) struct CliUpdates {
    inner: Arc<CliUpdatesInner>,
}

struct CliUpdatesInner {
    providers: Vec<Provider>,
    states: Mutex<HashMap<&'static str, UpdateEntry>>,
    checking: AtomicBool,
    latest_version: LatestVersion,
    update_timeout: Duration,
    events: Arc<EventBus>,
}

#[derive(Clone)]
struct Provider {
    id: &'static str,
    display_name: &'static str,
    package: &'static str,
    executable: PathBuf,
}

#[derive(Clone, Default, PartialEq)]
struct UpdateEntry {
    version: Option<String>,
    latest: Option<String>,
    /// A release this install's updater already declined, for example because
    /// the CLI is pinned or its package manager has not published it yet.
    settled: Option<String>,
    /// Older releases of a CLI may not have an `update` command yet.
    updatable: bool,
    state: &'static str,
    detail: Option<String>,
    /// Bumped by every update so a check that began earlier cannot overwrite it.
    generation: u64,
}

impl UpdateEntry {
    fn available(&self) -> bool {
        let (Some(version), Some(latest)) = (&self.version, &self.latest) else {
            return false;
        };
        self.settled.as_ref() != Some(latest) && is_newer(latest, version)
    }
}

impl CliUpdates {
    pub(crate) fn new(events: Arc<EventBus>) -> Self {
        let updates = Self::with_sources(
            events,
            vec![
                Provider {
                    id: "codex",
                    display_name: "Codex",
                    package: "@openai/codex",
                    executable: resolve_codex(),
                },
                Provider {
                    id: "claude",
                    display_name: "Claude Code",
                    package: "@anthropic-ai/claude-code",
                    executable: resolve_claude(),
                },
            ],
            latest_package_version,
            UPDATE_TIMEOUT,
        );
        if !cfg!(test) && env::var("XD_DISABLE_AUTO_UPDATE").as_deref() != Ok("1") {
            let weak = Arc::downgrade(&updates.inner);
            let _ = thread::Builder::new()
                .name("xd-cli-update-poll".into())
                .spawn(move || {
                    thread::sleep(STARTUP_DELAY);
                    while let Some(inner) = weak.upgrade() {
                        let _ = Self { inner }.check();
                        thread::sleep(POLL_INTERVAL);
                    }
                });
        }
        updates
    }

    fn with_sources(
        events: Arc<EventBus>,
        providers: Vec<Provider>,
        latest_version: LatestVersion,
        update_timeout: Duration,
    ) -> Self {
        let states = providers
            .iter()
            .map(|provider| {
                (
                    provider.id,
                    UpdateEntry {
                        state: "idle",
                        ..UpdateEntry::default()
                    },
                )
            })
            .collect();
        Self {
            inner: Arc::new(CliUpdatesInner {
                providers,
                states: Mutex::new(states),
                checking: AtomicBool::new(false),
                latest_version,
                update_timeout,
                events,
            }),
        }
    }

    pub(crate) fn perform(
        &self,
        action: Option<&str>,
        provider: Option<&str>,
    ) -> Result<Value, String> {
        match action.unwrap_or("status") {
            "status" => {}
            "check" => self.check()?,
            "install" => {
                self.install(provider.ok_or_else(|| "Choose an assistant to update.".to_owned())?)?
            }
            _ => return Err("No such agent-cli-updates action.".into()),
        }
        Ok(self.snapshots())
    }

    pub(crate) fn is_updating(&self) -> bool {
        self.inner
            .states
            .lock()
            .is_ok_and(|states| states.values().any(|entry| entry.state == "updating"))
    }

    fn check(&self) -> Result<(), String> {
        if self.inner.checking.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let updates = self.clone();
        thread::Builder::new()
            .name("xd-cli-update-check".into())
            .spawn(move || {
                for provider in &updates.inner.providers {
                    updates.check_provider(provider);
                }
                updates.inner.checking.store(false, Ordering::Release);
            })
            .map(|_| ())
            .map_err(|error| {
                self.inner.checking.store(false, Ordering::Release);
                format!("Cannot start the assistant update check: {error}")
            })
    }

    fn check_provider(&self, provider: &Provider) {
        let generation = {
            let Ok(states) = self.inner.states.lock() else {
                return;
            };
            match states.get(provider.id) {
                Some(entry) if entry.state != "updating" => entry.generation,
                _ => return,
            }
        };
        let version = installed_version(&provider.executable);
        let latest = version
            .as_ref()
            .and_then(|_| (self.inner.latest_version)(provider.package).ok())
            .filter(|latest| release_in(latest).is_some());
        let installed = version.is_some();
        let updatable = installed && supports_update_command(&provider.executable);
        self.change(provider.id, |entry| {
            if entry.generation != generation {
                return;
            }
            entry.version = version;
            entry.updatable = updatable;
            if !installed {
                entry.latest = None;
            } else if latest.is_some() {
                entry.latest = latest;
            }
            if entry.state == "updated" || !entry.available() {
                entry.state = "idle";
                entry.detail = None;
            }
        });
    }

    fn install(&self, provider_id: &str) -> Result<(), String> {
        let provider = self
            .inner
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .cloned()
            .ok_or_else(|| "xd cannot update that assistant.".to_owned())?;
        let previous = {
            let mut states = self
                .inner
                .states
                .lock()
                .map_err(|_| "Assistant update state is unavailable.".to_owned())?;
            let entry = states
                .get_mut(provider.id)
                .ok_or_else(|| "xd cannot update that assistant.".to_owned())?;
            if entry.state == "updating" || !entry.available() {
                return Ok(());
            }
            if !entry.updatable {
                return Err(format!(
                    "This version of {} cannot update itself. Update it the way you installed it.",
                    provider.display_name
                ));
            }
            entry.state = "updating";
            entry.detail = None;
            entry.generation += 1;
            entry.version.clone()
        };
        self.publish(provider.id);
        let updates = self.clone();
        let id = provider.id;
        let spawned = thread::Builder::new()
            .name(format!("xd-cli-update-{id}"))
            .spawn(move || {
                let result = run_update(&provider.executable, updates.inner.update_timeout);
                let version = installed_version(&provider.executable);
                updates.change(id, |entry| {
                    finish_update(entry, previous, version, result);
                });
            });
        spawned.map(|_| ()).map_err(|error| {
            let message = format!("Cannot start the assistant update: {error}");
            self.change(id, |entry| {
                entry.state = "failed";
                entry.detail = Some(message.clone());
                entry.generation += 1;
            });
            message
        })
    }

    fn snapshots(&self) -> Value {
        let providers = self
            .inner
            .states
            .lock()
            .map(|states| {
                self.inner
                    .providers
                    .iter()
                    .filter_map(|provider| {
                        states
                            .get(provider.id)
                            .map(|entry| snapshot_value(provider, entry))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        json!({"ok": true, "providers": providers})
    }

    fn change(&self, id: &str, update: impl FnOnce(&mut UpdateEntry)) {
        let changed = self.inner.states.lock().is_ok_and(|mut states| {
            states.get_mut(id).is_some_and(|entry| {
                let before = entry.clone();
                update(entry);
                *entry != before
            })
        });
        if changed {
            self.publish(id);
        }
    }

    fn publish(&self, id: &str) {
        let snapshot = self.inner.states.lock().ok().and_then(|states| {
            let entry = states.get(id)?;
            let provider = self
                .inner
                .providers
                .iter()
                .find(|provider| provider.id == id)?;
            Some(snapshot_value(provider, entry))
        });
        if let Some(mut event) = snapshot {
            event["event"] = Value::String("agent-cli-update".into());
            self.inner.events.publish(event);
        }
    }
}

fn finish_update(
    entry: &mut UpdateEntry,
    previous: Option<String>,
    version: Option<String>,
    result: Result<String, String>,
) {
    entry.generation += 1;
    let changed = version.is_some() && version != previous;
    if version.is_some() {
        entry.version = version;
    }
    match result {
        Ok(_) if changed => {
            entry.state = "updated";
            entry.detail = None;
        }
        Ok(output) => {
            entry.state = "failed";
            entry.settled = entry.latest.clone();
            entry.detail = Some(if output.is_empty() {
                "The updater finished but the version did not change.".into()
            } else {
                output
            });
        }
        Err(error) => {
            entry.state = "failed";
            entry.detail = Some(error);
        }
    }
}

fn snapshot_value(provider: &Provider, entry: &UpdateEntry) -> Value {
    let mut value = json!({
        "provider": provider.id,
        "display_name": provider.display_name,
        "state": entry.state,
        "available": entry.available(),
        "updatable": entry.updatable,
    });
    if let Some(version) = &entry.version {
        value["version"] = Value::String(version.clone());
    }
    if let Some(latest) = &entry.latest {
        value["latest"] = Value::String(latest.clone());
    }
    if let Some(detail) = &entry.detail {
        value["detail"] = Value::String(detail.clone());
    }
    value
}

/// The `major.minor.patch` in a CLI's version banner, such as `codex-cli 1.2.3`
/// or `1.2.3 (Claude Code)`.
fn release_in(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|word| release_parts(word).is_some())
        .map(|word| word.trim_start_matches('v').to_owned())
}

fn release_parts(word: &str) -> Option<[u64; 3]> {
    let core = word
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    let parts = core
        .split('.')
        .map(str::parse)
        .collect::<Result<Vec<u64>, _>>()
        .ok()?;
    parts.try_into().ok()
}

/// A prerelease comes before the release it leads to, but two prereleases of
/// one release are not compared: a CLI's prerelease tags have no shared order.
fn is_newer(latest: &str, current: &str) -> bool {
    let (Some(latest_parts), Some(current_parts)) = (release_parts(latest), release_parts(current))
    else {
        return false;
    };
    latest_parts > current_parts
        || (latest_parts == current_parts && is_prerelease(current) && !is_prerelease(latest))
}

fn is_prerelease(version: &str) -> bool {
    version.split('+').next().unwrap_or_default().contains('-')
}

fn latest_package_version(package: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let encoded = package.replace('/', "%2f");
    let mut response = agent
        .get(format!("{REGISTRY_URL}/{encoded}/latest"))
        .header("Accept", "application/json")
        .header("User-Agent", "xd")
        .call()
        .map_err(|_| "Could not reach the package registry.".to_owned())?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_REGISTRY_BYTES)
        .read_to_string()
        .map_err(|_| "The package registry response was unreadable.".to_owned())?;
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|manifest| manifest.get("version")?.as_str().map(str::to_owned))
        .ok_or_else(|| "The package registry returned invalid data.".to_owned())
}

/// Asks the CLI something without running its updater. Bounded even if the CLI
/// hangs or leaves a helper holding its output open.
fn query(executable: &Path, argument: &str) -> Option<String> {
    let mut child = background_command(executable)
        .arg(argument)
        .env("DISABLE_AUTOUPDATER", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let stdout = child.stdout.take().map(drain);
    let status = wait_with_timeout(&mut child, VERSION_TIMEOUT).ok()?;
    status.success().then(|| collect_output(stdout))
}

fn installed_version(executable: &Path) -> Option<String> {
    query(executable, "--version").and_then(|output| release_in(&output))
}

/// Whether the CLI's help lists an `update` command. Asking first matters: a
/// release without one can treat the word as a prompt instead of a command.
fn supports_update_command(executable: &Path) -> bool {
    query(executable, "--help").is_some_and(|help| lists_update_command(&help))
}

/// Commands sit at a two-space indent. Wrapped descriptions are indented further
/// and may begin with the same word.
fn lists_update_command(help: &str) -> bool {
    help.lines().any(|line| {
        line.strip_prefix("  ")
            .filter(|rest| !rest.starts_with(char::is_whitespace))
            .and_then(|rest| rest.split_whitespace().next())
            .is_some_and(|name| name.split(['|', ',']).any(|name| name == "update"))
    })
}

/// Runs the CLI's updater in its own process group, so a timeout also stops
/// any package manager it started. Returns the updater's final output.
fn run_update(executable: &Path, timeout: Duration) -> Result<String, String> {
    let mut child = background_command(executable)
        .arg("update")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("Cannot run {} update: {error}", executable.display()))?;
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let status = wait_with_timeout(&mut child, timeout);
    let stdout = collect_output(stdout).trim().to_owned();
    let stderr = collect_output(stderr).trim().to_owned();
    let status = status?;
    let output = if status.success() || stderr.is_empty() {
        stdout
    } else {
        stderr
    };
    let output = limit_detail(&output);
    if status.success() {
        Ok(output)
    } else if output.is_empty() {
        Err("The updater failed.".into())
    } else {
        Err(output)
    }
}

/// Waits only briefly: a helper left running can hold the pipes open for good.
fn collect_output(output: Option<Receiver<Vec<u8>>>) -> String {
    output
        .and_then(|output| output.recv_timeout(OUTPUT_DRAIN_TIMEOUT).ok())
        .map(|bytes| clean_output(&bytes))
        .unwrap_or_default()
}

fn drain(stream: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(read_tail(stream, OUTPUT_LIMIT));
    });
    receiver
}

/// Reads to the end but keeps only the last `limit` bytes, where a failing
/// command reports what went wrong.
fn read_tail(mut stream: impl Read, limit: usize) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    while let Ok(count) = stream.read(&mut buffer) {
        if count == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..count]);
        if output.len() > limit {
            output.drain(..output.len() - limit);
        }
    }
    output
}

fn wait_with_timeout(
    child: &mut Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(format!("Cannot wait for the updater: {error}")),
        }
    }
    // SAFETY: the child leads a process group of its own, and has not been
    // reaped, so this signal can only reach that group.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.wait();
    Err(format!(
        "The updater timed out after {} minutes.",
        timeout.as_secs() / 60
    ))
}

/// Keeps the end of the output, where updaters say what went wrong.
fn limit_detail(output: &str) -> String {
    let characters = output.chars().count();
    if characters <= DETAIL_LIMIT {
        return output.to_owned();
    }
    let kept = output
        .chars()
        .skip(characters - DETAIL_LIMIT)
        .collect::<String>();
    format!("…{kept}")
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::*;

    struct FakeCli {
        directory: PathBuf,
        updates: CliUpdates,
    }

    impl FakeCli {
        fn start(name: &str, script: &str, latest: LatestVersion, timeout: Duration) -> Self {
            let directory =
                env::temp_dir().join(format!("xd-rust-cli-update-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&directory);
            fs::create_dir_all(&directory).unwrap();
            let executable = directory.join("assistant");
            // Every fake lists an `update` command unless it answers --help itself.
            let script = if script.contains("--help") {
                script.to_owned()
            } else {
                script.replacen(
                    '\n',
                    "\nif [ \"$1\" = --help ]; then echo '  update  Update it'; exit 0; fi\n",
                    1,
                )
            };
            fs::write(
                &executable,
                script.replace("$DIR", directory.to_str().unwrap()),
            )
            .unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            let updates = CliUpdates::with_sources(
                Arc::new(EventBus::default()),
                vec![Provider {
                    id: "assistant",
                    display_name: "Assistant",
                    package: "assistant",
                    executable,
                }],
                latest,
                timeout,
            );
            Self { directory, updates }
        }

        fn entry(&self) -> Value {
            self.updates.snapshots()["providers"][0].clone()
        }

        fn wait_for(&self, ready: impl Fn(&Value) -> bool) -> Value {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let entry = self.entry();
                if ready(&entry) {
                    return entry;
                }
                assert!(Instant::now() < deadline, "timed out at {entry}");
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn check(&self) -> Value {
            self.updates.perform(Some("check"), None).unwrap();
            self.wait_for(|_| !self.updates.inner.checking.load(Ordering::Acquire));
            self.entry()
        }
    }

    impl Drop for FakeCli {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    // Reports the version stored beside it, so `update` can change the answer.
    const UPGRADING_CLI: &str = "#!/bin/sh\n\
        if [ \"$1\" = update ]; then echo 2.0.0 > \"$DIR/version\"; echo done; exit 0; fi\n\
        if [ -f \"$DIR/version\" ]; then cat \"$DIR/version\"; else echo '1.0.0 (Assistant)'; fi\n";

    fn newer(_: &str) -> Result<String, String> {
        Ok("2.0.0".into())
    }

    #[test]
    fn extracts_and_compares_releases_from_version_banners() {
        assert_eq!(release_in("codex-cli 0.167.0").as_deref(), Some("0.167.0"));
        assert_eq!(
            release_in("2.1.319 (Claude Code)").as_deref(),
            Some("2.1.319")
        );
        assert_eq!(release_in("assistant v1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(release_in("not a version"), None);
        assert!(is_newer("2.1.320", "2.1.319"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("2.1.319", "2.1.319"));
        assert!(!is_newer("2.1.318", "2.1.319"));
        assert!(!is_newer("garbage", "2.1.319"));
        assert!(is_newer("1.2.3", "1.2.3-beta.1"));
        assert!(is_newer("1.2.4-beta.1", "1.2.3"));
        assert!(!is_newer("1.2.3-beta.1", "1.2.3"));
        assert!(!is_newer("1.2.3-beta.2", "1.2.3-beta.1"));
        assert!(!is_newer("1.2.3+build.2", "1.2.3+build.1"));
    }

    #[test]
    fn offers_a_newer_release_and_updates_on_request() {
        let cli = FakeCli::start("offer", UPGRADING_CLI, newer, UPDATE_TIMEOUT);
        assert_eq!(cli.entry()["available"], false);
        let entry = cli.check();
        assert_eq!(entry["version"], "1.0.0");
        assert_eq!(entry["latest"], "2.0.0");
        assert_eq!(entry["available"], true);

        cli.updates
            .perform(Some("install"), Some("assistant"))
            .unwrap();
        let entry = cli.wait_for(|entry| entry["state"] == "updated");
        assert_eq!(entry["version"], "2.0.0");
        assert_eq!(entry["available"], false);
        assert!(!cli.updates.is_updating());
        assert_eq!(cli.check()["state"], "idle");
    }

    #[test]
    fn an_updater_that_changes_nothing_is_reported_and_not_offered_again() {
        let cli = FakeCli::start(
            "pinned",
            "#!/bin/sh\nif [ \"$1\" = update ]; then echo 'managed by your package manager'; exit 0; fi\necho 1.0.0\n",
            newer,
            UPDATE_TIMEOUT,
        );
        assert_eq!(cli.check()["available"], true);
        cli.updates
            .perform(Some("install"), Some("assistant"))
            .unwrap();
        let entry = cli.wait_for(|entry| entry["state"] == "failed");
        assert_eq!(entry["detail"], "managed by your package manager");
        assert_eq!(entry["available"], false);
        let entry = cli.check();
        assert_eq!(entry["available"], false);
        assert_eq!(entry["state"], "idle");
    }

    #[test]
    fn a_failed_update_keeps_its_error_and_stays_available() {
        let cli = FakeCli::start(
            "failing",
            "#!/bin/sh\nif [ \"$1\" = update ]; then echo 'no permission' >&2; exit 1; fi\necho 1.0.0\n",
            newer,
            UPDATE_TIMEOUT,
        );
        cli.check();
        cli.updates
            .perform(Some("install"), Some("assistant"))
            .unwrap();
        let entry = cli.wait_for(|entry| entry["state"] == "failed");
        assert_eq!(entry["detail"], "no permission");
        assert_eq!(entry["available"], true);
        let entry = cli.check();
        assert_eq!(entry["state"], "failed");
        assert_eq!(entry["detail"], "no permission");
    }

    #[test]
    fn a_hung_updater_is_stopped_with_its_children() {
        let cli = FakeCli::start(
            "hung",
            "#!/bin/sh\nif [ \"$1\" = update ]; then sleep 60 & echo $! > \"$DIR/child\"; wait; fi\necho 1.0.0\n",
            newer,
            Duration::from_millis(300),
        );
        cli.check();
        cli.updates
            .perform(Some("install"), Some("assistant"))
            .unwrap();
        assert!(cli.updates.is_updating());
        let entry = cli.wait_for(|entry| entry["state"] == "failed");
        assert!(entry["detail"].as_str().unwrap().contains("timed out"));
        let child = fs::read_to_string(cli.directory.join("child")).unwrap();
        let child = child.trim().parse::<libc::pid_t>().unwrap();
        // SAFETY: signal 0 only tests whether the process still exists.
        let deadline = Instant::now() + Duration::from_secs(5);
        while unsafe { libc::kill(child, 0) } == 0 {
            assert!(Instant::now() < deadline, "updater child survived");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_missing_cli_or_registry_offers_nothing() {
        fn unreachable(_: &str) -> Result<String, String> {
            Err("offline".into())
        }
        let offline = FakeCli::start("offline", UPGRADING_CLI, unreachable, UPDATE_TIMEOUT);
        let entry = offline.check();
        assert_eq!(entry["version"], "1.0.0");
        assert_eq!(entry["available"], false);

        let missing = FakeCli::start("missing", "#!/bin/sh\nexit 127\n", newer, UPDATE_TIMEOUT);
        let entry = missing.check();
        assert!(entry.get("version").is_none());
        assert_eq!(entry["available"], false);
    }

    #[test]
    fn refuses_unknown_requests_and_ignores_an_update_with_nothing_to_install() {
        let cli = FakeCli::start("requests", UPGRADING_CLI, newer, UPDATE_TIMEOUT);
        assert!(cli.updates.perform(Some("install"), None).is_err());
        assert!(cli.updates.perform(Some("install"), Some("other")).is_err());
        assert!(cli.updates.perform(Some("restart"), None).is_err());
        let reply = cli
            .updates
            .perform(Some("install"), Some("assistant"))
            .unwrap();
        assert_eq!(reply["providers"][0]["state"], "idle");
    }

    #[test]
    fn a_release_without_an_update_command_is_never_run() {
        let cli = FakeCli::start(
            "older",
            "#!/bin/sh\nif [ \"$1\" = --help ]; then echo '  exec  Run'; exit 0; fi\nif [ \"$1\" = update ]; then touch \"$DIR/ran\"; fi\necho 1.0.0\n",
            newer,
            UPDATE_TIMEOUT,
        );
        let entry = cli.check();
        assert_eq!(entry["available"], true);
        assert_eq!(entry["updatable"], false);
        let refused = cli
            .updates
            .perform(Some("install"), Some("assistant"))
            .unwrap_err();
        assert!(refused.contains("cannot update itself"), "{refused}");
        assert!(!cli.directory.join("ran").exists());
        assert_eq!(cli.entry()["state"], "idle");
    }

    #[test]
    fn finds_the_update_command_in_real_help_layouts() {
        assert!(lists_update_command(
            "Commands:\n  update            Update Codex to the latest version\n"
        ));
        assert!(lists_update_command(
            "Commands:\n  update|upgrade   Check for updates and install if\n"
        ));
        assert!(!lists_update_command(
            "Commands:\n  doctor  Check health, then\n          update through the manager\n"
        ));
        assert!(!lists_update_command("update  at the margin\n"));
        assert!(!lists_update_command("  updater  Not the command\n"));
    }

    #[test]
    fn captured_output_keeps_its_end() {
        let output = read_tail(&b"0123456789"[..], 4);
        assert_eq!(output, b"6789");
        let long = vec![b'x'; OUTPUT_LIMIT * 2];
        let mut failing = long.clone();
        failing.extend_from_slice(b"the reason");
        let output = read_tail(failing.as_slice(), OUTPUT_LIMIT);
        assert_eq!(output.len(), OUTPUT_LIMIT);
        assert!(output.ends_with(b"the reason"));
    }

    #[test]
    fn a_query_is_not_held_up_by_a_helper_left_running() {
        let cli = FakeCli::start(
            "helper",
            "#!/bin/sh\nif [ \"$1\" = update ]; then echo 2.0.0 > \"$DIR/version\"; exit 0; fi\nsleep 4 &\ncat \"$DIR/version\" 2>/dev/null || echo 1.0.0\n",
            newer,
            UPDATE_TIMEOUT,
        );
        let executable = cli.directory.join("assistant");
        let started = Instant::now();
        assert_eq!(installed_version(&executable), None);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn long_updater_output_keeps_its_end() {
        let output = format!("{}the reason", "x".repeat(DETAIL_LIMIT * 2));
        let limited = limit_detail(&output);
        assert_eq!(limited.chars().count(), DETAIL_LIMIT + 1);
        assert!(limited.ends_with("the reason"));
    }
}
