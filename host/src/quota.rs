//! Holds chats to a share of their account's usage limit.
//!
//! The assistants report usage per account, not per chat, so a chat's use is
//! estimated: while a chat with a quota works, its account's limit is read
//! about once a minute and when a turn ends, and each rise is shared evenly
//! among the chats that worked on the account since the reading before. A chat
//! whose share reaches its quota is stopped, and only the user's new quota lets
//! it go on.

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

use crate::{EventBus, accounts::Headroom, runtime::TurnRuntime, storage::StateStore};

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
}

/// The limit quotas count against: the 5-hour one, or the weekly one when an
/// account has no 5-hour limit. A weekly limit of one model alone is never it.
pub(crate) fn quota_window(windows: &[Headroom]) -> Option<Reading> {
    let pick = |kind: &str| {
        windows
            .iter()
            .find(|window| window.kind == kind && !window.label.contains('·'))
    };
    pick("session")
        .or_else(|| pick("weekly"))
        .filter(|window| window.used_percent.is_finite())
        .map(|window| Reading {
            percent: window.used_percent,
            resets_at_ms: window.resets_at_ms,
        })
}

#[derive(Debug, Default)]
struct AccountLedger {
    /// The reading the next rise is measured from.
    last: Option<Reading>,
    read_at: Option<Instant>,
    /// The chats running a turn here, by their starts and ends.
    running: HashSet<String>,
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
}

/// What the meter knows of each account, by assistant and account id.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    accounts: HashMap<(String, String), AccountLedger>,
}

impl AccountLedger {
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

    /// A chat started or ended a turn on the account.
    pub(crate) fn turn(&mut self, provider: &str, account: &str, chat_id: &str, started: bool) {
        self.turn_at(provider, account, chat_id, started, Instant::now());
    }

    fn turn_at(
        &mut self,
        provider: &str,
        account: &str,
        chat_id: &str,
        started: bool,
        now: Instant,
    ) {
        self.note(provider, account, chat_id);
        let Some(entry) = self
            .accounts
            .get_mut(&(provider.to_owned(), account.to_owned()))
        else {
            return;
        };
        if started {
            entry.running.insert(chat_id.to_owned());
            if entry
                .idle_since
                .take()
                .is_some_and(|since| now.saturating_duration_since(since) >= IDLE_GAP)
            {
                entry.restarted_after_idle = true;
            }
        } else {
            entry.running.remove(chat_id);
            if entry.running.is_empty() {
                entry.idle_since = Some(now);
            }
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
            .filter(|_| !entry.restarted_after_idle)
            .map_or(0.0, |last| rise(last, reading));
        entry.restarted_after_idle = false;
        let since_ms = std::mem::replace(&mut entry.asked_ms, asked_ms);
        entry.last = Some(reading);
        entry.read_at = Some(now);
        entry.failed = false;
        entry.idle = working.is_empty();
        login.clone_into(&mut entry.login);
        entry.sharers = working.iter().cloned().collect();
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
    let new_window = match (last.resets_at_ms, now.resets_at_ms) {
        (Some(before), Some(after)) => after - before > NEW_WINDOW_MS,
        _ => false,
    } || now.percent + 0.5 < last.percent;
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
    wake: Mutex<Sender<()>>,
}

struct Pass {
    store: Arc<StateStore>,
    runtime: TurnRuntime,
    events: Arc<EventBus>,
    reader: Reader,
    stopper: Stopper,
    ledger: Arc<Mutex<Ledger>>,
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
        let (wake, wakes) = mpsc::channel::<()>();
        {
            let ledger = ledger.clone();
            let wake = Mutex::new(wake.clone());
            runtime.set_activity_handler(Box::new(move |chat_id, provider, tag, started| {
                if let Ok(mut ledger) = ledger.lock() {
                    ledger.turn(
                        provider,
                        crate::accounts::tag_account(tag),
                        chat_id,
                        started,
                    );
                }
                if let Ok(wake) = wake.lock() {
                    let _ = wake.send(());
                }
            }));
        }
        let pass = Pass {
            store,
            runtime,
            events,
            reader,
            stopper,
            ledger: ledger.clone(),
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
        Self {
            ledger,
            wake: Mutex::new(wake),
        }
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
        let working = by_account(self.runtime.working_accounts());
        for (chat_id, provider, account) in &working {
            if let Ok(mut ledger) = self.ledger.lock() {
                ledger.note(provider, account, chat_id);
            }
        }
        let limited = |chat_id: &str| {
            self.store
                .chat_quota(chat_id)
                .is_ok_and(|quota| quota.percent.is_some() && !quota.reached)
        };
        // A chat already past its quota, such as after the user lowered it,
        // stops without waiting for a reading.
        for (chat_id, _, _) in &working {
            if self
                .store
                .chat_quota(chat_id)
                .is_ok_and(|quota| !quota.reached && quota.spent())
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
            // Read without the ledger, which turns starting and ending wait on.
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
                let idle = self.working_on(&provider, &account).is_empty();
                if let Ok(mut ledger) = self.ledger.lock()
                    && let Some(entry) = ledger.accounts.get_mut(&key)
                {
                    entry.failed = true;
                    if idle {
                        entry.last = None;
                    }
                }
                next = next.min(FAILED_READ_GAP);
                continue;
            };
            // Who works on the account now, after the reading.
            let working_now = self.working_on(&provider, &account);
            // A turn known to run as another login than the one read, such as
            // after the user's own CLI signed in to someone else, is not
            // measured by this reading, nor is anyone beside it.
            if working_now
                .iter()
                .any(|(_, tag)| !tag.ends_with("#?") && !crate::accounts::same_login(tag, &login))
            {
                if let Ok(mut ledger) = self.ledger.lock()
                    && let Some(entry) = ledger.accounts.get_mut(&key)
                {
                    entry.last = None;
                    entry.failed = true;
                }
                next = next.min(FAILED_READ_GAP);
                continue;
            }
            let here_now = working_now
                .into_iter()
                .map(|(chat_id, _)| chat_id)
                .collect::<Vec<_>>();
            let (shares, since_ms) = match self.ledger.lock() {
                Ok(mut ledger) => ledger.record(
                    &provider,
                    &account,
                    reading,
                    &login,
                    &here_now,
                    Instant::now(),
                    asked_ms,
                ),
                Err(_) => return next,
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
                self.events
                    .publish(json!({"event": "changed", "chat": chat_id}));
                if counted.spent() {
                    (self.stopper)(&chat_id);
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
        assert_eq!(quota_window(&weekly), Some(reading(40.0, 7)));
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
        let meter = QuotaMeter::start(
            store,
            runtime.clone(),
            events,
            Box::new(|_, _| None),
            Box::new(|_| {}),
        );
        runtime.note_activity_for_test("chat", "claude", "second#login", true);
        let ledger = meter.ledger();
        let ledger = ledger.lock().unwrap();
        // Filed by account; which login a reading was of is checked apart.
        let entry = &ledger.accounts[&("claude".to_owned(), "second".to_owned())];
        assert!(entry.sharers.contains("chat"));
        drop(ledger);
        meter.wake();
        let _ = std::fs::remove_dir_all(root);
    }
}
