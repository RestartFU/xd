//! Who a workspace belongs to. A folder whose `origin` remote points at
//! GitHub is named by its organization or user, and that account's avatar
//! stands in for the folder icon so a project reads as itself in a list.
//!
//! Resolution is two cached steps: the remote URL of a directory, which is a
//! local `git` call, and the avatar for an owner, which is a network fetch and
//! therefore never made on the request path. A client asking for an uncached
//! avatar is told the answer is pending and receives an `owner-icon` event
//! when the fetch lands.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use crate::EventBus;

/// A remote is re-read this often, so a repository that gains an `origin`
/// gets its avatar without a host restart.
const REMOTE_TTL: Duration = Duration::from_secs(5 * 60);
/// Avatars change rarely, and a stale one costs nothing but a day of delay.
const ICON_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// A failed fetch is retried soon, but not on every tree read.
const FAILURE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_ICON_BYTES: u64 = 512 * 1024;
/// Large enough for a crisp 14 px icon on a retina display.
const AVATAR_SIZE: u32 = 64;
const MAX_LOGIN: usize = 39;

/// A fetched avatar, already encoded for the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Icon {
    mime: String,
    data: String,
}

type Fetcher = dyn Fn(&str) -> Result<Option<Icon>, String> + Send + Sync;

struct IconEntry {
    icon: Option<Icon>,
    fetched_at: Instant,
    ttl: Duration,
    /// The fetch failed rather than finding no avatar, so this answer is
    /// worth asking about again once the cooldown passes.
    retry: bool,
}

pub(crate) struct OwnerIcons {
    remotes: Mutex<HashMap<PathBuf, (Option<String>, Instant)>>,
    icons: Arc<Mutex<HashMap<String, IconEntry>>>,
    /// Owners being fetched, with the connections waiting on each of them.
    inflight: Arc<Mutex<HashMap<String, HashSet<u64>>>>,
    fetcher: Arc<Fetcher>,
    events: Arc<EventBus>,
}

impl OwnerIcons {
    pub(crate) fn new(events: Arc<EventBus>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .user_agent("xd/0.1")
            .build()
            .into();
        Self::with_fetcher(move |owner| fetch_avatar(&agent, owner), events)
    }

    fn with_fetcher(
        fetcher: impl Fn(&str) -> Result<Option<Icon>, String> + Send + Sync + 'static,
        events: Arc<EventBus>,
    ) -> Self {
        Self {
            remotes: Mutex::new(HashMap::new()),
            icons: Arc::new(Mutex::new(HashMap::new())),
            inflight: Arc::new(Mutex::new(HashMap::new())),
            fetcher: Arc::new(fetcher),
            events,
        }
    }

    /// The GitHub account that owns the repository checked out in `directory`,
    /// absent when it has no GitHub `origin` remote.
    pub(crate) fn owner(&self, directory: &Path) -> Option<String> {
        let now = Instant::now();
        if let Ok(remotes) = self.remotes.lock()
            && let Some((owner, read_at)) = remotes.get(directory)
            && now.saturating_duration_since(*read_at) < REMOTE_TTL
        {
            return owner.clone();
        }
        let owner = origin_url(directory).as_deref().and_then(github_owner);
        if let Ok(mut remotes) = self.remotes.lock() {
            remotes.insert(directory.to_owned(), (owner.clone(), now));
        }
        owner
    }

    /// Answers an `owner-icon` request for `subscriber`, starting a fetch when
    /// the avatar is not cached yet.
    pub(crate) fn icon(&self, subscriber: u64, request: &Value) -> Result<Value, String> {
        let owner = request
            .get("owner")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|owner| valid_login(owner))
            .ok_or_else(|| "owner-icon needs a GitHub owner login.".to_owned())?;
        let now = Instant::now();
        if let Some(entry) = self
            .icons
            .lock()
            .map_err(|_| "The owner icon cache is unavailable.".to_owned())?
            .get(&owner)
            .filter(|entry| now.saturating_duration_since(entry.fetched_at) < entry.ttl)
        {
            return Ok(icon_reply(&owner, entry.icon.as_ref(), entry.retry));
        }
        self.start_fetch(subscriber, owner.clone())?;
        Ok(json!({"ok": true, "owner": owner, "pending": true}))
    }

    fn start_fetch(&self, subscriber: u64, owner: String) -> Result<(), String> {
        let mut inflight = self
            .inflight
            .lock()
            .map_err(|_| "The owner icon cache is unavailable.".to_owned())?;
        if let Some(waiting) = inflight.get_mut(&owner) {
            waiting.insert(subscriber);
            return Ok(());
        }
        inflight.insert(owner.clone(), HashSet::from([subscriber]));
        drop(inflight);

        let icons = self.icons.clone();
        let inflight = self.inflight.clone();
        let fetcher = self.fetcher.clone();
        let events = self.events.clone();
        let worker_owner = owner.clone();
        thread::Builder::new()
            .name("xd-owner-icon".into())
            .spawn(move || {
                let fetched = fetcher(&worker_owner);
                let (icon, ttl, retry) = match fetched {
                    Ok(icon) => (icon, ICON_TTL, false),
                    Err(error) => {
                        eprintln!("xd-host: cannot read the {worker_owner} avatar: {error}");
                        (None, FAILURE_TTL, true)
                    }
                };
                if let Ok(mut icons) = icons.lock() {
                    icons.insert(
                        worker_owner.clone(),
                        IconEntry {
                            icon: icon.clone(),
                            fetched_at: Instant::now(),
                            ttl,
                            retry,
                        },
                    );
                }
                let waiting = inflight
                    .lock()
                    .ok()
                    .and_then(|mut inflight| inflight.remove(&worker_owner))
                    .unwrap_or_default();
                let event = icon_event(&worker_owner, icon.as_ref(), retry);
                for subscriber in waiting {
                    events.publish_to(subscriber, event.clone());
                }
            })
            .map_err(|error| {
                if let Ok(mut inflight) = self.inflight.lock() {
                    inflight.remove(&owner);
                }
                format!("Cannot start the owner avatar fetch: {error}")
            })?;
        Ok(())
    }

    /// Forgets what a directory's remote was, so a repository that has just
    /// arrived -- a finished clone -- is read again rather than kept nameless
    /// until the cache expires.
    pub(crate) fn forget(&self, directory: &Path) {
        if let Ok(mut remotes) = self.remotes.lock() {
            remotes.remove(directory);
        }
    }

    /// Caches an avatar so a test does not reach the network.
    #[cfg(test)]
    fn remember(&self, owner: &str, icon: Option<Icon>, retry: bool) {
        self.icons.lock().expect("icon cache").insert(
            owner.to_owned(),
            IconEntry {
                icon,
                fetched_at: Instant::now(),
                ttl: ICON_TTL,
                retry,
            },
        );
    }
}

fn icon_reply(owner: &str, icon: Option<&Icon>, retry: bool) -> Value {
    let mut reply = json!({"ok": true, "owner": owner});
    describe_icon(&mut reply, icon, retry);
    reply
}

fn icon_event(owner: &str, icon: Option<&Icon>, retry: bool) -> Value {
    let mut event = json!({"event": "owner-icon", "owner": owner});
    describe_icon(&mut event, icon, retry);
    event
}

/// An answer carries the avatar, or says the fetch failed and is worth
/// asking about again. Neither means the account simply has no avatar.
fn describe_icon(answer: &mut Value, icon: Option<&Icon>, retry: bool) {
    if let Some(icon) = icon {
        answer["mime"] = Value::String(icon.mime.clone());
        answer["data"] = Value::String(icon.data.clone());
    } else if retry {
        answer["retry"] = Value::Bool(true);
    }
}

/// The `origin` remote of a repository, read without touching the network.
fn origin_url(directory: &Path) -> Option<String> {
    if !directory.is_dir() {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["remote", "get-url", "origin"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!url.is_empty()).then_some(url)
}

/// The owner segment of a GitHub remote URL, in every form git accepts:
/// `git@github.com:owner/repo.git`, `https://github.com/owner/repo`, and
/// `ssh://git@github.com:22/owner/repo`. Other hosts have no avatar to show.
fn github_owner(remote: &str) -> Option<String> {
    let remote = remote.trim();
    // A URL separates its authority from the path with `/`; the scp-like form
    // git also accepts (`user@host:owner/repo`) separates it with `:`.
    let (authority, path) = match remote.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        None => remote.split_once(':')?,
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split_once(':').map_or(host, |(host, _)| host);
    if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
        return None;
    }
    let owner = path.trim_start_matches('/').split('/').next()?;
    valid_login(owner).then(|| owner.to_owned())
}

/// GitHub logins are alphanumeric with inner hyphens. Rejecting anything else
/// keeps a remote URL out of the avatar request path.
fn valid_login(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= MAX_LOGIN
        && !owner.starts_with('-')
        && !owner.ends_with('-')
        && owner
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn fetch_avatar(agent: &ureq::Agent, owner: &str) -> Result<Option<Icon>, String> {
    let url = format!("https://github.com/{owner}.png?size={AVATAR_SIZE}");
    let mut response = match agent.get(&url).call() {
        Ok(response) => response,
        // An account without an avatar, or none by that name, is an answer:
        // the folder keeps its icon until the cache expires.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(ureq::Error::StatusCode(code)) => return Err(format!("GitHub returned HTTP {code}.")),
        Err(_) => return Err("Cannot reach GitHub for the owner avatar.".to_owned()),
    };
    let mime = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    if !matches!(mime.as_str(), "image/png" | "image/jpeg") {
        return Ok(None);
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_ICON_BYTES)
        .read_to_vec()
        .map_err(|_| "GitHub returned an unreadable avatar.".to_owned())?;
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(Icon {
        mime,
        data: STANDARD.encode(&bytes),
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn icons() -> OwnerIcons {
        OwnerIcons::with_fetcher(
            |_| Err("the test fetcher never reaches GitHub".to_owned()),
            Arc::new(EventBus::default()),
        )
    }

    #[test]
    fn github_remotes_name_their_owner_in_every_url_form() {
        for remote in [
            "git@github.com:RestartFU/xd.git",
            "https://github.com/RestartFU/xd.git",
            "https://github.com/RestartFU/xd",
            "ssh://git@github.com/RestartFU/xd.git",
            "git://github.com/RestartFU/xd.git",
            "https://token@github.com/RestartFU/xd.git",
            "ssh://git@github.com:22/RestartFU/xd.git",
        ] {
            assert_eq!(
                github_owner(remote).as_deref(),
                Some("RestartFU"),
                "{remote}"
            );
        }
    }

    #[test]
    fn remotes_without_a_github_owner_have_no_avatar() {
        for remote in [
            "git@gitlab.com:RestartFU/xd.git",
            "https://example.com/RestartFU/xd.git",
            "https://github.com.example.com/RestartFU/xd.git",
            "/srv/git/xd.git",
            "https://github.com/",
            "https://github.com/-bad-/xd.git",
            "",
        ] {
            assert_eq!(github_owner(remote), None, "{remote}");
        }
    }

    #[test]
    fn a_cached_avatar_answers_without_a_fetch() {
        let icons = icons();
        icons.remember(
            "RestartFU",
            Some(Icon {
                mime: "image/png".into(),
                data: "aGk=".into(),
            }),
            false,
        );
        assert_eq!(
            icons.icon(0, &json!({"owner": "RestartFU"})).unwrap(),
            json!({"ok": true, "owner": "RestartFU", "mime": "image/png", "data": "aGk="})
        );
    }

    #[test]
    fn a_cached_miss_answers_without_an_image() {
        let icons = icons();
        icons.remember("RestartFU", None, false);
        assert_eq!(
            icons.icon(0, &json!({"owner": "RestartFU"})).unwrap(),
            json!({"ok": true, "owner": "RestartFU"})
        );
    }

    #[test]
    fn a_failed_fetch_is_answered_as_worth_asking_again() {
        let icons = icons();
        icons.remember("RestartFU", None, true);
        assert_eq!(
            icons.icon(0, &json!({"owner": "RestartFU"})).unwrap(),
            json!({"ok": true, "owner": "RestartFU", "retry": true})
        );
    }

    #[test]
    fn a_directory_is_read_again_once_its_repository_arrives() {
        let directory = std::env::temp_dir().join(format!("xd-owner-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("fixture directory");
        let icons = icons();
        assert_eq!(icons.owner(&directory), None);

        let git = |arguments: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&directory)
                    .args(arguments)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .expect("git")
                    .success()
            );
        };
        git(&["init"]);
        git(&["remote", "add", "origin", "git@github.com:RestartFU/xd.git"]);

        assert_eq!(
            icons.owner(&directory),
            None,
            "the cached answer stands until the directory is forgotten"
        );
        icons.forget(&directory);
        assert_eq!(icons.owner(&directory).as_deref(), Some("RestartFU"));

        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn an_uncached_avatar_is_pending_and_fetched_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let icons = OwnerIcons::with_fetcher(
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(Some(Icon {
                    mime: "image/png".into(),
                    data: "aGk=".into(),
                }))
            },
            Arc::new(EventBus::default()),
        );
        assert_eq!(
            icons.icon(0, &json!({"owner": "RestartFU"})).unwrap(),
            json!({"ok": true, "owner": "RestartFU", "pending": true})
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && icons.icon(0, &json!({"owner": "RestartFU"})).unwrap()["pending"] == json!(true)
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            icons.icon(0, &json!({"owner": "RestartFU"})).unwrap(),
            json!({"ok": true, "owner": "RestartFU", "mime": "image/png", "data": "aGk="})
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_invalid_owner_is_rejected_before_any_fetch() {
        let icons = icons();
        for owner in [
            json!("../../etc"),
            json!("a".repeat(40)),
            json!(""),
            json!(7),
        ] {
            assert!(icons.icon(0, &json!({"owner": owner})).is_err(), "{owner}");
        }
        assert!(icons.icon(0, &json!({})).is_err());
    }

    #[test]
    fn a_directory_without_a_repository_has_no_owner() {
        let icons = icons();
        assert_eq!(icons.owner(Path::new("/nonexistent-xd-workspace")), None);
    }
}
