//! Holds chats to a share of their account's usage limit.
//!
//! The assistants report usage per account, not per chat, so a chat's use is
//! estimated: while a chat with a quota works, its account's limit is read
//! about once a minute and when a turn ends, and each rise is shared evenly
//! among the chats that worked on the account since the reading before. An
//! agent a chat started with xd's start_agent tool counts against that chat's
//! quota, so a chat and its agents share one. A chat whose share reaches its
//! quota is stopped, with its agents, and only the user's new quota lets them
//! go on.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::json;

use crate::{
    EventBus,
    accounts::Headroom,
    runtime::{AgentActivity, TurnRuntime},
    storage::StateStore,
};

/// How often a limit is read while a chat with a quota works on its account.
#[cfg(not(test))]
const READ_INTERVAL: Duration = Duration::from_secs(60);
#[cfg(test)]
const READ_INTERVAL: Duration = Duration::from_millis(50);
/// A limit is never read more often than this, however many turns start and
/// end on the account.
#[cfg(not(test))]
const MIN_READ_GAP: Duration = Duration::from_secs(5);
#[cfg(test)]
const MIN_READ_GAP: Duration = Duration::ZERO;
/// After a reading fails, or reads a login no turn there runs as, the account
/// is not tried again for this long, so a probe that keeps failing is not
/// started for every turn.
#[cfg(not(test))]
const FAILED_READ_GAP: Duration = READ_INTERVAL;
#[cfg(test)]
const FAILED_READ_GAP: Duration = Duration::from_millis(10);
/// A pause between one turn ending and the next starting that counts as the
/// account going idle. A queued message starts the instant the turn before
/// it ends, and that handover is still work.
const IDLE_GAP: Duration = Duration::from_secs(10);
/// A limit whose reset moved later by more than this has started a new window.
const NEW_WINDOW_MS: i64 = 15 * 60 * 1000;

/// How much of the limit a quota counts against was used at one reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reading {
    pub(crate) percent: f64,
    pub(crate) resets_at_ms: Option<i64>,
    /// Which limit it is: `session` (5-hour) or `weekly`. Readings of
    /// different limits measure nothing against each other.
    pub(crate) kind: &'static str,
}

/// The limit quotas count against: the 5-hour one, or the weekly one when an
/// account has no 5-hour limit. A weekly limit of one model alone is never it.
pub(crate) fn quota_window(windows: &[Headroom]) -> Option<Reading> {
    let pick = |kind: &str| {
        windows
            .iter()
            .find(|window| window.kind == kind && !window.label.contains('·'))
    };
    let (kind, window) = pick("session")
        .map(|window| ("session", window))
        .or_else(|| pick("weekly").map(|window| ("weekly", window)))?;
    window.used_percent.is_finite().then_some(Reading {
        percent: window.used_percent,
        resets_at_ms: window.resets_at_ms,
        kind,
    })
}

#[derive(Debug, Default)]
struct AccountLedger {
    /// The reading the next rise is measured from.
    last: Option<Reading>,
    read_at: Option<Instant>,
    /// The chats running a turn here, and those whose background agents or
    /// shells go on between turns, as the runtime reported them.
    turns: HashSet<String>,
    background: HashSet<String>,
    /// When the runtime last reported each chat in `turns` or `background`.
    reported_at: HashMap<String, Instant>,
    /// When every turn here last ended, while none has begun since.
    idle_since: Option<Instant>,
    /// Since `last`, every turn here ended and one began again more than
    /// [`IDLE_GAP`] later. What the account used in that gap cannot be told
    /// apart from the work around it, so the interval is counted for nobody.
    restarted_after_idle: bool,
    /// When a reading was last begun, and whether it failed.
    tried_at: Option<Instant>,
    failed: bool,
    /// When `last` was asked for, by the clock, so a chat whose count started
    /// again since is not charged with use from before.
    asked_ms: i64,
    /// Nothing worked on the account when `last` was read.
    idle: bool,
    /// The login `last` read, as [`crate::accounts::Accounts::session_tag`]
    /// gives it. A reading of another login measures nothing against it.
    login: String,
    /// Every chat that worked on the account since `last`.
    sharers: BTreeSet<String>,
    /// Every (chat, account and login) pair seen since `last`, so a reading
    /// of another login is not counted against any of them: a chat whose
    /// turns changed login within one interval leaves both.
    logins: BTreeSet<(String, String)>,
}

/// What the meter knows of each account, by assistant and account id.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    accounts: HashMap<(String, String), AccountLedger>,
}

impl AccountLedger {
    fn busy(&self) -> bool {
        !self.turns.is_empty() || !self.background.is_empty()
    }

    /// Whether the next rise can be measured from `last`: it was not read
    /// while nothing worked here. What the account used while idle, perhaps
    /// outside xd, is nobody's here, so the next turn reads afresh. A turn
    /// shorter than a reading takes is then counted short, never long.
    fn baseline(&self, login: &str) -> Option<Reading> {
        self.last.filter(|_| !self.idle && self.login == login)
    }
}

impl Ledger {
    /// A chat started or ended a turn on the account.
    pub(crate) fn note(&mut self, provider: &str, account: &str, chat_id: &str) {
        self.accounts
            .entry((provider.to_owned(), account.to_owned()))
            .or_default()
            .sharers
            .insert(chat_id.to_owned());
    }

    /// A chat started a turn (`true`) or ended one with nothing left running
    /// (`false`), as the account and login in `tag`.
    #[cfg(test)]
    pub(crate) fn turn(&mut self, provider: &str, tag: &str, chat_id: &str, working: bool) {
        self.turn_tagged(
            provider,
            tag,
            chat_id,
            test_activity(working),
            Instant::now(),
        );
    }

    /// What the runtime reported of a chat on the account and login in
    /// `tag`; a report of background work ending without either is about the
    /// chat wherever it worked.
    fn turn_tagged(
        &mut self,
        provider: &str,
        tag: &str,
        chat_id: &str,
        activity: AgentActivity,
        at: Instant,
    ) {
        if provider.is_empty() {
            for entry in self.accounts.values_mut() {
                if entry.background.remove(chat_id) && !entry.busy() {
                    entry.idle_since.get_or_insert(at);
                }
            }
            return;
        }
        let account = crate::accounts::tag_account(tag).to_owned();
        self.note_login(provider, tag, chat_id);
        self.apply_at(provider, &account, chat_id, activity, at);
    }

    /// [`Self::note`] for a chat whose turn runs as the login in `tag`.
    fn note_login(&mut self, provider: &str, tag: &str, chat_id: &str) {
        let account = crate::accounts::tag_account(tag);
        self.note(provider, account, chat_id);
        if let Some(entry) = self
            .accounts
            .get_mut(&(provider.to_owned(), account.to_owned()))
        {
            entry.logins.insert((chat_id.to_owned(), tag.to_owned()));
        }
    }

    /// Takes in who works on the account now: a chat whose background work
    /// has ended since its turn did no longer keeps the account busy.
    /// `here` is what the runtime answered when asked at `asked_at`; a chat
    /// reported since is newer than that answer and stays.
    fn settle_running(
        &mut self,
        provider: &str,
        account: &str,
        here: &[String],
        asked_at: Instant,
        now: Instant,
    ) {
        if let Some(entry) = self
            .accounts
            .get_mut(&(provider.to_owned(), account.to_owned()))
        {
            let was_busy = entry.busy();
            let reported_at = &entry.reported_at;
            let stays = |chat_id: &String| {
                here.contains(chat_id) || reported_at.get(chat_id).is_some_and(|at| *at >= asked_at)
            };
            entry.turns.retain(stays);
            entry.background.retain(stays);
            let (turns, background) = (&entry.turns, &entry.background);
            entry
                .reported_at
                .retain(|chat_id, _| turns.contains(chat_id) || background.contains(chat_id));
            if was_busy && !entry.busy() && entry.idle_since.is_none() {
                entry.idle_since = Some(now);
            }
        }
    }

    #[cfg(test)]
    fn turn_at(
        &mut self,
        provider: &str,
        account: &str,
        chat_id: &str,
        working: bool,
        now: Instant,
    ) {
        self.apply_at(provider, account, chat_id, test_activity(working), now);
    }

    fn apply_at(
        &mut self,
        provider: &str,
        account: &str,
        chat_id: &str,
        activity: AgentActivity,
        now: Instant,
    ) {
        self.note(provider, account, chat_id);
        let Some(entry) = self
            .accounts
            .get_mut(&(provider.to_owned(), account.to_owned()))
        else {
            return;
        };
        entry.reported_at.insert(chat_id.to_owned(), now);
        match activity {
            AgentActivity::TurnStarted => {
                entry.turns.insert(chat_id.to_owned());
                if entry
                    .idle_since
                    .take()
                    .is_some_and(|since| now.saturating_duration_since(since) >= IDLE_GAP)
                {
                    entry.restarted_after_idle = true;
                }
                return;
            }
            AgentActivity::TurnEnded { background } => {
                entry.turns.remove(chat_id);
                if background {
                    entry.background.insert(chat_id.to_owned());
                } else {
                    entry.background.remove(chat_id);
                }
            }
            AgentActivity::BackgroundEnded => {
                entry.background.remove(chat_id);
            }
        }
        if !entry.busy() {
            entry.idle_since.get_or_insert(now);
        }
    }

    /// Takes in a reading of the account's limit as `login`, asked for at
    /// `asked_ms`, with the chats working on it now, and returns each chat's
    /// share of the rise since the last one, and when that last one was asked
    /// for. Chats without a quota take their share too; the caller drops
    /// theirs.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(
        &mut self,
        provider: &str,
        account: &str,
        reading: Reading,
        login: &str,
        working: &[String],
        now: Instant,
        asked_ms: i64,
    ) -> (Vec<(String, f64)>, i64) {
        let entry = self
            .accounts
            .entry((provider.to_owned(), account.to_owned()))
            .or_default();
        let mut sharers = std::mem::take(&mut entry.sharers);
        sharers.extend(working.iter().cloned());
        let rise = entry
            .baseline(login)
            .filter(|last| last.kind == reading.kind && !entry.restarted_after_idle)
            .map_or(0.0, |last| rise(last, reading));
        entry.restarted_after_idle = false;
        // The next interval starts here; work going on now means no gap yet.
        if !working.is_empty() {
            entry.idle_since = None;
        }
        let since_ms = std::mem::replace(&mut entry.asked_ms, asked_ms);
        entry.last = Some(reading);
        entry.read_at = Some(now);
        entry.failed = false;
        entry.idle = working.is_empty();
        login.clone_into(&mut entry.login);
        entry.sharers = working.iter().cloned().collect();
        let kept = &entry.sharers;
        entry.logins.retain(|(chat_id, _)| kept.contains(chat_id));
        if rise <= 0.0 || sharers.is_empty() {
            return (Vec::new(), since_ms);
        }
        let share = rise / sharers.len() as f64;
        (
            sharers.into_iter().map(|chat| (chat, share)).collect(),
            since_ms,
        )
    }

    fn forget(&mut self, provider: &str, account: &str) {
        self.accounts
            .remove(&(provider.to_owned(), account.to_owned()));
    }
}

/// Corrects who the runtime said works on one account with the reports that
/// came from when it was asked: they are newer than its answer, and win.
fn apply_reports(
    ledger: &Ledger,
    provider: &str,
    account: &str,
    asked_at: Instant,
    working: &mut Vec<(String, String)>,
    reports: Vec<Activity>,
) {
    for (chat_id, p, tag, what, at) in reports {
        let here = p == provider && crate::accounts::tag_account(&tag) == account;
        // A process gone is reported without where it worked.
        if at < asked_at || !(here || p.is_empty()) {
            continue;
        }
        let still = match what {
            AgentActivity::TurnStarted | AgentActivity::TurnEnded { background: true } => true,
            AgentActivity::TurnEnded { background: false } => false,
            // Background work ending leaves a turn that still runs at work.
            AgentActivity::BackgroundEnded => ledger
                .accounts
                .get(&(provider.to_owned(), account.to_owned()))
                .is_some_and(|entry| entry.turns.contains(&chat_id)),
        };
        if still && here {
            if !working.iter().any(|(id, _)| *id == chat_id) {
                working.push((chat_id, tag));
            }
        } else if !still {
            working.retain(|(id, _)| *id != chat_id);
        }
    }
}

/// Who works where, with each turn's login dropped from its account.
fn by_account(working: Vec<(String, String, String)>) -> Vec<(String, String, String)> {
    working
        .into_iter()
        .map(|(chat_id, provider, tag)| {
            let account = crate::accounts::tag_account(&tag).to_owned();
            (chat_id, provider, account)
        })
        .collect()
}

/// How much of the limit was used between two readings.
fn rise(last: Reading, now: Reading) -> f64 {
    // Readings that say when their window resets tell a new window by that
    // alone, so use that fell within one window, such as after a reset
    // credit or a correction, is no rise. Without it a clear fall is taken
    // for a new window.
    let new_window = match (last.resets_at_ms, now.resets_at_ms) {
        (Some(before), Some(after)) => after - before > NEW_WINDOW_MS,
        _ => now.percent + 0.5 < last.percent,
    };
    if new_window {
        now.percent.max(0.0)
    } else {
        (now.percent - last.percent).max(0.0)
    }
}

/// Reads an account's limit for the meter, given the assistant and account
/// id, and says which login it read, as
/// [`crate::accounts::Accounts::session_tag`] gives it.
pub(crate) type Reader = Box<dyn Fn(&str, &str) -> Option<(Reading, String)> + Send + Sync>;
/// Stops a chat that has used its quota.
pub(crate) type Stopper = Box<dyn Fn(&str) + Send + Sync>;

/// Counts each chat's use against its quota and stops it at the quota.
pub(crate) struct QuotaMeter {
    /// Kept for tests, which look at what the runtime told the meter.
    #[cfg_attr(not(test), allow(dead_code))]
    ledger: Arc<Mutex<Ledger>>,
    /// The only strong hold on the wake channel's sender. The runtime's hook
    /// holds it weakly, so dropping the meter disconnects the channel and
    /// its thread ends, letting go of the runtime.
    wake: Arc<Mutex<Sender<()>>>,
}

/// What the runtime reported of a chat: the chat, assistant, account and
/// login, what began or stopped, and when.
type Activity = (String, String, String, AgentActivity, Instant);

#[cfg(test)]
fn test_activity(working: bool) -> AgentActivity {
    if working {
        AgentActivity::TurnStarted
    } else {
        AgentActivity::TurnEnded { background: false }
    }
}

struct Pass {
    store: Arc<StateStore>,
    runtime: TurnRuntime,
    events: Arc<EventBus>,
    reader: Reader,
    stopper: Stopper,
    ledger: Arc<Mutex<Ledger>>,
    activity: Arc<Mutex<Vec<Activity>>>,
}

/// What the runtime calls as each turn starts and ends. The runtime calls it
/// with its own locks held, a session's router among them, so it only queues
/// the news under a lock held for nothing else, and wakes the meter, which
/// takes it into the ledger itself.
fn activity_handler(
    activity: Arc<Mutex<Vec<Activity>>>,
    wake: std::sync::Weak<Mutex<Sender<()>>>,
) -> crate::runtime::ActivityHandler {
    Box::new(move |chat_id, provider, tag, what| {
        if let Ok(mut activity) = activity.lock() {
            activity.push((
                chat_id.to_owned(),
                provider.to_owned(),
                tag.to_owned(),
                what,
                Instant::now(),
            ));
        }
        if let Some(wake) = wake.upgrade()
            && let Ok(wake) = wake.lock()
        {
            let _ = wake.send(());
        }
    })
}

impl QuotaMeter {
    /// Starts the meter's thread and has the runtime tell it of every turn.
    pub(crate) fn start(
        store: Arc<StateStore>,
        runtime: TurnRuntime,
        events: Arc<EventBus>,
        reader: Reader,
        stopper: Stopper,
    ) -> Self {
        let ledger = Arc::new(Mutex::new(Ledger::default()));
        let activity = Arc::new(Mutex::new(Vec::<Activity>::new()));
        let (wake, wakes) = mpsc::channel::<()>();
        let wake = Arc::new(Mutex::new(wake));
        runtime.set_activity_handler(activity_handler(activity.clone(), Arc::downgrade(&wake)));
        let pass = Pass {
            store,
            runtime,
            events,
            reader,
            stopper,
            ledger: ledger.clone(),
            activity: activity.clone(),
        };
        let spawned = thread::Builder::new()
            .name("xd-chat-quotas".into())
            .spawn(move || {
                let mut wait = READ_INTERVAL;
                loop {
                    match wakes.recv_timeout(wait) {
                        Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    // Turns that start and end together are read once.
                    while wakes.try_recv().is_ok() {}
                    wait = pass.run(Instant::now());
                }
            });
        if let Err(error) = spawned {
            eprintln!("xd-host: cannot start chat quotas: {error}");
        }
        Self { ledger, wake }
    }

    /// Looks again now, such as after a quota changed.
    pub(crate) fn wake(&self) {
        if let Ok(wake) = self.wake.lock() {
            let _ = wake.send(());
        }
    }

    #[cfg(test)]
    fn ledger(&self) -> Arc<Mutex<Ledger>> {
        self.ledger.clone()
    }
}

impl Pass {
    /// Takes the turns the runtime reported into the ledger, in order, and
    /// returns them.
    fn take_activity(&self, ledger: &mut Ledger) -> Vec<Activity> {
        let queued = self
            .activity
            .lock()
            .map(|mut activity| std::mem::take(&mut *activity))
            .unwrap_or_default();
        for (chat_id, provider, tag, what, at) in &queued {
            ledger.turn_tagged(provider, tag, chat_id, *what, *at);
        }
        queued
    }

    /// A reading of the account told nothing: it failed, or read another
    /// login. It is tried again after [`FAILED_READ_GAP`] while anything
    /// works there. With nothing at work any more, what the account uses until
    /// the next turn is nobody's here: the chats `seen` before the reading are
    /// retired, and the next turn reads afresh, so an idle account whose
    /// readings fail is not tried again and again.
    /// `other_login` when the reading was of another login, which leaves the
    /// baseline nothing to measure against; a failed one keeps it while the
    /// account works.
    fn unreadable(&self, key: &(String, String), seen: &[String], other_login: bool) {
        let Ok(mut ledger) = self.ledger.lock() else {
            return;
        };
        let working = self.workers(&mut ledger, &key.0, &key.1);
        let idle = working.is_empty();
        let Some(entry) = ledger.accounts.get_mut(key) else {
            return;
        };
        entry.failed = true;
        if !idle && !other_login {
            return;
        }
        // The interval is dropped, and the chats seen before the reading that
        // no longer work leave with it, their logins too, so one that ran as
        // an old login cannot keep every later reading from counting.
        entry.last = None;
        entry.sharers.retain(|chat_id| {
            !seen.contains(chat_id) || working.iter().any(|(id, _)| id == chat_id)
        });
        let kept = &entry.sharers;
        entry.logins.retain(|(chat_id, _)| kept.contains(chat_id));
        // What still works keeps only the login it runs as now.
        entry.logins.retain(|pair| {
            working
                .iter()
                .any(|(id, tag)| (id, tag) == (&pair.0, &pair.1))
        });
        entry.logins.extend(working.iter().cloned());
        if idle && entry.sharers.is_empty() {
            ledger.accounts.remove(key);
        }
    }

    /// Who works on the account now, read with the ledger held, then every
    /// start and end reported up to here taken into it: one reported from
    /// when the runtime was asked is newer than what it said, and wins.
    fn workers(&self, ledger: &mut Ledger, provider: &str, account: &str) -> Vec<(String, String)> {
        let asked_at = Instant::now();
        let mut working = self.working_on(provider, account);
        let reports = self.take_activity(ledger);
        apply_reports(ledger, provider, account, asked_at, &mut working, reports);
        working
    }

    /// The chats working on one account now, each with the account and login
    /// its turn runs as.
    fn working_on(&self, provider: &str, account: &str) -> Vec<(String, String)> {
        self.runtime
            .working_accounts()
            .into_iter()
            .filter(|(_, p, tag)| p == provider && crate::accounts::tag_account(tag) == account)
            .map(|(chat_id, _, tag)| (chat_id, tag))
            .collect()
    }

    /// One look at every account a chat with a quota works on. Returns how
    /// long until the next reading is due.
    fn run(&self, now: Instant) -> Duration {
        let mut next = READ_INTERVAL;
        // Filed by account: turns on one account share its every rise, even
        // if some began before its login was known. The logins they ran as
        // decide below whether a reading is theirs at all.
        let asked_at = Instant::now();
        let tagged = self.runtime.working_accounts();
        if let Ok(mut ledger) = self.ledger.lock() {
            self.take_activity(&mut ledger);
            for (chat_id, provider, tag) in &tagged {
                ledger.note_login(provider, tag, chat_id);
            }
        }
        let working = by_account(tagged);
        let limited = |chat_id: &str| {
            self.store
                .chat_quota(chat_id)
                .is_ok_and(|quota| quota.percent.is_some() && !quota.reached)
        };
        // A chat already past its quota, such as after the user lowered it,
        // stops without waiting for a reading, and so does one marked
        // stopped whose background work still runs.
        for (chat_id, _, _) in &working {
            if self
                .store
                .chat_quota(chat_id)
                .is_ok_and(|quota| quota.reached || quota.spent())
            {
                (self.stopper)(chat_id);
            }
        }
        let due = {
            let Ok(ledger) = self.ledger.lock() else {
                return next;
            };
            ledger
                .accounts
                .iter()
                .map(|((provider, account), entry)| {
                    (
                        provider.clone(),
                        account.clone(),
                        entry.sharers.iter().cloned().collect::<Vec<_>>(),
                        entry.last.is_some() && !entry.idle,
                        entry.read_at,
                        entry
                            .tried_at
                            .map(|at| (now.saturating_duration_since(at), entry.failed)),
                    )
                })
                .collect::<Vec<_>>()
        };
        for (provider, account, sharers, has_baseline, read_at, tried) in due {
            let here = working
                .iter()
                .filter(|(_, p, a)| *p == provider && *a == account)
                .map(|(chat_id, _, _)| chat_id.clone())
                .collect::<Vec<_>>();
            if let Ok(mut ledger) = self.ledger.lock() {
                ledger.settle_running(&provider, &account, &here, asked_at, now);
            }
            let mut involved = sharers.iter().chain(&here).collect::<HashSet<_>>();
            involved.retain(|chat_id| limited(chat_id));
            if involved.is_empty() {
                // Nothing to count here. A baseline stays while nothing works
                // on the account, for the next turn of a chat with a quota.
                // Chats without one that worked here since would leave a rise
                // nobody is charged for, so the next turn reads afresh. Chats
                // noted since the list above was taken stay for the next look.
                if let Ok(mut ledger) = self.ledger.lock()
                    && let Some(entry) = ledger
                        .accounts
                        .get_mut(&(provider.clone(), account.clone()))
                {
                    let seen = sharers.iter().collect::<HashSet<_>>();
                    if !seen.is_empty() || !here.is_empty() {
                        entry.last = None;
                    }
                    entry.sharers.retain(|chat_id| !seen.contains(chat_id));
                    entry.sharers.extend(here.iter().cloned());
                    if entry.last.is_none() && entry.sharers.is_empty() {
                        ledger.forget(&provider, &account);
                    }
                }
                continue;
            }
            let since = read_at.map(|at| now.saturating_duration_since(at));
            let ended = sharers.iter().any(|chat_id| !here.contains(chat_id));
            // A turn that leaves the account idle is read at once, not after
            // the gap between readings: a turn starting meanwhile would
            // otherwise have the idle time between them charged to it.
            let leaves_idle = ended && here.is_empty();
            let held_for = tried.map_or(Duration::ZERO, |(since_try, failed)| {
                let gap = if failed {
                    FAILED_READ_GAP
                } else if leaves_idle {
                    Duration::ZERO
                } else {
                    MIN_READ_GAP
                };
                gap.saturating_sub(since_try)
            });
            let due_in = if !has_baseline || ended {
                Duration::ZERO
            } else {
                since.map_or(Duration::ZERO, |since| READ_INTERVAL.saturating_sub(since))
            }
            .max(held_for);
            if !due_in.is_zero() {
                next = next.min(due_in);
                continue;
            }
            // Read without the ledger held: a probe can take seconds.
            let asked_ms = crate::accounts::now_ms();
            let key = (provider.clone(), account.clone());
            if let Ok(mut ledger) = self.ledger.lock()
                && let Some(entry) = ledger.accounts.get_mut(&key)
            {
                entry.tried_at = Some(Instant::now());
            }
            let Some((reading, login)) = (self.reader)(&provider, &account) else {
                // With nothing at work here any more, what the account uses
                // until the next reading is nobody's here: the next turn reads
                // afresh rather than from before the gap.
                self.unreadable(&key, &sharers, false);
                next = next.min(FAILED_READ_GAP);
                continue;
            };
            let recorded = {
                let Ok(mut ledger) = self.ledger.lock() else {
                    return next;
                };
                // Who works on the account now, then every start and end
                // reported up to here, so the ledger records no older picture
                // than this. Nothing but this thread takes the ledger.
                let working_now = self.workers(&mut ledger, &provider, &account);
                // A turn known to run as another login than the one read, such
                // as after the user's own CLI signed in to someone else, is not
                // measured by this reading, nor is anyone beside it: whether it
                // still works or worked since the last reading.
                let other_login = working_now
                    .iter()
                    .map(|(_, tag)| tag)
                    .chain(
                        ledger
                            .accounts
                            .get(&key)
                            .into_iter()
                            .flat_map(|entry| entry.logins.iter().map(|(_, tag)| tag)),
                    )
                    .any(|tag| !tag.ends_with("#?") && !crate::accounts::same_login(tag, &login));
                if other_login {
                    None
                } else {
                    let here_now = working_now
                        .iter()
                        .map(|(chat_id, _)| chat_id.clone())
                        .collect::<Vec<_>>();
                    let recorded = ledger.record(
                        &provider,
                        &account,
                        reading,
                        &login,
                        &here_now,
                        Instant::now(),
                        asked_ms,
                    );
                    // The next interval starts with the logins working now.
                    if let Some(entry) = ledger.accounts.get_mut(&key) {
                        entry.logins = working_now.into_iter().collect();
                    }
                    Some(recorded)
                }
            };
            let Some((shares, since_ms)) = recorded else {
                self.unreadable(&key, &sharers, true);
                next = next.min(FAILED_READ_GAP);
                continue;
            };
            for (chat_id, share) in shares {
                let counted = match self.store.add_quota_use(&chat_id, share, since_ms) {
                    Ok(Some(quota)) => quota,
                    Ok(None) => continue,
                    Err(error) => {
                        eprintln!("xd-host: cannot count chat {chat_id}'s quota: {error}");
                        continue;
                    }
                };
                // The quota is shared with the chat that started this one as
                // an agent, or with the agents this one started.
                let family = self
                    .store
                    .quota_family(&chat_id)
                    .unwrap_or_else(|_| vec![chat_id.clone()]);
                for chat in &family {
                    self.events
                        .publish(json!({"event": "changed", "chat": chat}));
                }
                if counted.spent() {
                    for chat in &family {
                        (self.stopper)(chat);
                    }
                }
            }
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(percent: f64, resets_at_ms: i64) -> Reading {
        Reading {
            percent,
            resets_at_ms: Some(resets_at_ms),
            kind: "session",
        }
    }

    fn record_in(
        ledger: &mut Ledger,
        provider: &str,
        account: &str,
        reading: Reading,
        working: &[String],
        now: Instant,
    ) -> Vec<(String, f64)> {
        ledger
            .record(provider, account, reading, "login", working, now, 0)
            .0
    }

    #[test]
    fn shares_say_when_the_use_they_count_began() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        let a = chats(&["a"]);
        ledger.record("claude", "default", reading(10.0, 1), "login", &a, now, 100);
        let (shares, since) =
            ledger.record("claude", "default", reading(12.0, 1), "login", &a, now, 200);
        assert_eq!((shares, since), (vec![("a".to_owned(), 2.0)], 100));
        // A reading of another login, such as once the CLI signed in to
        // someone else, starts afresh rather than counting the difference.
        let (shares, _) =
            ledger.record("claude", "default", reading(80.0, 1), "other", &a, now, 300);
        assert!(shares.is_empty());
        let (shares, _) =
            ledger.record("claude", "default", reading(81.0, 1), "other", &a, now, 400);
        assert_eq!(shares, vec![("a".to_owned(), 1.0)]);
    }

    fn chats(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_first_reading_is_the_baseline_and_later_rises_are_shared() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        ledger.note("claude", "default", "a");
        assert!(
            record_in(
                &mut ledger,
                "claude",
                "default",
                reading(10.0, 1),
                &chats(&["a"]),
                now
            )
            .is_empty()
        );
        ledger.note("claude", "default", "b");
        let shares = record_in(
            &mut ledger,
            "claude",
            "default",
            reading(16.0, 1),
            &chats(&["a", "b"]),
            now,
        );
        assert_eq!(shares, vec![("a".to_owned(), 3.0), ("b".to_owned(), 3.0)]);
        // A chat that ended since the last reading still takes its share of it.
        let shares = record_in(
            &mut ledger,
            "claude",
            "default",
            reading(18.0, 1),
            &chats(&["a"]),
            now,
        );
        assert_eq!(shares, vec![("a".to_owned(), 1.0), ("b".to_owned(), 1.0)]);
        let shares = record_in(
            &mut ledger,
            "claude",
            "default",
            reading(20.0, 1),
            &chats(&["a"]),
            now,
        );
        assert_eq!(shares, vec![("a".to_owned(), 2.0)]);
    }

    #[test]
    fn a_new_window_counts_from_zero_and_an_idle_account_reads_afresh() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        let hour = 60 * 60 * 1000;
        record_in(
            &mut ledger,
            "codex",
            "default",
            reading(97.0, hour),
            &chats(&["a"]),
            now,
        );
        let shares = record_in(
            &mut ledger,
            "codex",
            "default",
            reading(4.0, 6 * hour),
            &chats(&["a"]),
            now,
        );
        assert_eq!(shares, vec![("a".to_owned(), 4.0)]);
        // Nothing works on the account after this reading, so the next turn
        // starts from a reading of its own, however soon it comes: what the
        // account used in between is nobody's here.
        let shares = record_in(
            &mut ledger,
            "codex",
            "default",
            reading(5.0, 6 * hour),
            &[],
            now,
        );
        assert_eq!(shares, vec![("a".to_owned(), 1.0)]);
        ledger.note("codex", "default", "b");
        let soon = now + Duration::from_secs(1);
        assert!(
            record_in(
                &mut ledger,
                "codex",
                "default",
                reading(30.0, 6 * hour),
                &chats(&["b"]),
                soon
            )
            .is_empty()
        );
        let shares = record_in(
            &mut ledger,
            "codex",
            "default",
            reading(31.0, 6 * hour),
            &chats(&["b"]),
            soon,
        );
        assert_eq!(shares, vec![("b".to_owned(), 1.0)]);
        // Accounts are apart.
        assert!(
            record_in(
                &mut ledger,
                "codex",
                "second",
                reading(50.0, 6 * hour),
                &chats(&["c"]),
                now
            )
            .is_empty()
        );
    }

    #[test]
    fn the_login_of_a_turn_that_ended_is_kept_until_the_next_reading() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        ledger.turn("claude", "default#b", "a", true);
        ledger.turn("claude", "default#b", "a", false);
        let key = ("claude".to_owned(), "default".to_owned());
        assert!(
            ledger.accounts[&key]
                .logins
                .contains(&("a".to_owned(), "default#b".to_owned()))
        );
        // The same chat on another login within the interval leaves both.
        ledger.turn("claude", "default#c", "a", true);
        assert_eq!(ledger.accounts[&key].logins.len(), 2);
        ledger.turn("claude", "default#c", "a", false);
        record_in(&mut ledger, "claude", "default", reading(1.0, 1), &[], now);
        assert!(ledger.accounts[&key].logins.is_empty());
    }

    #[test]
    fn background_work_that_ends_by_itself_leaves_the_account_idle() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        let a = chats(&["a"]);
        let ended = AgentActivity::TurnEnded { background: true };
        ledger.apply_at("claude", "default", "a", AgentActivity::TurnStarted, now);
        record_in(&mut ledger, "claude", "default", reading(10.0, 1), &a, now);
        // The turn ends with its background work going on: still busy.
        ledger.apply_at("claude", "default", "a", ended, now);
        ledger.apply_at(
            "claude",
            "default",
            "a",
            AgentActivity::TurnStarted,
            now + IDLE_GAP,
        );
        let shares = record_in(&mut ledger, "claude", "default", reading(12.0, 1), &a, now);
        assert_eq!(shares, vec![("a".to_owned(), 2.0)]);
        // That work ends by itself; a turn long after it is an idle gap, even
        // before any pass saw the account empty.
        ledger.apply_at("claude", "default", "a", ended, now);
        ledger.apply_at(
            "claude",
            "default",
            "a",
            AgentActivity::BackgroundEnded,
            now,
        );
        ledger.apply_at(
            "claude",
            "default",
            "a",
            AgentActivity::TurnStarted,
            now + IDLE_GAP,
        );
        assert!(record_in(&mut ledger, "claude", "default", reading(20.0, 1), &a, now).is_empty());
        // A process gone is reported without where it worked.
        ledger.apply_at("claude", "default", "a", ended, now);
        ledger.turn_tagged("", "", "a", AgentActivity::BackgroundEnded, now);
        ledger.apply_at(
            "claude",
            "default",
            "a",
            AgentActivity::TurnStarted,
            now + IDLE_GAP,
        );
        assert!(record_in(&mut ledger, "claude", "default", reading(25.0, 1), &a, now).is_empty());
    }

    #[test]
    fn a_turn_reported_after_the_runtime_answered_is_not_settled_away() {
        let mut ledger = Ledger::default();
        let asked = Instant::now();
        ledger.apply_at("claude", "default", "a", AgentActivity::TurnStarted, asked);
        // B starts after the runtime answered with only A.
        let later = asked + Duration::from_millis(1);
        ledger.apply_at("claude", "default", "b", AgentActivity::TurnStarted, later);
        ledger.settle_running("claude", "default", &chats(&["a"]), asked, later);
        // A ends while B works on: no idle gap for C to start after.
        ledger.apply_at(
            "claude",
            "default",
            "a",
            AgentActivity::TurnEnded { background: false },
            later,
        );
        ledger.apply_at(
            "claude",
            "default",
            "c",
            AgentActivity::TurnStarted,
            later + IDLE_GAP,
        );
        let entry = &ledger.accounts[&("claude".to_owned(), "default".to_owned())];
        assert!(!entry.restarted_after_idle);
        assert!(entry.turns.contains("b"));
    }

    #[test]
    fn reports_newer_than_the_runtimes_answer_correct_it() {
        let mut ledger = Ledger::default();
        let asked = Instant::now();
        let later = asked + Duration::from_millis(1);
        let report = |chat: &str, provider: &str, tag: &str, what, at| -> Activity {
            (
                chat.to_owned(),
                provider.to_owned(),
                tag.to_owned(),
                what,
                at,
            )
        };
        ledger.turn_tagged(
            "claude",
            "default#x",
            "a",
            AgentActivity::TurnStarted,
            asked,
        );
        let mut working = vec![("a".to_owned(), "default#x".to_owned())];
        // Background work ending while its turn runs leaves it at work.
        apply_reports(
            &ledger,
            "claude",
            "default",
            asked,
            &mut working,
            vec![report(
                "a",
                "claude",
                "default#x",
                AgentActivity::BackgroundEnded,
                later,
            )],
        );
        assert_eq!(working.len(), 1);
        // A turn that ended with nothing left running leaves; one that started
        // joins; reports from before the answer change nothing.
        let ended = AgentActivity::TurnEnded { background: false };
        apply_reports(
            &ledger,
            "claude",
            "default",
            asked,
            &mut working,
            vec![
                report("a", "claude", "default#x", ended, later),
                report(
                    "b",
                    "claude",
                    "default#x",
                    AgentActivity::TurnStarted,
                    later,
                ),
                report(
                    "c",
                    "claude",
                    "default#x",
                    AgentActivity::TurnStarted,
                    asked - Duration::from_millis(1),
                ),
                report("d", "codex", "default#x", AgentActivity::TurnStarted, later),
            ],
        );
        assert_eq!(working, vec![("b".to_owned(), "default#x".to_owned())]);
        // A process gone, reported without where it worked, takes its chat.
        apply_reports(
            &ledger,
            "claude",
            "default",
            asked,
            &mut working,
            vec![report("b", "", "", AgentActivity::BackgroundEnded, later)],
        );
        assert!(working.is_empty());
    }

    #[test]
    fn a_reading_of_another_limit_starts_afresh() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        let a = chats(&["a"]);
        record_in(&mut ledger, "codex", "default", reading(10.0, 5), &a, now);
        let weekly = Reading {
            kind: "weekly",
            ..reading(80.0, 5 + 7 * 24 * 60 * 60 * 1000)
        };
        assert!(record_in(&mut ledger, "codex", "default", weekly, &a, now).is_empty());
        let later = Reading {
            percent: 81.0,
            ..weekly
        };
        let shares = record_in(&mut ledger, "codex", "default", later, &a, now);
        assert_eq!(shares, vec![("a".to_owned(), 1.0)]);
    }

    #[test]
    fn a_fall_within_one_window_is_no_rise() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        let a = chats(&["a"]);
        record_in(&mut ledger, "codex", "default", reading(70.0, 5), &a, now);
        // Same reset time, less used: a reset credit, not a new window.
        assert!(record_in(&mut ledger, "codex", "default", reading(50.0, 5), &a, now).is_empty());
        let shares = record_in(&mut ledger, "codex", "default", reading(52.0, 5), &a, now);
        assert_eq!(shares, vec![("a".to_owned(), 2.0)]);
    }

    #[test]
    fn an_idle_gap_between_readings_is_counted_for_nobody() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        let a = chats(&["a"]);
        ledger.turn_at("claude", "default", "a", true, now);
        record_in(&mut ledger, "claude", "default", reading(10.0, 1), &a, now);
        // A queued message starting as the turn before it ends is still work.
        ledger.turn_at("claude", "default", "a", false, now);
        ledger.turn_at(
            "claude",
            "default",
            "a",
            true,
            now + Duration::from_millis(5),
        );
        let shares = record_in(&mut ledger, "claude", "default", reading(12.0, 1), &a, now);
        assert_eq!(shares, vec![("a".to_owned(), 2.0)]);
        // A turn that ends and another that starts a while later, before the
        // next reading: the rise holds idle time nobody can be charged with.
        ledger.turn_at("claude", "default", "a", false, now);
        ledger.turn_at("claude", "default", "a", true, now + IDLE_GAP);
        assert!(record_in(&mut ledger, "claude", "default", reading(20.0, 1), &a, now).is_empty());
        // Work that never stopped is counted again from there.
        let shares = record_in(&mut ledger, "claude", "default", reading(22.0, 1), &a, now);
        assert_eq!(shares, vec![("a".to_owned(), 2.0)]);
        // A turn that ends with another still running is no idle gap.
        ledger.turn("claude", "default", "b", true);
        ledger.turn("claude", "default", "a", false);
        ledger.turn("claude", "default", "a", true);
        let ab = chats(&["a", "b"]);
        let shares = record_in(&mut ledger, "claude", "default", reading(26.0, 1), &ab, now);
        assert_eq!(shares, vec![("a".to_owned(), 2.0), ("b".to_owned(), 2.0)]);
        // A turn that ends while its background work goes on: a reading that
        // finds the work still running means the account never went idle.
        ledger.turn_at("claude", "default", "b", false, now);
        ledger.turn_at("claude", "default", "a", false, now);
        record_in(&mut ledger, "claude", "default", reading(27.0, 1), &a, now);
        ledger.turn_at("claude", "default", "a", true, now + IDLE_GAP);
        let shares = record_in(&mut ledger, "claude", "default", reading(29.0, 1), &a, now);
        assert_eq!(shares, vec![("a".to_owned(), 2.0)]);
        // A turn that ends with background work going on keeps the account
        // busy until a pass finds that work gone.
        ledger.turn_at("claude", "default", "a", false, now);
        ledger.turn_at("claude", "default", "c", true, now);
        ledger.turn_at("claude", "default", "c", true, now + IDLE_GAP);
        ledger.settle_running(
            "claude",
            "default",
            &chats(&["c"]),
            now + IDLE_GAP,
            now + IDLE_GAP,
        );
        ledger.turn_at("claude", "default", "a", true, now + IDLE_GAP * 2);
        let ac = chats(&["a", "c"]);
        let shares = record_in(&mut ledger, "claude", "default", reading(33.0, 1), &ac, now);
        assert_eq!(shares, vec![("a".to_owned(), 2.0), ("c".to_owned(), 2.0)]);
        // Once that work is gone, the account is idle from then on.
        ledger.turn_at("claude", "default", "a", false, now);
        ledger.settle_running("claude", "default", &[], now + IDLE_GAP * 3, now);
        ledger.turn_at("claude", "default", "a", true, now + IDLE_GAP);
        assert!(record_in(&mut ledger, "claude", "default", reading(40.0, 1), &a, now).is_empty());
    }

    #[test]
    fn quotas_count_against_the_five_hour_limit_or_else_the_weekly() {
        let window = |kind: &str, label: &str, used: f64| Headroom {
            kind: kind.into(),
            label: label.into(),
            used_percent: used,
            resets_at_ms: Some(7),
        };
        let both = [
            window("weekly", "Weekly", 40.0),
            window("session", "5-hour", 12.0),
        ];
        assert_eq!(quota_window(&both), Some(reading(12.0, 7)));
        let weekly = [
            window("weekly", "Weekly · Fable", 80.0),
            window("weekly", "Weekly", 40.0),
        ];
        assert_eq!(
            quota_window(&weekly),
            Some(Reading {
                kind: "weekly",
                ..reading(40.0, 7)
            })
        );
        assert_eq!(quota_window(&[window("monthly", "Monthly", 1.0)]), None);
    }

    #[test]
    fn the_runtime_tells_the_meter_of_each_turn() {
        let root = std::env::temp_dir().join(format!(
            "xd-quota-meter-{}-{}",
            std::process::id(),
            crate::accounts::now_ms()
        ));
        let store =
            Arc::new(StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap());
        let events = Arc::new(EventBus::default());
        let runtime = TurnRuntime::new(
            store.clone(),
            events.clone(),
            Arc::new(crate::secrets::SecretsStore::new(None)),
        );
        // The runtime's news is queued, and the meter woken, without the
        // ledger, which the meter may hold while it waits on the runtime.
        let activity = Arc::new(Mutex::new(Vec::new()));
        let (sender, woken) = mpsc::channel();
        let sender = Arc::new(Mutex::new(sender));
        runtime.set_activity_handler(activity_handler(activity.clone(), Arc::downgrade(&sender)));
        runtime.note_activity_for_test(
            "chat",
            "claude",
            "second#login",
            AgentActivity::TurnStarted,
        );
        assert!(woken.try_recv().is_ok());
        let queued = activity.lock().unwrap().clone();
        assert_eq!(queued.len(), 1);
        assert_eq!(
            (
                &queued[0].0[..],
                &queued[0].1[..],
                &queued[0].2[..],
                queued[0].3
            ),
            ("chat", "claude", "second#login", AgentActivity::TurnStarted)
        );

        // Dropping the meter ends its thread, which lets go of the runtime.
        let meter = QuotaMeter::start(
            store,
            runtime.clone(),
            events,
            Box::new(|_, _| None),
            Box::new(|_| {}),
        );
        let _ = meter.ledger();
        meter.wake();
        let held = runtime.handles();
        drop(meter);
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.handles() >= held {
            assert!(
                Instant::now() < deadline,
                "the meter's thread kept the runtime"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
