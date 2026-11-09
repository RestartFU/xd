//! Several sign-ins per assistant, and which one a turn runs on.
//!
//! The user's own CLI sign-in is always the `default` account and runs exactly
//! as it did before there were others. Each account added in xd gets a private
//! provider home under the host's data directory (`CODEX_HOME` for Codex,
//! `CLAUDE_CONFIG_DIR` for Claude Code). The CLI signs in, stores and refreshes
//! its own credentials there; xd never reads or copies a token.
//!
//! Everything else in the user's own home is linked into each account's home,
//! so configuration, MCP servers, skills, plugins and session transcripts stay
//! shared. Credentials, sign-in locks, the CLI's background daemon, and caches
//! that depend on the plan stay private to each home.
//!
//! An account is only used for a turn while it is signed in, belongs to the
//! account it was added as, and has not run out of its limits. A login that
//! turns out to be another account's (a shared keychain entry, a sign-in in the
//! wrong browser profile) is held back rather than used.
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};

pub(crate) const PROVIDERS: [&str; 2] = ["codex", "claude"];
pub(crate) const DEFAULT_ACCOUNT: &str = "default";
const FILE: &str = "accounts.json";
const MAX_ACCOUNTS: usize = 8;
/// A limit at or past this share is spent: the CLI rounds, and a turn started
/// at 99.6% fails as surely as one at 100%.
const SPENT_PERCENT: f64 = 99.5;
/// A limit a turn ran into is lifted early only when a reading shows every
/// main limit below this share.
const LIFT_PERCENT: f64 = 95.0;
/// How long an account that hit a limit with no stated reset is left alone
/// before it is tried again. A probe usually replaces this with the real time.
const UNTIMED_LIMIT_MS: i64 = 30 * 60 * 1000;
/// Probe results older than this no longer decide whether an account is spent.
const STALE_PROBE_MS: i64 = 6 * 60 * 60 * 1000;

/// Entries of a provider home that belong to one sign-in and are never shared.
fn private_entry(provider: &str, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if ["auth", "credential", "token", "keychain", "oauth"]
        .iter()
        .any(|part| lower.contains(part))
    {
        return true;
    }
    match provider {
        // The app-server daemon runs under whichever login started it, so a
        // second account must never attach to the first one's.
        "codex" => matches!(
            name,
            "app-server-control"
                | "app-server-daemon"
                | "installation_id"
                | "models_cache.json"
                | "cache"
        ),
        // `.claude.json` holds the signed-in account; `backups` holds copies of
        // it that Claude restores from.
        _ => {
            matches!(
                name,
                ".claude.json" | "backups" | "statsig" | "cache" | ".last-cleanup"
            ) || lower.starts_with(".claude.json")
        }
    }
}

/// Directories every account's sessions are written to, created in the user's
/// home first so they are shared rather than started afresh in an account home.
fn shared_directories(provider: &str) -> &'static [&'static str] {
    match provider {
        "codex" => &["sessions", "archived_sessions"],
        _ => &["projects", "todos"],
    }
}

/// Settings in Claude's `.claude.json` that are the user's rather than the
/// account's, copied from the user's own file into each account's before a run.
const CLAUDE_SHARED_SETTINGS: [&str; 9] = [
    "mcpServers",
    "projects",
    "hasCompletedOnboarding",
    "lastOnboardingVersion",
    "theme",
    "installMethod",
    "autoUpdates",
    "editorMode",
    "bypassPermissionsModeAccepted",
];

pub(crate) fn provider_name(provider: &str) -> &'static str {
    match provider {
        "claude" => "Claude Code",
        _ => "Codex",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Headroom {
    pub kind: String,
    pub label: String,
    pub used_percent: f64,
    pub resets_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Live {
    windows: Vec<Headroom>,
    checked_at_ms: Option<i64>,
    /// When the login alone was last read, before a Codex session was resumed.
    login_read_at_ms: Option<i64>,
    /// When the newest reading of the login began, by probe or on its own.
    identity_at_ms: Option<i64>,
    /// When a turn last ran into a limit. Only a reading begun after it can
    /// lift the hold early.
    limited_at_ms: Option<i64>,
    /// Set when a turn was turned away for usage, until the stated reset.
    limited_until_ms: Option<i64>,
    limited_label: Option<String>,
    /// Why the account is held back even though it is signed in.
    problem: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
struct Account {
    id: String,
    email: Option<String>,
    plan: Option<String>,
    account_key: Option<String>,
    added_at_ms: i64,
    live: Live,
}

impl Account {
    fn new(id: &str, now: i64) -> Self {
        Self {
            id: id.into(),
            email: None,
            plan: None,
            account_key: None,
            added_at_ms: now,
            live: Live::default(),
        }
    }

    /// Whether the latest reading shows every main limit well short of spent.
    fn has_clear_room(&self) -> bool {
        let main = self
            .live
            .windows
            .iter()
            .filter(|window| matches!(window.kind.as_str(), "session" | "weekly" | "monthly"))
            .collect::<Vec<_>>();
        !main.is_empty() && main.iter().all(|window| window.used_percent < LIFT_PERCENT)
    }

    /// When the account can next take a turn, if it cannot now.
    fn blocked_until(&self, now: i64) -> Option<(i64, String)> {
        let mut blocked: Option<(i64, String)> = None;
        let mut consider = |until: i64, label: &str| {
            if until > now && blocked.as_ref().is_none_or(|(at, _)| until > *at) {
                blocked = Some((until, label.to_owned()));
            }
        };
        if let Some(until) = self.live.limited_until_ms {
            consider(
                until,
                self.live.limited_label.as_deref().unwrap_or("usage limit"),
            );
        }
        let fresh = self
            .live
            .checked_at_ms
            .is_some_and(|checked| now - checked < STALE_PROBE_MS);
        if fresh {
            for window in &self.live.windows {
                if window.used_percent < SPENT_PERCENT
                    || !matches!(window.kind.as_str(), "session" | "weekly" | "monthly")
                {
                    continue;
                }
                let label = limit_name(&window.kind, &window.label);
                match window.resets_at_ms {
                    Some(at) => consider(at, &label),
                    None => consider(
                        self.live.checked_at_ms.unwrap_or(now) + UNTIMED_LIMIT_MS,
                        &label,
                    ),
                }
            }
        }
        blocked
    }
}

/// How a spent window reads in a sentence: "5-hour limit", "weekly limit".
pub(crate) fn limit_name(kind: &str, label: &str) -> String {
    match kind {
        "session" => "5-hour limit".into(),
        "weekly" if label.contains('·') => format!("{} limit", label.to_lowercase()),
        "weekly" => "weekly limit".into(),
        "monthly" => "monthly limit".into(),
        _ => "usage limit".into(),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Provider {
    auto_switch: bool,
    /// In priority order. Always holds the default account.
    accounts: Vec<Account>,
    /// An account being signed in for the first time. It joins `accounts`
    /// once the CLI reports success and its identity checks out.
    pending: Option<Account>,
}

impl Provider {
    fn new(now: i64) -> Self {
        Self {
            auto_switch: true,
            accounts: vec![Account::new(DEFAULT_ACCOUNT, now)],
            pending: None,
        }
    }

    fn account(&self, id: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .chain(self.pending.as_ref())
            .find(|account| account.id == id)
    }

    fn account_mut(&mut self, id: &str) -> Option<&mut Account> {
        self.accounts
            .iter_mut()
            .chain(self.pending.as_mut())
            .find(|account| account.id == id)
    }
}

/// The account chosen for a turn, and what it takes to run on it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Choice {
    pub account: String,
    pub environment: Vec<(String, String)>,
    /// Why the chat's own account was passed over, when it was.
    pub passed_over: Option<PassedOver>,
    /// Every usable account is out of usage; this is the soonest back.
    pub all_spent_until: Option<i64>,
    /// No account can take the turn: each is signed out or held back for its
    /// login. Only said when there are several; a lone sign-in runs as before
    /// and the CLI says what is wrong.
    pub unavailable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PassedOver {
    pub from: String,
    pub reason: String,
    pub resets_at_ms: Option<i64>,
}

#[derive(Clone)]
pub(crate) struct Accounts {
    inner: Arc<Inner>,
}

struct Inner {
    /// `<data>/accounts`; `None` keeps the host to the default accounts.
    directory: Option<PathBuf>,
    providers: Mutex<HashMap<&'static str, Provider>>,
    /// Where the user's own provider homes are. Tests point these elsewhere.
    homes: Mutex<HashMap<&'static str, PathBuf>>,
}

impl Accounts {
    pub(crate) fn new(data_directory: Option<&Path>) -> Self {
        let directory = data_directory.map(|data| data.join("accounts"));
        let now = now_ms();
        let mut providers: HashMap<&'static str, Provider> = PROVIDERS
            .iter()
            .map(|provider| (*provider, Provider::new(now)))
            .collect();
        if let Some(stored) = directory
            .as_ref()
            .and_then(|directory| fs::read(directory.join(FILE)).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        {
            for provider in PROVIDERS {
                if let Some(loaded) = load_provider(provider, &stored["providers"][provider], now) {
                    let accounts = loaded
                        .accounts
                        .into_iter()
                        .filter(|account| {
                            account.id == DEFAULT_ACCOUNT
                                || directory.as_ref().is_some_and(|directory| {
                                    directory.join(provider).join(&account.id).is_dir()
                                })
                        })
                        .collect();
                    providers.insert(provider, Provider { accounts, ..loaded });
                }
            }
        }
        Self {
            inner: Arc::new(Inner {
                directory,
                providers: Mutex::new(providers),
                homes: Mutex::new(HashMap::new()),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_homes(data_directory: &Path, homes: &[(&'static str, PathBuf)]) -> Self {
        let accounts = Self::new(Some(data_directory));
        if let Ok(mut known) = accounts.inner.homes.lock() {
            known.clear();
            known.extend(homes.iter().cloned());
            // A provider with no home given has none, rather than the tester's.
            for provider in PROVIDERS {
                known
                    .entry(provider)
                    .or_insert_with(|| data_directory.join("no-such-home").join(provider));
            }
        }
        accounts
    }

    pub(crate) fn supports_more(&self) -> bool {
        self.inner.directory.is_some()
    }

    /// Account ids in priority order, the default included.
    pub(crate) fn ids(&self, provider: &str) -> Vec<String> {
        self.with(provider, |provider| {
            provider
                .accounts
                .iter()
                .map(|account| account.id.clone())
                .collect()
        })
        .unwrap_or_else(|| vec![DEFAULT_ACCOUNT.into()])
    }

    pub(crate) fn pending(&self, provider: &str) -> Option<String> {
        self.with(provider, |provider| {
            provider.pending.as_ref().map(|account| account.id.clone())
        })
        .flatten()
    }

    pub(crate) fn exists(&self, provider: &str, id: &str) -> bool {
        self.with(provider, |provider| provider.account(id).is_some())
            .unwrap_or(false)
    }

    /// What a chat's session is filed under: the account, and the login it was
    /// signed in to when known. The user's own sign-in can change login under
    /// the same account, and a Codex session must not follow it there.
    pub(crate) fn session_tag(&self, provider: &str, id: &str) -> String {
        self.with(provider, |state| {
            state
                .account(id)
                .and_then(|account| account.account_key.clone())
        })
        .flatten()
        .map_or_else(|| id.to_owned(), |key| format!("{id}#{key}"))
    }

    /// When the account's login was last read, by a usage probe or on its own.
    pub(crate) fn login_read_at(&self, provider: &str, id: &str) -> Option<i64> {
        self.with(provider, |state| {
            state.account(id).and_then(|account| {
                account
                    .live
                    .login_read_at_ms
                    .max(account.live.checked_at_ms)
            })
        })
        .flatten()
    }

    /// Takes in a login read on its own, without limits.
    pub(crate) fn record_login(
        &self,
        provider: &str,
        id: &str,
        identity: &Identity,
        read_at_ms: i64,
    ) {
        let _ = self.update(provider, |state| {
            if state.account(id).is_some_and(|account| {
                account
                    .live
                    .identity_at_ms
                    .is_some_and(|newer| newer > read_at_ms)
            }) {
                return Ok(());
            }
            if let Some(account) = state.account_mut(id) {
                account.live.identity_at_ms = Some(read_at_ms);
            }
            let position = |id: &str| state.accounts.iter().position(|account| account.id == id);
            let duplicate = identity.account_key.as_ref().is_some_and(|key| {
                state.accounts.iter().any(|other| {
                    other.id != id
                        && other.account_key.as_ref() == Some(key)
                        && position(&other.id) < position(id)
                })
            });
            let Some(account) = state.account_mut(id) else {
                return Ok(());
            };
            let changed = matches!(
                (&account.account_key, &identity.account_key),
                (Some(known), Some(found)) if known != found
            );
            account.live.problem = if changed && id != DEFAULT_ACCOUNT {
                Some("mismatch")
            } else if duplicate {
                Some("duplicate")
            } else {
                None
            };
            if id == DEFAULT_ACCOUNT || account.account_key.is_none() {
                account.account_key = identity.account_key.clone().or(account.account_key.take());
                if identity.email.is_some() {
                    account.email = identity.email.clone();
                }
            }
            account.live.login_read_at_ms = Some(read_at_ms);
            Ok(())
        });
    }

    /// Forgets when the account's login was read, after it signed in or out:
    /// the reading may be of the login it had before.
    pub(crate) fn forget_login_reading(&self, provider: &str, id: &str) {
        let _ = self.update(provider, |state| {
            if let Some(account) = state.account_mut(id) {
                account.live.login_read_at_ms = None;
                account.live.checked_at_ms = None;
                // A reading begun before now may be of the old login.
                account.live.identity_at_ms = Some(now_ms());
            }
            Ok(())
        });
    }

    /// The account last seen signed in as this login.
    pub(crate) fn with_key(&self, provider: &str, account_key: &str) -> Option<String> {
        self.with(provider, |state| {
            state
                .accounts
                .iter()
                .find(|account| account.account_key.as_deref() == Some(account_key))
                .map(|account| account.id.clone())
        })
        .flatten()
    }

    pub(crate) fn has_several(&self, provider: &str) -> bool {
        self.with(provider, |provider| provider.accounts.len() > 1)
            .unwrap_or(false)
    }

    pub(crate) fn label(&self, provider: &str, id: &str) -> String {
        self.with(provider, |state| {
            state
                .account(id)
                .map(|account| account_label(provider, account, state))
        })
        .flatten()
        .unwrap_or_else(|| format!("{} account", provider_name(provider)))
    }

    fn with<T>(&self, provider: &str, read: impl FnOnce(&Provider) -> T) -> Option<T> {
        let providers = self.inner.providers.lock().ok()?;
        providers.get(provider).map(read)
    }

    fn update<T>(
        &self,
        provider: &str,
        change: impl FnOnce(&mut Provider) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut providers = self
            .inner
            .providers
            .lock()
            .map_err(|_| "Accounts are unavailable.".to_owned())?;
        let state = providers
            .get_mut(provider)
            .ok_or_else(|| "No such assistant.".to_owned())?;
        let before = state.clone();
        let result = change(state)?;
        // Saved under the lock, so saves land in the order changes were made.
        if persisted(&before) != persisted(state) {
            self.save(&providers)?;
        }
        Ok(result)
    }

    fn save(&self, providers: &HashMap<&'static str, Provider>) -> Result<(), String> {
        let Some(directory) = &self.inner.directory else {
            return Ok(());
        };
        let value = json!({
            "version": 1,
            "providers": PROVIDERS.iter().filter_map(|provider| {
                providers.get(provider).map(|state| (provider.to_string(), persisted(state)))
            }).collect::<serde_json::Map<_, _>>(),
        });
        write_private(directory, &directory.join(FILE), &value)
    }

    /// The private home of an added account. The default has none: it runs
    /// in the user's own.
    pub(crate) fn home(&self, provider: &str, id: &str) -> Option<PathBuf> {
        if id == DEFAULT_ACCOUNT || !valid_id(id) || !PROVIDERS.contains(&provider) {
            return None;
        }
        Some(self.inner.directory.as_ref()?.join(provider).join(id))
    }

    /// What a process needs in its environment to run as this account. Links
    /// in anything new in the user's home first.
    pub(crate) fn environment(&self, provider: &str, id: &str) -> Vec<(String, String)> {
        let Some(home) = self.home(provider, id) else {
            return Vec::new();
        };
        // Found on first use: it waits on the user's login shell.
        let source = self.inner.homes.lock().ok().and_then(|mut homes| {
            let provider = PROVIDERS.iter().find(|known| **known == provider)?;
            if !homes.contains_key(provider) {
                homes.insert(provider, user_home(provider)?);
            }
            homes.get(provider).cloned()
        });
        if let Some(source) = source
            && let Err(error) = link_home(provider, &source, &home)
        {
            eprintln!("xd: cannot prepare the {provider} account home: {error}");
        }
        let variable = match provider {
            "claude" => "CLAUDE_CONFIG_DIR",
            _ => "CODEX_HOME",
        };
        vec![(variable.into(), home.to_string_lossy().into_owned())]
    }

    /// Copies the user's own Claude settings (MCP servers, projects) into an
    /// added account's `.claude.json`. Claude writes that file too, with no lock
    /// xd can take, so callers only do this while no Claude process runs as the
    /// account.
    pub(crate) fn share_settings(&self, provider: &str, id: &str) {
        if provider != "claude" {
            return;
        }
        let Some(home) = self.home(provider, id) else {
            return;
        };
        let source = self
            .inner
            .homes
            .lock()
            .ok()
            .and_then(|homes| homes.get("claude").cloned())
            .or_else(|| user_home("claude"));
        if let Some(source) = source
            && let Err(error) =
                sync_claude_settings(&user_claude_settings(&source), &home.join(".claude.json"))
        {
            eprintln!("xd: cannot share settings with the Claude account: {error}");
        }
    }

    /// Makes a home for an account that is about to sign in for the first time.
    pub(crate) fn begin_adding(&self, provider: &str) -> Result<String, String> {
        if self.inner.directory.is_none() {
            return Err("This host cannot keep more than one account.".into());
        }
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
        let home = self
            .home(provider, &id)
            .ok_or_else(|| "No such assistant.".to_owned())?;
        self.update(provider, |state| {
            if state.accounts.len() >= MAX_ACCOUNTS {
                return Err(format!(
                    "{} can keep at most {MAX_ACCOUNTS} accounts.",
                    provider_name(provider)
                ));
            }
            if state.pending.is_some() {
                return Err(format!(
                    "Another {} account is already signing in.",
                    provider_name(provider)
                ));
            }
            state.pending = Some(Account::new(&id, now_ms()));
            Ok(())
        })?;
        let created = fs::create_dir_all(&home)
            .and_then(|()| crate::private_fs::secure_directory(&home))
            .and_then(|()| {
                home.parent()
                    .map_or(Ok(()), crate::private_fs::secure_directory)
            })
            .and_then(|()| {
                self.inner
                    .directory
                    .as_deref()
                    .map_or(Ok(()), crate::private_fs::secure_directory)
            });
        if let Err(error) = created {
            self.forget(provider, &id);
            return Err(format!("Cannot make a home for the account: {error}"));
        }
        Ok(id)
    }

    /// Takes a newly signed-in account into the list, unless it is one already
    /// there. Returns the label of the account it duplicates when it is.
    pub(crate) fn finish_adding(
        &self,
        provider: &str,
        id: &str,
        identity: &Identity,
    ) -> Result<Result<(), String>, String> {
        self.update(provider, |state| {
            let Some(mut account) = state.pending.take().filter(|pending| pending.id == id) else {
                return Err("That sign-in is no longer pending.".into());
            };
            if let Some(key) = &identity.account_key
                && let Some(existing) = state
                    .accounts
                    .iter()
                    .find(|other| other.account_key.as_ref() == Some(key))
            {
                return Ok(Err(account_label(provider, existing, state)));
            }
            account.email = identity.email.clone();
            account.plan = identity.plan.clone();
            account.account_key = identity.account_key.clone();
            state.accounts.push(account);
            Ok(Ok(()))
        })
    }

    /// Drops an added account and deletes its home, credentials included.
    /// Links in the home are removed, never followed.
    pub(crate) fn forget(&self, provider: &str, id: &str) {
        if id == DEFAULT_ACCOUNT {
            return;
        }
        let _ = self.update(provider, |state| {
            state.accounts.retain(|account| account.id != id);
            if state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.id == id)
            {
                state.pending = None;
            }
            Ok(())
        });
        if let Some(home) = self.home(provider, id)
            && let Err(error) = fs::remove_dir_all(&home)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!("xd: cannot remove {home:?}: {error}");
        }
    }

    pub(crate) fn reorder(&self, provider: &str, order: &[String]) -> Result<(), String> {
        self.update(provider, |state| {
            let mut sorted: Vec<_> = order.iter().map(String::as_str).collect();
            sorted.sort_unstable();
            sorted.dedup();
            let mut known: Vec<_> = state.accounts.iter().map(|a| a.id.as_str()).collect();
            known.sort_unstable();
            if sorted.len() != order.len() || sorted != known {
                return Err("The account order must name each account once.".into());
            }
            let mut accounts = std::mem::take(&mut state.accounts);
            state.accounts = order
                .iter()
                .filter_map(|id| {
                    accounts
                        .iter()
                        .position(|account| &account.id == id)
                        .map(|index| accounts.swap_remove(index))
                })
                .collect();
            Ok(())
        })
    }

    pub(crate) fn set_auto_switch(&self, provider: &str, enabled: bool) -> Result<(), String> {
        self.update(provider, |state| {
            state.auto_switch = enabled;
            Ok(())
        })
    }

    pub(crate) fn auto_switch(&self, provider: &str) -> bool {
        self.with(provider, |state| state.auto_switch)
            .unwrap_or(false)
    }

    /// Takes in what a usage probe of the account found. Checks that the
    /// account is still the one it was added as, and that no two accounts are
    /// the same login.
    pub(crate) fn record_probe(
        &self,
        provider: &str,
        id: &str,
        identity: &Identity,
        windows: Vec<Headroom>,
        checked_at_ms: i64,
    ) {
        let _ = self.update(provider, |state| {
            // Readings land in the order they were taken: one that began
            // before a newer reading of this login must not undo it.
            if state.account(id).is_some_and(|account| {
                account
                    .live
                    .identity_at_ms
                    .is_some_and(|newer| newer > checked_at_ms)
            }) {
                return Ok(());
            }
            if let Some(account) = state.account_mut(id) {
                account.live.identity_at_ms = Some(checked_at_ms);
            }
            let duplicate = identity.account_key.as_ref().is_some_and(|key| {
                state.accounts.iter().any(|other| {
                    other.id != id
                        && other.account_key.as_ref() == Some(key)
                        // The earlier account in the list keeps the login.
                        && state
                            .accounts
                            .iter()
                            .position(|account| account.id == other.id)
                            < state.accounts.iter().position(|account| account.id == id)
                })
            });
            let Some(account) = state.account_mut(id) else {
                return Ok(());
            };
            let changed = match (&account.account_key, &identity.account_key) {
                (Some(known), Some(found)) => known != found,
                _ => false,
            };
            account.live.problem = if changed && id != DEFAULT_ACCOUNT {
                Some("mismatch")
            } else if duplicate {
                Some("duplicate")
            } else {
                None
            };
            // The default account is whatever the user's CLI is signed in to.
            if id == DEFAULT_ACCOUNT || account.account_key.is_none() {
                account.account_key = identity.account_key.clone().or(account.account_key.take());
            }
            if !changed || id == DEFAULT_ACCOUNT {
                if identity.email.is_some() {
                    account.email = identity.email.clone();
                }
                if identity.plan.is_some() {
                    account.plan = identity.plan.clone();
                }
                account.live.windows = windows;
                account.live.checked_at_ms = Some(checked_at_ms);
                // A fresh reading with clear room left lifts a limit a turn hit,
                // such as after a reset credit. Near the limit it stays held: a
                // reading can lag the limit that just turned a turn away.
                let read_after_limit = account
                    .live
                    .limited_at_ms
                    .is_none_or(|limited| checked_at_ms > limited);
                if account
                    .live
                    .limited_until_ms
                    .is_none_or(|until| until <= checked_at_ms)
                    || (read_after_limit && account.has_clear_room())
                {
                    account.live.limited_until_ms = None;
                    account.live.limited_label = None;
                }
            }
            Ok(())
        });
    }

    /// A turn on this account was turned away for usage.
    pub(crate) fn note_limited(
        &self,
        provider: &str,
        id: &str,
        resets_at_ms: Option<i64>,
        label: Option<&str>,
        now: i64,
    ) {
        let _ = self.update(provider, |state| {
            if let Some(account) = state.account_mut(id) {
                let until = resets_at_ms
                    .filter(|at| *at > now)
                    .unwrap_or(now + UNTIMED_LIMIT_MS);
                account.live.limited_until_ms = Some(until);
                account.live.limited_label = label.map(str::to_owned);
                account.live.limited_at_ms = Some(now);
            }
            Ok(())
        });
    }

    /// Picks the account a chat's next turn runs on.
    ///
    /// A chat stays on the account its session belongs to while that account
    /// can take turns, which keeps the session (and, for Codex, its reasoning)
    /// intact. Otherwise the first account in the list that can take a turn
    /// is used. With switching off, the first usable account always is.
    pub(crate) fn choose(
        &self,
        provider: &str,
        chat_account: Option<&str>,
        signed_in: impl Fn(&str) -> bool,
        now: i64,
    ) -> Choice {
        let decided = self
            .with(provider, |state| {
                choose(state, chat_account, &signed_in, now).map(|mut choice| {
                    if let Some(passed) = choice.passed_over.as_mut() {
                        passed.from = state
                            .account(&passed.from)
                            .map(|account| account_label(provider, account, state))
                            .unwrap_or_else(|| passed.from.clone());
                    }
                    choice
                })
            })
            .flatten();
        let mut choice = decided.unwrap_or(Choice {
            account: DEFAULT_ACCOUNT.into(),
            environment: Vec::new(),
            passed_over: None,
            all_spent_until: None,
            unavailable: self.has_several(provider),
        });
        choice.environment = self.environment(provider, &choice.account);
        choice
    }

    /// The accounts as clients show them, without any sign-in state.
    pub(crate) fn describe(&self, provider: &str, now: i64) -> Vec<Value> {
        self.with(provider, |state| {
            state
                .accounts
                .iter()
                .chain(state.pending.as_ref())
                .map(|account| {
                    let blocked = account.blocked_until(now);
                    json!({
                        "id": account.id,
                        "default": account.id == DEFAULT_ACCOUNT,
                        "pending": state.pending.as_ref().is_some_and(|pending| pending.id == account.id),
                        "label": account_label(provider, account, state),
                        "email": account.email,
                        "plan": account.plan,
                        "problem": account.live.problem,
                        "limited_until_ms": blocked.as_ref().map(|(until, _)| until),
                        "limited_reason": blocked.map(|(_, label)| label),
                        "checked_at_ms": account.live.checked_at_ms,
                        "windows": account.live.windows.iter().map(|window| json!({
                            "kind": window.kind,
                            "label": window.label,
                            "used_percent": window.used_percent,
                            "resets_at_ms": window.resets_at_ms,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
    }
}

/// The account part of a session tag; see [`Accounts::session_tag`].
pub(crate) fn tag_account(tag: &str) -> &str {
    tag.split_once('#').map_or(tag, |(id, _)| id)
}

/// Whether a session filed under `stored` may be opened as `current`: the same
/// account, and not known to be a different login. A tag from before logins
/// were recorded names no login and matches any.
pub(crate) fn same_login(stored: &str, current: &str) -> bool {
    let split = |tag: &str| {
        let (id, key) = tag
            .split_once('#')
            .map_or((tag, None), |(id, key)| (id, Some(key)));
        (id.to_owned(), key.map(str::to_owned))
    };
    let ((stored_id, stored_key), (current_id, current_key)) = (split(stored), split(current));
    stored_id == current_id
        && match (stored_key, current_key) {
            (Some(stored), Some(current)) => stored == current,
            _ => true,
        }
}

/// Who a probe found signed in.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Identity {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub account_key: Option<String>,
}

fn choose(
    state: &Provider,
    chat_account: Option<&str>,
    signed_in: &impl Fn(&str) -> bool,
    now: i64,
) -> Option<Choice> {
    let usable = |account: &&Account| account.live.problem.is_none() && signed_in(&account.id);
    let choice = |account: &Account| Choice {
        account: account.id.clone(),
        environment: Vec::new(),
        passed_over: None,
        all_spent_until: None,
        unavailable: false,
    };
    let first_usable = state.accounts.iter().find(usable);
    if !state.auto_switch || state.accounts.len() < 2 {
        return first_usable.map(choice);
    }
    // Only a chat with a session keeps to an account; a new one starts on the
    // first account in the list that can take it.
    let own = chat_account.and_then(|id| state.accounts.iter().find(|account| account.id == id));
    if let Some(own) = own.filter(usable)
        && own.blocked_until(now).is_none()
    {
        return Some(choice(own));
    }
    let passed_over = own.map(|own| {
        let blocked = own.blocked_until(now);
        PassedOver {
            from: own.id.clone(),
            reason: match (&blocked, own.live.problem) {
                (Some((_, label)), _) => format!("reached its {label}"),
                (None, Some("mismatch")) => "is now signed in to a different account".into(),
                (None, Some(_)) => "is the same login as another account".into(),
                (None, None) => "is signed out".into(),
            },
            resets_at_ms: blocked.map(|(until, _)| until),
        }
    });
    if let Some(open) = state
        .accounts
        .iter()
        .filter(usable)
        .find(|account| account.blocked_until(now).is_none())
    {
        let mut chosen = choice(open);
        chosen.passed_over = passed_over.filter(|passed| passed.from != chosen.account);
        return Some(chosen);
    }
    // Every usable account is spent. Stay where the chat is, so the CLI says
    // so in its own words, and report when the first one is back.
    let soonest = state
        .accounts
        .iter()
        .filter(usable)
        .filter_map(|account| account.blocked_until(now).map(|(until, _)| until))
        .min();
    let stay = own.filter(usable).or(first_usable)?;
    let mut chosen = choice(stay);
    chosen.all_spent_until = soonest;
    Some(chosen)
}

fn account_label(provider: &str, account: &Account, state: &Provider) -> String {
    if let Some(email) = &account.email {
        return email.clone();
    }
    if account.id == DEFAULT_ACCOUNT {
        return format!("{} sign-in", provider_name(provider));
    }
    let position = state
        .accounts
        .iter()
        .position(|other| other.id == account.id)
        .unwrap_or(state.accounts.len());
    format!("{} account {}", provider_name(provider), position + 1)
}

fn persisted(state: &Provider) -> Value {
    json!({
        "auto_switch": state.auto_switch,
        "accounts": state.accounts.iter().map(|account| json!({
            "id": account.id,
            "email": account.email,
            "plan": account.plan,
            "account_key": account.account_key,
            "added_at_ms": account.added_at_ms,
        })).collect::<Vec<_>>(),
    })
}

fn load_provider(provider: &str, value: &Value, now: i64) -> Option<Provider> {
    let entries = value["accounts"].as_array()?;
    let mut accounts = Vec::new();
    for entry in entries {
        let Some(id) = entry["id"].as_str().filter(|id| valid_id(id)) else {
            continue;
        };
        if accounts.iter().any(|account: &Account| account.id == id) {
            continue;
        }
        let text = |name: &str| entry[name].as_str().map(str::to_owned);
        accounts.push(Account {
            id: id.into(),
            email: text("email"),
            plan: text("plan"),
            account_key: text("account_key"),
            added_at_ms: entry["added_at_ms"].as_i64().unwrap_or(now),
            live: Live::default(),
        });
        if accounts.len() >= MAX_ACCOUNTS {
            break;
        }
    }
    if !accounts.iter().any(|account| account.id == DEFAULT_ACCOUNT) {
        accounts.insert(0, Account::new(DEFAULT_ACCOUNT, now));
    }
    let _ = provider;
    Some(Provider {
        auto_switch: value["auto_switch"].as_bool().unwrap_or(true),
        accounts,
        pending: None,
    })
}

fn valid_id(id: &str) -> bool {
    id == DEFAULT_ACCOUNT
        || (id.len() == 12
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}

/// The provider home the user's own CLI uses.
fn user_home(provider: &str) -> Option<PathBuf> {
    let values = crate::user_environment::values();
    let lookup = |name: &str| {
        values
            .get(name)
            .cloned()
            .or_else(|| std::env::var_os(name))
            .filter(|value| !value.is_empty())
    };
    let (variable, directory) = match provider {
        "claude" => ("CLAUDE_CONFIG_DIR", ".claude"),
        _ => ("CODEX_HOME", ".codex"),
    };
    lookup(variable)
        .map(PathBuf::from)
        .or_else(|| lookup("HOME").map(|home| PathBuf::from(home).join(directory)))
}

/// Claude keeps `.claude.json` beside its home when no home is configured.
fn user_claude_settings(home: &Path) -> PathBuf {
    let configured = crate::user_environment::values()
        .get("CLAUDE_CONFIG_DIR")
        .cloned()
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR"))
        .is_some_and(|value| !value.is_empty());
    if configured {
        home.join(".claude.json")
    } else {
        home.parent().map_or_else(
            || home.join(".claude.json"),
            |parent| parent.join(".claude.json"),
        )
    }
}

/// Links the user's own provider home into an account's, leaving out what is
/// private to a sign-in. An entry the account's CLI made for itself is kept.
fn link_home(provider: &str, source: &Path, home: &Path) -> io::Result<()> {
    fs::create_dir_all(home)?;
    crate::private_fs::secure_directory(home)?;
    if !source.is_dir() {
        return Ok(());
    }
    for directory in shared_directories(provider) {
        let shared = source.join(directory);
        if !shared.exists() {
            fs::create_dir_all(&shared)?;
        }
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(text) = name.to_str() else { continue };
        if private_entry(provider, text) {
            continue;
        }
        let target = home.join(&name);
        let wanted = entry.path();
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                std::os::unix::fs::symlink(&wanted, &target)?;
            }
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if fs::read_link(&target)? != wanted {
                    fs::remove_file(&target)?;
                    std::os::unix::fs::symlink(&wanted, &target)?;
                }
            }
            Ok(_) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Copies the user's own settings into an account's `.claude.json`, keeping
/// everything there that belongs to the account's sign-in.
fn sync_claude_settings(source: &Path, target: &Path) -> io::Result<()> {
    // One sync at a time, so two turns starting together cannot undo each other.
    static SYNC: Mutex<()> = Mutex::new(());
    let _sync = SYNC
        .lock()
        .map_err(|_| io::Error::other("settings sync is unavailable"))?;
    let Ok(bytes) = fs::read(source) else {
        return Ok(());
    };
    let Ok(Value::Object(user)) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(());
    };
    // Claude writes this file too. If it changed while the merge was made, the
    // merge is made again from what Claude wrote, rather than undo it.
    for _ in 0..3 {
        let before = fs::read(target).ok();
        let current = before
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
        let mut merged = match &current {
            Some(Value::Object(account)) => account.clone(),
            _ => serde_json::Map::new(),
        };
        for key in CLAUDE_SHARED_SETTINGS {
            match user.get(key) {
                Some(value) => merged.insert(key.into(), value.clone()),
                None => merged.remove(key),
            };
        }
        let merged = Value::Object(merged);
        if current.as_ref() == Some(&merged) {
            return Ok(());
        }
        if fs::read(target).ok() != before {
            continue;
        }
        let parent = target.parent().unwrap_or(Path::new("."));
        return write_private(parent, target, &merged).map_err(io::Error::other);
    }
    Ok(())
}

fn write_private(directory: &Path, path: &Path, value: &Value) -> Result<(), String> {
    use std::io::Write;
    fs::create_dir_all(directory)
        .and_then(|()| crate::private_fs::secure_directory(directory))
        .map_err(|error| format!("Cannot create {directory:?}: {error}"))?;
    let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = crate::private_fs::create_private_file(&temporary)?;
        file.write_all(value.to_string().as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Cannot save {path:?}: {error}"));
    }
    Ok(())
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "xd-accounts-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn identity(email: &str) -> Identity {
        Identity {
            email: Some(email.into()),
            plan: Some("Pro".into()),
            account_key: Some(format!("key-{email}")),
        }
    }

    fn spent(kind: &str, resets_at_ms: i64) -> Headroom {
        Headroom {
            kind: kind.into(),
            label: "Session".into(),
            used_percent: 100.0,
            resets_at_ms: Some(resets_at_ms),
        }
    }

    fn added(accounts: &Accounts, provider: &str, email: &str) -> String {
        let id = accounts.begin_adding(provider).unwrap();
        accounts
            .finish_adding(provider, &id, &identity(email))
            .unwrap()
            .unwrap();
        id
    }

    #[test]
    fn a_session_is_filed_under_its_account_and_login() {
        let data = temp("tags");
        let accounts = Accounts::with_homes(&data, &[]);
        assert_eq!(accounts.session_tag("codex", DEFAULT_ACCOUNT), "default");
        accounts.record_probe(
            "codex",
            DEFAULT_ACCOUNT,
            &identity("a@example.com"),
            vec![],
            0,
        );
        let tag = accounts.session_tag("codex", DEFAULT_ACCOUNT);
        assert_eq!(tag, "default#key-a@example.com");
        assert_eq!(tag_account(&tag), DEFAULT_ACCOUNT);
        assert!(
            same_login("default", &tag),
            "a session from before logins were kept"
        );
        assert!(same_login(&tag, &tag));
        // The user's own CLI signed in to another login: the session stays put.
        accounts.record_probe(
            "codex",
            DEFAULT_ACCOUNT,
            &identity("b@example.com"),
            vec![],
            1,
        );
        assert!(!same_login(
            &tag,
            &accounts.session_tag("codex", DEFAULT_ACCOUNT)
        ));
        assert!(!same_login(&tag, "a1b2c3d4e5f6#key-a@example.com"));
    }

    #[test]
    fn a_login_read_on_its_own_moves_the_session_tag_and_flags_a_changed_account() {
        let data = temp("login");
        let accounts = Accounts::with_homes(&data, &[]);
        let second = added(&accounts, "codex", "b@example.com");
        assert_eq!(accounts.login_read_at("codex", DEFAULT_ACCOUNT), None);
        accounts.record_login("codex", DEFAULT_ACCOUNT, &identity("a@example.com"), 7);
        assert_eq!(accounts.login_read_at("codex", DEFAULT_ACCOUNT), Some(7));
        assert_eq!(
            accounts.session_tag("codex", DEFAULT_ACCOUNT),
            "default#key-a@example.com"
        );
        accounts.record_login("codex", &second, &identity("c@example.com"), 8);
        assert_eq!(accounts.describe("codex", 9)[1]["problem"], "mismatch");
    }

    #[test]
    fn an_older_reading_never_undoes_a_newer_one() {
        let data = temp("order-of-readings");
        let accounts = Accounts::with_homes(&data, &[]);
        accounts.record_login("codex", DEFAULT_ACCOUNT, &identity("b@example.com"), 10);
        // A probe that began before that login read finishes after it.
        accounts.record_probe(
            "codex",
            DEFAULT_ACCOUNT,
            &identity("a@example.com"),
            vec![],
            5,
        );
        assert_eq!(
            accounts.session_tag("codex", DEFAULT_ACCOUNT),
            "default#key-b@example.com"
        );
        accounts.record_probe(
            "codex",
            DEFAULT_ACCOUNT,
            &identity("c@example.com"),
            vec![],
            11,
        );
        assert_eq!(
            accounts.session_tag("codex", DEFAULT_ACCOUNT),
            "default#key-c@example.com"
        );
    }

    #[test]
    fn a_new_chat_starts_on_the_first_account_in_the_list() {
        let data = temp("order");
        let accounts = Accounts::with_homes(&data, &[]);
        let second = added(&accounts, "codex", "b@example.com");
        accounts
            .reorder("codex", &[second.clone(), DEFAULT_ACCOUNT.into()])
            .unwrap();
        assert_eq!(accounts.choose("codex", None, |_| true, 0).account, second);
        assert_eq!(
            accounts
                .choose("codex", Some(DEFAULT_ACCOUNT), |_| true, 0)
                .account,
            DEFAULT_ACCOUNT,
            "a chat with a session keeps its account"
        );
    }

    #[test]
    fn a_single_account_runs_in_the_users_own_home() {
        let data = temp("single");
        let accounts = Accounts::with_homes(&data, &[]);
        let choice = accounts.choose("codex", None, |_| true, 0);
        assert_eq!(choice.account, DEFAULT_ACCOUNT);
        assert!(choice.environment.is_empty());
        assert_eq!(choice.passed_over, None);
    }

    #[test]
    fn a_spent_account_hands_new_turns_to_the_next_with_room() {
        let data = temp("switch");
        let accounts = Accounts::with_homes(&data, &[]);
        let second = added(&accounts, "claude", "b@example.com");
        let now = 10 * HOUR;
        accounts.record_probe(
            "claude",
            DEFAULT_ACCOUNT,
            &identity("a@example.com"),
            vec![spent("session", now + HOUR)],
            now,
        );
        let choice = accounts.choose("claude", Some(DEFAULT_ACCOUNT), |_| true, now);
        assert_eq!(choice.account, second);
        assert_eq!(
            choice.environment,
            vec![(
                "CLAUDE_CONFIG_DIR".into(),
                data.join("accounts/claude")
                    .join(&second)
                    .display()
                    .to_string()
            )]
        );
        let passed = choice.passed_over.unwrap();
        assert_eq!(passed.from, "a@example.com");
        assert_eq!(passed.reason, "reached its 5-hour limit");
        assert_eq!(passed.resets_at_ms, Some(now + HOUR));
        // Once the window resets, a chat on the second account stays there,
        // and a new chat starts on the first again.
        let later = now + 2 * HOUR;
        assert_eq!(
            accounts
                .choose("claude", Some(&second), |_| true, later)
                .account,
            second
        );
        assert_eq!(
            accounts.choose("claude", None, |_| true, later).account,
            DEFAULT_ACCOUNT
        );
    }

    #[test]
    fn a_weekly_limit_and_a_turned_away_turn_also_switch() {
        let data = temp("weekly");
        let accounts = Accounts::with_homes(&data, &[]);
        let second = added(&accounts, "codex", "b@example.com");
        let now = 100 * HOUR;
        accounts.record_probe(
            "codex",
            &second,
            &identity("b@example.com"),
            vec![spent("weekly", now + 48 * HOUR)],
            now,
        );
        let choice = accounts.choose("codex", Some(&second), |_| true, now);
        assert_eq!(choice.account, DEFAULT_ACCOUNT);
        assert_eq!(
            choice.passed_over.unwrap().reason,
            "reached its weekly limit"
        );
        accounts.note_limited("codex", DEFAULT_ACCOUNT, None, Some("usage limit"), now);
        let choice = accounts.choose("codex", Some(DEFAULT_ACCOUNT), |_| true, now);
        // Both are spent: stay put and say when the first comes back.
        assert_eq!(choice.account, DEFAULT_ACCOUNT);
        assert_eq!(choice.all_spent_until, Some(now + UNTIMED_LIMIT_MS));
    }

    #[test]
    fn a_reading_with_clear_room_lifts_a_limit_a_turn_ran_into() {
        let data = temp("lift");
        let accounts = Accounts::with_homes(&data, &[]);
        added(&accounts, "codex", "b@example.com");
        let reading = |used: f64| Headroom {
            kind: "session".into(),
            label: "Session".into(),
            used_percent: used,
            resets_at_ms: Some(10 * HOUR),
        };
        accounts.note_limited("codex", DEFAULT_ACCOUNT, Some(5 * HOUR), None, 0);
        // Near the limit: the reading may lag, so the hold stays.
        accounts.record_probe(
            "codex",
            DEFAULT_ACCOUNT,
            &identity("a@example.com"),
            vec![reading(97.0)],
            1,
        );
        assert!(accounts.describe("codex", 2)[0]["limited_until_ms"].is_i64());
        // A reset credit spent: clear room, so the account is back at once.
        accounts.record_probe(
            "codex",
            DEFAULT_ACCOUNT,
            &identity("a@example.com"),
            vec![reading(3.0)],
            3,
        );
        assert!(accounts.describe("codex", 4)[0]["limited_until_ms"].is_null());
        assert_eq!(
            accounts.choose("codex", None, |_| true, 4).account,
            DEFAULT_ACCOUNT
        );
    }

    #[test]
    fn a_reading_from_before_a_limit_does_not_lift_it() {
        let data = temp("lift-order");
        let accounts = Accounts::with_homes(&data, &[]);
        added(&accounts, "codex", "b@example.com");
        let room = vec![Headroom {
            kind: "session".into(),
            label: "Session".into(),
            used_percent: 10.0,
            resets_at_ms: Some(10 * HOUR),
        }];
        accounts.note_limited("codex", DEFAULT_ACCOUNT, Some(5 * HOUR), None, 100);
        // Began at 50, before the turn ran into the limit at 100.
        accounts.record_probe(
            "codex",
            DEFAULT_ACCOUNT,
            &identity("a@example.com"),
            room.clone(),
            50,
        );
        assert!(accounts.describe("codex", 200)[0]["limited_until_ms"].is_i64());
        accounts.record_probe(
            "codex",
            DEFAULT_ACCOUNT,
            &identity("a@example.com"),
            room,
            150,
        );
        assert!(accounts.describe("codex", 200)[0]["limited_until_ms"].is_null());
    }

    #[test]
    fn with_several_accounts_and_none_usable_the_turn_is_refused() {
        let data = temp("none");
        let accounts = Accounts::with_homes(&data, &[]);
        assert!(!accounts.choose("codex", None, |_| false, 0).unavailable);
        added(&accounts, "codex", "b@example.com");
        let choice = accounts.choose("codex", None, |_| false, 0);
        assert!(choice.unavailable);
    }

    #[test]
    fn switching_off_keeps_turns_on_the_first_signed_in_account() {
        let data = temp("manual");
        let accounts = Accounts::with_homes(&data, &[]);
        let second = added(&accounts, "codex", "b@example.com");
        accounts.set_auto_switch("codex", false).unwrap();
        accounts.note_limited("codex", DEFAULT_ACCOUNT, Some(5 * HOUR), None, 0);
        assert_eq!(
            accounts.choose("codex", Some(&second), |_| true, 0).account,
            DEFAULT_ACCOUNT
        );
        assert_eq!(
            accounts
                .choose("codex", None, |id| id != DEFAULT_ACCOUNT, 0)
                .account,
            second
        );
    }

    #[test]
    fn a_login_that_changed_or_repeats_another_is_never_used() {
        let data = temp("identity");
        let accounts = Accounts::with_homes(&data, &[]);
        accounts.record_probe(
            "claude",
            DEFAULT_ACCOUNT,
            &identity("a@example.com"),
            vec![],
            0,
        );
        let id = accounts.begin_adding("claude").unwrap();
        assert_eq!(
            accounts
                .finish_adding("claude", &id, &identity("a@example.com"))
                .unwrap(),
            Err("a@example.com".into()),
            "adding the same login twice is refused"
        );
        let second = added(&accounts, "claude", "b@example.com");
        accounts.record_probe("claude", &second, &identity("c@example.com"), vec![], 1);
        let choice = accounts.choose("claude", Some(&second), |_| true, 2);
        assert_eq!(choice.account, DEFAULT_ACCOUNT);
        assert_eq!(
            choice.passed_over.unwrap().reason,
            "is now signed in to a different account"
        );
        let described = accounts.describe("claude", 2);
        assert_eq!(described[1]["problem"], "mismatch");
        assert_eq!(described[1]["label"], "b@example.com");
    }

    #[test]
    fn accounts_and_their_order_survive_a_restart_and_removal_deletes_the_home() {
        let data = temp("persist");
        let accounts = Accounts::with_homes(&data, &[]);
        let second = added(&accounts, "codex", "b@example.com");
        accounts
            .reorder("codex", &[second.clone(), DEFAULT_ACCOUNT.into()])
            .unwrap();
        assert!(
            accounts
                .reorder("codex", std::slice::from_ref(&second))
                .is_err()
        );
        accounts.set_auto_switch("codex", false).unwrap();
        let reloaded = Accounts::with_homes(&data, &[]);
        assert_eq!(
            reloaded.ids("codex"),
            vec![second.clone(), DEFAULT_ACCOUNT.into()]
        );
        assert!(!reloaded.auto_switch("codex"));
        assert_eq!(reloaded.label("codex", &second), "b@example.com");
        let home = reloaded.home("codex", &second).unwrap();
        assert!(home.is_dir());
        reloaded.forget("codex", &second);
        assert!(!home.exists());
        assert_eq!(reloaded.ids("codex"), vec![DEFAULT_ACCOUNT.to_owned()]);
        assert!(reloaded.home("codex", "../escape").is_none());
    }

    #[test]
    fn account_homes_share_everything_but_the_sign_in() {
        let data = temp("link");
        let user = temp("user-codex");
        fs::write(user.join("auth.json"), "{\"token\":\"user\"}").unwrap();
        fs::write(user.join("config.toml"), "model = \"x\"").unwrap();
        fs::create_dir_all(user.join("skills")).unwrap();
        fs::create_dir_all(user.join("app-server-daemon")).unwrap();
        let accounts = Accounts::with_homes(&data, &[("codex", user.clone())]);
        let id = added(&accounts, "codex", "b@example.com");
        let environment = accounts.environment("codex", &id);
        let home = PathBuf::from(&environment[0].1);
        assert_eq!(environment[0].0, "CODEX_HOME");
        assert_eq!(
            fs::read_link(home.join("config.toml")).unwrap(),
            user.join("config.toml")
        );
        assert_eq!(
            fs::read_link(home.join("skills")).unwrap(),
            user.join("skills")
        );
        assert_eq!(
            fs::read_link(home.join("sessions")).unwrap(),
            user.join("sessions")
        );
        assert!(user.join("sessions").is_dir());
        assert!(fs::symlink_metadata(home.join("auth.json")).is_err());
        assert!(fs::symlink_metadata(home.join("app-server-daemon")).is_err());
        // The account's own sign-in is left alone, and removing the account
        // never reaches through a link into the user's home.
        fs::write(home.join("auth.json"), "{\"token\":\"b\"}").unwrap();
        accounts.environment("codex", &id);
        assert_eq!(
            fs::read_to_string(home.join("auth.json")).unwrap(),
            "{\"token\":\"b\"}"
        );
        accounts.forget("codex", &id);
        assert!(user.join("config.toml").is_file());
        assert!(user.join("skills").is_dir());
        assert_eq!(
            fs::read_to_string(user.join("auth.json")).unwrap(),
            "{\"token\":\"user\"}"
        );
    }

    #[test]
    fn claude_account_settings_follow_the_user_but_keep_their_own_login() {
        let directory = temp("claude-settings");
        let user = directory.join("user.json");
        let account = directory.join("account.json");
        fs::write(
            &user,
            json!({"mcpServers":{"lens":{}}, "oauthAccount":{"emailAddress":"a@example.com"}, "theme":"dark"}).to_string(),
        )
        .unwrap();
        fs::write(
            &account,
            json!({"oauthAccount":{"emailAddress":"b@example.com"}, "mcpServers":{"old":{}}})
                .to_string(),
        )
        .unwrap();
        sync_claude_settings(&user, &account).unwrap();
        let merged: Value = serde_json::from_slice(&fs::read(&account).unwrap()).unwrap();
        assert_eq!(merged["oauthAccount"]["emailAddress"], "b@example.com");
        assert_eq!(merged["mcpServers"], json!({"lens":{}}));
        assert_eq!(merged["theme"], "dark");
    }

    #[test]
    fn private_entries_cover_credentials_locks_and_daemons() {
        for name in [
            "auth.json",
            "auth.json.lock",
            "auth.lock",
            "app-server-daemon",
            "models_cache.json",
        ] {
            assert!(private_entry("codex", name), "{name}");
        }
        for name in [
            "config.toml",
            "sessions",
            "skills",
            "state_5.sqlite",
            "AGENTS.md",
        ] {
            assert!(!private_entry("codex", name), "{name}");
        }
        for name in [
            ".credentials.json",
            ".claude.json",
            ".claude.json.backup",
            "backups",
        ] {
            assert!(private_entry("claude", name), "{name}");
        }
        for name in [
            "settings.json",
            "projects",
            "CLAUDE.md",
            "skills",
            "plugins",
        ] {
            assert!(!private_entry("claude", name), "{name}");
        }
    }
}
