use std::{
    collections::HashMap,
    io::{Read, Write},
    process::{ChildStdin, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    EventBus,
    accounts::{Accounts, Choice, DEFAULT_ACCOUNT, PROVIDERS, now_ms, provider_name},
    agent::{resolve_claude, resolve_codex},
    background_process::command as background_command,
};

const OUTPUT_LIMIT: usize = 64 * 1024;
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// How often accounts are read again while an assistant has several, so a
/// spent one is passed over before a turn has to fail on it.
const WATCH_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Frees an account for removal: refuses while a chat is working on it, and
/// otherwise stops the agent processes kept running as it.
pub(crate) type Releaser = Box<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub struct AuthManager {
    inner: Arc<AuthInner>,
}

struct AuthInner {
    accounts: Accounts,
    /// By provider, then account.
    states: Mutex<HashMap<(String, String), AuthSnapshot>>,
    /// At most one sign-in per provider, so its code and input are unambiguous.
    sessions: Mutex<HashMap<String, AuthSession>>,
    /// The outcome of the last attempt to add an account, by provider.
    notices: Mutex<HashMap<String, String>>,
    /// Stops whatever runs as an account before it is removed; see
    /// [`AuthManager::set_releaser`].
    releaser: Mutex<Option<Releaser>>,
    next_session: AtomicU64,
    next_generation: AtomicU64,
    events: Arc<EventBus>,
}

#[derive(Clone, Default)]
struct AuthSnapshot {
    state: String,
    detail: Option<String>,
    login_url: Option<String>,
    device_code: Option<String>,
    needs_input: bool,
    /// Changes whenever the state is replaced, so a status check that started
    /// before a newer change cannot overwrite it.
    generation: u64,
    /// The last state a check or sign-in established, kept while a refresh is
    /// still checking so a known signed-out account is not used meanwhile.
    settled: Option<String>,
}

impl AuthSnapshot {
    fn unknown() -> Self {
        Self {
            state: "unknown".into(),
            ..Default::default()
        }
    }

    fn established(state: &str) -> bool {
        matches!(state, "signed-in" | "signed-out" | "failed")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SessionKind {
    SignIn,
    Add,
    SignOut,
    Remove,
}

struct AuthSession {
    serial: u64,
    account: String,
    kind: SessionKind,
    pid: Option<u32>,
    input: Option<Arc<Mutex<ChildStdin>>>,
    output: String,
    cancelled: bool,
}

fn key(provider: &str, account: &str) -> (String, String) {
    (provider.to_owned(), account.to_owned())
}

fn known_provider(provider: &str) -> Result<&'static str, String> {
    PROVIDERS
        .iter()
        .find(|known| **known == provider)
        .copied()
        .ok_or_else(|| "No such assistant.".to_owned())
}

impl AuthManager {
    pub(crate) fn new(events: Arc<EventBus>, accounts: Accounts) -> Self {
        let states = PROVIDERS
            .iter()
            .flat_map(|provider| {
                accounts
                    .ids(provider)
                    .into_iter()
                    .map(|id| (key(provider, &id), AuthSnapshot::unknown()))
            })
            .collect();
        Self {
            inner: Arc::new(AuthInner {
                accounts,
                states: Mutex::new(states),
                sessions: Mutex::new(HashMap::new()),
                notices: Mutex::new(HashMap::new()),
                releaser: Mutex::new(None),
                next_session: AtomicU64::new(1),
                next_generation: AtomicU64::new(1),
                events,
            }),
        }
    }

    pub(crate) fn set_releaser(&self, releaser: Releaser) {
        if let Ok(mut current) = self.inner.releaser.lock() {
            *current = Some(releaser);
        }
    }

    pub(crate) fn accounts(&self) -> &Accounts {
        &self.inner.accounts
    }

    pub(crate) fn refresh_all(&self) {
        for provider in PROVIDERS {
            self.refresh(provider);
        }
    }

    /// Checks every account of one assistant.
    pub(crate) fn refresh(&self, provider: &str) -> bool {
        let mut started = true;
        for id in self.inner.accounts.ids(provider) {
            started &= self.refresh_account(provider, &id);
        }
        started
    }

    fn refresh_account(&self, provider: &str, account: &str) -> bool {
        let generation = {
            // Sessions, then states, as `begin_session` takes them: a sign-in
            // or sign-out cannot begin between seeing none and marking checking.
            let Ok(sessions) = self.inner.sessions.lock() else {
                return false;
            };
            if sessions.contains_key(provider) {
                return true;
            }
            let Ok(mut states) = self.inner.states.lock() else {
                return false;
            };
            let snapshot = states
                .entry(key(provider, account))
                .or_insert_with(AuthSnapshot::unknown);
            if snapshot.state == "checking" {
                return true;
            }
            if AuthSnapshot::established(&snapshot.state) {
                snapshot.settled = Some(snapshot.state.clone());
            }
            snapshot.generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
            snapshot.state = "checking".into();
            snapshot.detail = None;
            snapshot.login_url = None;
            snapshot.device_code = None;
            snapshot.needs_input = false;
            // Taken under the same lock, so it is this check's and no one else's.
            let generation = snapshot.generation;
            drop(states);
            drop(sessions);
            generation
        };
        self.publish(provider);
        let manager = self.clone();
        let checked_provider = provider.to_owned();
        let checked_account = account.to_owned();
        let spawned = thread::Builder::new()
            .name(format!("xd-auth-{provider}"))
            .spawn(move || {
                // A removed account is not checked: the check would run in a
                // home that is being deleted.
                let Some(_lease) = manager
                    .inner
                    .accounts
                    .lease(&checked_provider, &checked_account)
                else {
                    return;
                };
                let environment = manager
                    .inner
                    .accounts
                    .environment(&checked_provider, &checked_account);
                let snapshot = check_status(&checked_provider, &environment);
                manager.apply_check(&checked_provider, &checked_account, generation, snapshot);
                manager.publish(&checked_provider);
            });
        if let Err(error) = spawned {
            self.set_snapshot(
                provider,
                account,
                failed(format!("Cannot check sign-in status: {error}")),
            );
            return false;
        }
        true
    }

    /// Installs a status check's answer unless anything replaced the state
    /// since the check began: a sign-in, a sign-out, or another check.
    fn apply_check(
        &self,
        provider: &str,
        account: &str,
        generation: u64,
        mut snapshot: AuthSnapshot,
    ) -> bool {
        let Ok(mut states) = self.inner.states.lock() else {
            return false;
        };
        let entry = key(provider, account);
        if states
            .get(&entry)
            .is_none_or(|current| current.generation != generation)
        {
            return false;
        }
        snapshot.generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        snapshot.settled = Some(snapshot.state.clone());
        states.insert(entry, snapshot);
        true
    }

    fn snapshot(&self, provider: &str, account: &str) -> AuthSnapshot {
        self.inner
            .states
            .lock()
            .ok()
            .and_then(|states| states.get(&key(provider, account)).cloned())
            .unwrap_or_else(AuthSnapshot::unknown)
    }

    /// Whether a turn may run on the account. One whose check has not come
    /// back yet gets the benefit of the doubt; the CLI says so if it is wrong.
    pub(crate) fn signed_in(&self, provider: &str, account: &str) -> bool {
        let snapshot = self.snapshot(provider, account);
        let state = match snapshot.state.as_str() {
            "checking" | "unknown" => snapshot.settled.as_deref().unwrap_or("unknown"),
            state => state,
        };
        // An account whose sign-in is being replaced runs as nobody yet.
        !matches!(
            state,
            "signed-out" | "failed" | "signing-out" | "signing-in"
        )
    }

    /// The account the chat's next turn runs on; see [`Accounts::choose`].
    pub(crate) fn choose(&self, provider: &str, chat_account: Option<&str>) -> Choice {
        let mut choice = self.inner.accounts.choose(
            provider,
            chat_account,
            |account| self.signed_in(provider, account),
            now_ms(),
        );
        // Even a lone sign-in takes no turn while it is being signed in or out.
        if matches!(
            self.snapshot(provider, &choice.account).state.as_str(),
            "signing-in" | "signing-out"
        ) {
            choice.unavailable = true;
        }
        choice
    }

    /// Reads the account's login again before a Codex session is resumed on it,
    /// unless it was read moments ago. The CLI can be signed in to another
    /// login outside xd, and a Codex session cannot open under another login.
    /// Returns whether the login is known now.
    pub(crate) fn confirm_login(&self, provider: &str, account: &str) -> bool {
        const FRESH_MS: i64 = 2 * 60 * 1000;
        let accounts = &self.inner.accounts;
        let fresh = accounts
            .login_read_at(provider, account)
            .is_some_and(|read| now_ms() - read < FRESH_MS);
        // Unit tests never start a real CLI for this.
        if fresh || cfg!(test) {
            return true;
        }
        let Some(_lease) = accounts.lease(provider, account) else {
            return false;
        };
        let environment = accounts.environment(provider, account);
        let started = now_ms();
        let ticket = accounts.ticket();
        match crate::usage::read_codex_login(&environment) {
            Some(identity) => {
                accounts.record_login_from(ticket, provider, account, &identity, started);
                true
            }
            None => false,
        }
    }

    /// Sign-in state as a chat shows it: that of the account a new turn would use.
    pub(crate) fn state(&self, provider: &str) -> String {
        let account = self
            .inner
            .accounts
            .choose(
                provider,
                None,
                |account| self.signed_in(provider, account),
                now_ms(),
            )
            .account;
        self.snapshot(provider, &account).state
    }

    /// Signs in the user's own CLI, or an account already in the list again.
    pub(crate) fn login(&self, provider: &str, account: Option<&str>) -> Result<(), String> {
        let provider = known_provider(provider)?;
        let account = account.unwrap_or(DEFAULT_ACCOUNT);
        if !self.inner.accounts.exists(provider, account)
            || self.inner.accounts.pending(provider).as_deref() == Some(account)
        {
            return Err("No such account.".into());
        }
        self.begin_session(provider, account, SessionKind::SignIn)
    }

    /// Signs in another account, in a home of its own.
    pub(crate) fn add(&self, provider: &str) -> Result<(), String> {
        let provider = known_provider(provider)?;
        if self
            .inner
            .sessions
            .lock()
            .map_err(|_| "Authentication service is unavailable.".to_owned())?
            .contains_key(provider)
        {
            return Err(format!(
                "{} is already signing in. Finish or cancel that first.",
                provider_name(provider)
            ));
        }
        let account = self.inner.accounts.begin_adding(provider)?;
        if let Ok(mut notices) = self.inner.notices.lock() {
            notices.remove(provider);
        }
        let started = self.begin_session(provider, &account, SessionKind::Add);
        if started.is_err() {
            self.inner.accounts.forget(provider, &account);
            self.publish(provider);
        }
        started
    }

    /// Signs the user's own CLI out, or removes an added account: its CLI signs
    /// out and its home, credentials included, is deleted.
    pub(crate) fn logout(&self, provider: &str, account: Option<&str>) -> Result<(), String> {
        let provider = known_provider(provider)?;
        let account = account.unwrap_or(DEFAULT_ACCOUNT);
        if !self.inner.accounts.exists(provider, account) {
            return Err("No such account.".into());
        }
        let kind = if account == DEFAULT_ACCOUNT {
            SessionKind::SignOut
        } else {
            SessionKind::Remove
        };
        self.begin_session(provider, account, kind)
    }

    pub(crate) fn reorder(&self, provider: &str, order: &[String]) -> Result<(), String> {
        let provider = known_provider(provider)?;
        self.inner.accounts.reorder(provider, order)?;
        self.publish(provider);
        Ok(())
    }

    pub(crate) fn set_auto_switch(&self, provider: &str, enabled: bool) -> Result<(), String> {
        let provider = known_provider(provider)?;
        self.inner.accounts.set_auto_switch(provider, enabled)?;
        self.publish(provider);
        Ok(())
    }

    /// A turn on this account was turned away for usage. Reads the account
    /// again to learn when it resets.
    pub(crate) fn note_limited(
        &self,
        provider: &str,
        account: &str,
        resets_at_ms: Option<i64>,
        label: Option<&str>,
    ) {
        self.inner
            .accounts
            .note_limited(provider, account, resets_at_ms, label, now_ms());
        self.publish(provider);
        self.check_soon(provider, account);
    }

    /// Reads every account's limits now and then while an assistant has
    /// several. Only the daemon runs this.
    pub(crate) fn start_watch(&self) {
        let manager = Arc::downgrade(&self.inner);
        let _ = thread::Builder::new()
            .name("xd-account-watch".into())
            .spawn(move || {
                loop {
                    let Some(inner) = manager.upgrade() else {
                        return;
                    };
                    let auth = AuthManager { inner };
                    for provider in PROVIDERS {
                        if !auth.inner.accounts.has_several(provider) {
                            continue;
                        }
                        for id in auth.inner.accounts.ids(provider) {
                            if auth.signed_in(provider, &id) {
                                crate::usage::check_account(&auth.inner.accounts, provider, &id);
                            }
                        }
                        auth.publish(provider);
                    }
                    drop(auth);
                    thread::sleep(WATCH_INTERVAL);
                }
            });
    }

    fn session_account(&self, provider: &str) -> Option<String> {
        self.inner
            .sessions
            .lock()
            .ok()?
            .get(provider)
            .map(|session| session.account.clone())
    }

    pub(crate) fn input(&self, provider: &str, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() || text.len() > 4096 {
            return Err("Authentication input must contain 1 to 4096 bytes.".into());
        }
        let input = self
            .inner
            .sessions
            .lock()
            .map_err(|_| "Authentication service is unavailable.".to_owned())?
            .get(provider)
            .and_then(|session| session.input.clone())
            .ok_or_else(|| format!("{provider} is not waiting for authentication input."))?;
        let mut input = input
            .lock()
            .map_err(|_| "Authentication input is unavailable.".to_owned())?;
        writeln!(input, "{text}")
            .map_err(|error| format!("Cannot send authentication input: {error}"))
    }

    pub(crate) fn cancel(&self, provider: &str) -> Result<(), String> {
        let pid = {
            let mut sessions = self
                .inner
                .sessions
                .lock()
                .map_err(|_| "Authentication service is unavailable.".to_owned())?;
            let session = sessions
                .get_mut(provider)
                .ok_or_else(|| format!("{provider} is not signing in."))?;
            session.cancelled = true;
            session.pid
        };
        if let Some(pid) = pid {
            let _ = background_command("kill")
                .args(["-INT", &pid.to_string()])
                .status();
        }
        Ok(())
    }

    fn begin_session(
        &self,
        provider: &str,
        account: &str,
        kind: SessionKind,
    ) -> Result<(), String> {
        let serial = self.inner.next_session.fetch_add(1, Ordering::Relaxed);
        {
            let mut sessions = self
                .inner
                .sessions
                .lock()
                .map_err(|_| "Authentication service is unavailable.".to_owned())?;
            if sessions.contains_key(provider) {
                return Err(format!(
                    "{} is already signing in. Finish or cancel that first.",
                    provider_name(provider)
                ));
            }
            // Checked again with the slot held: the account may have been
            // removed since the request looked. Only adding works on a pending one.
            let pending = self.inner.accounts.pending(provider).as_deref() == Some(account);
            if account != DEFAULT_ACCOUNT
                && (!self.inner.accounts.exists(provider, account)
                    || (pending != (kind == SessionKind::Add)))
            {
                return Err("No such account.".into());
            }
            sessions.insert(
                provider.to_owned(),
                AuthSession {
                    serial,
                    account: account.to_owned(),
                    kind,
                    pid: None,
                    input: None,
                    output: String::new(),
                    cancelled: false,
                },
            );
        }
        let login = matches!(kind, SessionKind::SignIn | SessionKind::Add);
        self.set_snapshot(
            provider,
            account,
            AuthSnapshot {
                state: if login { "signing-in" } else { "signing-out" }.into(),
                detail: Some(match kind {
                    SessionKind::SignIn | SessionKind::Add => "Starting sign-in…".into(),
                    SessionKind::SignOut => "Signing out…".into(),
                    SessionKind::Remove => "Removing the account…".into(),
                }),
                login_url: None,
                device_code: None,
                needs_input: false,
                ..Default::default()
            },
        );
        let manager = self.clone();
        let provider = provider.to_owned();
        let session_provider = provider.clone();
        let session_account = account.to_owned();
        if let Err(error) = thread::Builder::new()
            .name(format!("xd-auth-session-{provider}"))
            .spawn(move || manager.run_session(&session_provider, &session_account, serial, kind))
        {
            if let Ok(mut sessions) = self.inner.sessions.lock() {
                sessions.remove(&provider);
            }
            self.set_snapshot(
                &provider,
                account,
                failed(format!("Cannot start authentication: {error}")),
            );
            return Err(format!("Cannot start authentication: {error}"));
        }
        Ok(())
    }

    fn run_session(&self, provider: &str, account: &str, serial: u64, kind: SessionKind) {
        // Nothing may still run as an account whose home is about to go, or
        // whose sign-in is about to be replaced. New turns already pass it
        // over: it reads as signing out or in.
        // For the user's own sign-in this only waits for turns already choosing
        // an account to register, so none can start as the login being replaced.
        if matches!(
            kind,
            SessionKind::Remove | SessionKind::SignIn | SessionKind::SignOut
        ) {
            let released = self
                .inner
                .releaser
                .lock()
                .ok()
                .and_then(|releaser| releaser.as_ref().map(|release| release(provider, account)))
                .unwrap_or(Ok(()));
            if let Err(error) = released {
                // Settled before the slot is given up, so a sign-in that takes
                // the slot next cannot have its state overwritten by this one.
                if let Ok(mut sessions) = self.inner.sessions.lock()
                    && sessions
                        .get(provider)
                        .is_some_and(|session| session.serial == serial)
                {
                    if let Ok(mut states) = self.inner.states.lock() {
                        let entry = key(provider, account);
                        let settled = states
                            .get(&entry)
                            .and_then(|previous| previous.settled.clone());
                        states.insert(
                            entry,
                            AuthSnapshot {
                                state: "unknown".into(),
                                generation: self
                                    .inner
                                    .next_generation
                                    .fetch_add(1, Ordering::Relaxed),
                                settled,
                                ..Default::default()
                            },
                        );
                    }
                    sessions.remove(provider);
                }
                self.publish(provider);
                self.set_notice(provider, error);
                self.refresh_account(provider, account);
                return;
            }
        }
        // Whatever login the account had is about to change; a reading of it no
        // longer confirms anything. Only now: a refused or rejected request
        // changes nothing.
        self.inner.accounts.forget_login_reading(provider, account);
        let login = matches!(kind, SessionKind::SignIn | SessionKind::Add);
        let mut command = match provider {
            "claude" => {
                let mut command = background_command(resolve_claude());
                command.args(if login {
                    ["auth", "login"].as_slice()
                } else {
                    ["auth", "logout"].as_slice()
                });
                command
            }
            _ => {
                let mut command = background_command(resolve_codex());
                command.args(if login {
                    ["login", "--device-auth"].as_slice()
                } else {
                    ["logout"].as_slice()
                });
                command
            }
        };
        // An added account's command runs in its own home or not at all: an
        // empty environment would be the user's own sign-in.
        let environment = self.inner.accounts.environment(provider, account);
        if account != DEFAULT_ACCOUNT && environment.is_empty() {
            self.finish_session(provider, serial, Err("That account was removed.".into()));
            return;
        }
        command
            .envs(environment)
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .stdin(if login { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.finish_session(
                    provider,
                    serial,
                    Err(format!("Cannot start authentication: {error}")),
                );
                return;
            }
        };
        let input = child.stdin.take().map(|input| Arc::new(Mutex::new(input)));
        if let Ok(mut sessions) = self.inner.sessions.lock()
            && let Some(session) = sessions.get_mut(provider)
            && session.serial == serial
        {
            session.pid = Some(child.id());
            session.input = input;
        }
        let stdout = child
            .stdout
            .take()
            .map(|stream| self.drain_login_stream(provider, serial, stream, login));
        let stderr = child
            .stderr
            .take()
            .map(|stream| self.drain_login_stream(provider, serial, stream, login));
        let status = child.wait();
        let stdout = stdout
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        let stderr = stderr
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        let result = match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(command_detail(&stdout, &stderr, status.success())),
            Err(error) => Err(format!("Authentication process failed: {error}")),
        };
        self.finish_session(provider, serial, result);
    }

    fn drain_login_stream(
        &self,
        provider: &str,
        serial: u64,
        mut stream: impl Read + Send + 'static,
        publish: bool,
    ) -> thread::JoinHandle<Vec<u8>> {
        let manager = self.clone();
        let provider = provider.to_owned();
        thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let remaining = OUTPUT_LIMIT.saturating_sub(kept.len());
                        kept.extend_from_slice(&buffer[..count.min(remaining)]);
                        if publish {
                            manager.append_login_output(&provider, serial, &buffer[..count]);
                        }
                    }
                }
            }
            kept
        })
    }

    fn append_login_output(&self, provider: &str, serial: u64, bytes: &[u8]) {
        let (output, account) = {
            let Ok(mut sessions) = self.inner.sessions.lock() else {
                return;
            };
            let Some(session) = sessions
                .get_mut(provider)
                .filter(|session| session.serial == serial)
            else {
                return;
            };
            session.output.push_str(&String::from_utf8_lossy(bytes));
            if session.output.len() > OUTPUT_LIMIT {
                session.output.drain(..session.output.len() - OUTPUT_LIMIT);
            }
            (session.output.clone(), session.account.clone())
        };
        let (login_url, device_code, needs_input) = login_instructions(provider, &output);
        if let Ok(mut states) = self.inner.states.lock()
            && let Some(snapshot) = states.get_mut(&key(provider, &account))
            && (
                snapshot.login_url.as_ref(),
                snapshot.device_code.as_ref(),
                snapshot.needs_input,
            ) != (login_url.as_ref(), device_code.as_ref(), needs_input)
        {
            snapshot.detail = Some(if needs_input {
                "Sign in in your browser, then paste the code it shows.".into()
            } else if device_code.is_some() {
                "Sign in in your browser and enter this code.".into()
            } else {
                "Waiting for sign-in…".into()
            });
            snapshot.login_url = login_url;
            snapshot.device_code = device_code;
            snapshot.needs_input = needs_input;
            drop(states);
            self.publish(provider);
        }
    }

    fn finish_session(&self, provider: &str, serial: u64, result: Result<(), String>) {
        // A new account is checked before the provider's slot is given up, so
        // no other sign-in can start in its home or race its outcome.
        let adding = self.inner.sessions.lock().ok().and_then(|sessions| {
            sessions
                .get(provider)
                .filter(|session| session.serial == serial)
                .filter(|session| session.kind == SessionKind::Add && !session.cancelled)
                .map(|session| session.account.clone())
        });
        if let Some(account) = adding.filter(|_| result.is_ok()) {
            self.inner.accounts.forget_login_reading(provider, &account);
            let added = self.adopt(provider, serial, &account);
            self.release_slot(provider, serial);
            self.publish(provider);
            if added {
                self.refresh_account(provider, &account);
            }
            return;
        }
        // A removal likewise keeps the slot until the home is gone, so no
        // sign-in can start in a home about to be deleted.
        let removing = self.inner.sessions.lock().ok().and_then(|sessions| {
            sessions
                .get(provider)
                .filter(|session| session.serial == serial && session.kind == SessionKind::Remove)
                .map(|session| session.account.clone())
        });
        if let Some(account) = removing {
            // The home goes even if the CLI could not sign out, so no
            // credential is left behind.
            if let Err(error) = &result {
                eprintln!("xd: {provider} sign-out before removal failed: {error}");
            }
            self.inner.accounts.forget(provider, &account);
            self.forget_snapshot(provider, &account);
            self.release_slot(provider, serial);
            self.publish(provider);
            return;
        }
        // A sign-in or sign-out of an account in the list settles that account's
        // state before giving up the slot: once given up, another sign-in can
        // hold the provider and nothing would settle this one.
        let settled = |session: &AuthSession| -> Option<AuthSnapshot> {
            if !matches!(session.kind, SessionKind::SignIn | SessionKind::SignOut) {
                return None;
            }
            Some(match (&result, session.cancelled) {
                (_, true) => failed("Sign-in canceled.".into()),
                (Err(error), false) => failed(error.clone()),
                (Ok(()), false) if session.kind == SessionKind::SignIn => AuthSnapshot {
                    state: "signed-in".into(),
                    detail: Some("Signed in.".into()),
                    ..Default::default()
                },
                (Ok(()), false) => signed_out(),
            })
        };
        let Some(session) = self.settle_and_release(provider, serial, settled) else {
            return;
        };
        let account = session.account.as_str();
        let accounts = &self.inner.accounts;
        // The login may have changed while the session ran.
        accounts.forget_login_reading(provider, account);
        match (session.kind, result) {
            (SessionKind::Add, _) if session.cancelled => {
                accounts.forget(provider, account);
                self.forget_snapshot(provider, account);
                self.publish(provider);
            }
            (SessionKind::Add, Err(error)) => {
                accounts.forget(provider, account);
                self.forget_snapshot(provider, account);
                self.set_notice(provider, format!("Sign-in failed: {error}"));
            }
            // Adopted above, before the slot was released.
            (SessionKind::Add, Ok(())) => {}
            // Removed above, before the slot was released.
            (SessionKind::Remove, _) => {}
            // Settled above, before the slot was released.
            (_, Ok(())) | (_, Err(_)) if session.cancelled => {
                self.refresh_account(provider, account);
            }
            (_, Ok(())) => {
                self.refresh_account(provider, account);
                self.check_soon(provider, account);
            }
            (_, Err(_)) => {}
        }
    }

    /// Gives up the provider's slot if it is still this session's, first
    /// installing the state `settle` gives the session's account, if any.
    fn settle_and_release(
        &self,
        provider: &str,
        serial: u64,
        settle: impl FnOnce(&AuthSession) -> Option<AuthSnapshot>,
    ) -> Option<AuthSession> {
        let session = {
            let mut sessions = self.inner.sessions.lock().ok()?;
            let session = sessions
                .get(provider)
                .filter(|session| session.serial == serial)?;
            if let Some(mut snapshot) = settle(session)
                && let Ok(mut states) = self.inner.states.lock()
            {
                snapshot.generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
                snapshot.settled = Some(snapshot.state.clone());
                states.insert(key(provider, &session.account), snapshot);
            }
            sessions.remove(provider)
        };
        self.publish(provider);
        session
    }

    /// Checks who a new sign-in belongs to before it joins the list.
    /// Returns whether the account joined the list.
    fn adopt(&self, provider: &str, serial: u64, account: &str) -> bool {
        self.set_snapshot(
            provider,
            account,
            AuthSnapshot {
                state: "checking".into(),
                detail: Some("Checking the account…".into()),
                login_url: None,
                device_code: None,
                needs_input: false,
                ..Default::default()
            },
        );
        let accounts = &self.inner.accounts;
        // Every account already in the list must be known by its login first,
        // or a repeat of it could not be told apart.
        // Read again even when known: any of them may have signed in to
        // another login since it was last read. A signed-in account whose
        // login cannot be read leaves the new one undecided, so it is not added.
        for existing in accounts.ids(provider) {
            let read = crate::usage::check_account(accounts, provider, &existing)
                .is_some_and(|identity| identity.account_key.is_some());
            if !read && self.signed_in(provider, &existing) {
                accounts.forget(provider, account);
                self.forget_snapshot(provider, account);
                self.set_notice(
                    provider,
                    format!(
                        "xd could not confirm which login {} is signed in to, so the new account \
                         was not added. Try again.",
                        accounts.label(provider, &existing)
                    ),
                );
                return false;
            }
        }
        // An account whose login cannot be read is never taken in: it could be
        // another account's sign-in under a shared keychain entry.
        let Some(identity) = crate::usage::check_account(accounts, provider, account)
            .filter(|identity| identity.account_key.is_some())
        else {
            accounts.forget(provider, account);
            self.forget_snapshot(provider, account);
            self.set_notice(
                provider,
                "xd could not confirm which account signed in, so it was not added. Try again."
                    .into(),
            );
            return false;
        };
        // Shared while the account is still pending: nothing can run as it yet,
        // so this cannot race Claude's own writes to the file.
        accounts.share_settings(provider, account);
        // Joins the list only if the sign-in was not cancelled meanwhile, decided
        // under the lock cancelling takes, so the two cannot both win.
        let joined = match self.inner.sessions.lock() {
            Ok(sessions) => {
                if sessions
                    .get(provider)
                    .is_some_and(|session| session.serial == serial && !session.cancelled)
                {
                    Some(accounts.finish_adding(provider, account, &identity))
                } else {
                    None
                }
            }
            Err(_) => None,
        };
        let Some(joined) = joined else {
            accounts.forget(provider, account);
            self.forget_snapshot(provider, account);
            return false;
        };
        match joined {
            Ok(Ok(())) => {
                self.set_notice(
                    provider,
                    format!("Added {}.", accounts.label(provider, account)),
                );
                // Clears "checking" so the status check that follows really runs.
                self.set_snapshot(provider, account, AuthSnapshot::unknown());
                true
            }
            Ok(Err(existing)) => {
                // Signing the duplicate out could sign the original out too,
                // where the CLI shares a keychain entry. Delete the home only.
                accounts.forget(provider, account);
                self.forget_snapshot(provider, account);
                self.set_notice(
                    provider,
                    format!("{existing} is already in the list, so it was not added again."),
                );
                false
            }
            Err(error) => {
                accounts.forget(provider, account);
                self.forget_snapshot(provider, account);
                self.set_notice(provider, error);
                false
            }
        }
    }

    fn check_soon(&self, provider: &str, account: &str) {
        // Unit tests never reach a CLI from a thread that outlives them: it
        // would run whatever the executable override names by then.
        if cfg!(test) {
            return;
        }
        let manager = self.clone();
        let provider = provider.to_owned();
        let account = account.to_owned();
        let _ = thread::Builder::new()
            .name("xd-account-check".into())
            .spawn(move || {
                crate::usage::check_account(&manager.inner.accounts, &provider, &account);
                manager.publish(&provider);
            });
    }

    /// Gives up the provider's sign-in slot if it is still this session's.
    fn release_slot(&self, provider: &str, serial: u64) -> Option<AuthSession> {
        let mut sessions = self.inner.sessions.lock().ok()?;
        if sessions
            .get(provider)
            .is_some_and(|session| session.serial == serial)
        {
            sessions.remove(provider)
        } else {
            None
        }
    }

    fn set_notice(&self, provider: &str, notice: String) {
        if let Ok(mut notices) = self.inner.notices.lock() {
            notices.insert(provider.to_owned(), notice);
        }
        self.publish(provider);
    }

    fn forget_snapshot(&self, provider: &str, account: &str) {
        if let Ok(mut states) = self.inner.states.lock() {
            states.remove(&key(provider, account));
        }
    }

    fn set_snapshot(&self, provider: &str, account: &str, mut snapshot: AuthSnapshot) {
        snapshot.generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut states) = self.inner.states.lock() {
            let entry = key(provider, account);
            snapshot.settled = if AuthSnapshot::established(&snapshot.state) {
                Some(snapshot.state.clone())
            } else {
                states
                    .get(&entry)
                    .and_then(|previous| previous.settled.clone())
            };
            states.insert(entry, snapshot);
        }
        self.publish(provider);
    }

    pub(crate) fn snapshots(&self) -> Value {
        let providers = PROVIDERS
            .iter()
            .map(|provider| self.provider_value(provider))
            .collect::<Vec<_>>();
        json!({"ok": true, "providers": providers, "multiple_accounts": self.inner.accounts.supports_more()})
    }

    /// One assistant with each of its accounts. The top-level sign-in fields
    /// are the user's own CLI sign-in, as clients before accounts read them.
    fn provider_value(&self, provider: &str) -> Value {
        let mut value = snapshot_value(provider, &self.snapshot(provider, DEFAULT_ACCOUNT));
        let now = now_ms();
        let in_use = self.choose(provider, None).account;
        let busy = self.session_account(provider);
        let accounts = self
            .inner
            .accounts
            .describe(provider, now)
            .into_iter()
            .map(|mut account| {
                let id = account["id"].as_str().unwrap_or_default().to_owned();
                let snapshot = snapshot_value(provider, &self.snapshot(provider, &id));
                for field in ["state", "detail", "login_url", "device_code", "needs_input"] {
                    if let Some(found) = snapshot.get(field) {
                        account[field] = found.clone();
                    }
                }
                account["in_use"] = json!(id == in_use);
                account["busy"] = json!(busy.as_deref() == Some(id.as_str()));
                account
            })
            .collect::<Vec<_>>();
        value["accounts"] = Value::Array(accounts);
        value["auto_switch"] = json!(self.inner.accounts.auto_switch(provider));
        value["adding"] = json!(self.inner.accounts.pending(provider).is_some());
        if let Some(notice) = self
            .inner
            .notices
            .lock()
            .ok()
            .and_then(|notices| notices.get(provider).cloned())
        {
            value["notice"] = Value::String(notice);
        }
        value
    }

    fn publish(&self, provider: &str) {
        let mut event = self.provider_value(provider);
        event["event"] = Value::String("agent-auth-changed".into());
        self.inner.events.publish(event);
    }
}

fn snapshot_value(provider: &str, snapshot: &AuthSnapshot) -> Value {
    let mut value = json!({
        "provider": provider,
        "display_name": provider_name(provider),
        "state": snapshot.state,
        "needs_input": snapshot.needs_input,
    });
    if let Some(detail) = &snapshot.detail {
        value["detail"] = Value::String(detail.clone());
    }
    if let Some(login_url) = &snapshot.login_url {
        value["login_url"] = Value::String(login_url.clone());
    }
    if let Some(device_code) = &snapshot.device_code {
        value["device_code"] = Value::String(device_code.clone());
    }
    value
}

fn check_status(provider: &str, environment: &[(String, String)]) -> AuthSnapshot {
    let mut command = match provider {
        "claude" => {
            let mut command = background_command(resolve_claude());
            command.args(["auth", "status", "--json"]);
            command
        }
        _ => {
            let mut command = background_command(resolve_codex());
            command.args(["login", "status"]);
            command
        }
    };
    command
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failed(format!("Cannot check sign-in status: {error}")),
    };
    let stdout = child.stdout.take().map(drain_bounded);
    let stderr = child.stderr.take().map(drain_bounded);
    let deadline = Instant::now() + STATUS_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(_) => break None,
        }
    };
    let stdout = stdout
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let Some(status) = status else {
        return failed("Sign-in status check timed out.".into());
    };
    if provider == "claude" {
        return parse_claude_status(&stdout, &stderr, status.success());
    }
    parse_codex_status(&stdout, &stderr, status.success())
}

fn parse_claude_status(stdout: &[u8], stderr: &[u8], success: bool) -> AuthSnapshot {
    let parsed = serde_json::from_slice::<Value>(stdout).ok();
    let logged_in = parsed
        .as_ref()
        .and_then(|value| value.get("loggedIn"))
        .and_then(Value::as_bool);
    match logged_in {
        Some(true) => AuthSnapshot {
            state: "signed-in".into(),
            detail: parsed
                .as_ref()
                .and_then(|value| value.get("authMethod"))
                .and_then(Value::as_str)
                .filter(|method| *method != "none")
                .map(|method| format!("Signed in with {method}."))
                .or_else(|| Some("Signed in.".into())),
            login_url: None,
            device_code: None,
            needs_input: false,
            ..Default::default()
        },
        Some(false) => signed_out(),
        None => failed(command_detail(stdout, stderr, success)),
    }
}

fn parse_codex_status(stdout: &[u8], stderr: &[u8], success: bool) -> AuthSnapshot {
    let detail = command_detail(stdout, stderr, success);
    if detail.to_lowercase().contains("not logged in") {
        signed_out()
    } else if success {
        AuthSnapshot {
            state: "signed-in".into(),
            detail: Some(detail),
            login_url: None,
            device_code: None,
            needs_input: false,
            ..Default::default()
        }
    } else {
        failed(detail)
    }
}

fn drain_bounded(mut stream: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let remaining = OUTPUT_LIMIT.saturating_sub(kept.len());
                    kept.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            }
        }
        kept
    })
}

fn command_detail(stdout: &[u8], stderr: &[u8], success: bool) -> String {
    [stdout, stderr]
        .into_iter()
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .flat_map(str::lines)
        .map(str::trim)
        .rfind(|line| !line.is_empty() && !line.starts_with("WARNING:"))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            if success {
                "Signed in.".into()
            } else {
                "Could not read sign-in status.".into()
            }
        })
}

fn signed_out() -> AuthSnapshot {
    AuthSnapshot {
        state: "signed-out".into(),
        detail: Some("Not signed in.".into()),
        login_url: None,
        device_code: None,
        needs_input: false,
        ..Default::default()
    }
}

fn failed(detail: String) -> AuthSnapshot {
    AuthSnapshot {
        state: "failed".into(),
        detail: Some(detail),
        login_url: None,
        device_code: None,
        needs_input: false,
        ..Default::default()
    }
}

fn login_instructions(provider: &str, output: &str) -> (Option<String>, Option<String>, bool) {
    let clean = strip_ansi(output);
    let login_url = clean
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .map(|word| {
            word.trim_end_matches(['.', ',', ';', ':', ')', ']', '}'])
                .to_owned()
        });
    let device_code = (provider == "codex")
        .then(|| {
            clean.split_whitespace().find_map(|word| {
                let word = word.trim_matches(|character: char| {
                    !character.is_ascii_alphanumeric() && character != '-'
                });
                (word.len() >= 9
                    && word.contains('-')
                    && word.chars().all(|character| {
                        character.is_ascii_uppercase()
                            || character.is_ascii_digit()
                            || character == '-'
                    }))
                .then(|| word.to_owned())
            })
        })
        .flatten();
    let needs_input = provider == "claude" && clean.to_ascii_lowercase().contains("paste code");
    (login_url, device_code, needs_input)
}

fn strip_ansi(text: &str) -> String {
    let mut clean = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == 0x1b {
            for next in bytes.by_ref() {
                if (0x40..=0x7e).contains(&next) {
                    break;
                }
            }
        } else {
            clean.push(byte);
        }
    }
    String::from_utf8_lossy(&clean).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adding an account shows its sign-in code, and cancelling it takes the
    /// account and its home away and tells clients so.
    #[cfg(unix)]
    #[test]
    fn a_cancelled_new_account_is_discarded_and_reported() {
        use std::{
            os::unix::fs::PermissionsExt,
            sync::{atomic::AtomicBool, mpsc},
        };

        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("xd-auth-add-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let codex = root.join("codex");
        std::fs::write(
            &codex,
            "#!/bin/sh\nif [ \"$1\" = login ] && [ \"$2\" = --device-auth ]; then\n  echo 'Open https://auth.openai.com/codex/device and enter ABCD-12345'\n  exec sleep 30\nfi\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::set_var("XD_CODEX_EXECUTABLE", &codex) };
        let events = Arc::new(EventBus::default());
        let (sender, receiver) = mpsc::sync_channel(256);
        events
            .subscribe_with_auth(sender, None, Arc::new(AtomicBool::new(true)))
            .unwrap();
        let accounts = Accounts::with_homes(&root.join("data"), &[]);
        let auth = AuthManager::new(events, accounts.clone());
        let next = |wanted: &dyn Fn(&Value) -> bool| loop {
            let event = receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("an agent-auth-changed event");
            if event["event"] == "agent-auth-changed"
                && event["provider"] == "codex"
                && wanted(&event)
            {
                return event;
            }
        };

        auth.add("codex").unwrap();
        let pending =
            next(&|event| {
                event["accounts"].as_array().unwrap().iter().any(|account| {
                    account["pending"] == true && account["device_code"] == "ABCD-12345"
                })
            });
        assert_eq!(pending["adding"], true);
        let id = accounts.pending("codex").unwrap();
        let home = accounts.home("codex", &id).unwrap();
        assert!(home.is_dir());
        assert!(auth.add("codex").is_err(), "one sign-in at a time");

        auth.cancel("codex").unwrap();
        let settled = next(&|event| event["adding"] == false);
        assert_eq!(settled["accounts"].as_array().unwrap().len(), 1);
        assert!(!home.exists());
        assert_eq!(accounts.ids("codex"), vec![DEFAULT_ACCOUNT.to_owned()]);
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::remove_var("XD_CODEX_EXECUTABLE") };
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_status_check_never_outlives_a_newer_change() {
        let auth = AuthManager::new(Arc::new(EventBus::default()), Accounts::new(None));
        auth.set_snapshot("codex", DEFAULT_ACCOUNT, signed_out());
        // A refresh is under way: the account stays unusable meanwhile.
        if let Ok(mut states) = auth.inner.states.lock() {
            let snapshot = states.get_mut(&key("codex", DEFAULT_ACCOUNT)).unwrap();
            snapshot.settled = Some("signed-out".into());
            snapshot.state = "checking".into();
        }
        assert!(!auth.signed_in("codex", DEFAULT_ACCOUNT));
        let started = auth.snapshot("codex", DEFAULT_ACCOUNT).generation;
        // A sign-out (or anything else) lands before the check answers.
        auth.set_snapshot("codex", DEFAULT_ACCOUNT, signed_out());
        let stale = AuthSnapshot {
            state: "signed-in".into(),
            ..Default::default()
        };
        assert!(!auth.apply_check("codex", DEFAULT_ACCOUNT, started, stale.clone()));
        assert!(!auth.signed_in("codex", DEFAULT_ACCOUNT));
        let current = auth.snapshot("codex", DEFAULT_ACCOUNT).generation;
        assert!(auth.apply_check("codex", DEFAULT_ACCOUNT, current, stale));
        assert!(auth.signed_in("codex", DEFAULT_ACCOUNT));
    }

    #[test]
    fn parses_codex_and_claude_authentication_status() {
        let codex = parse_codex_status(b"Logged in using ChatGPT\n", b"", true);
        assert_eq!(codex.state, "signed-in");
        assert_eq!(codex.detail.as_deref(), Some("Logged in using ChatGPT"));
        assert_eq!(
            parse_codex_status(b"Not logged in\n", b"", false).state,
            "signed-out"
        );
        let claude =
            parse_claude_status(br#"{"loggedIn":true,"authMethod":"claude.ai"}"#, b"", true);
        assert_eq!(claude.state, "signed-in");
        assert_eq!(claude.detail.as_deref(), Some("Signed in with claude.ai."));
        assert_eq!(
            parse_claude_status(br#"{"loggedIn":false}"#, b"", true).state,
            "signed-out"
        );
    }

    #[test]
    fn bounds_status_details_to_the_drained_output() {
        let detail = command_detail(b"WARNING: ignored\nfirst\nlast\n", b"", true);
        assert_eq!(detail, "last");
        assert_eq!(
            parse_claude_status(b"not json", b"bad status", false).state,
            "failed"
        );
    }

    #[test]
    fn extracts_only_structured_login_instructions() {
        assert_eq!(
            login_instructions(
                "codex",
                "Open https://auth.openai.com/device. Enter ABCD-EFGH."
            ),
            (
                Some("https://auth.openai.com/device".into()),
                Some("ABCD-EFGH".into()),
                false
            )
        );
        assert_eq!(
            login_instructions(
                "claude",
                "Visit https://claude.ai/login then paste code here"
            ),
            (Some("https://claude.ai/login".into()), None, true)
        );
    }
}
