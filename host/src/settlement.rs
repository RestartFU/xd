//! Settling: moving finished sessions out of the active list without deleting
//! them, by hand or on the host's own schedule.
//!
//! The rules run in the daemon so they keep working while every client is
//! closed. A sweep only ever changes where a session is listed: it stops no
//! agent process and closes no terminal.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::{EventBus, StateStore, runtime::TurnRuntime};

pub(crate) const DEFAULT_AFTER_DAYS: i64 = 3;
pub(crate) const MIN_AFTER_DAYS: i64 = 1;
pub(crate) const MAX_AFTER_DAYS: i64 = 90;
const DAY_SECONDS: i64 = 24 * 60 * 60;
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// A pull request is looked up over the network, and most sweeps change
/// nothing about it, so one answer serves several sweeps.
const PULL_REQUEST_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// How a host settles sessions by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoSettleSettings {
    /// Settle after this many days without activity; `None` turns that off.
    pub after_days: Option<i64>,
    /// Settle when the session's pull request merges. A closed pull request
    /// settles the session either way, as long as any rule is on.
    pub on_merge: bool,
    /// Do not let inactivity settle a session that has a pull request.
    pub keep_pull_requests: bool,
}

impl Default for AutoSettleSettings {
    fn default() -> Self {
        Self {
            after_days: Some(DEFAULT_AFTER_DAYS),
            on_merge: true,
            keep_pull_requests: false,
        }
    }
}

impl AutoSettleSettings {
    fn configured(&self) -> bool {
        self.on_merge || self.after_days.is_some()
    }

    /// How the rules are stored: what a client sees, less the reply's `ok`.
    pub(crate) fn encoded(&self) -> String {
        json!({
            "after_days": self.after_days,
            "on_merge": self.on_merge,
            "keep_pull_requests": self.keep_pull_requests,
        })
        .to_string()
    }

    /// Reads the stored rules. Anything unreadable falls back to its default,
    /// so a damaged row cannot stop the host, and the next change replaces it.
    pub(crate) fn decode(stored: &str) -> Self {
        let defaults = Self::default();
        let Ok(stored) = serde_json::from_str::<Value>(stored) else {
            return defaults;
        };
        Self {
            after_days: match stored.get("after_days") {
                Some(Value::Null) => None,
                Some(days) => days
                    .as_i64()
                    .filter(|days| (MIN_AFTER_DAYS..=MAX_AFTER_DAYS).contains(days))
                    .or(defaults.after_days),
                None => defaults.after_days,
            },
            on_merge: stored
                .get("on_merge")
                .and_then(Value::as_bool)
                .unwrap_or(defaults.on_merge),
            keep_pull_requests: stored
                .get("keep_pull_requests")
                .and_then(Value::as_bool)
                .unwrap_or(defaults.keep_pull_requests),
        }
    }

    pub(crate) fn reply(&self) -> Value {
        json!({
            "ok": true,
            "after_days": self.after_days,
            "on_merge": self.on_merge,
            "keep_pull_requests": self.keep_pull_requests,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PullRequestState {
    Open,
    Closed,
    Merged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PullRequestSnapshot {
    pub(crate) state: PullRequestState,
    /// When it merged or closed, in seconds.
    pub(crate) finished_at: Option<i64>,
}

/// A session a sweep may settle: nothing is running, queued, or overridden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettlementCandidate {
    pub(crate) session: String,
    pub(crate) chats: Vec<String>,
    /// The session's newest message or start, in seconds.
    pub(crate) activity_at: i64,
    /// When the user last spoke in the session, or else when it started.
    pub(crate) user_anchor_at: i64,
    pub(crate) workdir: Option<PathBuf>,
}

/// A pull request that finished before the user's latest message records work
/// the user has since resumed, so it says nothing about this session any more.
/// GitHub reports whole seconds, so the same second proves nothing either way,
/// and it is not taken as a reason to hide a session the user just spoke in.
fn pull_request_settles(
    candidate: &SettlementCandidate,
    pull_request: &PullRequestSnapshot,
    settings: &AutoSettleSettings,
) -> bool {
    let counts = match pull_request.state {
        PullRequestState::Open => false,
        PullRequestState::Closed => true,
        PullRequestState::Merged => settings.on_merge,
    };
    counts
        && pull_request
            .finished_at
            .is_some_and(|finished_at| finished_at > candidate.user_anchor_at)
}

/// When the rules say this session settled, or `None` if they leave it active.
///
/// The answer is the moment work stopped rather than now, so the settled shelf
/// orders sessions by when they went quiet.
pub(crate) fn auto_settled_at(
    candidate: &SettlementCandidate,
    pull_request: Option<&PullRequestSnapshot>,
    now: i64,
    settings: &AutoSettleSettings,
) -> Option<i64> {
    if pull_request
        .is_some_and(|pull_request| pull_request_settles(candidate, pull_request, settings))
    {
        return Some(candidate.activity_at);
    }
    if settings.keep_pull_requests && pull_request.is_some() {
        return None;
    }
    let days = settings.after_days?;
    (candidate.activity_at < now - days * DAY_SECONDS).then_some(candidate.activity_at)
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Runs the rules in the background until it is dropped.
pub(crate) struct AutoSettler {
    wake: Mutex<Sender<()>>,
}

impl AutoSettler {
    pub(crate) fn start(
        store: Arc<StateStore>,
        runtime: TurnRuntime,
        events: Arc<EventBus>,
    ) -> Self {
        let (wake, wakes) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("xd-auto-settle".into())
            .spawn(move || {
                let mut pull_requests = HashMap::new();
                loop {
                    match wakes.recv_timeout(SWEEP_INTERVAL) {
                        Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    let has_interaction = |chat: &str| runtime.has_interactions(chat);
                    match sweep(&store, &has_interaction, &mut pull_requests, now_seconds()) {
                        Ok(0) => {}
                        Ok(_) => events.publish(json!({"event": "tree"})),
                        Err(error) => eprintln!("xd: automatic settling skipped: {error}"),
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("xd: cannot start automatic settling: {error}");
        }
        Self {
            wake: Mutex::new(wake),
        }
    }

    /// Applies changed rules now rather than at the next sweep.
    pub(crate) fn wake(&self) {
        if let Ok(wake) = self.wake.lock() {
            let _ = wake.send(());
        }
    }
}

/// Answers by directory and branch: switching branches in a directory must not
/// settle a session on the strength of the branch it left.
type PullRequestCache = HashMap<(PathBuf, String), (Instant, Option<PullRequestSnapshot>)>;

/// One pass of the rules. Returns how many sessions it settled.
fn sweep(
    store: &StateStore,
    has_interaction: &dyn Fn(&str) -> bool,
    pull_requests: &mut PullRequestCache,
    now: i64,
) -> Result<usize, crate::StorageError> {
    if !store.auto_settle_settings()?.configured() {
        return Ok(0);
    }
    pull_requests.retain(|_, (read_at, _)| read_at.elapsed() < PULL_REQUEST_LIFETIME);
    let mut settled = 0;
    for candidate in store.settlement_candidates()? {
        if candidate.chats.iter().any(|chat| has_interaction(chat)) {
            continue;
        }
        // Read for each session, since a pull request lookup can outlast a
        // change in Settings. The update below refuses if one still slips in.
        let settings = store.auto_settle_settings()?;
        if !settings.configured() {
            return Ok(settled);
        }
        // Inactivity alone decides unless the pull request could veto it, so
        // the network is only asked about sessions that are still in doubt.
        let without_lookup = (!settings.keep_pull_requests)
            .then(|| auto_settled_at(&candidate, None, now, &settings))
            .flatten();
        let settled_at = match without_lookup {
            Some(settled_at) => Some(settled_at),
            None => {
                let Some(pull_request) = pull_request_for(store, &candidate, pull_requests) else {
                    continue;
                };
                auto_settled_at(&candidate, pull_request.as_ref(), now, &settings)
            }
        };
        let Some(settled_at) = settled_at else {
            continue;
        };
        if store.auto_settle_session(&candidate, settled_at, &settings)? {
            settled += 1;
        }
    }
    Ok(settled)
}

/// The session's pull request, or `None` when the lookup failed and the
/// session has to wait for a sweep that can tell.
fn pull_request_for(
    store: &StateStore,
    candidate: &SettlementCandidate,
    cache: &mut PullRequestCache,
) -> Option<Option<PullRequestSnapshot>> {
    let Some(workdir) = &candidate.workdir else {
        return Some(None);
    };
    let Some(branch) = store.checked_out_branch(workdir) else {
        return Some(None);
    };
    let key = (workdir.clone(), branch);
    if let Some((_, known)) = cache.get(&key) {
        return Some(*known);
    }
    match store.pull_request_snapshot(workdir) {
        Ok(pull_request) => {
            // The lookup takes a while, and it answers for the branch that was
            // checked out when it began. If that has changed, ask again later.
            if store.checked_out_branch(workdir).as_ref() != Some(&key.1) {
                return None;
            }
            cache.insert(key, (Instant::now(), pull_request));
            Some(pull_request)
        }
        Err(error) => {
            eprintln!(
                "xd: cannot read the pull request for {}: {error}",
                workdir.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 100 * DAY_SECONDS;

    fn candidate(idle_days: i64) -> SettlementCandidate {
        let activity_at = NOW - idle_days * DAY_SECONDS;
        SettlementCandidate {
            session: "session".into(),
            chats: vec!["chat".into()],
            activity_at,
            user_anchor_at: activity_at,
            workdir: None,
        }
    }

    fn pull_request(state: PullRequestState, finished_at: i64) -> PullRequestSnapshot {
        PullRequestSnapshot {
            state,
            finished_at: Some(finished_at),
        }
    }

    #[test]
    fn inactivity_settles_only_past_the_chosen_days() {
        let settings = AutoSettleSettings::default();
        assert_eq!(auto_settled_at(&candidate(2), None, NOW, &settings), None);
        assert_eq!(auto_settled_at(&candidate(3), None, NOW, &settings), None);
        let old = candidate(4);
        assert_eq!(
            auto_settled_at(&old, None, NOW, &settings),
            Some(old.activity_at),
            "a settled session is dated by when it went quiet"
        );
        let off = AutoSettleSettings {
            after_days: None,
            ..settings
        };
        assert_eq!(auto_settled_at(&candidate(40), None, NOW, &off), None);
    }

    #[test]
    fn a_merge_settles_at_once_unless_turned_off_but_a_close_always_does() {
        let settings = AutoSettleSettings::default();
        let idle = candidate(0);
        let merged = pull_request(PullRequestState::Merged, NOW + 1);
        let closed = pull_request(PullRequestState::Closed, NOW + 1);
        assert!(auto_settled_at(&idle, Some(&merged), NOW, &settings).is_some());
        let keep_merged = AutoSettleSettings {
            on_merge: false,
            ..settings
        };
        assert_eq!(
            auto_settled_at(&idle, Some(&merged), NOW, &keep_merged),
            None
        );
        assert!(auto_settled_at(&idle, Some(&closed), NOW, &keep_merged).is_some());
    }

    #[test]
    fn a_pull_request_that_finished_before_the_latest_message_does_not_count() {
        let settings = AutoSettleSettings::default();
        let resumed = candidate(0);
        let old = pull_request(PullRequestState::Merged, resumed.user_anchor_at - 1);
        assert_eq!(auto_settled_at(&resumed, Some(&old), NOW, &settings), None);
        let same_second = pull_request(PullRequestState::Merged, resumed.user_anchor_at);
        assert_eq!(
            auto_settled_at(&resumed, Some(&same_second), NOW, &settings),
            None,
            "the same second cannot show the merge came after the message"
        );
        let unknown_time = PullRequestSnapshot {
            state: PullRequestState::Merged,
            finished_at: None,
        };
        assert_eq!(
            auto_settled_at(&resumed, Some(&unknown_time), NOW, &settings),
            None
        );
    }

    #[test]
    fn keeping_pull_requests_stops_inactivity_but_not_a_merge() {
        let settings = AutoSettleSettings {
            keep_pull_requests: true,
            ..AutoSettleSettings::default()
        };
        let old = candidate(30);
        let open = PullRequestSnapshot {
            state: PullRequestState::Open,
            finished_at: None,
        };
        assert_eq!(auto_settled_at(&old, Some(&open), NOW, &settings), None);
        assert!(auto_settled_at(&old, None, NOW, &settings).is_some());
        assert!(
            auto_settled_at(
                &old,
                Some(&pull_request(PullRequestState::Merged, NOW)),
                NOW,
                &settings
            )
            .is_some()
        );
    }

    #[test]
    fn an_open_pull_request_does_not_stop_inactivity_by_default() {
        let open = PullRequestSnapshot {
            state: PullRequestState::Open,
            finished_at: None,
        };
        assert!(
            auto_settled_at(
                &candidate(30),
                Some(&open),
                NOW,
                &AutoSettleSettings::default()
            )
            .is_some()
        );
    }

    /// A store with one session of one chat, removed when the test ends.
    struct Fixture {
        root: PathBuf,
        store: StateStore,
        session: String,
        chat: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "xd-settlement-test-{}",
                uuid::Uuid::new_v4().simple()
            ));
            let workspaces = root.join("Workspaces");
            std::fs::create_dir_all(&workspaces).unwrap();
            let store = StateStore::open(&root.join("chats.db"), &workspaces).unwrap();
            let folder = store.new_folder(&json!({"name": "Project"})).unwrap()["id"].clone();
            let chat = store
                .new_chat(&json!({"folder": folder, "title": "Task"}))
                .unwrap();
            Self {
                root,
                store,
                session: chat["session"].as_str().unwrap().to_owned(),
                chat: chat["id"].as_str().unwrap().to_owned(),
            }
        }

        fn sql(&self, statement: &str) {
            rusqlite::Connection::open(self.root.join("chats.db"))
                .unwrap()
                .execute_batch(statement)
                .unwrap();
        }

        /// Makes the session look like nothing has happened in it for `days`.
        fn age(&self, days: i64) {
            let at = now_seconds() - days * DAY_SECONDS;
            self.sql(&format!(
                "UPDATE chats SET created_at = {at}, last_user_message_at = {micros}; \
                 UPDATE messages SET created_at = {at}; \
                 UPDATE workspace_sessions SET created_at = {at};",
                micros = at * 1_000_000
            ));
        }

        fn request(&self) -> Value {
            json!({"session": self.session})
        }

        fn listed_settled_at(&self) -> Option<i64> {
            let tree = self.store.tree().unwrap();
            let chat = tree["chats"][0]["settled_at"].as_i64();
            assert_eq!(chat, tree["sessions"][0]["settled_at"].as_i64());
            chat
        }

        fn sweep(&self) -> usize {
            sweep(&self.store, &|_| false, &mut HashMap::new(), now_seconds()).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_settled_session_is_listed_as_settled_until_a_message_arrives() {
        let fixture = Fixture::new();
        assert_eq!(fixture.store.tree().unwrap()["settlement"], true);
        assert_eq!(fixture.listed_settled_at(), None);
        fixture.store.settle_session(&fixture.request()).unwrap();
        let settled_at = fixture.listed_settled_at().expect("settled");

        fixture.sql(&format!(
            "UPDATE workspace_sessions SET settled_at = {}",
            settled_at - 100
        ));
        fixture.store.settle_session(&fixture.request()).unwrap();
        assert_eq!(
            fixture.listed_settled_at(),
            Some(settled_at - 100),
            "settling twice keeps the original time"
        );

        assert!(
            fixture
                .store
                .chat_has_settlement_override(&fixture.chat)
                .unwrap()
        );
        fixture
            .store
            .prepare_send(&json!({"chat": fixture.chat, "text": "more"}))
            .unwrap();
        assert_eq!(fixture.listed_settled_at(), None);
        assert!(
            !fixture
                .store
                .chat_has_settlement_override(&fixture.chat)
                .unwrap(),
            "new activity hands the session back to the rules"
        );
    }

    #[test]
    fn settling_waits_for_a_running_turn_and_a_queued_message() {
        let fixture = Fixture::new();
        fixture
            .store
            .prepare_send(&json!({"chat": fixture.chat, "text": "work"}))
            .unwrap();
        let running = fixture
            .store
            .settle_session(&fixture.request())
            .unwrap_err();
        assert!(running.to_string().contains("still running"));

        fixture
            .store
            .prepare_send(&json!({"chat": fixture.chat, "text": "later"}))
            .unwrap();
        fixture.sql("UPDATE chats SET host_working = 0");
        let queued = fixture
            .store
            .settle_session(&fixture.request())
            .unwrap_err();
        assert!(queued.to_string().contains("queued message"));
        assert_eq!(fixture.listed_settled_at(), None);
    }

    #[test]
    fn a_message_queued_for_a_settled_session_also_brings_it_back() {
        let fixture = Fixture::new();
        fixture.store.settle_session(&fixture.request()).unwrap();
        fixture.sql("UPDATE chats SET host_working = 1");
        fixture
            .store
            .prepare_send(&json!({"chat": fixture.chat, "text": "queued"}))
            .unwrap();
        assert_eq!(fixture.listed_settled_at(), None);
    }

    #[test]
    fn un_settling_keeps_the_rules_away_until_new_activity() {
        let fixture = Fixture::new();
        fixture.store.settle_session(&fixture.request()).unwrap();
        fixture.store.unsettle_session(&fixture.request()).unwrap();
        assert_eq!(fixture.listed_settled_at(), None);

        fixture.age(30);
        assert_eq!(fixture.sweep(), 0, "the user just asked for it back");

        assert!(fixture.store.clear_chat_settlement(&fixture.chat).unwrap());
        assert!(!fixture.store.clear_chat_settlement(&fixture.chat).unwrap());
        assert_eq!(fixture.sweep(), 1);
    }

    #[test]
    fn a_session_can_opt_out_of_the_rules_but_still_settle_by_hand() {
        let fixture = Fixture::new();
        fixture.age(30);
        fixture
            .store
            .set_session_auto_settle(&json!({"session": fixture.session, "enabled": false}))
            .unwrap();
        let tree = fixture.store.tree().unwrap();
        assert_eq!(tree["chats"][0]["auto_settle_disabled"], true);
        assert_eq!(tree["sessions"][0]["auto_settle_disabled"], true);
        assert_eq!(fixture.sweep(), 0);

        fixture.store.settle_session(&fixture.request()).unwrap();
        assert!(fixture.listed_settled_at().is_some());
        fixture.store.unsettle_session(&fixture.request()).unwrap();

        fixture.store.clear_chat_settlement(&fixture.chat).unwrap();
        fixture
            .store
            .set_session_auto_settle(&json!({"session": fixture.session, "enabled": true}))
            .unwrap();
        assert!(fixture.store.tree().unwrap()["chats"][0]["auto_settle_disabled"].is_null());
        assert_eq!(fixture.sweep(), 1);
    }

    #[test]
    fn the_rules_are_saved_and_checked() {
        let fixture = Fixture::new();
        let store = &fixture.store;
        assert_eq!(
            store.auto_settle_settings().unwrap(),
            AutoSettleSettings::default()
        );
        for bad in [json!(0), json!(91), json!("3"), json!(1.5)] {
            assert!(
                store
                    .set_auto_settle_settings(&json!({"after_days": bad}))
                    .is_err()
            );
        }
        assert!(
            store
                .set_auto_settle_settings(&json!({"on_merge": "yes"}))
                .is_err()
        );
        let changed = store
            .set_auto_settle_settings(&json!({"after_days": 14, "keep_pull_requests": true}))
            .unwrap();
        assert_eq!(
            changed,
            AutoSettleSettings {
                after_days: Some(14),
                on_merge: true,
                keep_pull_requests: true,
            }
        );
        let off = store
            .set_auto_settle_settings(&json!({"after_days": null}))
            .unwrap();
        assert_eq!(off.after_days, None);
        assert!(off.keep_pull_requests, "members left out keep their value");
        let reopened = StateStore::open(
            &fixture.root.join("chats.db"),
            &fixture.root.join("Workspaces"),
        )
        .unwrap();
        assert_eq!(reopened.auto_settle_settings().unwrap(), off);

        fixture.sql("UPDATE meta SET value = 'garbage' WHERE key = 'auto_settle'");
        assert_eq!(
            store.auto_settle_settings().unwrap(),
            AutoSettleSettings::default()
        );
    }

    #[test]
    fn a_sweep_settles_only_what_has_gone_quiet() {
        let fixture = Fixture::new();
        fixture.age(1);
        assert_eq!(fixture.sweep(), 0);

        fixture.age(4);
        fixture.sql("UPDATE chats SET host_working = 1");
        assert_eq!(fixture.sweep(), 0, "a running turn is not quiet");
        fixture.sql("UPDATE chats SET host_working = 0, queued = '[\"next\"]'");
        assert_eq!(fixture.sweep(), 0, "a queued message is not quiet");
        fixture.sql("UPDATE chats SET queued = NULL");
        let waiting = sweep(
            &fixture.store,
            &|chat| chat == fixture.chat,
            &mut HashMap::new(),
            now_seconds(),
        )
        .unwrap();
        assert_eq!(waiting, 0, "a pending approval is not quiet");

        fixture
            .store
            .set_auto_settle_settings(&json!({"after_days": null, "on_merge": false}))
            .unwrap();
        assert_eq!(fixture.sweep(), 0, "both rules are off");
        fixture
            .store
            .set_auto_settle_settings(&json!({"after_days": 3, "on_merge": true}))
            .unwrap();

        assert_eq!(fixture.sweep(), 1);
        let settled_at = fixture.listed_settled_at().expect("settled");
        assert!(
            settled_at <= now_seconds() - 4 * DAY_SECONDS,
            "dated by when the session went quiet"
        );
        assert_eq!(fixture.sweep(), 0, "already settled");
    }

    #[test]
    fn the_newest_tab_decides_how_quiet_a_session_is() {
        let fixture = Fixture::new();
        let second = fixture
            .store
            .new_chat(&json!({"session": fixture.session}))
            .unwrap();
        fixture.age(10);
        fixture.sql(&format!(
            "INSERT INTO messages (chat_id, role, content, created_at) VALUES ('{}', 'assistant', 'hi', {})",
            second["id"].as_str().unwrap(),
            now_seconds()
        ));
        let candidates = fixture.store.settlement_candidates().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].chats.len(), 2);
        assert_eq!(fixture.sweep(), 0);
    }

    #[test]
    fn activity_after_the_sweep_looked_stops_it_settling() {
        let fixture = Fixture::new();
        fixture.age(10);
        let candidate = fixture.store.settlement_candidates().unwrap().remove(0);
        fixture
            .store
            .prepare_send(&json!({"chat": fixture.chat, "text": "hold on"}))
            .unwrap();
        fixture.sql("UPDATE chats SET host_working = 0");
        assert!(
            !fixture
                .store
                .auto_settle_session(
                    &candidate,
                    candidate.activity_at,
                    &AutoSettleSettings::default()
                )
                .unwrap()
        );
        assert_eq!(fixture.listed_settled_at(), None);
    }

    #[test]
    fn rules_changed_after_the_sweep_decided_stop_it_settling() {
        let fixture = Fixture::new();
        fixture.age(10);
        let candidate = fixture.store.settlement_candidates().unwrap().remove(0);
        let decided_under = fixture.store.auto_settle_settings().unwrap();
        let changed = fixture
            .store
            .set_auto_settle_settings(&json!({"after_days": null, "on_merge": false}))
            .unwrap();
        assert!(
            !fixture
                .store
                .auto_settle_session(&candidate, candidate.activity_at, &decided_under)
                .unwrap()
        );
        assert_eq!(fixture.listed_settled_at(), None);
        assert!(
            fixture
                .store
                .auto_settle_session(&candidate, candidate.activity_at, &changed)
                .unwrap()
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_session_in_a_subdirectory_still_answers_for_its_branch() {
        with_fake_gh(
            "echo '{\"state\":\"OPEN\",\"finished_at\":null}'",
            |fixture, workdir| {
                let nested = workdir.join("packages/app");
                std::fs::create_dir_all(&nested).unwrap();
                fixture.sql(&format!(
                    "UPDATE chats SET workdir = '{}'",
                    nested.display()
                ));
                assert_eq!(
                    fixture.store.checked_out_branch(&nested).as_deref(),
                    Some("feature")
                );
                fixture
                    .store
                    .set_auto_settle_settings(&json!({"keep_pull_requests": true}))
                    .unwrap();
                fixture.age(30);
                assert_eq!(fixture.sweep(), 0, "its pull request keeps it");
            },
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_detached_head_or_no_repository_has_no_branch() {
        with_fake_gh("exit 1", |fixture, workdir| {
            let detached = std::process::Command::new("git")
                .args(["checkout", "-q", "--detach"])
                .current_dir(workdir)
                .status()
                .unwrap();
            assert!(detached.success());
            assert_eq!(fixture.store.checked_out_branch(workdir), None);
        });
        let fixture = Fixture::new();
        assert_eq!(
            fixture
                .store
                .checked_out_branch(&fixture.root.join("Workspaces")),
            None
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_branch_switched_during_the_lookup_discards_its_answer() {
        let merged_at = now_seconds() + 60;
        with_fake_gh(
            &format!(
                "git checkout -q -b other; \
                 echo '{{\"state\":\"MERGED\",\"finished_at\":{merged_at}}}'"
            ),
            |fixture, _| {
                let mut cache = HashMap::new();
                assert_eq!(
                    sweep(&fixture.store, &|_| false, &mut cache, now_seconds()).unwrap(),
                    0
                );
                assert!(cache.is_empty(), "the answer was for the branch it left");
                assert_eq!(fixture.listed_settled_at(), None);
            },
        );
    }

    #[test]
    fn a_session_without_a_repository_has_no_pull_request_to_keep_it() {
        let fixture = Fixture::new();
        fixture
            .store
            .set_auto_settle_settings(&json!({"keep_pull_requests": true}))
            .unwrap();
        fixture.age(10);
        assert_eq!(fixture.sweep(), 1);
    }

    /// Runs `body` with a stand-in for the GitHub CLI inside a real repository.
    fn with_fake_gh(script: &str, body: impl FnOnce(&Fixture, &std::path::Path)) {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = Fixture::new();
        let workdir = fixture.root.join("Workspaces/Project");
        let git = |arguments: &[&str]| {
            let status = std::process::Command::new("git")
                .args(arguments)
                .current_dir(&workdir)
                .env("GIT_AUTHOR_NAME", "x")
                .env("GIT_AUTHOR_EMAIL", "x@example.com")
                .env("GIT_COMMITTER_NAME", "x")
                .env("GIT_COMMITTER_EMAIL", "x@example.com")
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&["init", "-q", "-b", "feature"]);
        git(&["commit", "-q", "--allow-empty", "-m", "start"]);
        let gh = fixture.root.join("gh");
        std::fs::write(&gh, format!("#!/bin/sh\n{script}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::set_var("XD_GH_EXECUTABLE", &gh) };
        body(&fixture, &workdir);
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::remove_var("XD_GH_EXECUTABLE") };
    }

    #[test]
    #[cfg(unix)]
    fn a_merged_pull_request_settles_a_session_right_away() {
        let merged_at = now_seconds() + 60;
        with_fake_gh(
            &format!("echo '{{\"state\":\"MERGED\",\"finished_at\":{merged_at}}}'"),
            |fixture, workdir| {
                assert_eq!(
                    fixture.store.pull_request_snapshot(workdir).unwrap(),
                    Some(PullRequestSnapshot {
                        state: PullRequestState::Merged,
                        finished_at: Some(merged_at),
                    })
                );
                assert_eq!(fixture.sweep(), 1);
            },
        );
    }

    #[test]
    #[cfg(unix)]
    fn an_open_pull_request_keeps_a_session_when_asked_to() {
        with_fake_gh(
            "echo '{\"state\":\"OPEN\",\"finished_at\":null}'",
            |fixture, _| {
                fixture
                    .store
                    .set_auto_settle_settings(&json!({"keep_pull_requests": true}))
                    .unwrap();
                fixture.age(30);
                assert_eq!(fixture.sweep(), 0);
                fixture
                    .store
                    .set_auto_settle_settings(&json!({"keep_pull_requests": false}))
                    .unwrap();
                assert_eq!(fixture.sweep(), 1);
            },
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_failed_lookup_is_not_taken_to_mean_no_pull_request() {
        with_fake_gh("echo 'HTTP 502' >&2; exit 1", |fixture, workdir| {
            assert!(fixture.store.pull_request_snapshot(workdir).is_err());
            fixture
                .store
                .set_auto_settle_settings(&json!({"keep_pull_requests": true}))
                .unwrap();
            fixture.age(30);
            assert_eq!(fixture.sweep(), 0, "it may have a pull request to keep it");
        });
    }

    #[test]
    #[cfg(unix)]
    fn a_branch_without_a_pull_request_settles_on_inactivity_alone() {
        with_fake_gh(
            "echo 'no pull requests found for branch \"feature\"' >&2; exit 1",
            |fixture, workdir| {
                assert_eq!(fixture.store.pull_request_snapshot(workdir).unwrap(), None);
                fixture
                    .store
                    .set_auto_settle_settings(&json!({"keep_pull_requests": true}))
                    .unwrap();
                assert_eq!(fixture.sweep(), 0, "and it is not old enough yet");
                fixture.age(30);
                assert_eq!(fixture.sweep(), 1);
            },
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_pull_request_lookup_serves_several_sweeps() {
        // The merge came before the user's latest message, so the session
        // stays active and each sweep needs the answer again.
        // The stand-in counts only this test's calls: a test elsewhere that asks
        // for pull requests without the lock can reach it while it is installed.
        let merged_at = now_seconds() - DAY_SECONDS * 5;
        with_fake_gh(
            &format!(
                "case \"$PWD\" in \"$(dirname \"$0\")\"/*) echo x >> \"$(dirname \"$0\")/calls\";; esac; \
                 echo '{{\"state\":\"MERGED\",\"finished_at\":{merged_at}}}'"
            ),
            |fixture, _| {
                let mut cache = HashMap::new();
                let now = now_seconds();
                for _ in 0..3 {
                    assert_eq!(
                        sweep(&fixture.store, &|_| false, &mut cache, now).unwrap(),
                        0
                    );
                }
                let calls = || {
                    std::fs::read_to_string(fixture.root.join("calls"))
                        .unwrap()
                        .lines()
                        .count()
                };
                assert_eq!(calls(), 1);

                // Another branch in the same directory is another pull request.
                let switched = std::process::Command::new("git")
                    .args(["checkout", "-q", "-b", "other"])
                    .current_dir(fixture.root.join("Workspaces/Project"))
                    .status()
                    .unwrap();
                assert!(switched.success());
                sweep(&fixture.store, &|_| false, &mut cache, now).unwrap();
                assert_eq!(calls(), 2);
            },
        );
    }

    #[test]
    fn a_turn_the_host_resumes_is_not_the_user_coming_back() {
        let fixture = Fixture::new();
        fixture
            .store
            .prepare_send(&json!({"chat": fixture.chat, "text": "work"}))
            .unwrap();
        fixture.age(1);
        fixture.sql("UPDATE chats SET host_working = 0");
        let anchor = fixture.store.settlement_candidates().unwrap()[0].user_anchor_at;
        let last_user_message: i64 = rusqlite::Connection::open(fixture.root.join("chats.db"))
            .unwrap()
            .query_row(
                "SELECT MAX(id) FROM messages WHERE role = 'user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        fixture
            .store
            .prepare_continuation(
                &json!({"chat": fixture.chat, "text": "continue"}),
                last_user_message,
            )
            .unwrap();
        fixture.sql("UPDATE chats SET host_working = 0");
        let candidate = fixture.store.settlement_candidates().unwrap().remove(0);
        assert_eq!(
            candidate.user_anchor_at, anchor,
            "a pull request that finished meanwhile still counts"
        );
        assert!(
            candidate.activity_at > anchor,
            "but the turn is still activity, so inactivity counts from it"
        );
    }

    #[test]
    fn databases_from_before_settling_gain_the_columns() {
        let fixture = Fixture::new();
        fixture.sql(
            "ALTER TABLE workspace_sessions DROP COLUMN settled_override; \
             ALTER TABLE workspace_sessions DROP COLUMN settled_at; \
             ALTER TABLE workspace_sessions DROP COLUMN auto_settle_disabled_at;",
        );
        let reopened = StateStore::open(
            &fixture.root.join("chats.db"),
            &fixture.root.join("Workspaces"),
        )
        .unwrap();
        assert!(reopened.tree().unwrap()["chats"][0]["settled_at"].is_null());
        reopened.settle_session(&fixture.request()).unwrap();
        assert!(reopened.tree().unwrap()["chats"][0]["settled_at"].is_i64());
    }
}
