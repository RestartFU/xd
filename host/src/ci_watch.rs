//! Watching a pull request's CI for a chat's agent, through xd's `watch_ci`
//! tool.
//!
//! An agent that pushed would otherwise sit on `gh run watch` or a polling
//! loop in its shell, holding its turn open and filling the transcript with
//! raw commands. Instead it asks xd, ends its turn, and xd sends the chat one
//! message when the checks on the pull request's head have all finished:
//! which passed, which failed, and the end of each failed job's log.
//!
//! Each watch is a row in `ci_watches` with a token of its own. A thread polls
//! for it only while that row, with that token, is still there: asking again
//! for the same pull request replaces the token, and deleting the chat removes
//! the row, so either one ends the old thread without a word. The send that
//! stores the report removes the row in the same transaction, and only while
//! it still holds the token. So a report is sent once, never by a replaced
//! watch, and a restarted daemon goes on with any watch that had not.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock, Weak},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::{
    Engine, error_reply, pull_requests, required_string,
    storage::{AddedCiWatch, CiWatch, StateStore},
};

/// How many pull requests one chat may have watched at once.
const MAX_PER_CHAT: usize = 8;
/// How often xd asks GitHub, unless `XD_CI_POLL_MILLIS` says otherwise.
const POLL: Duration = Duration::from_secs(15);
/// Polls to wait for GitHub to show a commit the chat's branch has pushed.
const HEAD_GRACE_POLLS: u32 = 20;
/// Polls to wait for a head with no checks to get some.
const NO_CHECKS_POLLS: u32 = 12;
/// Polls in a row that may fail before the watch gives up.
const FAILED_POLLS: u32 = 8;
/// How long a watch lasts at most.
const MAX_WATCH: Duration = Duration::from_secs(6 * 60 * 60);
/// A head whose checks all ended at least this long ago is reported at once.
const SETTLED_FOR: Duration = Duration::from_secs(120);
/// How long after its last check ended a watch reports, so checks other
/// workflows or services register a little later are not missed.
const QUIET_FOR: Duration = Duration::from_secs(60);
/// Failed jobs whose logs a report quotes, and how much of each.
const LOGGED_JOBS: usize = 3;
const LOG_LINES: usize = 30;
const LOG_LINE_CHARS: usize = 300;
/// How much of the end of a failed job's log is read for its last lines.
const LOG_TAIL_BYTES: usize = 256 * 1024;
const LISTED_CHECKS: usize = 20;

/// Starts watches once the daemon is up; see [`Engine::start_ci_watches`].
#[derive(Default)]
pub(crate) struct CiWatches {
    engine: OnceLock<Weak<Engine>>,
    /// The token of the thread watching each chat's pull request, so a
    /// replaced thread stops within a moment rather than at its next poll.
    live: Mutex<HashMap<(String, String), String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PullRequestState {
    Open,
    Closed,
    Merged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Pending,
    Passed,
    Skipped,
    /// GitHub's conclusion, in lowercase words, such as `failure`.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Check {
    name: String,
    workflow: Option<String>,
    outcome: Outcome,
    link: Option<String>,
    /// When it finished, in seconds since the epoch.
    completed_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Status {
    url: String,
    number: u64,
    state: PullRequestState,
    head: String,
    head_branch: String,
    title: String,
    /// `owner/name` of the repository the head branch is in, in lowercase.
    head_repository: Option<String>,
    checks: Vec<Check>,
}

impl Status {
    fn pending(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.outcome == Outcome::Pending)
            .count()
    }

    /// Whether every check has finished, of a head that has some.
    fn complete(&self) -> bool {
        !self.checks.is_empty() && self.pending() == 0
    }

    fn short_head(&self) -> &str {
        &self.head[..self.head.len().min(7)]
    }

    /// Its head and the names of its checks, to tell one poll's set from the
    /// next.
    fn names(&self) -> Vec<String> {
        // Each run of a check, with how it ended: a check run again between
        // polls has a new link and time, so it needs a poll of its own.
        let mut names = self
            .checks
            .iter()
            .map(|check| {
                format!(
                    "{}/{}/{:?}/{}/{:?}",
                    check.workflow.as_deref().unwrap_or(""),
                    check.name,
                    check.outcome,
                    check.link.as_deref().unwrap_or(""),
                    check.completed_at,
                )
            })
            .collect::<Vec<_>>();
        names.sort();
        names.insert(0, self.head.clone());
        names
    }
}

/// Content prefix of a CI card: a `tool` row the host writes when a watch
/// starts and rewrites in place as the checks go.
pub(crate) const CARD_PREFIX: &str = "ci_run\n";
/// Checks a card lists at most.
const CARD_CHECKS: usize = 50;

/// Where a watch is, as its card shows it.
enum Phase<'a> {
    /// Still watching; `waiting_for` is a pushed commit GitHub does not show.
    Running { waiting_for: Option<&'a str> },
    /// The checks finished.
    Finished,
    /// The watch gave up, for `reason`.
    Stopped(&'a str),
}

/// The card for a watch of `url` begun at `started_at`, at `phase`, from what
/// GitHub said last, if it said anything.
fn card(url: &str, status: Option<&Status>, phase: Phase<'_>, started_at: i64) -> String {
    let failed = status.is_some_and(|status| {
        status
            .checks
            .iter()
            .any(|check| matches!(check.outcome, Outcome::Failed(_)))
    });
    let state = match phase {
        Phase::Running { .. } => "running",
        Phase::Finished if failed => "failed",
        Phase::Finished => "passed",
        Phase::Stopped(_) => "stopped",
    };
    let mut record = json!({
        "url": url,
        "status": state,
        "started_at": started_at,
        "checks": [],
    });
    if let Some(status) = status {
        record["number"] = json!(status.number);
        record["head"] = json!(status.short_head());
        if !status.title.is_empty() {
            record["title"] = json!(status.title);
        }
        // Failed and running checks first, so a long list cut short keeps the
        // ones worth seeing; the counts cover them all.
        let rank = |check: &&Check| -> u8 {
            match check.outcome {
                Outcome::Failed(_) => 0,
                Outcome::Pending => 1,
                Outcome::Passed => 2,
                Outcome::Skipped => 3,
            }
        };
        let mut checks = status.checks.iter().collect::<Vec<_>>();
        checks.sort_by_key(rank);
        let count = |wanted: u8| checks.iter().filter(|check| rank(check) == wanted).count();
        record["counts"] = json!({
            "failed": count(0),
            "pending": count(1),
            "passed": count(2),
            "skipped": count(3),
        });
        record["checks"] = checks
            .into_iter()
            .take(CARD_CHECKS)
            .map(|check| {
                let (state, conclusion) = match &check.outcome {
                    Outcome::Pending => ("pending", None),
                    Outcome::Passed => ("passed", None),
                    Outcome::Skipped => ("skipped", None),
                    Outcome::Failed(conclusion) => ("failed", Some(conclusion.as_str())),
                };
                let mut entry = json!({"name": check.name, "state": state});
                if let Some(workflow) = &check.workflow {
                    entry["workflow"] = json!(workflow);
                }
                if let Some(conclusion) = conclusion {
                    entry["conclusion"] = json!(conclusion);
                }
                if let Some(link) = &check.link {
                    entry["link"] = json!(link);
                }
                entry
            })
            .collect();
    } else if let Some(number) = url
        .rsplit('/')
        .next()
        .and_then(|number| number.parse::<u64>().ok())
    {
        record["number"] = json!(number);
    }
    match phase {
        Phase::Running {
            waiting_for: Some(commit),
        } => record["waiting_for"] = json!(&commit[..commit.len().min(7)]),
        Phase::Running { waiting_for: None } => {}
        Phase::Finished => record["finished_at"] = json!(now_seconds()),
        Phase::Stopped(reason) => {
            record["finished_at"] = json!(now_seconds());
            record["reason"] = json!(reason);
        }
    }
    format!("{CARD_PREFIX}{record}")
}

fn upstream_moved(store: &StateStore, workdir: &Path, recorded: Option<&str>) -> bool {
    upstream_commit(store, workdir).as_deref() != recorded
}

/// The commit the checkout's upstream is at, if it has one.
fn upstream_commit(store: &StateStore, workdir: &Path) -> Option<String> {
    store
        .run_git(
            workdir,
            &["rev-parse", "--verify", "--quiet", "@{upstream}"],
        )
        .ok()
        .filter(|(result, _, _)| result.success())
        .map(|(_, stdout, _)| String::from_utf8_lossy(&stdout).trim().to_ascii_lowercase())
        .filter(|commit| !commit.is_empty())
}

/// How a watch ended, for the message the chat gets.
enum Ending {
    Finished(Status),
    Stopped { url: String, reason: String },
}

impl Engine {
    /// Lets `watch-ci` start watches, and goes on with the ones a previous
    /// daemon left.
    pub fn start_ci_watches(self: &Arc<Self>) {
        let _ = self.ci_watches.engine.set(Arc::downgrade(self));
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match store.ci_watches() {
            Ok(watches) => {
                for watch in watches {
                    // Left recorded when it cannot start, for the next daemon.
                    let _ = self.ci_watches.register(|| {
                        Ok::<_, ()>(Some(AddedCiWatch {
                            watch: watch.clone(),
                            retired: None,
                        }))
                    });
                    self.ci_watches.start(watch, None);
                }
            }
            Err(error) => eprintln!("xd-host: cannot read CI watches: {error}"),
        }
    }

    /// `watch-ci`: watches the checks of `pull_request`, or of the chat's
    /// branch's pull request, and reports to the chat once they finish.
    /// Answers with the report itself when they already have.
    pub(crate) fn watch_ci(&self, request: &Value) -> Value {
        let chat = match required_string(request, "chat", "watch-ci needs a chat id.") {
            Ok(chat) => chat,
            Err(error) => return error_reply(error),
        };
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        if self.ci_watches.engine.get().is_none() {
            return error_reply("This xd host does not watch CI.");
        }
        let workdir = match store.chat_workdir(chat) {
            Ok(workdir) => workdir,
            Err(error) => return error_reply(error),
        };
        let url = match request
            .get("pull_request")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())
        {
            Some(url) => match pull_requests::normalize_url(url) {
                Some(url) => url,
                None => return error_reply("pull_request must be a pull request's full URL."),
            },
            None => match store.chat_pull_request(chat) {
                Some(found) => match found["url"].as_str().and_then(pull_requests::normalize_url) {
                    Some(url) => url,
                    None => {
                        return error_reply(
                            "This chat's branch has no pull request. Pass pull_request, its URL.",
                        );
                    }
                },
                None => {
                    return error_reply(
                        "xd could not find this chat's pull request. Pass pull_request, its URL.",
                    );
                }
            },
        };
        // Read before GitHub is, so what GitHub says is no older than it.
        let earlier = match store.ci_watch(chat, &url) {
            Ok(earlier) => earlier,
            Err(error) => return error_reply(error),
        };
        let status = match read_status(store, &workdir, &url) {
            Ok(status) => status,
            Err(error) => return error_reply(error),
        };
        if status.url != url {
            return error_reply(format!(
                "GitHub answered for {} rather than {url}.",
                status.url
            ));
        }
        let pushed = pushed_head(store, &workdir, &status);
        // A commit an earlier watch of it still waits for GitHub to show is
        // waited for still, wherever the checkout is now.
        let expected = pushed
            .clone()
            .or(earlier
                .as_ref()
                .and_then(|watch| watch.expected_head.clone()))
            .filter(|commit| {
                !commit.eq_ignore_ascii_case(&status.head)
                    && !head_contains(store, &workdir, &status.head, commit)
            });
        let mut status = status;
        // Only with no watch of it already: one that exists has promised a
        // report, which only its own end, stored with the report, may keep.
        if earlier.is_none() && expected.is_none() && status.complete() && settled_long_ago(&status)
        {
            let text = report(&Ending::Finished(status.clone()), store, &workdir);
            // Answered at once only if nothing moved while the report was
            // written, and no other request changed the watch meanwhile:
            // asking again starts over, so the watch there was before ends.
            // A push by the time it has ended starts a watch afresh below.
            let upstream = upstream_commit(store, &workdir);
            match read_status(store, &workdir, &url) {
                Ok(now) if same_checks(store, &workdir, &status, &now) => {
                    match store.end_ci_watch_if(chat, &url, earlier.as_ref()) {
                        Ok(Some(retired)) => {
                            self.publish_ci_card(chat, Ok(retired));
                            if !upstream_moved(store, &workdir, upstream.as_deref()) {
                                let shown =
                                    card(&url, Some(&status), Phase::Finished, now_seconds());
                                self.publish_ci_card(
                                    chat,
                                    store.add_ci_card(chat, &shown).map(Some),
                                );
                                return json!({"ok": true, "url": url, "number": status.number,
                                    "finished": true, "report": text});
                            }
                        }
                        Ok(None) => {}
                        Err(error) => return error_reply(error),
                    }
                }
                Ok(now) => status = now,
                Err(_) => {}
            }
        }
        if status.state != PullRequestState::Open && !status.complete() {
            return error_reply(format!(
                "Pull request #{} is {}, so its checks will not finish.",
                status.number,
                match status.state {
                    PullRequestState::Merged => "merged",
                    _ => "closed",
                }
            ));
        }
        // Only what the checkout says now is passed on; the watch keeps what
        // it waits for otherwise, as it is at this moment, not as read above.
        // Read again just now, so a push the earlier watch recorded meanwhile
        // is seen here too rather than replaced by an older read.
        let fresh = pushed_head(store, &workdir, &status)
            .filter(|commit| !head_contains(store, &workdir, &status.head, commit));
        // Stored and registered as the live watch together, so the thread
        // registered last is always the one the database holds.
        let added = self.ci_watches.register(|| {
            store.add_ci_watch(chat, &url, fresh.as_deref(), earlier.as_ref(), MAX_PER_CHAT)
        });
        let watch = match added {
            Ok(Some(added)) => {
                self.publish_ci_card(chat, Ok(added.retired));
                added.watch
            }
            Ok(None) => {
                return error_reply(format!(
                    "This chat already has {MAX_PER_CHAT} pull requests watched, the most it may."
                ));
            }
            Err(error) => return error_reply(error),
        };
        let head = watch
            .expected_head
            .clone()
            .unwrap_or_else(|| status.head.clone());
        let (checks, pending) = (status.checks.len(), status.pending());
        let waiting = watch
            .expected_head
            .as_deref()
            .filter(|commit| !commit.eq_ignore_ascii_case(&status.head));
        let shown = card(
            &url,
            Some(&status),
            Phase::Running {
                waiting_for: waiting,
            },
            watch.created_at,
        );
        self.publish_ci_card(chat, store.write_ci_card(chat, &url, &watch.token, &shown));
        if !self.ci_watches.start(watch.clone(), Some(status.clone())) {
            let removed = store.remove_ci_watch(
                chat,
                &url,
                Some(&watch.token),
                "xd could not start watching.",
            );
            self.publish_ci_card(chat, removed.map(|(_, retired)| retired));
            return error_reply("xd could not start watching CI. Try again in a moment.");
        }
        json!({
            "ok": true,
            "url": url,
            "number": status.number,
            "head": head,
            "checks": checks,
            "pending": pending,
            "finished": false,
        })
    }

    /// Ends `watch` with `report` for its agent and `shown` on its card, as
    /// one: the report waits for the chat out of its queue, as an agent's
    /// does, and goes as soon as the chat is free. Decided under the chat's
    /// gate. `upstream` is the commit the checkout's upstream was at when the
    /// poll that led here began: when the chat has pushed since, nothing is
    /// stored and the watch goes on. Answers whether the watch is over.
    fn deliver_ci_report(
        &self,
        watch: &CiWatch,
        report: &str,
        shown: &str,
        upstream: Option<&str>,
    ) -> bool {
        let mut written = None;
        let handed = self.hand_over_report(&watch.chat, |store| {
            if store
                .chat_workdir(&watch.chat)
                .is_ok_and(|workdir| upstream_moved(store, &workdir, upstream))
            {
                return Ok(None);
            }
            let (delivered, card) =
                store.deliver_ci_report(&watch.chat, &watch.url, &watch.token, report, shown);
            written = card;
            delivered.map_err(|error| error.to_string())
        });
        // The card says how it ended even when the turn could not start.
        if let Some(written) = written {
            self.publish_ci_card(&watch.chat, Ok(Some(written)));
        }
        if let Err(error) = &handed {
            eprintln!("xd-host: cannot report CI to chat {}: {error}", watch.chat);
        }
        // Stored, or no longer this watch's to tell: replaced, gone with its
        // chat, or stored before a turn failed to start. Otherwise the chat
        // pushed meanwhile, or the report was refused before it was stored.
        matches!(handed, Ok(Some(())))
            || !self.store.as_ref().is_some_and(|store| {
                store
                    .owns_ci_watch(&watch.chat, &watch.url, &watch.token)
                    .unwrap_or(false)
            })
    }

    /// Tells clients of a card write: a new card reloads the chat, a changed
    /// one is rewritten in place.
    fn publish_ci_card(
        &self,
        chat: &str,
        written: Result<Option<crate::storage::CardWrite>, crate::StorageError>,
    ) {
        match written {
            Ok(Some(card)) if card.created => self
                .events
                .publish(json!({"event": "changed", "chat": chat})),
            Ok(Some(card)) => self.events.publish(json!({
                "event": "record",
                "chat": chat,
                "record": format!("ci:{}", card.message_id),
                "message_id": card.message_id,
                "text": card.text,
            })),
            Ok(None) => {}
            Err(error) => eprintln!("xd-host: cannot write a CI card: {error}"),
        }
    }
}

impl CiWatches {
    /// Runs `add`, which stores a watch, and records the watch it stored as
    /// the live one, both under the record's lock.
    fn register<E>(
        &self,
        add: impl FnOnce() -> Result<Option<AddedCiWatch>, E>,
    ) -> Result<Option<AddedCiWatch>, E> {
        let mut live = self.live.lock().unwrap_or_else(|error| error.into_inner());
        let added = add()?;
        if let Some(AddedCiWatch { watch, .. }) = &added {
            live.insert((watch.chat.clone(), watch.url.clone()), watch.token.clone());
        }
        Ok(added)
    }

    /// Starts the thread for `watch`, registered already. `first` is what
    /// GitHub already said, which counts as its first poll. Answers whether
    /// it started.
    fn start(&self, watch: CiWatch, first: Option<Status>) -> bool {
        let Some(engine) = self.engine.get().cloned() else {
            return false;
        };
        let url = watch.url.clone();
        let key = (watch.chat.clone(), watch.url.clone());
        let token = watch.token.clone();
        let spawned = thread::Builder::new()
            .name("xd-ci-watch".into())
            .spawn(move || run(engine, watch, first));
        if let Err(error) = &spawned {
            eprintln!("xd-host: cannot watch CI on {url}: {error}");
            self.leave(&key, &token);
        }
        spawned.is_ok()
    }

    /// Whether the thread holding `token` is still the one watching `key`.
    fn is_live(&self, key: &(String, String), token: &str) -> bool {
        self.live
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(key)
            .is_some_and(|live| live == token)
    }

    /// Forgets the thread holding `token`, unless another replaced it.
    fn leave(&self, key: &(String, String), token: &str) {
        let mut live = self.live.lock().unwrap_or_else(|error| error.into_inner());
        if live.get(key).is_some_and(|live| live == token) {
            live.remove(key);
        }
    }
}

/// Forgets the watch's thread however [`run`] ends.
struct Leaving<'a> {
    engine: &'a Weak<Engine>,
    key: (String, String),
    token: String,
}

impl Drop for Leaving<'_> {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.upgrade() {
            engine.ci_watches.leave(&self.key, &self.token);
        }
    }
}

fn poll_interval() -> Duration {
    std::env::var("XD_CI_POLL_MILLIS")
        .ok()
        .and_then(|millis| millis.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map_or(POLL, Duration::from_millis)
}

/// The commit a watch waits for GitHub to show as the pull request's head:
/// the latest one the chat's checkout pushed to its branch, for at most
/// [`HEAD_GRACE_POLLS`] polls each.
#[derive(Default)]
struct PushedHead {
    commit: Option<String>,
    polls: u32,
}

/// What a watch does with the head one poll found.
#[derive(Debug, PartialEq, Eq)]
enum HeadCheck {
    /// It is the head to judge.
    Judge,
    /// GitHub does not show the pushed commit yet.
    Wait,
    /// GitHub never showed this pushed commit, so no verdict on the head it
    /// shows instead would answer for it.
    GaveUp(String),
}

impl PushedHead {
    /// Takes in what one poll saw. `pushed` is what the checkout says it
    /// pushed, when that is not GitHub's head; `contains` says whether
    /// GitHub's head is known to build on a commit.
    fn observe(
        &mut self,
        head: &str,
        pushed: Option<String>,
        contains: impl Fn(&str) -> bool,
    ) -> HeadCheck {
        if let Some(pushed) = pushed
            && self.commit.as_ref() != Some(&pushed)
        {
            self.commit = Some(pushed);
            self.polls = 0;
        }
        let Some(commit) = &self.commit else {
            return HeadCheck::Judge;
        };
        // A later push on top of it counts as GitHub having it.
        if commit.eq_ignore_ascii_case(head) || contains(commit) {
            self.commit = None;
            return HeadCheck::Judge;
        }
        if self.polls >= HEAD_GRACE_POLLS {
            return HeadCheck::GaveUp(commit.clone());
        }
        self.polls += 1;
        HeadCheck::Wait
    }
}

/// A watch's thread: asks GitHub until the checks finish or the watch has to
/// give up, then tells the chat. It holds the engine only while it works, and
/// stops without a word once its row is no longer the watch's.
fn run(engine: Weak<Engine>, watch: CiWatch, mut first: Option<Status>) {
    let (chat, url, token) = (
        watch.chat.as_str(),
        watch.url.as_str(),
        watch.token.as_str(),
    );
    let leaving = Leaving {
        engine: &engine,
        key: (chat.to_owned(), url.to_owned()),
        token: token.to_owned(),
    };
    let poll = poll_interval();
    let mut pushed = PushedHead {
        commit: watch.expected_head.clone(),
        ..PushedHead::default()
    };
    let mut empty = EmptyPolls::default();
    let (mut failed_polls, mut refused, mut unconfirmed) = (0_u32, 0_u32, 0_u32);
    // What GitHub said last, for the card.
    let mut last: Option<Status> = None;
    let mut last_complete: Option<(Vec<String>, i64)> = None;
    loop {
        {
            let Some(engine) = engine.upgrade() else {
                return;
            };
            if engine.shutdown_requested() {
                return;
            }
            let Some(store) = engine.store.as_ref() else {
                return;
            };
            if !store.owns_ci_watch(chat, url, token).unwrap_or(false) {
                return;
            }
            let workdir = store
                .chat_workdir(chat)
                .unwrap_or_else(|_| store.workspace_path().to_path_buf());
            // Taken before anything this poll decides, so a push from here
            // on holds back whatever report it leads to.
            let upstream = upstream_commit(store, &workdir);
            let read = match first.take() {
                Some(status) => Ok(status),
                None => read_status(store, &workdir, url),
            };
            let mut ending = match read {
                Err(error) => {
                    failed_polls += 1;
                    last_complete = None;
                    empty = EmptyPolls::default();
                    (failed_polls >= FAILED_POLLS).then(|| Ending::Stopped {
                        url: url.to_owned(),
                        reason: format!("xd could not read its checks: {error}"),
                    })
                }
                Ok(status) => {
                    failed_polls = 0;
                    // Finished checks that could not be read again count only
                    // in a row: checks still at work start the count over.
                    if !status.complete() {
                        unconfirmed = 0;
                    }
                    last = Some(status.clone());
                    let before = pushed.commit.clone();
                    let check = pushed.observe(
                        &status.head,
                        pushed_head(store, &workdir, &status),
                        |commit| head_contains(store, &workdir, &status.head, commit),
                    );
                    if pushed.commit != before {
                        let _ =
                            store.set_ci_watch_expected(chat, url, token, pushed.commit.as_deref());
                    }
                    match check {
                        HeadCheck::GaveUp(commit) => Some(Ending::Stopped {
                            url: status.url.clone(),
                            reason: format!(
                                "GitHub still shows {} as its head, not {}, which this chat \
                                 pushed. Push it again if it is missing, then watch again.",
                                status.short_head(),
                                &commit[..commit.len().min(7)]
                            ),
                        }),
                        check => judge(
                            &status,
                            check == HeadCheck::Wait,
                            &mut last_complete,
                            &mut empty,
                            now_seconds(),
                        ),
                    }
                }
            };
            // The deadline holds whatever this poll found, and so does a
            // run of finished checks that could not be read again.
            let overdue =
                now_seconds().saturating_sub(watch.created_at) >= MAX_WATCH.as_secs() as i64;
            if unconfirmed >= FAILED_POLLS {
                ending = Some(Ending::Stopped {
                    url: url.to_owned(),
                    reason: "xd could not confirm its finished checks.".into(),
                });
            } else if overdue && !matches!(ending, Some(Ending::Stopped { .. })) {
                ending = Some(Ending::Stopped {
                    url: url.to_owned(),
                    reason: "Its checks were still running after 6 hours.".into(),
                });
            }
            match ending {
                None => {
                    let shown = card(
                        url,
                        last.as_ref(),
                        Phase::Running {
                            waiting_for: pushed.commit.as_deref(),
                        },
                        watch.created_at,
                    );
                    engine.publish_ci_card(chat, store.write_ci_card(chat, url, token, &shown));
                }
                Some(ending) => {
                    let text = report(&ending, store, &workdir);
                    // Reading failed jobs' logs takes a while; a push, or a check run
                    // again, meanwhile means there is more to wait for. One that
                    // cannot be read again is not sent on trust: the watch goes on as
                    // after any failed poll.
                    let current = match &ending {
                        Ending::Finished(done) => match read_status(store, &workdir, url) {
                            Ok(now) if same_checks(store, &workdir, done, &now) => {
                                unconfirmed = 0;
                                true
                            }
                            Ok(now) => {
                                unconfirmed = 0;
                                first = Some(now);
                                false
                            }
                            Err(_) => {
                                unconfirmed += 1;
                                false
                            }
                        },
                        Ending::Stopped { .. } => true,
                    };
                    if !current {
                        last_complete = None;
                        if first.is_some() {
                            continue;
                        }
                    } else {
                        let shown = match &ending {
                            Ending::Finished(done) => {
                                card(url, Some(done), Phase::Finished, watch.created_at)
                            }
                            Ending::Stopped { reason, .. } => {
                                card(url, last.as_ref(), Phase::Stopped(reason), watch.created_at)
                            }
                        };
                        if engine.deliver_ci_report(&watch, &text, &shown, upstream.as_deref()) {
                            return;
                        }
                        last_complete = None;
                        // Refused before it was stored, or the chat pushed meanwhile:
                        // the next poll looks again, and a report refused every time
                        // ends the watch after a while.
                        refused += 1;
                        if refused >= FAILED_POLLS {
                            let removed = store.remove_ci_watch(
                                chat,
                                url,
                                Some(token),
                                "xd could not send its report.",
                            );
                            engine.publish_ci_card(chat, removed.map(|(_, retired)| retired));
                            return;
                        }
                    }
                }
            }
        }
        // Sleep in slices, so a stopping daemon or a replaced watch ends the
        // thread soon.
        let mut slept = Duration::ZERO;
        while slept < poll {
            let step = (poll - slept).min(Duration::from_millis(250));
            thread::sleep(step);
            slept += step;
            if engine.upgrade().is_none_or(|engine| {
                engine.shutdown_requested() || !engine.ci_watches.is_live(&leaving.key, token)
            }) {
                return;
            }
        }
    }
}

/// Polls in a row that found one head with no checks.
#[derive(Default)]
struct EmptyPolls {
    head: String,
    polls: u32,
}

/// Whether every check says when it ended, and the last did so at least
/// [`QUIET_FOR`] before `now`, in seconds since the epoch.
fn quiet(status: &Status, now: i64) -> bool {
    status
        .checks
        .iter()
        .map(|check| check.completed_at)
        .collect::<Option<Vec<_>>>()
        .and_then(|times| times.into_iter().max())
        .is_some_and(|newest| now - newest >= QUIET_FOR.as_secs() as i64)
}

/// Whether `now` shows the same head and the same run and outcome of every
/// check as `before`, with no push from the checkout GitHub does not show.
fn same_checks(store: &StateStore, workdir: &Path, before: &Status, now: &Status) -> bool {
    before.names() == now.names() && pushed_head(store, workdir, now).is_none()
}

/// How one poll's `status` ends the watch, if it does. `waiting` says GitHub
/// does not show the commit the chat pushed yet. `last_complete`, the
/// finished checks last seen and since when, and `empty`
/// carry over from poll to poll.
fn judge(
    status: &Status,
    waiting: bool,
    last_complete: &mut Option<(Vec<String>, i64)>,
    empty: &mut EmptyPolls,
    now: i64,
) -> Option<Ending> {
    // A poll that finds checks ends any run of polls that found none.
    if !status.checks.is_empty() {
        *empty = EmptyPolls::default();
    }
    if !waiting && status.complete() {
        // Done once two polls in a row agree, so a check that finished
        // before the others were queued is not taken for the whole run,
        // and the checks have been still for a while, so ones registered a
        // little later are not missed either.
        let names = status.names();
        let since = match last_complete {
            Some((seen, since)) if *seen == names => Some(*since),
            _ => None,
        };
        *last_complete = Some((names, since.unwrap_or(now)));
        let still = since.is_some_and(|since| now - since >= QUIET_FOR.as_secs() as i64)
            || quiet(status, now);
        return (since.is_some() && still).then(|| Ending::Finished(status.clone()));
    }
    *last_complete = None;
    if status.state != PullRequestState::Open {
        return Some(Ending::Stopped {
            url: status.url.clone(),
            reason: format!(
                "The pull request was {} before its checks finished.",
                if status.state == PullRequestState::Merged {
                    "merged"
                } else {
                    "closed"
                }
            ),
        });
    }
    if !waiting && status.checks.is_empty() {
        if empty.head != status.head {
            *empty = EmptyPolls {
                head: status.head.clone(),
                polls: 0,
            };
        }
        empty.polls += 1;
        return (empty.polls >= NO_CHECKS_POLLS).then(|| Ending::Stopped {
            url: status.url.clone(),
            reason: format!("No checks ran on its head, {}.", status.short_head()),
        });
    }
    *empty = EmptyPolls::default();
    None
}

/// What GitHub says of pull request `url` and the checks on its head.
fn read_status(store: &StateStore, workdir: &Path, url: &str) -> Result<Status, String> {
    let (status, stdout, stderr) = store
        .run_gh(
            workdir,
            &[
                "pr",
                "view",
                url,
                "--json",
                "url,number,title,state,headRefOid,headRefName,headRepository,\
                 headRepositoryOwner,statusCheckRollup",
            ],
        )
        .map_err(|error| error.to_string())?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("The GitHub CLI could not read this pull request.");
        return Err(detail.chars().take(240).collect());
    }
    let parsed: Value = serde_json::from_slice(&stdout)
        .map_err(|_| "The GitHub CLI gave an unreadable answer.".to_owned())?;
    parse_status(&parsed)
}

fn parse_status(parsed: &Value) -> Result<Status, String> {
    let url = parsed["url"]
        .as_str()
        .and_then(pull_requests::normalize_url)
        .ok_or("GitHub gave no pull request address.")?;
    let state = match parsed["state"].as_str() {
        Some("OPEN") => PullRequestState::Open,
        Some("CLOSED") => PullRequestState::Closed,
        Some("MERGED") => PullRequestState::Merged,
        _ => return Err("GitHub gave an unknown pull request state.".into()),
    };
    let head = parsed["headRefOid"]
        .as_str()
        .filter(|head| !head.is_empty() && head.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("GitHub gave no head commit.")?
        .to_ascii_lowercase();
    let checks = parsed["statusCheckRollup"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_check)
        .collect();
    Ok(Status {
        url,
        number: parsed["number"].as_u64().unwrap_or_default(),
        state,
        head,
        title: parsed["title"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect(),
        head_branch: parsed["headRefName"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        head_repository: match (
            parsed["headRepositoryOwner"]["login"].as_str(),
            parsed["headRepository"]["name"].as_str(),
        ) {
            (Some(owner), Some(name)) if !owner.is_empty() && !name.is_empty() => {
                Some(format!("{owner}/{name}").to_ascii_lowercase())
            }
            _ => None,
        },
        checks,
    })
}

/// One entry of GitHub's status check rollup: a check run, such as an Actions
/// job, or a commit status another service set.
fn parse_check(entry: &Value) -> Option<Check> {
    let text = |key: &str| {
        entry[key]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| text.chars().take(160).collect::<String>())
    };
    // Addresses are kept whole, or not at all: a cut one loses the job id.
    let address = |key: &str| {
        entry[key]
            .as_str()
            .map(str::trim)
            .filter(|address| !address.is_empty() && address.len() <= 2_048)
            .map(str::to_owned)
    };
    let words = |text: &str| text.to_ascii_lowercase().replace('_', " ");
    let (name, outcome, link) = if entry["__typename"] == "StatusContext" {
        let state = entry["state"].as_str().unwrap_or_default();
        let outcome = match state {
            "SUCCESS" => Outcome::Passed,
            "PENDING" | "EXPECTED" | "" => Outcome::Pending,
            other => Outcome::Failed(words(other)),
        };
        (text("context")?, outcome, address("targetUrl"))
    } else {
        let outcome = if entry["status"].as_str() != Some("COMPLETED") {
            Outcome::Pending
        } else {
            match entry["conclusion"].as_str().unwrap_or_default() {
                "SUCCESS" => Outcome::Passed,
                "NEUTRAL" | "SKIPPED" => Outcome::Skipped,
                "" => Outcome::Failed("no conclusion".into()),
                other => Outcome::Failed(words(other)),
            }
        };
        (text("name")?, outcome, address("detailsUrl"))
    };
    Some(Check {
        name,
        workflow: text("workflowName"),
        outcome,
        link: link.filter(|link| link.starts_with("https://")),
        completed_at: entry["completedAt"].as_str().and_then(parse_timestamp),
    })
}

/// Seconds since the epoch of an RFC 3339 UTC time such as GitHub's
/// `2026-11-11T12:23:16Z`.
fn parse_timestamp(text: &str) -> Option<i64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let time = time.split('.').next()?;
    let mut time = time.split(':').map(|part| part.parse::<i64>().ok());
    let (hour, minute, second) = (time.next()??, time.next()??, time.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil date, after Howard Hinnant's algorithm.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Whether the checks of a complete head all finished long enough ago that
/// no more are coming. Commit statuses other services set carry no time, so
/// a head with one is watched rather than answered for at once.
fn settled_long_ago(status: &Status) -> bool {
    let newest = status
        .checks
        .iter()
        .map(|check| check.completed_at)
        .collect::<Option<Vec<_>>>()
        .and_then(|times| times.into_iter().max());
    newest.is_some_and(|newest| now_seconds() - newest >= SETTLED_FOR.as_secs() as i64)
}

/// Whether `head`, GitHub's head of the pull request, is a commit the
/// checkout knows that builds on `commit`.
fn head_contains(store: &StateStore, workdir: &Path, head: &str, commit: &str) -> bool {
    let succeeds = |arguments: &[&str]| {
        store
            .run_git(workdir, arguments)
            .is_ok_and(|(result, _, _)| result.success())
    };
    commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        && succeeds(&["cat-file", "-e", &format!("{head}^{{commit}}")])
        && succeeds(&["merge-base", "--is-ancestor", commit, head])
}

/// The host and `owner/name`, in lowercase, of a GitHub remote's address:
/// `https://host/owner/name(.git)`, `ssh://git@host/owner/name(.git)` or
/// `git@host:owner/name(.git)`.
fn remote_repository(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("ssh://"))
    {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, rest)| rest);
        rest.split_once('/')?
    } else {
        let rest = url.split_once('@')?.1;
        rest.split_once(':')?
    };
    let host = host.split(':').next()?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    (!host.is_empty() && !owner.is_empty() && !name.is_empty() && !name.contains('/')).then(|| {
        (
            host.to_ascii_lowercase(),
            format!("{owner}/{name}").to_ascii_lowercase(),
        )
    })
}

/// The commit the chat has pushed to the pull request's branch when GitHub
/// does not show it as the head yet: the checkout's branch tracks the pull
/// request's head branch in the pull request's head repository, and the head
/// GitHub shows is one the upstream was built on or held before. The upstream
/// is what was pushed, whatever was committed since. Any other head came from
/// elsewhere, so nothing is waited for.
fn pushed_head(store: &StateStore, workdir: &Path, status: &Status) -> Option<String> {
    let repository = status.head_repository.as_deref()?;
    let branch = store.checked_out_branch(workdir)?;
    let git = |arguments: &[&str]| {
        store
            .run_git(workdir, arguments)
            .ok()
            .filter(|(result, _, _)| result.success())
            .map(|(_, stdout, _)| String::from_utf8_lossy(&stdout).trim().to_owned())
    };
    let remote = git(&["config", "--get", &format!("branch.{branch}.remote")])?;
    let merge = git(&["config", "--get", &format!("branch.{branch}.merge")])?;
    if remote.is_empty()
        || remote.starts_with('-')
        || merge != format!("refs/heads/{}", status.head_branch)
    {
        return None;
    }
    // The address as configured, not as rewritten for fetching.
    let address = git(&["config", "--get", &format!("remote.{remote}.url")])?;
    let pull_request_host = status
        .url
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())?;
    let (host, remote_repository) = remote_repository(&address)?;
    if host != pull_request_host || remote_repository != repository {
        return None;
    }
    let upstream = git(&["rev-parse", "--verify", "--quiet", "@{upstream}"])?;
    if upstream.is_empty()
        || !upstream.bytes().all(|byte| byte.is_ascii_hexdigit())
        || upstream.eq_ignore_ascii_case(&status.head)
    {
        return None;
    }
    // GitHub is behind the push when its head is part of what was pushed, or
    // is where the upstream was before, as after a force push. A head that
    // went another way came from elsewhere.
    let behind = head_contains(store, workdir, &upstream, &status.head) || {
        let reference = git(&["rev-parse", "--symbolic-full-name", "@{upstream}"])?;
        git(&[
            "reflog",
            "show",
            "--format=%H",
            "-n",
            "100",
            &reference,
            "--",
        ])
        .is_some_and(|former| {
            former
                .lines()
                .any(|commit| commit.trim().eq_ignore_ascii_case(&status.head))
        })
    };
    behind.then(|| upstream.to_ascii_lowercase())
}

/// The message the chat gets when a watch ends.
fn report(ending: &Ending, store: &StateStore, workdir: &Path) -> String {
    let status = match ending {
        Ending::Stopped { url, reason } => {
            return format!("[xd] Stopped watching CI on {url}.\n\n{reason}");
        }
        Ending::Finished(status) => status,
    };
    let failed = status
        .checks
        .iter()
        .filter(|check| matches!(check.outcome, Outcome::Failed(_)))
        .collect::<Vec<_>>();
    let passed = status
        .checks
        .iter()
        .filter(|check| check.outcome == Outcome::Passed)
        .count();
    let skipped = status
        .checks
        .iter()
        .filter(|check| check.outcome == Outcome::Skipped)
        .count();
    let verdict = if failed.is_empty() {
        "passed"
    } else {
        "failed"
    };
    let mut text = format!(
        "[xd] CI {verdict} on {} at {}.\n\n",
        status.url,
        status.short_head()
    );
    let skipped = match skipped {
        0 => String::new(),
        1 => ", 1 skipped".into(),
        count => format!(", {count} skipped"),
    };
    if failed.is_empty() {
        text.push_str(&format!(
            "{} passed{skipped}.",
            plural(passed, "check", "checks")
        ));
        return text;
    }
    text.push_str(&format!(
        "{} of {} did not pass ({passed} passed{skipped}):\n",
        failed.len(),
        plural(status.checks.len(), "check", "checks"),
    ));
    for check in failed.iter().take(LISTED_CHECKS) {
        let Outcome::Failed(conclusion) = &check.outcome else {
            continue;
        };
        text.push_str(&format!("- {}: {conclusion}", check_name(check)));
        if let Some(link) = &check.link {
            text.push_str(&format!(" ({link})"));
        }
        text.push('\n');
    }
    if failed.len() > LISTED_CHECKS {
        text.push_str(&format!("- and {} more\n", failed.len() - LISTED_CHECKS));
    }
    for check in failed
        .iter()
        .filter_map(|check| Some((check, actions_job(check.link.as_deref()?)?)))
        .take(LOGGED_JOBS)
    {
        let ((repository, job), check) = (check.1, check.0);
        if let Some(log) = failed_log(store, workdir, &repository, &job) {
            text.push_str(&format!(
                "\nThe end of {}'s failed steps:\n\n```\n{log}\n```\n",
                check_name(check)
            ));
        }
    }
    text.trim_end().to_owned()
}

fn check_name(check: &Check) -> String {
    match &check.workflow {
        Some(workflow) => format!("{workflow} / {}", check.name),
        None => check.name.clone(),
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The repository and job id of a GitHub Actions job's address,
/// `https://github.com/OWNER/REPO/actions/runs/RUN/job/JOB`.
fn actions_job(link: &str) -> Option<(String, String)> {
    let path = link.strip_prefix("https://github.com/")?;
    let (repository, rest) = path.split_once("/actions/runs/")?;
    let (_, job) = rest.split_once("/job/")?;
    let job = job.split(['/', '?', '#']).next()?;
    let safe = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    };
    let mut parts = repository.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none()
        && safe(owner)
        && safe(name)
        && !job.is_empty()
        && job.bytes().all(|byte| byte.is_ascii_digit()))
    .then(|| (format!("{owner}/{name}"), job.to_owned()))
}

/// The last lines a failed Actions job logged in its failed steps.
fn failed_log(store: &StateStore, workdir: &Path, repository: &str, job: &str) -> Option<String> {
    // Only the end is quoted, so only the end is kept, however long the log.
    let (succeeded, stdout) = store
        .run_gh_tail(
            workdir,
            &[
                "run",
                "view",
                "--job",
                job,
                "--log-failed",
                "--repo",
                repository,
            ],
            LOG_TAIL_BYTES,
        )
        .ok()?;
    if !succeeded {
        return None;
    }
    let output = String::from_utf8_lossy(&stdout);
    let lines = output
        .lines()
        .map(log_line)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let tail = &lines[lines.len().saturating_sub(LOG_LINES)..];
    (!tail.is_empty()).then(|| tail.join("\n").replace("```", "'''"))
}

/// A `gh run view --log` line, `JOB<tab>STEP<tab>TIME TEXT`, as its text.
fn log_line(line: &str) -> String {
    let mut parts = line.splitn(3, '\t');
    let (Some(_), Some(_), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return clip(line);
    };
    let text = match rest.split_once(' ') {
        Some((time, text)) if time.ends_with('Z') && time.contains('T') => text,
        _ => rest,
    };
    clip(text)
}

fn clip(line: &str) -> String {
    let line = line.trim_end();
    match line.char_indices().nth(LOG_LINE_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rollup(checks: Value) -> Value {
        json!({"url": "https://github.com/RestartFU/xd/pull/291", "number": 291,
            "state": "OPEN", "headRefOid": "1E3D4A495F0A0CE7", "headRefName": "fix",
            "statusCheckRollup": checks})
    }

    #[test]
    fn the_rollup_reads_as_one_outcome_per_check() {
        let status = parse_status(&rollup(json!([
            {"__typename": "CheckRun", "name": "linux", "workflowName": "pr",
                "status": "COMPLETED", "conclusion": "SUCCESS",
                "completedAt": "2026-11-11T12:23:16Z",
                "detailsUrl": "https://github.com/RestartFU/xd/actions/runs/4/job/13"},
            {"__typename": "CheckRun", "name": "macos", "workflowName": "pr",
                "status": "IN_PROGRESS", "conclusion": ""},
            {"__typename": "CheckRun", "name": "windows", "workflowName": "pr",
                "status": "COMPLETED", "conclusion": "TIMED_OUT"},
            {"__typename": "CheckRun", "name": "docs", "workflowName": "",
                "status": "COMPLETED", "conclusion": "SKIPPED"},
            {"__typename": "StatusContext", "context": "ci/legacy", "state": "ERROR",
                "targetUrl": "http://insecure.example"},
            {"__typename": "StatusContext", "context": "review", "state": "PENDING"},
            {"__typename": "CheckRun", "status": "COMPLETED"},
        ])))
        .unwrap();
        assert_eq!(status.head, "1e3d4a495f0a0ce7");
        assert_eq!(status.short_head(), "1e3d4a4");
        assert_eq!(status.checks.len(), 6);
        let outcomes = status
            .checks
            .iter()
            .map(|check| check.outcome.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes,
            [
                Outcome::Passed,
                Outcome::Pending,
                Outcome::Failed("timed out".into()),
                Outcome::Skipped,
                Outcome::Failed("error".into()),
                Outcome::Pending,
            ]
        );
        assert_eq!(status.checks[0].workflow.as_deref(), Some("pr"));
        assert_eq!(status.checks[3].workflow, None);
        assert_eq!(status.checks[0].completed_at, Some(1_794_399_796));
        // Only secure addresses are passed on.
        assert_eq!(status.checks[4].link, None);
        assert_eq!(status.pending(), 2);
        assert!(!status.complete());
        assert!(!parse_status(&rollup(json!([]))).unwrap().complete());
        assert!(parse_status(&json!({"url": "nope"})).is_err());
    }

    #[test]
    fn a_pushed_commit_is_waited_for_until_github_shows_it_or_for_a_while() {
        let mut pushed = PushedHead::default();
        assert_eq!(pushed.observe("aaa", None, |_| false), HeadCheck::Judge);
        // The checkout pushed bbb; GitHub still shows aaa.
        assert_eq!(
            pushed.observe("aaa", Some("bbb".into()), |_| false),
            HeadCheck::Wait
        );
        // Waited for even when the checkout no longer says so.
        assert_eq!(pushed.observe("aaa", None, |_| false), HeadCheck::Wait);
        assert_eq!(pushed.observe("BBB", None, |_| false), HeadCheck::Judge);
        assert_eq!(pushed.commit, None);
        // A later push gets its own grace; one that never shows ends the
        // watch rather than letting the old head answer for it.
        for _ in 0..HEAD_GRACE_POLLS {
            assert_eq!(
                pushed.observe("bbb", Some("ccc".into()), |_| false),
                HeadCheck::Wait
            );
        }
        assert_eq!(
            pushed.observe("bbb", Some("ccc".into()), |_| false),
            HeadCheck::GaveUp("ccc".into())
        );
        // A head built on the pushed commit has it.
        assert_eq!(
            pushed.observe("eee", None, |commit| commit == "ccc"),
            HeadCheck::Judge
        );
        assert_eq!(
            pushed.observe("bbb", Some("ddd".into()), |_| false),
            HeadCheck::Wait
        );
    }

    #[test]
    fn a_pushed_commit_is_expected_only_over_an_older_head_the_checkout_knows() {
        let root = std::env::temp_dir().join(format!("xd-ci-push-{}", uuid::Uuid::new_v4()));
        let (remote, work) = (root.join("remote.git"), root.join("work"));
        std::fs::create_dir_all(root.join("Workspaces")).unwrap();
        let git = |directory: &Path, arguments: &[&str]| {
            let output = std::process::Command::new("git")
                .args(arguments)
                .current_dir(directory)
                .env("GIT_AUTHOR_NAME", "x")
                .env("GIT_AUTHOR_EMAIL", "x@example.com")
                .env("GIT_COMMITTER_NAME", "x")
                .env("GIT_COMMITTER_EMAIL", "x@example.com")
                .output()
                .unwrap();
            assert!(output.status.success(), "{arguments:?}: {output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&root, &["init", "-q", "--bare", "remote.git"]);
        git(&root, &["init", "-q", "-b", "fix", "work"]);
        // The remote reads as the GitHub repository, and is the local one.
        git(
            &work,
            &[
                "config",
                &format!("url.{}.insteadOf", remote.display()),
                "https://github.com/RestartFU/xd.git",
            ],
        );
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/RestartFU/xd.git",
            ],
        );
        git(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let old = git(&work, &["rev-parse", "HEAD"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "two"]);
        let pushed = git(&work, &["rev-parse", "HEAD"]);
        git(&work, &["push", "-q", "-u", "origin", "fix"]);
        let store = StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap();
        let status = |head: &str, branch: &str| {
            let mut status = parse_status(&rollup(json!([]))).unwrap();
            status.head = head.to_owned();
            status.head_branch = branch.to_owned();
            status.head_repository = Some("restartfu/xd".into());
            status
        };
        // GitHub still shows the commit before the push.
        assert_eq!(
            pushed_head(&store, &work, &status(&old, "fix")),
            Some(pushed.clone())
        );
        // It shows the push already, or the pull request is another branch's.
        assert_eq!(pushed_head(&store, &work, &status(&pushed, "fix")), None);
        assert_eq!(pushed_head(&store, &work, &status(&old, "other")), None);
        // A fork's branch of the same name is not this checkout's.
        let mut fork = status(&old, "fix");
        fork.head_repository = Some("someone/xd".into());
        assert_eq!(pushed_head(&store, &work, &fork), None);
        // A head that went another way, such as someone else's force push,
        // came from elsewhere.
        git(&work, &["checkout", "-q", "-b", "other", &old]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "theirs"]);
        let theirs = git(&work, &["rev-parse", "HEAD"]);
        git(&work, &["checkout", "-q", "fix"]);
        assert_eq!(pushed_head(&store, &work, &status(&theirs, "fix")), None);
        // The head built on the push has it.
        assert!(head_contains(&store, &work, &pushed, &old));
        assert!(!head_contains(&store, &work, &old, &pushed));
        // A head the checkout never saw came from elsewhere.
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(pushed_head(&store, &work, &status(unknown, "fix")), None);
        // One built on this checkout's HEAD is newer than what it pushed.
        git(&work, &["commit", "-q", "--allow-empty", "-m", "three"]);
        let newer = git(&work, &["rev-parse", "HEAD"]);
        git(&work, &["reset", "-q", "--hard", &pushed]);
        assert_eq!(pushed_head(&store, &work, &status(&newer, "fix")), None);
        // What was pushed is waited for, whatever was committed since.
        git(&work, &["commit", "-q", "--allow-empty", "-m", "local"]);
        assert_eq!(
            pushed_head(&store, &work, &status(&old, "fix")),
            Some(pushed.clone())
        );
        // A report checked against one upstream is held back once the
        // checkout pushes again.
        let recorded = upstream_commit(&store, &work);
        assert_eq!(recorded.as_deref(), Some(pushed.as_str()));
        assert!(!upstream_moved(&store, &work, recorded.as_deref()));
        // After this checkout's own force push, the head it replaced is behind.
        git(&work, &["reset", "-q", "--hard", &old]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "rewritten"]);
        let rewritten = git(&work, &["rev-parse", "HEAD"]);
        git(&work, &["push", "-q", "-f", "origin", "fix"]);
        assert_eq!(
            pushed_head(&store, &work, &status(&pushed, "fix")),
            Some(rewritten)
        );
        assert!(upstream_moved(&store, &work, recorded.as_deref()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checks_are_done_once_two_polls_in_a_row_agree() {
        let running = parse_status(&rollup(json!([
            {"__typename": "CheckRun", "name": "linux", "status": "QUEUED"},
        ])))
        .unwrap();
        let done = parse_status(&rollup(json!([
            {"__typename": "CheckRun", "name": "linux", "status": "COMPLETED",
                "conclusion": "SUCCESS"},
        ])))
        .unwrap();
        let mut moved = done.clone();
        moved.head = "ffff".into();
        // Each poll a minute and more after the last, so stillness is not
        // what these wait on.
        let clock = std::cell::Cell::new(0);
        let t = || {
            clock.set(clock.get() + 100);
            clock.get()
        };
        let (mut last, mut empty) = (None, EmptyPolls::default());
        assert!(judge(&done, false, &mut last, &mut empty, t()).is_none());
        assert!(judge(&moved, false, &mut last, &mut empty, t()).is_none());
        assert!(matches!(
            judge(&moved, false, &mut last, &mut empty, t()),
            Some(Ending::Finished(_))
        ));
        // A failed check run again before the next poll is a new run.
        let mut failed = done.clone();
        failed.checks[0].outcome = Outcome::Failed("failure".into());
        failed.checks[0].link = Some("https://github.com/o/r/actions/runs/1/job/1".into());
        let mut rerun = done.clone();
        rerun.checks[0].link = Some("https://github.com/o/r/actions/runs/1/job/2".into());
        let mut last = None;
        assert!(judge(&failed, false, &mut last, &mut empty, t()).is_none());
        assert!(judge(&rerun, false, &mut last, &mut empty, t()).is_none());
        assert!(judge(&rerun, false, &mut last, &mut empty, t()).is_some());
        let mut last = None;
        assert!(judge(&done, false, &mut last, &mut empty, t()).is_none());
        assert!(judge(&running, false, &mut last, &mut empty, t()).is_none());
        assert!(judge(&done, false, &mut last, &mut empty, t()).is_none());
        // Waiting on a pushed commit, a complete old head never counts.
        let mut last = None;
        assert!(judge(&done, true, &mut last, &mut empty, t()).is_none());
        assert!(judge(&done, true, &mut last, &mut empty, t()).is_none());
        let mut merged = running.clone();
        merged.state = PullRequestState::Merged;
        assert!(matches!(
            judge(&merged, false, &mut None, &mut empty, t()),
            Some(Ending::Stopped { .. })
        ));
        let mut none = running;
        none.checks.clear();
        let mut empty = EmptyPolls::default();
        for _ in 1..NO_CHECKS_POLLS {
            assert!(judge(&none, false, &mut None, &mut empty, t()).is_none());
        }
        // Another head with no checks gets its own grace.
        let mut other = none.clone();
        other.head = "eeee".into();
        assert!(judge(&other, false, &mut None, &mut empty, t()).is_none());
        for _ in 2..NO_CHECKS_POLLS {
            assert!(judge(&other, false, &mut None, &mut empty, t()).is_none());
        }
        assert!(judge(&other, false, &mut None, &mut empty, t()).is_some());
        // A poll that finds checks, even finished ones, starts the grace over.
        let mut empty = EmptyPolls::default();
        for _ in 1..NO_CHECKS_POLLS {
            assert!(judge(&none, false, &mut None, &mut empty, t()).is_none());
        }
        assert!(judge(&done, false, &mut None, &mut empty, t()).is_none());
        assert!(judge(&none, false, &mut None, &mut empty, t()).is_none());
    }

    #[test]
    fn a_watch_is_only_ever_ended_by_its_own_token_and_keeps_its_card() {
        let root = std::env::temp_dir().join(format!("xd-ci-rows-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("Workspaces")).unwrap();
        let engine = Engine::with_store(
            StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap(),
        );
        let folder = engine.dispatch(json!({"op": "new-folder", "name": "Project"}))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let chat = engine
            .dispatch(json!({"op": "new-chat", "folder": folder, "backend": "claude"}))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let store = engine.store.as_ref().unwrap();
        let url = "https://github.com/o/r/pull/7";
        let rows = || {
            store.messages(&json!({"chat": chat})).unwrap()["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["content"].as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
        };
        let running = format!("{CARD_PREFIX}{}", json!({"status": "running", "url": url}));
        let first = store
            .add_ci_watch(&chat, url, Some("abc"), None, 1)
            .unwrap()
            .unwrap();
        assert!(first.retired.is_none());
        let first = first.watch;
        // The card is added once, then rewritten only when it changes.
        let card = store
            .write_ci_card(&chat, url, &first.token, &running)
            .unwrap()
            .unwrap();
        assert!(card.created);
        assert_eq!(
            store
                .write_ci_card(&chat, url, &first.token, &running)
                .unwrap(),
            None
        );
        // Another watch of it takes over; the first card says so.
        let second = store
            .add_ci_watch(&chat, url, None, Some(&first), 1)
            .unwrap()
            .unwrap();
        let retired = second.retired.unwrap();
        assert_eq!(retired.message_id, card.message_id);
        assert!(retired.text.contains("\"status\":\"stopped\""));
        assert!(retired.text.contains(crate::storage::RETIRED_BY_ANOTHER));
        let second = second.watch;
        assert_ne!(first.token, second.token);
        // It still waits for the commit the watch it replaced waited for.
        assert_eq!(second.expected_head.as_deref(), Some("abc"));
        assert!(!store.owns_ci_watch(&chat, url, &first.token).unwrap());
        assert_eq!(
            store
                .write_ci_card(&chat, url, &first.token, &running)
                .unwrap(),
            None
        );
        store
            .set_ci_watch_expected(&chat, url, &first.token, Some("zzz"))
            .unwrap();
        assert_eq!(store.ci_watches().unwrap(), std::slice::from_ref(&second));
        store
            .set_ci_watch_expected(&chat, url, &second.token, Some("def"))
            .unwrap();
        // A replacement that read the watch before it recorded def keeps def
        // over the older push it read itself; one that read it after wins.
        let third = store
            .add_ci_watch(&chat, url, Some("old"), Some(&second), 1)
            .unwrap()
            .unwrap()
            .watch;
        assert_eq!(third.expected_head.as_deref(), Some("def"));
        let second = store
            .add_ci_watch(&chat, url, Some("ggg"), Some(&third), 1)
            .unwrap()
            .unwrap()
            .watch;
        assert_eq!(second.expected_head.as_deref(), Some("ggg"));
        // One other pull request is the most this chat may watch here.
        assert!(
            store
                .add_ci_watch(&chat, "https://github.com/o/r/pull/8", None, None, 1)
                .unwrap()
                .is_none()
        );
        // An answer given at once ends only the watch it read beforehand.
        assert!(
            store
                .end_ci_watch_if(&chat, url, Some(&first))
                .unwrap()
                .is_none()
        );
        assert!(store.end_ci_watch_if(&chat, url, None).unwrap().is_none());
        let mut stale = store.ci_watch(&chat, url).unwrap().unwrap();
        stale.expected_head = Some("fff".into());
        assert!(
            store
                .end_ci_watch_if(&chat, url, Some(&stale))
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .end_ci_watch_if(&chat, "https://github.com/o/r/pull/9", None)
                .unwrap()
                .is_some()
        );
        // The report of a replaced watch is refused, storing nothing; the
        // current watch's ends it, and its card says how CI went.
        let finished = format!("{CARD_PREFIX}{}", json!({"status": "passed", "url": url}));
        let before = rows().len();
        let (refused, card) =
            store.deliver_ci_report(&chat, url, &first.token, "[xd] CI passed", &finished);
        assert!(refused.unwrap().is_none());
        assert_eq!(card, None);
        assert_eq!(store.ci_watches().unwrap().len(), 1);
        assert_eq!(rows().len(), before);
        let (delivered, card) =
            store.deliver_ci_report(&chat, url, &second.token, "[xd] CI passed", &finished);
        assert!(delivered.unwrap().is_some());
        assert!(card.unwrap().created);
        assert!(store.ci_watches().unwrap().is_empty());
        assert!(rows().contains(&finished));
        // Delivered as a report, out of the queue, as an agent's is.
        assert_eq!(store.chat(&chat).unwrap()["queue"], json!([]));
        assert!(
            store
                .deliver_ci_report(&chat, url, &second.token, "[xd] CI passed", &finished)
                .0
                .unwrap()
                .is_none()
        );
        let (removed, _) = store
            .remove_ci_watch(&chat, url, Some(&second.token), "gone")
            .unwrap();
        assert!(!removed);
        assert!(store.add_ci_watch("nope", url, None, None, 1).is_err());
        engine.runtime.as_ref().unwrap().shutdown();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_watch_reports_only_once_its_checks_have_been_quiet_a_while() {
        let status = parse_status(&rollup(json!([
            {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED",
                "conclusion": "SUCCESS", "completedAt": "2026-11-11T12:00:00Z"},
            {"__typename": "StatusContext", "context": "x", "state": "SUCCESS"},
        ])))
        .unwrap();
        let ended = parse_timestamp("2026-11-11T12:00:00Z").unwrap();
        let mut first = status.clone();
        first.checks.truncate(1);
        assert!(!quiet(&first, ended + 30));
        // A check that never says when it ended leaves stillness to be seen.
        let mut timed = status.clone();
        timed.checks.truncate(1);
        assert!(quiet(&timed, ended + 60));
        assert!(!quiet(&status, ended + 60));
        let (mut last, mut empty) = (None, EmptyPolls::default());
        assert!(judge(&status, false, &mut last, &mut empty, 1_000).is_none());
        assert!(judge(&status, false, &mut last, &mut empty, 1_015).is_none());
        assert!(judge(&status, false, &mut last, &mut empty, 1_060).is_some());
    }

    #[test]
    fn remote_addresses_name_their_repository() {
        for address in [
            "https://github.com/RestartFU/xd.git",
            "https://token@github.com/RestartFU/xd/",
            "git@github.com:RestartFU/xd.git",
            "ssh://git@github.com:22/RestartFU/xd",
        ] {
            assert_eq!(
                remote_repository(address),
                Some(("github.com".into(), "restartfu/xd".into())),
                "{address}"
            );
        }
        assert_eq!(remote_repository("/srv/git/xd.git"), None);
        assert_eq!(remote_repository("https://github.com/RestartFU"), None);
    }

    #[test]
    fn only_checks_that_all_say_when_they_ended_can_be_settled() {
        let mut status = parse_status(&rollup(json!([
            {"__typename": "CheckRun", "name": "linux", "status": "COMPLETED",
                "conclusion": "SUCCESS", "completedAt": "2020-01-01T00:00:00Z"},
        ])))
        .unwrap();
        assert!(settled_long_ago(&status));
        status.checks.push(
            parse_check(&json!({"__typename": "StatusContext", "context": "x",
                "state": "SUCCESS"}))
            .unwrap(),
        );
        assert!(!settled_long_ago(&status));
    }

    #[test]
    fn timestamps_count_from_the_epoch() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2000-03-01T00:00:01.5Z"), Some(951_868_801));
        assert_eq!(parse_timestamp("2026-11-11T12:23:16"), None);
        assert_eq!(parse_timestamp("2026-13-11T12:23:16Z"), None);
    }

    #[test]
    fn job_addresses_and_log_lines_are_read_strictly() {
        assert_eq!(
            actions_job("https://github.com/RestartFU/xd/actions/runs/43/job/130?pr=1"),
            Some(("RestartFU/xd".into(), "130".into()))
        );
        assert_eq!(actions_job("https://github.com/o/r/actions/runs/43"), None);
        assert_eq!(
            actions_job("https://github.com/o/r;x/actions/runs/4/job/1"),
            None
        );
        assert_eq!(
            actions_job("https://example.com/o/r/actions/runs/4/job/1"),
            None
        );
        assert_eq!(
            log_line("linux\tRun tests\t2026-11-11T12:20:01.1234567Z error[E0425]: oops"),
            "error[E0425]: oops"
        );
        assert_eq!(log_line("plain"), "plain");
        // A long address is kept whole, so its job id is not cut off.
        let long = format!(
            "https://github.com/{}/{}/actions/runs/43100/job/130600",
            "o".repeat(39),
            "r".repeat(100)
        );
        let check = parse_check(&json!({"__typename": "CheckRun", "name": "linux",
            "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": long}))
        .unwrap();
        assert_eq!(check.link.as_deref(), Some(long.as_str()));
        assert_eq!(
            actions_job(&long).map(|(_, job)| job),
            Some("130600".into())
        );
        assert_eq!(
            log_line(&"x".repeat(400)).chars().count(),
            LOG_LINE_CHARS + 1
        );
    }

    /// An agent asks xd to watch a pull request; xd tells the chat once its
    /// checks finish, with the end of the failed job's log, and forgets the
    /// watch. Asked again, it answers at once.
    #[cfg(unix)]
    #[test]
    fn a_watch_reports_to_its_chat_once_the_checks_finish() {
        use std::os::unix::fs::PermissionsExt;

        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("xd-ci-watch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("Workspaces")).unwrap();
        let executable = |name: &str, script: String| {
            let path = root.join(name);
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let check = |status: &str, conclusion: &str, completed: &str| {
            json!({"__typename": "CheckRun", "name": "linux", "workflowName": "pr",
                "status": status, "conclusion": conclusion, "completedAt": completed,
                "detailsUrl": "https://github.com/o/r/actions/runs/4/job/13"})
        };
        let pull_request = |checks: Value| {
            json!({"url": "https://github.com/O/r/pull/7", "number": 7, "state": "OPEN",
                "headRefOid": "1e3d4a495f0a0ce7", "headRefName": "fix",
                "statusCheckRollup": checks})
            .to_string()
        };
        std::fs::write(
            root.join("pending.json"),
            pull_request(json!([check("IN_PROGRESS", "", "")])),
        )
        .unwrap();
        std::fs::write(
            root.join("done.json"),
            pull_request(json!([
                check("COMPLETED", "FAILURE", "2020-01-01T00:00:00Z"),
                {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED",
                    "conclusion": "SUCCESS", "completedAt": "2020-01-01T00:00:00Z"},
            ])),
        )
        .unwrap();
        let root_text = root.display();
        let gh = executable(
            "gh",
            format!(
                r#"#!/bin/sh
echo "$*" >> "{root_text}/gh.log"
case "$1 $2" in
"pr view")
  n=$(cat "{root_text}/polls" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "{root_text}/polls"
  if [ $n -le 2 ]; then cat "{root_text}/pending.json"; else cat "{root_text}/done.json"; fi ;;
"run view")
  # A long log: only its end may be quoted, past any output limit.
  yes "$(printf 'linux\tTest\t2026-11-11T12:20:00.0Z filler')" | head -n 230000
  printf 'linux\tTest\t2026-11-11T12:20:01.1Z error: it broke\n' ;;
*) exit 1 ;;
esac
"#
            ),
        );
        let agent = executable(
            "claude",
            r#"#!/bin/sh
case "$*" in *--strict-mcp-config*) exit 0;; esac
while read message; do
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"On it."}]}}'
echo '{"type":"result","terminal_reason":"completed","result":"On it."}'
done
"#
            .into(),
        );
        // SAFETY: executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe {
            std::env::set_var("XD_GH_EXECUTABLE", &gh);
            std::env::set_var("XD_CLAUDE_EXECUTABLE", &agent);
            std::env::set_var("XD_CI_POLL_MILLIS", "40");
        }
        let engine = Arc::new(Engine::with_store(
            StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap(),
        ));
        let refused = engine.dispatch(json!({"op": "watch-ci", "chat": "nope",
            "pull_request": "https://github.com/o/r/pull/7"}));
        assert_eq!(refused["ok"], false);
        engine.start_ci_watches();
        let folder = engine.dispatch(json!({"op": "new-folder", "name": "Project"}))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let chat = engine
            .dispatch(json!({"op": "new-chat", "folder": folder, "backend": "claude"}))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let store = engine.store.as_ref().unwrap();

        let bad = engine.dispatch(json!({"op": "watch-ci", "chat": chat,
            "pull_request": "https://github.com/o/r/issues/7"}));
        assert_eq!(bad["ok"], false, "{bad}");
        let watching = engine.dispatch(json!({"op": "watch-ci", "chat": chat,
            "pull_request": "https://github.com/o/r/pull/7/checks"}));
        assert_eq!(watching["ok"], true, "{watching}");
        assert_eq!(watching["finished"], false);
        assert_eq!(watching["url"], "https://github.com/o/r/pull/7");
        assert_eq!(watching["pending"], 1);

        let reports = || {
            store.messages(&json!({"chat": chat})).unwrap()["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|row| row["content"].as_str().map(str::to_owned))
                .filter(|text| text.starts_with("[xd] CI"))
                .collect::<Vec<_>>()
        };
        for _ in 0..400 {
            if !reports().is_empty() && store.chat(&chat).unwrap()["working"] == false {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let reports = reports();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert_eq!(
            reports[0],
            format!(
                "[xd] CI failed on https://github.com/o/r/pull/7 at 1e3d4a4.\n\n\
                 1 of 2 checks did not pass (1 passed):\n\
                 - pr / linux: failure (https://github.com/o/r/actions/runs/4/job/13)\n\n\
                 The end of pr / linux's failed steps:\n\n```\n{}error: it broke\n```",
                "filler\n".repeat(LOG_LINES - 1)
            )
        );
        assert!(store.ci_watches().unwrap().is_empty());
        // One card stands for the watch, rewritten in place as it went.
        let cards = || {
            store.messages(&json!({"chat": chat})).unwrap()["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|row| row["content"].as_str())
                .filter_map(|text| text.strip_prefix(CARD_PREFIX))
                .map(|record| serde_json::from_str::<Value>(record).unwrap())
                .collect::<Vec<_>>()
        };
        let shown = cards();
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert_eq!(shown[0]["status"], "failed");
        assert_eq!(shown[0]["number"], 7);
        assert_eq!(shown[0]["head"], "1e3d4a4");
        assert_eq!(shown[0]["checks"][0]["name"], "linux");
        assert_eq!(shown[0]["checks"][0]["state"], "failed");
        assert_eq!(shown[0]["checks"][0]["conclusion"], "failure");
        assert_eq!(shown[0]["checks"][1]["state"], "passed");
        let log = std::fs::read_to_string(root.join("gh.log")).unwrap();
        assert!(
            log.contains("run view --job 13 --log-failed --repo o/r"),
            "{log}"
        );

        // The checks finished long ago, so asking again answers at once.
        let finished = engine.dispatch(json!({"op": "watch-ci", "chat": chat,
            "pull_request": "https://github.com/o/r/pull/7"}));
        assert_eq!(finished["finished"], true, "{finished}");
        assert!(
            finished["report"]
                .as_str()
                .unwrap()
                .starts_with("[xd] CI failed on https://github.com/o/r/pull/7 at 1e3d4a4.")
        );
        assert!(store.ci_watches().unwrap().is_empty());
        // It gets a card of its own, finished from the start.
        let shown = cards();
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[1]["status"], "failed");

        // A watch ends with its chat, without a word.
        std::fs::write(root.join("polls"), "-1000000").unwrap();
        let watching = engine.dispatch(json!({"op": "watch-ci", "chat": chat,
            "pull_request": "https://github.com/o/r/pull/7"}));
        assert_eq!(watching["finished"], false, "{watching}");
        let first = store.ci_watches().unwrap();
        assert_eq!(first.len(), 1);
        // Asking again starts the watch over in its place.
        engine.dispatch(json!({"op": "watch-ci", "chat": chat,
            "pull_request": "https://github.com/o/r/pull/7"}));
        let second = store.ci_watches().unwrap();
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].token, second[0].token);
        // The first watch's card says it was watched again; the new one runs.
        let shown = cards();
        assert_eq!(shown.len(), 4, "{shown:?}");
        assert_eq!(shown[2]["status"], "stopped");
        assert_eq!(shown[3]["status"], "running");
        let deleted = engine.dispatch(json!({"op": "delete-chat", "chat": chat}));
        assert_eq!(deleted["ok"], true, "{deleted}");
        assert!(store.ci_watches().unwrap().is_empty());

        // SAFETY: executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe {
            std::env::remove_var("XD_GH_EXECUTABLE");
            std::env::remove_var("XD_CLAUDE_EXECUTABLE");
            std::env::remove_var("XD_CI_POLL_MILLIS");
        }
        engine.runtime.as_ref().unwrap().shutdown();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_say_what_passed_and_why_a_watch_stopped() {
        let root = std::env::temp_dir().join(format!("xd-ci-report-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("Workspaces")).unwrap();
        let store = StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap();
        let mut status = parse_status(&rollup(json!([
            {"__typename": "CheckRun", "name": "linux", "workflowName": "pr",
                "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "name": "docs", "status": "COMPLETED",
                "conclusion": "SKIPPED"},
        ])))
        .unwrap();
        assert_eq!(
            report(&Ending::Finished(status.clone()), &store, &root),
            "[xd] CI passed on https://github.com/restartfu/xd/pull/291 at 1e3d4a4.\n\n\
             1 check passed, 1 skipped."
        );
        status.checks.truncate(1);
        status.checks.push(status.checks[0].clone());
        assert!(report(&Ending::Finished(status), &store, &root).ends_with("\n\n2 checks passed."));
        assert_eq!(
            report(
                &Ending::Stopped {
                    url: "https://github.com/o/r/pull/7".into(),
                    reason: "The pull request was merged before its checks finished.".into(),
                },
                &store,
                &root
            ),
            "[xd] Stopped watching CI on https://github.com/o/r/pull/7.\n\n\
             The pull request was merged before its checks finished."
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
