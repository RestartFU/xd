use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Stdio},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    EventBus, StateStore,
    agent::{AgentCommand, AgentEvent, AgentParser, TodoItem, TodoUpdate},
    ask::{self, Ask},
    reply_images::ReplyImages,
    secrets::SecretsStore,
    storage::TurnSpec,
    subagent::{SubagentPatch, SubagentRun, SubagentStatus},
};

/// How often a streamed reply is parked on the chat so an interrupted turn
/// keeps most of its text without turning every token into a database write.
const PARK_STREAMED_REPLY_EVERY: Duration = Duration::from_millis(500);

/// What the chat shows for a stopped turn the agent never recorded.
const CUT_OFF_TURN: &str = "Stopped before this turn was saved. Its work is lost, and the agent only knows what you asked.";

/// How much of a process's stderr is kept for the message that reports it.
const STDERR_KEPT_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct TurnRuntime {
    inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    store: Arc<StateStore>,
    events: Arc<EventBus>,
    active: Mutex<HashMap<String, ActiveProcess>>,
    next_turn: AtomicU64,
    secrets: Arc<SecretsStore>,
    commands: Mutex<HashMap<String, (String, Vec<String>)>>,
    /// Agent processes kept between a chat's turns, by chat.
    sessions: Mutex<HashMap<String, AgentSession>>,
    todos: Mutex<HashMap<String, Vec<TodoItem>>>,
    interactions: Mutex<HashMap<String, PendingInteractions>>,
    next_interaction: AtomicU64,
    interaction_epoch: String,
    /// Tool records rewritten in place -- subagent cards, a turn's diff -- by
    /// chat, so a later report updates the card it belongs to.
    records: Mutex<HashMap<String, ChatRecords>>,
}

/// Records kept per chat before the oldest are forgotten. A forgotten record
/// stays in the transcript; it just stops receiving updates.
const MAX_RECORDS_PER_CHAT: usize = 200;
/// Lines held between turns for a process whose idle output waits for the
/// next turn.
const MAX_BACKLOG_LINES: usize = 10_000;
/// A turn the agent started by itself ends if it goes this long without a
/// word, so a lost result cannot leave the chat working forever.
const BACKGROUND_QUIET_LIMIT: Duration = Duration::from_secs(15 * 60);
/// How long a turn that has finished waits on a steer it has not heard back
/// about. Long enough for an agent to start on it; if it has not by then, the
/// turn really is over.
#[cfg(not(test))]
const STEER_SETTLE_LIMIT: Duration = Duration::from_secs(10);
#[cfg(test)]
const STEER_SETTLE_LIMIT: Duration = Duration::from_millis(300);
/// Marks a live `tool` row that is really a message the user sent into the
/// turn. The stored row is an ordinary user message; a client that does not
/// know the marker shows a small tool row rather than the agent's own words.
const STEER_ROW_PREFIX: &str = "steer\n";

/// How long a kept process waits for its next turn before it is stopped. The
/// next message resumes the conversation in a new process.
const IDLE_AGENT_LIMIT: Duration = Duration::from_secs(30 * 60);
/// The same for a settled chat, which nobody is expected to come back to soon.
/// A moment's quiet still counts, so a process is not stopped mid-sentence.
const SETTLED_AGENT_LIMIT: Duration = Duration::from_secs(60);
/// Tools that have a kept process act later on nothing but its own timers, with
/// no child process or running task to show for it.
const WAKEUP_TOOLS: [&str; 2] = ["ScheduleWakeup", "CronCreate"];

#[derive(Default)]
struct ChatRecords {
    entries: HashMap<String, RecordEntry>,
    /// Other ids an agent is reported under, such as Claude's task id.
    aliases: HashMap<String, String>,
    /// Tasks the agent marks as housekeeping, which get no card.
    hidden: HashSet<String>,
    order: VecDeque<String>,
}

struct RecordEntry {
    /// Zero until the record's first snapshot is stored.
    message_id: i64,
    run: Option<SubagentRun>,
}

/// What storing a record's newest snapshot takes.
enum RecordWrite {
    New {
        key: String,
        text: String,
    },
    Update {
        message_id: i64,
        key: String,
        text: String,
    },
}

/// One line of a kept process's output, or its end.
enum Feed {
    Line(String),
    /// A message for the running turn. It travels with the output so the turn
    /// handles it in order, between one line and the next.
    Steer(String),
    End,
}

/// Where a kept process's output goes.
///
/// One reader thread owns the pipe for the process's whole life. A turn
/// installs itself as the sink while it runs; between turns the runtime
/// handles what arrives -- a background agent's progress, or a turn the agent
/// starts by itself to report back -- instead of leaving it in the pipe for the
/// next turn to misread as its own answer.
struct Router {
    sink: Option<mpsc::Sender<Feed>>,
    backlog: VecDeque<String>,
    backend: String,
    child: Arc<Mutex<Child>>,
    complaint: Arc<Mutex<String>>,
    /// The latest turn this process answered. A turn the agent starts by
    /// itself is modelled on it.
    template: TurnSpec,
    /// When the process last said anything or finished a turn.
    quiet_since: Instant,
    /// It has timers of its own that only keep running while it does.
    wakes_itself: bool,
    /// A background task has finished, and the agent has not yet begun the turn
    /// in which it may report on it.
    report_due: bool,
    /// It was stopped for being idle, so what it said last has nowhere to go.
    released: bool,
    /// Claude's tasks it has started and not yet reported finished, by task id:
    /// background agents and shells, which die with the process. See
    /// [`track_tasks`].
    tasks: HashSet<String>,
}

impl Router {
    /// Whether the process has had nothing to do for at least `limit`.
    fn at_rest_for(&self, limit: Duration) -> bool {
        self.sink.is_none()
            && !self.wakes_itself
            && self.tasks.is_empty()
            // Lines left over from Claude are waiting for a turn about to start.
            // Codex always leaves its idle output for the next turn.
            && (self.backend != "claude" || self.backlog.is_empty())
            && self.quiet_since.elapsed() >= limit
    }
}

/// Where a turn reads its agent's output from.
enum LineSource {
    /// A process that answers one turn and exits.
    Pipe(BufReader<ChildStdout>),
    /// A kept process, through its router.
    Feed {
        lines: mpsc::Receiver<Feed>,
        background: bool,
        /// How long to wait for a line before calling the turn over.
        patience: Option<Duration>,
    },
}

enum NextLine {
    Line,
    Steer(String),
    End,
    Quiet,
    Failed,
}

impl LineSource {
    fn next(&mut self, line: &mut String) -> NextLine {
        line.clear();
        match self {
            Self::Pipe(reader) => match reader.read_line(line) {
                Ok(0) => NextLine::End,
                Ok(_) => NextLine::Line,
                Err(_) => NextLine::Failed,
            },
            Self::Feed {
                lines,
                background,
                patience,
            } => {
                let limit = patience.or(background.then_some(BACKGROUND_QUIET_LIMIT));
                let received = if let Some(limit) = limit {
                    match lines.recv_timeout(limit) {
                        Ok(feed) => feed,
                        Err(mpsc::RecvTimeoutError::Timeout) => return NextLine::Quiet,
                        Err(mpsc::RecvTimeoutError::Disconnected) => Feed::End,
                    }
                } else {
                    lines.recv().unwrap_or(Feed::End)
                };
                match received {
                    Feed::Line(text) => {
                        *line = text;
                        NextLine::Line
                    }
                    Feed::Steer(prompt) => NextLine::Steer(prompt),
                    Feed::End => NextLine::End,
                }
            }
        }
    }

    fn set_patience(&mut self, limit: Option<Duration>) {
        if let Self::Feed { patience, .. } = self {
            *patience = limit;
        }
    }

    /// Steers that arrived too late for the turn to see, oldest first.
    fn stranded_steers(&self) -> Vec<String> {
        let Self::Feed { lines, .. } = self else {
            return Vec::new();
        };
        lines
            .try_iter()
            .filter_map(|feed| match feed {
                Feed::Steer(prompt) => Some(prompt),
                _ => None,
            })
            .collect()
    }
}

/// What a Claude line that arrives between turns calls for.
enum IdleLine {
    /// Progress for a background agent's card.
    Records(Vec<SubagentPatch>),
    /// The agent is talking or asking: that needs a turn.
    Wake,
    Ignore,
}

fn claude_idle_line(line: &str) -> IdleLine {
    let Ok(root) = serde_json::from_str::<Value>(line) else {
        return IdleLine::Ignore;
    };
    let kind = root.get("type").and_then(Value::as_str).unwrap_or_default();
    let subtype = root
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let child = root
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .is_some_and(|parent| !parent.is_empty());
    if child || (kind == "system" && subtype.starts_with("task_")) {
        let records = crate::agent::ClaudeParser::default()
            .feed(line)
            .into_iter()
            .filter_map(|event| match event {
                AgentEvent::Subagent(patch) => Some(patch),
                _ => None,
            })
            .collect();
        return IdleLine::Records(records);
    }
    match (kind, subtype) {
        ("system", "init") | ("stream_event", _) | ("assistant", _) | ("control_request", _) => {
            IdleLine::Wake
        }
        _ => IdleLine::Ignore,
    }
}

/// Whether the agent owes a report on a finished task after this line of output,
/// given whether it did before. A task finishing sets that and only the start of
/// a turn clears it, so a process that never starts one just waits longer.
fn report_due_after(line: &str, due: bool) -> bool {
    if !line.contains("task_notification") && !line.contains("init") {
        return due;
    }
    let Ok(root) = serde_json::from_str::<Value>(line) else {
        return due;
    };
    match (root["type"].as_str(), root["subtype"].as_str()) {
        (Some("system"), Some("task_notification")) => true,
        (Some("system"), Some("init")) => false,
        _ => due,
    }
}

/// Keeps `tasks`, the ids of Claude's unfinished tasks, up to date with one line
/// of its output.
///
/// Claude reports a background agent, a backgrounded shell command or a
/// workflow with `task_started`, and its end with `task_notification` or a
/// `task_updated` that gives it a final status. A task Claude marks as
/// housekeeping with `skip_transcript` is not counted: it gets no card, so the
/// user could not see what was holding things up, and nothing says it reports
/// its end.
fn track_tasks(line: &str, tasks: &mut HashSet<String>) {
    if !line.contains("task_") {
        return;
    }
    let Ok(root) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if root["type"] != "system" {
        return;
    }
    let Some(id) = root["task_id"].as_str().filter(|id| !id.is_empty()) else {
        return;
    };
    let status = |status: &Value| status.as_str().and_then(SubagentStatus::from_wire);
    match root["subtype"].as_str() {
        Some("task_started") if root["skip_transcript"] != true => {
            tasks.insert(id.to_owned());
        }
        Some("task_notification") if status(&root["status"]) != Some(SubagentStatus::Running) => {
            tasks.remove(id);
        }
        Some("task_updated")
            if status(&root["patch"]["status"])
                .is_some_and(|status| status != SubagentStatus::Running) =>
        {
            tasks.remove(id);
        }
        _ => {}
    }
}

/// Whether a line of agent output shows the agent setting a timer.
fn schedules_a_wakeup(line: &str) -> bool {
    if !WAKEUP_TOOLS.iter().any(|tool| line.contains(tool)) {
        return false;
    }
    let Ok(root) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let is_wakeup = |block: &Value| {
        block["type"] == "tool_use"
            && block["name"]
                .as_str()
                .is_some_and(|name| WAKEUP_TOOLS.contains(&name))
    };
    root.pointer("/event/content_block").is_some_and(is_wakeup)
        || root
            .pointer("/message/content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| blocks.iter().any(is_wakeup))
}

#[derive(Default)]
struct PendingInteractions {
    revision: u64,
    requests: HashMap<String, PendingInteraction>,
}

/// A message sent into a running turn.
struct SteeredMessage {
    prompt: String,
    /// Its stored row, once it has one.
    row: Option<i64>,
}

struct PendingInteraction {
    id: Value,
    method: String,
    params: Value,
    turn_id: u64,
    /// Claude asks over its control channel and is answered the same way.
    claude: bool,
}

/// An agent process that outlives the turn it was started for.
///
/// `claude -p <prompt>` answers once and exits, and everything it started goes
/// with it: a background shell, a watch, a build. The next message then meets a
/// process that never heard of any of it. With `--input-format stream-json` the
/// turns arrive on stdin instead and the process stays up, so what it started
/// is still running when the next one arrives. It stays up for as long as it
/// has something running; see [`TurnRuntime::release_idle_agents`].
struct AgentSession {
    child: Arc<Mutex<Child>>,
    /// The child's pid, readable without waiting on `child`.
    #[cfg(target_os = "linux")]
    pid: u32,
    stdin: ChildStdin,
    /// Where the process's output goes; see [`Router`].
    router: Arc<Mutex<Router>>,
    /// What this process was started with. A turn that wants anything else --
    /// another model, another directory -- needs its own process, because all
    /// of it was fixed in argv when this one started.
    fingerprint: String,
    /// Filled by a drain thread for the process's whole life: stderr has to be
    /// read continuously or it fills and the agent blocks on it.
    complaint: Arc<Mutex<String>>,
}

impl Drop for RuntimeInner {
    fn drop(&mut self) {
        if let Ok(active) = self.active.lock() {
            for process in active.values() {
                process.cancelled.store(true, Ordering::Release);
                if let Ok(mut child) = process.child.lock() {
                    let _ = terminate_agent(&mut child);
                }
            }
        }
        // Kept processes are idle rather than active, and nothing else will
        // reap them once this runtime is gone.
        if let Ok(mut sessions) = self.sessions.lock() {
            for session in sessions.values_mut() {
                if let Ok(mut child) = session.child.lock() {
                    let _ = terminate_agent(&mut child);
                }
            }
            sessions.clear();
        }
    }
}

struct ActiveProcess {
    message_boundary: i64,
    turn_id: u64,
    child: Arc<Mutex<Child>>,
    /// The child's pid, readable without waiting on `child`.
    #[cfg(target_os = "linux")]
    pid: u32,
    cancelled: Arc<AtomicBool>,
    started: Instant,
    label: String,
    progress: Arc<TurnProgress>,
}

/// What a turn has published so far, readable from outside the turn thread.
///
/// A client that opens a chat mid-turn has to be told where the turn already
/// is, or it shows a turn that started an unknown time ago with none of the
/// text it has produced.
#[derive(Default)]
struct ProgressState {
    sequence: u64,
    segment: String,
    items: Vec<Value>,
}
#[derive(Default)]
struct TurnProgress {
    state: Mutex<ProgressState>,
    ready: AtomicBool,
}
impl TurnProgress {
    fn sequence(&self) -> u64 {
        self.state.lock().map(|state| state.sequence).unwrap_or(0)
    }
}

/// Where a live turn has got to, for a client that just loaded the chat.
pub struct LiveTurn {
    pub epoch: String,
    pub message_boundary: i64,
    pub items: Vec<Value>,
    pub turn_id: u64,
    pub sequence: u64,
    pub label: String,
    /// Seconds the turn has been running.
    pub working_for: u64,
    /// The streamed reply so far. Blocks and tool calls are already stored as
    /// messages, so only the text that is not yet a message belongs here.
    pub segment: String,
}

impl TurnRuntime {
    pub(crate) fn new(
        store: Arc<StateStore>,
        events: Arc<EventBus>,
        secrets: Arc<SecretsStore>,
    ) -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                store,
                events,
                active: Mutex::new(HashMap::new()),
                next_turn: AtomicU64::new(0),
                secrets,
                commands: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                todos: Mutex::new(HashMap::new()),
                interactions: Mutex::new(HashMap::new()),
                next_interaction: AtomicU64::new(1),
                interaction_epoch: uuid::Uuid::new_v4().to_string(),
                records: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub(crate) fn has_active_turns(&self) -> bool {
        self.inner
            .active
            .lock()
            .map_or(true, |active| !active.is_empty())
    }

    /// The agent processes this runtime started and has not let go of, each
    /// with the chat it works for.
    #[cfg(target_os = "linux")]
    pub(crate) fn agent_processes(&self) -> Vec<(u32, String)> {
        let mut found = Vec::new();
        if let Ok(active) = self.inner.active.lock() {
            found.extend(
                active
                    .iter()
                    .map(|(chat_id, process)| (process.pid, chat_id.clone())),
            );
        }
        if let Ok(sessions) = self.inner.sessions.lock() {
            found.extend(
                sessions
                    .iter()
                    .map(|(chat_id, session)| (session.pid, chat_id.clone())),
            );
        }
        found
    }

    /// How many background agents and shells the chat's kept process is still
    /// running. While no turn runs, a chat with any is waiting on them.
    pub(crate) fn background_tasks(&self, chat_id: &str) -> usize {
        self.inner
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| {
                let router = sessions.get(chat_id)?.router.lock().ok()?;
                Some(router.tasks.len())
            })
            .unwrap_or(0)
    }

    /// [`Self::background_tasks`] for every chat that has any.
    pub(crate) fn background_task_counts(&self) -> HashMap<String, usize> {
        let Ok(sessions) = self.inner.sessions.lock() else {
            return HashMap::new();
        };
        sessions
            .iter()
            .filter_map(|(chat_id, session)| {
                let count = session.router.lock().ok()?.tasks.len();
                (count > 0).then(|| (chat_id.clone(), count))
            })
            .collect()
    }

    /// Tells clients how many background tasks the chat now waits on: those of
    /// its current process, none without one. A process that has been replaced
    /// or has exited never speaks for the chat.
    ///
    /// The count is read and published while holding the sessions and the
    /// current process's router, which every change to that count holds, so
    /// whichever publication comes last carries the latest count.
    fn publish_background_tasks(&self, chat_id: &str) {
        let publish = |count: usize| {
            self.inner.events.publish(json!({
                "event": "background-tasks",
                "chat": chat_id,
                "background_tasks": count,
            }));
        };
        let Ok(sessions) = self.inner.sessions.lock() else {
            return;
        };
        match sessions.get(chat_id) {
            Some(session) => {
                if let Ok(router) = session.router.lock() {
                    publish(router.tasks.len());
                }
            }
            None => publish(0),
        }
    }

    /// Whether a kept process between turns is still running a background
    /// agent or shell. Stopping the process, as [`Self::shutdown`] does, stops
    /// those too, and the next process never hears of them.
    pub(crate) fn has_background_tasks(&self) -> bool {
        self.inner.sessions.lock().map_or(true, |sessions| {
            sessions.values().any(|session| {
                session
                    .router
                    .lock()
                    .map_or(true, |router| !router.tasks.is_empty())
            })
        })
    }

    pub(crate) fn shutdown(&self) {
        if let Ok(active) = self.inner.active.lock() {
            for (chat_id, process) in active.iter() {
                let _ = self.inner.store.mark_resume_after_restart(chat_id);
                process.cancelled.store(true, Ordering::Release);
                if let Ok(mut child) = process.child.lock() {
                    let _ = terminate_agent(&mut child);
                }
            }
        }
        if let Ok(mut sessions) = self.inner.sessions.lock() {
            for session in sessions.values_mut() {
                if let Ok(mut child) = session.child.lock() {
                    let _ = terminate_agent(&mut child);
                }
            }
            sessions.clear();
        }
    }

    /// Hands `prompt` to the chat's running turn. False means the turn cannot
    /// take it: there is none, it needs an answer from the user first, or its
    /// agent has no process to write to. The caller keeps the message then.
    pub(crate) fn steer(&self, chat_id: &str, prompt: &str) -> bool {
        let running = self.inner.active.lock().is_ok_and(|active| {
            active
                .get(chat_id)
                .is_some_and(|process| !process.cancelled.load(Ordering::Acquire))
        });
        let asking = self.inner.interactions.lock().is_ok_and(|interactions| {
            interactions
                .get(chat_id)
                .is_some_and(|pending| !pending.requests.is_empty())
        });
        if !running || asking {
            return false;
        }
        let Some(router) = self.router(chat_id) else {
            return false;
        };
        let Ok(router) = router.lock() else {
            return false;
        };
        router
            .sink
            .as_ref()
            .is_some_and(|sink| sink.send(Feed::Steer(prompt.to_owned())).is_ok())
    }

    /// Tell the agent why its command disappeared. Use the normal steering
    /// acknowledgement/requeue path so a turn ending concurrently cannot lose
    /// the notice. Agents that cannot take input now read it on their next turn.
    #[cfg(target_os = "linux")]
    pub(crate) fn notify_memory_guard(&self, chat_id: &str, message: &str) {
        let prompt = format!(
            "[xd host notice: memory guard]\n{message}\n\nThe command was stopped by xd's memory guard, so its output may end abruptly without a normal tool error. Do not retry the same workload unchanged or move it into the background. Reduce memory use first (for example, run fewer build jobs or one test at a time), or explain that validation is blocked by the available memory limit."
        );
        if !self.steer(chat_id, &prompt) {
            match self.inner.store.requeue_front(chat_id, &prompt) {
                Ok(event) => self.inner.events.publish(event),
                Err(error) => eprintln!("xd: cannot queue memory guard notice: {error}"),
            }
        }
    }

    /// Puts a message the agent never read back at the front of the queue.
    /// `returned` holds those this turn has already put back, oldest first, so
    /// the message lands in the order it was sent, however they came back.
    fn requeue_steer(
        &self,
        chat_id: &str,
        steer: u64,
        prompt: &str,
        returned: &mut Vec<(u64, String)>,
    ) {
        let position = returned.partition_point(|(other, _)| *other < steer);
        let earlier = returned[..position]
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>();
        match self.inner.store.requeue_after(chat_id, &earlier, prompt) {
            Ok(event) => {
                self.inner.events.publish(event);
                returned.insert(position, (steer, prompt.to_owned()));
            }
            Err(error) => eprintln!("xd: cannot requeue a steered message: {error}"),
        }
    }

    /// Takes back a message the agent will not answer: it no longer belongs in
    /// the transcript, and returns to the queue to be sent again.
    fn return_steer(
        &self,
        chat_id: &str,
        progress: &TurnProgress,
        steered: &mut HashMap<u64, SteeredMessage>,
        steer: u64,
        returned: &mut Vec<(u64, String)>,
    ) {
        let Some(message) = steered.remove(&steer) else {
            return;
        };
        if let Some(message_id) = message.row {
            let _ = self.inner.store.delete_message(chat_id, message_id);
            if let Ok(mut state) = progress.state.lock() {
                state
                    .items
                    .retain(|item| item["message_id"].as_i64() != Some(message_id));
            }
        }
        self.requeue_steer(chat_id, steer, &message.prompt, returned);
        self.inner
            .events
            .publish(json!({"event": "changed", "chat": chat_id}));
    }

    fn write_rpc(&self, chat_id: &str, frame: &Value) -> Result<(), String> {
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .map_err(|_| "Agent sessions unavailable".to_owned())?;
        let session = sessions
            .get_mut(chat_id)
            .ok_or("The agent session has ended")?;
        writeln!(session.stdin, "{frame}")
            .and_then(|()| session.stdin.flush())
            .map_err(|e| e.to_string())
    }

    #[cfg(test)]
    pub(crate) fn hold_interaction_for_test(&self, chat_id: &str) {
        let mut all = self.inner.interactions.lock().unwrap();
        let pending = all
            .entry(chat_id.to_owned())
            .or_insert_with(|| PendingInteractions {
                revision: 0,
                requests: HashMap::new(),
            });
        pending.requests.insert(
            "test-request".into(),
            PendingInteraction {
                id: json!("test-request"),
                method: "item/commandExecution/requestApproval".into(),
                params: json!({}),
                turn_id: 0,
                claude: false,
            },
        );
    }

    /// Whether the agent is waiting on an approval or question from the user.
    pub(crate) fn has_interactions(&self, chat_id: &str) -> bool {
        self.inner.interactions.lock().is_ok_and(|all| {
            all.get(chat_id)
                .is_some_and(|pending| !pending.requests.is_empty())
        })
    }

    pub(crate) fn interactions(&self, chat_id: &str) -> Value {
        self.inner.interactions.lock().ok().and_then(|all| all.get(chat_id).map(|pending| json!({"interaction_epoch":self.inner.interaction_epoch,"interaction_revision":pending.revision,"interactions":pending.requests.iter().map(|(id, request)| json!({"request_id":id,"method":request.method,"params":request.params})).collect::<Vec<_>>()}))).unwrap_or_else(|| json!({"interaction_epoch":self.inner.interaction_epoch,"interaction_revision":0,"interactions":[]}))
    }

    /// Stops a running Claude agent, or sends one the turn is waiting on to
    /// the background so the turn can go on without it.
    pub(crate) fn control_subagent(
        &self,
        chat_id: &str,
        key: &str,
        action: &str,
    ) -> Result<(), String> {
        let task_id = {
            let records = self
                .inner
                .records
                .lock()
                .map_err(|_| "Agent records unavailable".to_owned())?;
            let book = records
                .get(chat_id)
                .ok_or("That agent is no longer tracked")?;
            let run = book
                .entries
                .get(&format!("subagent:{key}"))
                .and_then(|entry| entry.run.as_ref())
                .ok_or("That agent is no longer tracked")?;
            if run.status != SubagentStatus::Running {
                return Err("That agent has already finished".into());
            }
            // The assistant that started the agent is the one that controls
            // it: an agent run as a command belongs to the shell's owner.
            let controller = if run.launcher.is_empty() {
                &run.provider
            } else {
                &run.launcher
            };
            if controller != "Claude" {
                return Err("Only Claude agents can be stopped from xd".into());
            }
            book.aliases
                .iter()
                .find(|(_, target)| target.as_str() == key)
                .map(|(task, _)| task.clone())
        };
        let request = match action {
            "stop" => json!({
                "subtype": "stop_task",
                "task_id": task_id.ok_or("Claude has not reported this agent's task yet")?,
            }),
            "background" => json!({"subtype": "background_tasks", "tool_use_id": key}),
            _ => return Err("Unknown agent action".into()),
        };
        let id = format!(
            "xd-{action}-{}",
            self.inner.next_interaction.fetch_add(1, Ordering::Relaxed)
        );
        self.write_rpc(
            chat_id,
            &json!({"type": "control_request", "request_id": id, "request": request}),
        )
    }

    pub(crate) fn respond(
        &self,
        chat_id: &str,
        request_id: &str,
        response: &Value,
    ) -> Result<(), String> {
        let mut all = self
            .inner
            .interactions
            .lock()
            .map_err(|_| "Pending requests unavailable".to_owned())?;
        let pending = all
            .get_mut(chat_id)
            .ok_or("This request is no longer pending")?;
        let request = pending
            .requests
            .get(request_id)
            .ok_or("This request was already answered or expired")?;
        if !self.inner.active.lock().is_ok_and(|active| {
            active
                .get(chat_id)
                .is_some_and(|turn| turn.turn_id == request.turn_id)
        }) {
            return Err("This turn has ended".into());
        }
        if request.claude {
            let answer = claude_answer(request, response)?;
            self.write_rpc(
                chat_id,
                &json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "success",
                        "request_id": request.id,
                        "response": answer,
                    },
                }),
            )?;
            pending.requests.remove(request_id);
            pending.revision = self.inner.next_interaction.fetch_add(1, Ordering::Relaxed);
            self.inner.events.publish(json!({"event":"interaction-resolved","chat":chat_id,"request_id":request_id,"interaction_epoch":self.inner.interaction_epoch,"interaction_revision":pending.revision}));
            return Ok(());
        }
        let result = match request.method.as_str() {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let decision = response
                    .get("decision")
                    .and_then(Value::as_str)
                    .ok_or("Approval needs a decision")?;
                if !matches!(
                    decision,
                    "accept" | "acceptForSession" | "decline" | "cancel"
                ) {
                    return Err("Invalid approval decision".into());
                }
                json!({"decision":decision})
            }
            "item/permissions/requestApproval" => {
                let granted = match response.get("decision").and_then(Value::as_str) {
                    Some("accept" | "acceptForSession") => request
                        .params
                        .get("permissions")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                    Some("decline" | "cancel") => json!({}),
                    _ => return Err("Permission request needs an approval decision".into()),
                };
                json!({"permissions":granted,"scope":if response["decision"] == "acceptForSession" {"session"} else {"turn"}})
            }
            "item/tool/requestUserInput" => {
                let answers = response
                    .get("answers")
                    .and_then(Value::as_object)
                    .ok_or("Question needs answers")?;
                let questions = request
                    .params
                    .get("questions")
                    .and_then(Value::as_array)
                    .ok_or("Invalid question request")?;
                if questions
                    .iter()
                    .any(|q| q["id"].as_str().is_none_or(|id| !answers.contains_key(id)))
                    || answers.len() != questions.len()
                {
                    return Err("Answer each requested question".into());
                }
                if answers.values().any(|a| {
                    a.get("answers")
                        .and_then(Value::as_array)
                        .is_none_or(|a| a.is_empty() || a.iter().any(|s| s.as_str().is_none()))
                }) {
                    return Err("Invalid question answers".into());
                }
                json!({"answers":answers})
            }
            _ => return Err("Unsupported interaction".into()),
        };
        self.write_rpc(chat_id, &json!({"id":request.id,"result":result}))?;
        pending.requests.remove(request_id);
        pending.revision = self.inner.next_interaction.fetch_add(1, Ordering::Relaxed);
        self.inner.events.publish(json!({"event":"interaction-resolved","chat":chat_id,"request_id":request_id,"interaction_epoch":self.inner.interaction_epoch,"interaction_revision":pending.revision}));
        Ok(())
    }

    pub fn start(&self, mut turn: TurnSpec) -> Result<(), String> {
        let lineage = self
            .inner
            .store
            .folder_lineage_for_chat(&turn.chat_id)
            .map_err(|error| error.to_string())?;
        let secrets = self.inner.secrets.effective(&lineage)?;
        turn.environment = secrets.environment;
        if let Some(secret_prompt) = secrets.prompt {
            turn.system_prompt = Some(match turn.system_prompt.take() {
                Some(prompt) if !prompt.is_empty() => format!("{prompt}\n\n{secret_prompt}"),
                _ => secret_prompt,
            });
        }
        let command_backend = turn.backend.clone();
        let command_model = turn.model.clone();
        let command_effort = turn.effort.clone();
        let specification = AgentCommand {
            backend: &command_backend,
            prompt: &turn.prompt,
            system_prompt: turn.system_prompt.as_deref(),
            workdir: &turn.workdir,
            model: &command_model,
            effort: &command_effort,
            access: &turn.access,
            fast: turn.fast,
            session_id: turn.session_id.as_deref(),
            environment: &turn.environment,
        };
        let app_server = command_backend == "codex";
        let keeps_process = app_server || AgentCommand::keeps_its_process(&command_backend);
        // Everything baked into argv when a process starts. A turn that wants
        // any of it different cannot be answered by the process already up.
        let fingerprint = format!(
            "{command_backend}\x01{command_model}\x01{command_effort}\x01{}\x01{}\x01{}\x01{}\x01{:?}",
            turn.access,
            turn.workdir,
            turn.fast,
            turn.system_prompt.as_deref().unwrap_or_default(),
            turn.environment,
        );
        let mut command = if app_server {
            specification.build_app_server()
        } else {
            specification.build()
        };
        let (child, source, complaint) = if keeps_process {
            self.session_for(
                &turn.chat_id,
                &fingerprint,
                &turn,
                &mut command,
                &command_backend,
            )?
        } else {
            let mut child = command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| format!("Cannot start {command_backend}: {error}"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| format!("Cannot read {command_backend} output."))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| format!("Cannot read {command_backend} errors."))?;
            let complaint = Arc::new(Mutex::new(String::new()));
            drain_stderr(stderr, complaint.clone());
            (
                Arc::new(Mutex::new(child)),
                LineSource::Pipe(BufReader::new(stdout)),
                complaint,
            )
        };
        self.launch(turn, child, source, complaint, keeps_process, false)
    }

    /// Registers a turn and starts the thread that reads it.
    fn launch(
        &self,
        turn: TurnSpec,
        child: Arc<Mutex<Child>>,
        source: LineSource,
        complaint: Arc<Mutex<String>>,
        keeps_process: bool,
        background: bool,
    ) -> Result<(), String> {
        let app_server = turn.backend == "codex";
        let message_boundary = self
            .inner
            .store
            .latest_message_id(&turn.chat_id)
            .map_err(|e| e.to_string())?;
        let turn_id = self.inner.next_turn.fetch_add(1, Ordering::Relaxed) + 1;
        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(TurnProgress::default());
        #[cfg(target_os = "linux")]
        let pid = child.lock().map_or(0, |child| child.id());
        {
            let mut active = self
                .inner
                .active
                .lock()
                .map_err(|_| "Agent state is unavailable.".to_string())?;
            if active.contains_key(&turn.chat_id) {
                if !background && let Ok(mut process) = child.lock() {
                    let _ = terminate_agent(&mut process);
                }
                return Err("That chat already has a running turn.".into());
            }
            active.insert(
                turn.chat_id.clone(),
                ActiveProcess {
                    message_boundary,
                    turn_id,
                    child: child.clone(),
                    #[cfg(target_os = "linux")]
                    pid,
                    cancelled: cancelled.clone(),
                    started: Instant::now(),
                    label: turn.label.clone(),
                    progress: progress.clone(),
                },
            );
        }
        let chat_id = turn.chat_id.clone();
        let label = turn.label.clone();
        let backend = turn.backend.clone();
        self.inner.events.publish(json!({
            "event": "turn-started",
            "turn_epoch": self.inner.interaction_epoch,
            "chat": chat_id,
            "label": label,
            "turn_id": turn_id,
            "turn_sequence": 0,
            "background": background,
        }));
        if app_server && !background {
            let child = child.clone();
            let active = self.inner.clone();
            let chat = chat_id.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(30));
                let starting = active.active.lock().is_ok_and(|running| {
                    running.get(&chat).is_some_and(|process| {
                        process.turn_id == turn_id
                            && !process.progress.ready.load(Ordering::Acquire)
                    })
                });
                if starting && let Ok(mut child) = child.lock() {
                    let _ = terminate_agent(&mut child);
                }
            });
        }
        let runtime = self.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("xd-agent-turn-{turn_id}"))
            .spawn({
                let child = child.clone();
                let cancelled = cancelled.clone();
                let progress = progress.clone();
                move || {
                    runtime.run(
                        turn,
                        turn_id,
                        child,
                        cancelled,
                        progress,
                        source,
                        complaint,
                        keeps_process,
                        background,
                    )
                }
            })
        {
            if let Ok(mut active) = self.inner.active.lock() {
                active.remove(&chat_id);
            }
            cancelled.store(true, Ordering::Release);
            if let Ok(mut child) = child.lock() {
                let _ = terminate_agent(&mut child);
            }
            return Err(format!("Cannot supervise {backend}: {error}"));
        }
        Ok(())
    }

    /// Starts a turn for output the agent produced on its own, between turns:
    /// Claude reporting back once a background agent finishes, or a background
    /// agent asking for permission. `first` is the line that called for it.
    fn start_background(&self, chat_id: &str, router: &mut Router, first: String) -> bool {
        if !matches!(self.inner.store.begin_background_turn(chat_id), Ok(true)) {
            return false;
        }
        if matches!(self.inner.store.clear_chat_settlement(chat_id), Ok(true)) {
            self.inner.events.publish(json!({"event": "tree"}));
        }
        let (sink, lines) = mpsc::channel();
        let _ = sink.send(Feed::Line(first));
        let mut turn = router.template.clone();
        turn.prompt.clear();
        let started = self.launch(
            turn,
            router.child.clone(),
            LineSource::Feed {
                lines,
                background: true,
                patience: None,
            },
            router.complaint.clone(),
            true,
            true,
        );
        if started.is_err() {
            let _ = self.inner.store.clear_interrupted_turn(chat_id);
            return false;
        }
        router.sink = Some(sink);
        true
    }

    /// Hands lines that arrived between turns to whoever should have them.
    fn pump(&self, chat_id: &str, router: &mut Router) {
        if router.released {
            router.backlog.clear();
            return;
        }
        while let Some(line) = router.backlog.pop_front() {
            if let Some(sink) = &router.sink {
                if sink.send(Feed::Line(line.clone())).is_ok() {
                    continue;
                }
                router.sink = None;
            }
            if router.backend != "claude" {
                // Codex's idle output waits for its next turn, as it always has.
                router.backlog.push_front(line);
                while router.backlog.len() > MAX_BACKLOG_LINES {
                    router.backlog.pop_front();
                }
                return;
            }
            match claude_idle_line(&line) {
                IdleLine::Records(patches) => {
                    let workdir = router.template.workdir.clone();
                    for patch in patches {
                        self.idle_subagent(chat_id, &workdir, patch);
                    }
                }
                IdleLine::Wake => {
                    if !self.start_background(chat_id, router, line.clone()) {
                        // A turn is about to start; it takes these lines first.
                        router.backlog.push_front(line);
                        return;
                    }
                }
                IdleLine::Ignore => {}
            }
        }
    }

    /// Reads a kept process for its whole life.
    fn spawn_reader(
        inner: Weak<RuntimeInner>,
        chat_id: String,
        stdout: ChildStdout,
        router: Arc<Mutex<Router>>,
    ) {
        let _ = thread::Builder::new()
            .name(format!("xd-agent-output-{chat_id}"))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let text = line.trim_end_matches(['\r', '\n']).to_owned();
                    let Some(inner) = inner.upgrade() else {
                        return;
                    };
                    let runtime = TurnRuntime { inner };
                    let tasks_changed = {
                        let Ok(mut router) = router.lock() else {
                            return;
                        };
                        router.quiet_since = Instant::now();
                        router.wakes_itself |= schedules_a_wakeup(&text);
                        router.report_due = report_due_after(&text, router.report_due);
                        let tasks_before = router.tasks.len();
                        track_tasks(&text, &mut router.tasks);
                        router.backlog.push_back(text);
                        runtime.pump(&chat_id, &mut router);
                        router.tasks.len() != tasks_before
                    };
                    // After the line is handled, so a report that starts a turn
                    // reaches clients first and the chat goes straight from
                    // waiting to working.
                    if tasks_changed {
                        runtime.publish_background_tasks(&chat_id);
                    }
                }
                let mut had_tasks = false;
                let idle = router.lock().is_ok_and(|mut router| {
                    // Whatever the process was running ended with it.
                    had_tasks = !router.tasks.is_empty();
                    router.tasks.clear();
                    match router.sink.take() {
                        Some(sink) => {
                            let _ = sink.send(Feed::End);
                            false
                        }
                        None => true,
                    }
                });
                if had_tasks && let Some(inner) = inner.upgrade() {
                    TurnRuntime { inner }.publish_background_tasks(&chat_id);
                }
                // A turn that holds the output settles its agents when it ends.
                // Otherwise this is the only sign the process is gone. Holding
                // the sessions keeps a new process from reporting first.
                if idle && let Some(inner) = inner.upgrade() {
                    let runtime = TurnRuntime { inner };
                    if let Ok(sessions) = runtime.inner.sessions.lock()
                        && sessions
                            .get(&chat_id)
                            .is_some_and(|session| Arc::ptr_eq(&session.router, &router))
                    {
                        runtime.settle_orphaned_agents(&chat_id);
                    }
                }
            });
    }

    fn router(&self, chat_id: &str) -> Option<Arc<Mutex<Router>>> {
        self.inner
            .sessions
            .lock()
            .ok()?
            .get(chat_id)
            .map(|session| session.router.clone())
    }

    /// Ends a turn's hold on its process's output. Lines it had not read yet
    /// go back to the front of the line, for whatever handles them next.
    fn detach_feed(&self, chat_id: &str, source: LineSource) -> Vec<String> {
        let LineSource::Feed { lines, .. } = source else {
            return Vec::new();
        };
        let Some(router) = self.router(chat_id) else {
            return Vec::new();
        };
        let Ok(mut router) = router.lock() else {
            return Vec::new();
        };
        router.sink = None;
        router.quiet_since = Instant::now();
        let mut unread = Vec::new();
        let mut steers = Vec::new();
        while let Ok(feed) = lines.try_recv() {
            match feed {
                Feed::Line(line) => unread.push(line),
                Feed::Steer(prompt) => steers.push(prompt),
                Feed::End => break,
            }
        }
        for line in unread.into_iter().rev() {
            router.backlog.push_front(line);
        }
        steers
    }

    /// Deals with output that is waiting now that no turn holds the process.
    fn pump_idle(&self, chat_id: &str) {
        if let Some(router) = self.router(chat_id)
            && let Ok(mut router) = router.lock()
            && router.sink.is_none()
        {
            self.pump(chat_id, &mut router);
        }
    }

    /// Stops the running turn, which for every backend means killing it.
    ///
    /// A kept process cannot survive that -- there is no way to interrupt one
    /// turn and leave the process ready for the next -- so its session goes
    /// too, and the next message starts a fresh one.
    pub fn cancel(&self, chat_id: &str) -> bool {
        let killed = {
            let Ok(active) = self.inner.active.lock() else {
                return false;
            };
            let Some(process) = active.get(chat_id) else {
                return false;
            };
            process.cancelled.store(true, Ordering::Release);
            process
                .child
                .lock()
                .is_ok_and(|mut child| terminate_agent(&mut child).is_ok())
        };
        if killed {
            self.drop_session(chat_id);
        }
        killed
    }

    /// Releases running and idle resources when a conversation is deleted.
    pub(crate) fn discard_chat(&self, chat_id: &str) {
        self.cancel(chat_id);
        self.drop_session(chat_id);
        if let Ok(mut commands) = self.inner.commands.lock() {
            commands.remove(chat_id);
        }
        if let Ok(mut todos) = self.inner.todos.lock() {
            todos.remove(chat_id);
        }
        if let Ok(mut interactions) = self.inner.interactions.lock() {
            interactions.remove(chat_id);
        }
        if let Ok(mut records) = self.inner.records.lock() {
            records.remove(chat_id);
        }
    }

    /// Where the chat's turn has got to, or `None` when nothing is running.
    pub fn live_turn(&self, chat_id: &str) -> Option<LiveTurn> {
        let active = self.inner.active.lock().ok()?;
        let process = active.get(chat_id)?;
        let state = process.progress.state.lock().ok()?;
        Some(LiveTurn {
            epoch: self.inner.interaction_epoch.clone(),
            message_boundary: process.message_boundary,
            items: state.items.clone(),
            turn_id: process.turn_id,
            sequence: state.sequence,
            label: process.label.clone(),
            working_for: process.started.elapsed().as_secs(),
            segment: state.segment.clone(),
        })
    }

    pub fn commands(&self, chat_id: &str, backend: &str) -> Vec<String> {
        self.inner
            .commands
            .lock()
            .ok()
            .and_then(|commands| commands.get(chat_id).cloned())
            .filter(|(stored_backend, _)| stored_backend == backend)
            .map(|(_, commands)| commands)
            .unwrap_or_default()
    }

    /// The process answering this chat: the one already up, or a new one.
    ///
    /// A kept process is reused only when the turn wants exactly what it was
    /// started with, and only when it is still alive and not mid-turn. Anything
    /// else takes its place.
    #[allow(clippy::type_complexity)]
    fn session_for(
        &self,
        chat_id: &str,
        fingerprint: &str,
        turn: &TurnSpec,
        command: &mut std::process::Command,
        backend: &str,
    ) -> Result<(Arc<Mutex<Child>>, LineSource, Arc<Mutex<String>>), String> {
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .map_err(|_| "Agent sessions are unavailable.".to_string())?;

        if let Some(session) = sessions.get_mut(chat_id) {
            let dead = session
                .child
                .lock()
                .ok()
                .and_then(|mut child| child.try_wait().ok().flatten())
                .is_some();
            if !dead && session.fingerprint == fingerprint {
                let router = session.router.clone();
                let mut router = router
                    .lock()
                    .map_err(|_| "Agent output is unavailable.".to_string())?;
                if router.sink.is_some() {
                    return Err("That chat already has a running turn.".into());
                }
                // The turn's reader goes in before the turn does, so not one
                // line of the answer can land anywhere else.
                let (sink, lines) = mpsc::channel();
                while let Some(line) = router.backlog.pop_front() {
                    let _ = sink.send(Feed::Line(line));
                }
                router.sink = Some(sink);
                router.template = turn.clone();
                // Writing the turn is also the test of whether the pipe is
                // still there; a broken one falls through to a fresh process.
                let frame = if backend == "codex" {
                    crate::codex_app::CodexAppParser::new(turn)
                        .turn_request()
                        .to_string()
                } else {
                    AgentCommand::encode_turn(&turn.prompt)
                };
                let written = writeln!(session.stdin, "{}", frame)
                    .and_then(|()| session.stdin.flush())
                    .is_ok();
                if written {
                    return Ok((
                        session.child.clone(),
                        LineSource::Feed {
                            lines,
                            background: false,
                            patience: None,
                        },
                        session.complaint.clone(),
                    ));
                }
                router.sink = None;
            }
            if let Some(gone) = sessions.remove(chat_id)
                && let Ok(mut child) = gone.child.lock()
            {
                let _ = terminate_agent(&mut child);
            }
            self.settle_orphaned_agents(chat_id);
        }

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Cannot start {backend}: {error}"))?;
        #[cfg(target_os = "linux")]
        let pid = child.id();
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("Cannot write to {backend}."))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("Cannot read {backend} output."))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| format!("Cannot read {backend} errors."))?;
        let frame = if backend == "codex" {
            crate::codex_app::CodexAppParser::initialize().to_string()
        } else {
            AgentCommand::encode_turn(&turn.prompt)
        };
        let complaint = Arc::new(Mutex::new(String::new()));
        drain_stderr(stderr, complaint.clone());
        let child = Arc::new(Mutex::new(child));
        let (sink, lines) = mpsc::channel();
        let router = Arc::new(Mutex::new(Router {
            sink: Some(sink),
            backlog: VecDeque::new(),
            backend: backend.to_owned(),
            child: child.clone(),
            complaint: complaint.clone(),
            template: turn.clone(),
            quiet_since: Instant::now(),
            wakes_itself: false,
            report_due: false,
            released: false,
            tasks: HashSet::new(),
        }));
        Self::spawn_reader(
            Arc::downgrade(&self.inner),
            chat_id.to_owned(),
            stdout,
            router.clone(),
        );
        if let Err(error) = writeln!(stdin, "{}", frame).and_then(|()| stdin.flush()) {
            if let Ok(mut child) = child.lock() {
                let _ = terminate_agent(&mut child);
            }
            return Err(format!("Cannot send the turn to {backend}: {error}"));
        }
        sessions.insert(
            chat_id.to_owned(),
            AgentSession {
                child: child.clone(),
                #[cfg(target_os = "linux")]
                pid,
                stdin,
                router,
                fingerprint: fingerprint.to_owned(),
                complaint: complaint.clone(),
            },
        );
        Ok((
            child,
            LineSource::Feed {
                lines,
                background: false,
                patience: None,
            },
            complaint,
        ))
    }

    /// Ends the kept process for a chat, if there is one.
    fn drop_session(&self, chat_id: &str) {
        if let Ok(mut sessions) = self.inner.sessions.lock()
            && let Some(session) = sessions.remove(chat_id)
            && let Ok(mut child) = session.child.lock()
        {
            let _ = terminate_agent(&mut child);
            let _ = child.wait();
        }
        self.settle_orphaned_agents(chat_id);
    }

    /// Stops the kept processes nobody is using, so an idle chat costs no memory.
    ///
    /// The next message starts a new process that resumes the conversation, but
    /// nothing brings back what a process was running, so anything still at work
    /// keeps it up: a turn, an open question, a background agent or shell, a
    /// child process or another process in its group, a timer. That includes a
    /// tool server that lasts as long as the process does, since it cannot be told
    /// apart from a long build.
    pub(crate) fn release_idle_agents(&self) {
        self.release_quiet_agents(IDLE_AGENT_LIMIT, SETTLED_AGENT_LIMIT);
    }

    fn release_quiet_agents(&self, idle: Duration, settled: Duration) -> usize {
        let settled_chats = match self.inner.store.settled_chats() {
            Ok(chats) => chats,
            Err(error) => {
                eprintln!("xd: cannot look for idle agents: {error}");
                return 0;
            }
        };
        // No other lock is held while the rest of the chat's state is read: the
        // process's output is checked again below, and a turn or an agent that
        // begins after this point changes it.
        let mut due = Vec::new();
        if let Ok(sessions) = self.inner.sessions.lock() {
            for (chat_id, session) in sessions.iter() {
                let Ok(router) = session.router.lock() else {
                    continue;
                };
                // An agent can take its time to report on a finished task, and
                // that report is lost if it is stopped first.
                let limit = if settled_chats.contains(chat_id) && !router.report_due {
                    settled
                } else {
                    idle
                };
                if router.at_rest_for(limit) {
                    due.push((chat_id.clone(), router.quiet_since));
                }
            }
        }
        due.retain(|(chat_id, _)| !self.has_work_in_progress(chat_id));
        if due.is_empty() {
            return 0;
        }
        let Some(accompanied) = crate::process_tree::with_company() else {
            return 0;
        };
        due.iter()
            .filter(|(chat_id, quiet_since)| {
                self.release_agent(chat_id, *quiet_since, &accompanied)
            })
            .count()
    }

    fn has_work_in_progress(&self, chat_id: &str) -> bool {
        let turn = self
            .inner
            .active
            .lock()
            .map_or(true, |active| active.contains_key(chat_id));
        let agent = self.inner.records.lock().map_or(true, |records| {
            records.get(chat_id).is_some_and(|book| {
                book.entries
                    .values()
                    .filter_map(|entry| entry.run.as_ref())
                    .any(|run| run.status == SubagentStatus::Running)
            })
        });
        turn || agent || self.has_interactions(chat_id)
    }

    /// Stops the chat's kept process if it has stayed as it was found.
    ///
    /// Taking the sessions and then the router is what a new turn does, so the
    /// turn either finds the process gone and starts another, or gets here first
    /// and is seen.
    fn release_agent(
        &self,
        chat_id: &str,
        quiet_since: Instant,
        accompanied: &HashSet<u32>,
    ) -> bool {
        let Ok(mut sessions) = self.inner.sessions.lock() else {
            return false;
        };
        let Some(session) = sessions.get(chat_id) else {
            return false;
        };
        let Ok(child) = session.child.lock() else {
            return false;
        };
        if accompanied.contains(&child.id()) {
            return false;
        }
        drop(child);
        {
            let Ok(mut router) = session.router.lock() else {
                return false;
            };
            if router.sink.is_some() || router.quiet_since != quiet_since {
                return false;
            }
            router.released = true;
        }
        if let Some(session) = sessions.remove(chat_id)
            && let Ok(mut child) = session.child.lock()
        {
            let _ = terminate_agent(&mut child);
            let _ = child.wait();
        }
        true
    }

    /// Stops the card of every agent still marked running, once the process
    /// reporting on them is gone. A new process, even one resuming the same
    /// session, never reports on them again, so nothing else would end them.
    fn settle_orphaned_agents(&self, chat_id: &str) {
        let orphans = self
            .inner
            .records
            .lock()
            .ok()
            .and_then(|records| {
                let book = records.get(chat_id)?;
                Some(
                    book.entries
                        .values()
                        .filter_map(|entry| entry.run.as_ref())
                        .filter(|run| run.status == SubagentStatus::Running)
                        .map(|run| run.key.clone())
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        for key in orphans {
            self.idle_subagent(
                chat_id,
                "",
                SubagentPatch {
                    status: Some(SubagentStatus::Stopped),
                    ..SubagentPatch::new(key)
                },
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        turn: TurnSpec,
        turn_id: u64,
        child: Arc<Mutex<Child>>,
        cancelled: Arc<AtomicBool>,
        progress: Arc<TurnProgress>,
        mut source: LineSource,
        complaint: Arc<Mutex<String>>,
        keeps_process: bool,
        background: bool,
    ) {
        let started = Instant::now();
        let mut parser = match if turn.backend == "codex" {
            Ok(AgentParser::AppServer(
                crate::codex_app::CodexAppParser::new(&turn),
            ))
        } else {
            AgentParser::new(&turn.backend)
        } {
            Ok(parser) => parser,
            Err(error) => {
                self.finish(turn, turn_id, 0, false, Some(error), 0, true, None, None);
                return;
            }
        };
        let mut completed = false;
        let mut latest_error = None;
        let mut cut_short = false;
        let mut had_activity = false;
        let mut recorded = false;
        let mut streamed_text = String::new();
        let mut assistant_text = String::new();
        let mut visible_streamed_bytes = 0;
        let mut published_text = false;
        let mut context_usage = None;
        let mut parked = Instant::now();
        let mut reply_images = ReplyImages::default();
        let store = self.inner.store.clone();
        let workdir = turn.workdir.clone();
        let mut keep_image = move |path: &str| store.keep_reply_image(&workdir, path);
        // Messages sent into this turn, by the turn's number for each.
        let mut steered: HashMap<u64, SteeredMessage> = HashMap::new();
        let mut next_steer = 0;
        let mut returned_steers = Vec::new();
        // When waiting to hear what became of a steer runs out. Output from
        // elsewhere can't put it off, so this is a limit and not a silence.
        let mut settle_deadline: Option<Instant> = None;

        let mut line = String::new();
        // Stores the reply streamed so far as its own message, which a tool
        // card that follows it must come after. A line still held back as a
        // possible image ends with the message.
        macro_rules! close_segment {
            () => {
                let held = reply_images.finish(&mut keep_image);
                if !held.is_empty() {
                    streamed_text.push_str(&held);
                    assistant_text.push_str(&held);
                    if self.publish_streamed(
                        &turn,
                        turn_id,
                        &progress,
                        &streamed_text,
                        &mut visible_streamed_bytes,
                    ) {
                        published_text = true;
                    }
                }
                if !streamed_text.is_empty() {
                    let segment = std::mem::take(&mut streamed_text);
                    if let Err(error) = self.inner.store.append_turn_message(
                        &turn.chat_id,
                        "assistant",
                        &segment,
                        Some(&turn.label),
                    ) {
                        latest_error = Some(error.to_string());
                    } else {
                        had_activity = true;
                    }
                    visible_streamed_bytes = 0;
                    let _ = self.inner.store.set_partial_reply(&turn.chat_id, "", None);
                }
            };
        }
        loop {
            let now = Instant::now();
            settle_deadline = parser
                .settling()
                .then(|| settle_deadline.unwrap_or(now + STEER_SETTLE_LIMIT));
            source.set_patience(settle_deadline.map(|deadline| deadline - now.min(deadline)));
            let next = if settle_deadline.is_some_and(|deadline| now >= deadline) {
                NextLine::Quiet
            } else {
                source.next(&mut line)
            };
            match next {
                NextLine::Line => {}
                NextLine::Steer(prompt) => {
                    close_segment!();
                    // Whatever the agent was saying no longer stands, so a
                    // question in it is not the question this turn ends on.
                    assistant_text.clear();
                    next_steer += 1;
                    let steer = next_steer;
                    let Some(frame) = parser.steer_frame(steer, &prompt) else {
                        self.requeue_steer(&turn.chat_id, steer, &prompt, &mut returned_steers);
                        continue;
                    };
                    steered.insert(
                        steer,
                        SteeredMessage {
                            prompt: prompt.clone(),
                            row: None,
                        },
                    );
                    // It can't be told anything more, whether or not it has died
                    // and whatever is still holding its output open. Ending it
                    // ends the turn, which gives back what it did not read.
                    if self.write_rpc(&turn.chat_id, &frame).is_err() {
                        if let Ok(mut child) = child.lock() {
                            let _ = terminate_agent(&mut child);
                        }
                        continue;
                    }
                    match self
                        .inner
                        .store
                        .append_turn_message(&turn.chat_id, "user", &prompt, None)
                    {
                        Ok(message_id) => {
                            had_activity = true;
                            if let Some(message) = steered.get_mut(&steer) {
                                message.row = Some(message_id);
                            }
                            self.publish_sequenced(
                                "tool",
                                &turn,
                                turn_id,
                                &progress,
                                json!({
                                    "text": format!("{STEER_ROW_PREFIX}{prompt}"),
                                    "workdir": turn.workdir,
                                    "context": turn.workdir,
                                    "message_id": message_id,
                                }),
                            );
                        }
                        Err(error) => latest_error = Some(error.to_string()),
                    }
                    continue;
                }
                // End of output. For a process that answers once this is the
                // end of the turn; for a kept one it means it died.
                NextLine::End => break,
                NextLine::Quiet => {
                    let settled = parser.take_settled();
                    completed = settled.is_empty();
                    for event in settled {
                        match event {
                            AgentEvent::Completed => completed = true,
                            AgentEvent::Resumable => cut_short = true,
                            AgentEvent::Error(error) => latest_error = Some(error),
                            _ => {}
                        }
                    }
                    break;
                }
                NextLine::Failed => {
                    latest_error = Some(format!("Cannot read {} output.", turn.backend));
                    break;
                }
            }
            let line = line.trim_end_matches(['\r', '\n']).to_owned();
            for event in parser.feed(&line) {
                match event {
                    AgentEvent::Ready => {
                        progress.ready.store(true, Ordering::Release);
                    }
                    AgentEvent::Rpc(frame) => {
                        if let Err(error) = self.write_rpc(&turn.chat_id, &frame) {
                            latest_error = Some(error);
                        }
                    }
                    AgentEvent::Interaction {
                        id,
                        method,
                        mut params,
                    } => {
                        // What people read: paths relative to the project. The
                        // agent's own copy of the input stays as it sent it.
                        for field in ["command", "reason", "patch"] {
                            if let Some(text) = params.get(field).and_then(Value::as_str) {
                                params[field] = Value::String(relative_paths(text, &turn.workdir));
                            }
                        }
                        let request_id =
                            format!("{}:{turn_id}:{}", self.inner.interaction_epoch, id);
                        let claude = turn.backend == "claude";
                        if matches!(
                            method.as_str(),
                            "item/commandExecution/requestApproval"
                                | "item/fileChange/requestApproval"
                                | "item/tool/requestUserInput"
                                | "item/permissions/requestApproval"
                        ) {
                            if let Ok(mut interactions) = self.inner.interactions.lock() {
                                let pending = interactions.entry(turn.chat_id.clone()).or_default();
                                pending.requests.insert(
                                    request_id.clone(),
                                    PendingInteraction {
                                        id,
                                        method: method.clone(),
                                        params: params.clone(),
                                        turn_id,
                                        claude,
                                    },
                                );
                                pending.revision =
                                    self.inner.next_interaction.fetch_add(1, Ordering::Relaxed);
                                self.inner.events.publish(json!({"event":"interaction", "chat":turn.chat_id, "request_id":request_id, "method":method, "params":params,"interaction_epoch":self.inner.interaction_epoch,"interaction_revision":pending.revision}));
                            }
                        } else {
                            let _ = self.write_rpc(&turn.chat_id, &json!({"id":id, "error":{"code":-32601,"message":"Unsupported client request"}}));
                        }
                    }
                    AgentEvent::InteractionResolved(id) => {
                        let request_id =
                            format!("{}:{turn_id}:{}", self.inner.interaction_epoch, id);
                        if let Ok(mut interactions) = self.inner.interactions.lock()
                            && let Some(pending) = interactions.get_mut(&turn.chat_id)
                        {
                            pending.requests.remove(&request_id);
                            pending.revision =
                                self.inner.next_interaction.fetch_add(1, Ordering::Relaxed);
                            self.inner.events.publish(json!({"event":"interaction-resolved", "chat":turn.chat_id, "request_id":request_id,"interaction_epoch":self.inner.interaction_epoch,"interaction_revision":pending.revision}));
                        }
                    }
                    AgentEvent::Session(session) => {
                        recorded = true;
                        if let Err(error) = self.inner.store.set_session(
                            &turn.chat_id,
                            &turn.session_backend,
                            &session,
                        ) {
                            latest_error = Some(error.to_string());
                        }
                        // The session has the note now, even if this host dies
                        // before the turn ends.
                        if turn.carries_cut_off_note && !background {
                            let _ = self.inner.store.clear_cut_off_turn(&turn.chat_id);
                        }
                    }
                    AgentEvent::Commands(commands) => {
                        if let Ok(mut cached) = self.inner.commands.lock() {
                            cached.insert(
                                turn.chat_id.clone(),
                                (turn.backend.clone(), commands.clone()),
                            );
                        }
                        self.inner.events.publish(json!({
                            "event": "commands",
                            "chat": turn.chat_id,
                            "backend": turn.backend,
                            "commands": commands,
                        }));
                    }
                    AgentEvent::Text(text) => {
                        let text = reply_images.whole(&text, &mut keep_image);
                        assistant_text.push_str(&text);
                        match self.inner.store.append_turn_message(
                            &turn.chat_id,
                            "assistant",
                            &text,
                            Some(&turn.label),
                        ) {
                            Ok(_) => {
                                had_activity = true;
                                let visible = ask::visible_bytes(&text);
                                if visible > 0 {
                                    // Each block is stored as its own message, so
                                    // the live view needs the same break.
                                    let separator = if published_text { "\n\n" } else { "" };
                                    self.publish_sequenced(
                                        "text",
                                        &turn,
                                        turn_id,
                                        &progress,
                                        json!({"text": format!("{separator}{}", &text[..visible])}),
                                    );
                                    published_text = true;
                                }
                            }
                            Err(error) => latest_error = Some(error.to_string()),
                        }
                    }
                    AgentEvent::TextDelta(text) => {
                        let text = reply_images.push(&text, &mut keep_image);
                        streamed_text.push_str(&text);
                        assistant_text.push_str(&text);
                        if self.publish_streamed(
                            &turn,
                            turn_id,
                            &progress,
                            &streamed_text,
                            &mut visible_streamed_bytes,
                        ) {
                            published_text = true;
                            // A client that loads the chat now has to be handed
                            // the same text the live events already carried.
                        }
                        // Park the reply so far, throttled so a token stream is
                        // not a write storm. Only an interrupted turn ever
                        // reads it back.
                        if parked.elapsed() >= PARK_STREAMED_REPLY_EVERY {
                            if let Err(error) = self.inner.store.set_partial_reply(
                                &turn.chat_id,
                                &streamed_text,
                                Some(&turn.label),
                            ) {
                                latest_error = Some(error.to_string());
                            }
                            parked = Instant::now();
                        }
                    }
                    AgentEvent::Subagent(patch) => {
                        if let Some(write) = self.fold_subagent(&turn.chat_id, &turn.workdir, patch)
                        {
                            if matches!(write, RecordWrite::New { .. }) {
                                close_segment!();
                            }
                            match self.write_record(&turn, turn_id, &progress, write) {
                                Ok(()) => had_activity = true,
                                Err(error) => latest_error = Some(error),
                            }
                        }
                    }
                    AgentEvent::Record { key, text } => {
                        let write =
                            self.fold_record(&turn.chat_id, format!("{turn_id}:{key}"), text);
                        if matches!(write, RecordWrite::New { .. }) {
                            close_segment!();
                        }
                        match self.write_record(&turn, turn_id, &progress, write) {
                            Ok(()) => had_activity = true,
                            Err(error) => latest_error = Some(error),
                        }
                    }
                    AgentEvent::Tool(text) => {
                        close_segment!();
                        let text = relative_paths(&text, &turn.workdir);
                        match self.inner.store.append_turn_message(
                            &turn.chat_id,
                            "tool",
                            &text,
                            None,
                        ) {
                            Ok(_) => {
                                had_activity = true;
                                self.publish_sequenced(
                                    "tool",
                                    &turn,
                                    turn_id,
                                    &progress,
                                    json!({
                                        "text": text,
                                        "workdir": turn.workdir,
                                        "context": turn.workdir,
                                    }),
                                );
                            }
                            Err(error) => latest_error = Some(error.to_string()),
                        }
                    }
                    AgentEvent::Todos(update) => match self.todo_snapshot(&turn.chat_id, update) {
                        Ok(todos) => {
                            let text = format!("todo_list\n{}", todo_json(&todos));
                            match self.inner.store.append_turn_message(
                                &turn.chat_id,
                                "tool",
                                &text,
                                None,
                            ) {
                                Ok(_) => {
                                    had_activity = true;
                                    self.publish_sequenced(
                                        "tool",
                                        &turn,
                                        turn_id,
                                        &progress,
                                        json!({
                                            "text": text,
                                            "workdir": turn.workdir,
                                            "context": turn.workdir,
                                        }),
                                    );
                                }
                                Err(error) => latest_error = Some(error.to_string()),
                            }
                        }
                        Err(error) => latest_error = Some(error),
                    },
                    AgentEvent::Completed => {
                        completed = true;
                        latest_error = None;
                    }
                    AgentEvent::Resumable => cut_short = true,
                    AgentEvent::Error(error) => latest_error = Some(error),
                    AgentEvent::SteerRejected(steer) => self.return_steer(
                        &turn.chat_id,
                        &progress,
                        &mut steered,
                        steer,
                        &mut returned_steers,
                    ),
                    AgentEvent::Usage {
                        input,
                        output,
                        window,
                    } => {
                        if let Some(usage) =
                            usage_snapshot(input, output, window, turn.context_window)
                        {
                            context_usage = Some(usage);
                        }
                    }
                }
            }
            // A kept process does not close its output between turns, so the
            // turn ends at the result event rather than at end of file.
            if keeps_process && (completed || latest_error.is_some()) {
                break;
            }
        }

        let held = reply_images.finish(&mut keep_image);
        if !held.is_empty() {
            streamed_text.push_str(&held);
            assistant_text.push_str(&held);
            self.publish_streamed(
                &turn,
                turn_id,
                &progress,
                &streamed_text,
                &mut visible_streamed_bytes,
            );
        }
        if !streamed_text.is_empty() {
            match self.inner.store.append_turn_message(
                &turn.chat_id,
                "assistant",
                &streamed_text,
                Some(&turn.label),
            ) {
                Ok(_) => had_activity = true,
                Err(error) => latest_error = Some(error.to_string()),
            }
        }

        // Nothing will answer these now. The process may still read them later,
        // so it has to go before they can be sent again.
        let unread_steers = parser.take_unread_steers();

        let was_cancelled = cancelled.load(Ordering::Acquire);
        let success = if keeps_process {
            // Nothing to wait for: it is still up, ready for the next turn.
            // Whether the turn worked is what it said, not how it exited.
            let alive = child
                .lock()
                .ok()
                .and_then(|mut child| child.try_wait().ok().flatten())
                .is_none();
            let stranded = if alive && completed && !was_cancelled && unread_steers.is_empty() {
                self.detach_feed(&turn.chat_id, source)
            } else {
                // Closed before the drain, under the lock a steer sends with, or
                // one arriving between the two would land where nothing reads it.
                if let Some(router) = self.router(&turn.chat_id)
                    && let Ok(mut router) = router.lock()
                {
                    router.sink = None;
                }
                self.drop_session(&turn.chat_id);
                source.stranded_steers()
            };
            for steer in &unread_steers {
                self.return_steer(
                    &turn.chat_id,
                    &progress,
                    &mut steered,
                    *steer,
                    &mut returned_steers,
                );
            }
            // These never reached the turn, so they follow everything that did.
            for prompt in &stranded {
                next_steer += 1;
                self.requeue_steer(&turn.chat_id, next_steer, prompt, &mut returned_steers);
            }
            was_cancelled || completed
        } else {
            let status = child.lock().ok().and_then(|mut child| child.wait().ok());
            self.settle_orphaned_agents(&turn.chat_id);
            was_cancelled || (completed && status.is_some_and(|status| status.success()))
        };
        let stderr = complaint
            .lock()
            .map(|said| said.clone())
            .unwrap_or_default();
        // Claude holds `init` back until a forked skill finishes, and until then
        // keeps nothing of the turn in its session: not even the request.
        // Saving its id would not help, since it cannot resume an empty session.
        let cut_off = turn.backend == "claude" && !background && !completed && !recorded;
        if cut_off {
            let _ = self
                .inner
                .store
                .mark_cut_off_turn(&turn.chat_id, &turn.request);
        } else if turn.carries_cut_off_note && !background && completed {
            let _ = self.inner.store.clear_cut_off_turn(&turn.chat_id);
        }
        let error = if success {
            cut_off.then(|| CUT_OFF_TURN.to_owned())
        } else {
            latest_error.or_else(|| {
                let stderr = stderr.trim();
                Some(if stderr.is_empty() {
                    format!("{} stopped unexpectedly.", turn.backend)
                } else {
                    stderr.into()
                })
            })
        };
        if let Ok(mut active) = self.inner.active.lock()
            && active
                .get(&turn.chat_id)
                .is_some_and(|active| active.turn_id == turn_id)
        {
            active.remove(&turn.chat_id);
        }
        if let Ok(mut interactions) = self.inner.interactions.lock() {
            let pending = interactions.entry(turn.chat_id.clone()).or_default();
            pending.requests.clear();
            pending.revision = self.inner.next_interaction.fetch_add(1, Ordering::Relaxed);
            self.inner.events.publish(json!({"event":"interactions-cleared", "chat":turn.chat_id,"interaction_epoch":self.inner.interaction_epoch,"interaction_revision":pending.revision}));
        }
        let asked = ask::parse(&assistant_text);
        let chat_id = turn.chat_id.clone();
        if was_cancelled || cut_short {
            let _ = self.inner.store.mark_turn_resumable(&chat_id);
        }
        self.finish(
            turn,
            turn_id,
            progress.sequence(),
            success,
            error,
            started.elapsed().as_secs(),
            !had_activity && !was_cancelled && !background,
            asked,
            context_usage,
        );
        if keeps_process {
            self.pump_idle(&chat_id);
        }
    }

    fn todo_snapshot(&self, chat_id: &str, update: TodoUpdate) -> Result<Vec<TodoItem>, String> {
        let mut snapshots = self
            .inner
            .todos
            .lock()
            .map_err(|_| "Todo state is unavailable.".to_owned())?;
        if !snapshots.contains_key(chat_id) {
            let existing = self
                .inner
                .store
                .latest_todo_snapshot(chat_id)
                .map_err(|error| error.to_string())?
                .and_then(|marker| marker.strip_prefix("todo_list\n").map(str::to_owned))
                .and_then(|json| todos_from_json(&json))
                .unwrap_or_default();
            snapshots.insert(chat_id.to_owned(), existing);
        }
        let todos = snapshots.entry(chat_id.to_owned()).or_default();
        match update {
            TodoUpdate::Replace(replacement) => *todos = replacement,
            TodoUpdate::Upsert(item) => {
                if let Some(existing) = todos.iter_mut().find(|todo| todo.id == item.id) {
                    *existing = item;
                } else {
                    todos.push(item);
                }
            }
            TodoUpdate::Patch { id, text, status } => {
                if let Some(todo) = todos.iter_mut().find(|todo| todo.id == id) {
                    if let Some(text) = text {
                        todo.text = text;
                    }
                    if let Some(status) = status {
                        todo.status = status;
                    }
                } else if let Some(text) = text {
                    todos.push(TodoItem::new(
                        id,
                        text,
                        status.unwrap_or(crate::agent::TodoStatus::Pending),
                    ));
                }
            }
            TodoUpdate::Remove(id) => todos.retain(|todo| todo.id != id),
        }
        Ok(todos.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        turn: TurnSpec,
        turn_id: u64,
        sequence: u64,
        success: bool,
        error: Option<String>,
        duration: u64,
        silent: bool,
        asked: Option<Ask>,
        context_usage: Option<(u64, u64)>,
    ) {
        if let Some((used, window)) = context_usage {
            let _ = self.inner.store.set_context_usage(
                &turn.chat_id,
                &turn.session_backend,
                Some(&turn.model),
                used,
                window,
            );
        }
        let finish = self.inner.store.finish_turn(
            &turn.chat_id,
            success,
            error.as_deref(),
            duration,
            silent,
        );
        let Ok(finish) = finish else {
            return;
        };
        let mut event = json!({
            "event": "turn-finished",
            "turn_epoch": self.inner.interaction_epoch,
            "chat": turn.chat_id,
            "turn_id": turn_id,
            "turn_sequence": sequence,
            "ok": success,
            "waiting": asked.is_some(),
            "silent": success && silent,
            "duration": duration,
            "last_message_id": finish.last_message_id,
        });
        if let Some(error) = error {
            event["error"] = Value::String(error);
        }
        if let Some(asked) = asked {
            event["question"] = Value::String(asked.question);
            event["options"] = serde_json::to_value(asked.options).unwrap_or(Value::Array(vec![]));
            event["accepts_input"] = Value::Bool(asked.accepts_input);
        }
        self.inner.events.publish(event);
        if let Some(queue_event) = finish.queue_event {
            self.inner.events.publish(queue_event);
        }
        if let Some(next) = finish.next
            && let Err(error) = self.start(next.clone())
        {
            let _ = self.inner.store.abort_turn_start(&next.chat_id, &error);
            self.inner
                .events
                .publish(json!({"event": "changed", "chat": next.chat_id}));
        }
    }

    /// Folds an agent's report into its record. `None` when the report names
    /// no agent this host has seen.
    fn fold_subagent(
        &self,
        chat_id: &str,
        workdir: &str,
        mut patch: SubagentPatch,
    ) -> Option<RecordWrite> {
        {
            let mut records = self.inner.records.lock().ok()?;
            let book = records.entry(chat_id.to_owned()).or_default();
            let ids = || {
                [Some(patch.key.as_str()), patch.task_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .filter(|id| !id.is_empty())
            };
            // Claude's housekeeping tasks are not the reader's business.
            if patch.hidden {
                for id in ids() {
                    book.hidden.insert(id.to_owned());
                }
                return None;
            }
            if ids().any(|id| book.hidden.contains(id)) {
                return None;
            }
        }
        if let Some(step) = &mut patch.step {
            step.text = relative_paths(&step.text, workdir);
        }
        patch.activity = patch
            .activity
            .map(|activity| relative_paths(&activity, workdir));
        let mut records = self.inner.records.lock().ok()?;
        let book = records.entry(chat_id.to_owned()).or_default();
        let key = if patch.key.is_empty() {
            book.aliases.get(patch.task_id.as_deref()?)?.clone()
        } else {
            if let Some(task) = &patch.task_id {
                book.aliases.insert(task.clone(), patch.key.clone());
            }
            patch.key.clone()
        };
        let record_key = format!("subagent:{key}");
        let entry = Self::record_entry(book, &record_key);
        let run = entry.run.get_or_insert_with(|| SubagentRun::new(&key));
        run.apply(SubagentPatch { key, ..patch });
        let text = run.encode();
        Some(if entry.message_id == 0 {
            RecordWrite::New {
                key: record_key,
                text,
            }
        } else {
            RecordWrite::Update {
                message_id: entry.message_id,
                key: record_key,
                text,
            }
        })
    }

    fn fold_record(&self, chat_id: &str, key: String, text: String) -> RecordWrite {
        let Ok(mut records) = self.inner.records.lock() else {
            return RecordWrite::New { key, text };
        };
        let book = records.entry(chat_id.to_owned()).or_default();
        let entry = Self::record_entry(book, &key);
        if entry.message_id == 0 {
            RecordWrite::New { key, text }
        } else {
            RecordWrite::Update {
                message_id: entry.message_id,
                key,
                text,
            }
        }
    }

    fn record_entry<'a>(book: &'a mut ChatRecords, key: &str) -> &'a mut RecordEntry {
        if !book.entries.contains_key(key) {
            book.order.push_back(key.to_owned());
            while book.order.len() > MAX_RECORDS_PER_CHAT {
                if let Some(old) = book.order.pop_front() {
                    book.entries.remove(&old);
                    book.aliases
                        .retain(|_, target| format!("subagent:{target}") != old);
                }
            }
        }
        book.entries.entry(key.to_owned()).or_insert(RecordEntry {
            message_id: 0,
            run: None,
        })
    }

    fn bind_record(&self, chat_id: &str, key: &str, message_id: i64) {
        if let Ok(mut records) = self.inner.records.lock()
            && let Some(entry) = records
                .get_mut(chat_id)
                .and_then(|book| book.entries.get_mut(key))
        {
            entry.message_id = message_id;
        }
    }

    /// Stores a record's newest snapshot during a turn and tells clients.
    fn write_record(
        &self,
        turn: &TurnSpec,
        turn_id: u64,
        progress: &TurnProgress,
        write: RecordWrite,
    ) -> Result<(), String> {
        match write {
            RecordWrite::New { key, text } => {
                let id = self
                    .inner
                    .store
                    .append_turn_message(&turn.chat_id, "tool", &text, None)
                    .map_err(|error| error.to_string())?;
                self.bind_record(&turn.chat_id, &key, id);
                self.publish_sequenced(
                    "tool",
                    turn,
                    turn_id,
                    progress,
                    json!({
                        "text": text,
                        "record": key,
                        "message_id": id,
                        "workdir": turn.workdir,
                        "context": turn.workdir,
                    }),
                );
            }
            RecordWrite::Update {
                message_id,
                key,
                text,
            } => {
                self.inner
                    .store
                    .replace_message_content(&turn.chat_id, message_id, &text)
                    .map_err(|error| error.to_string())?;
                self.publish_sequenced(
                    "record",
                    turn,
                    turn_id,
                    progress,
                    json!({"text": text, "record": key, "message_id": message_id}),
                );
            }
        }
        Ok(())
    }

    /// A background agent's progress while no turn is running: its card is
    /// updated in place, and the chat stays free for the user.
    // The write below is the condition, so it stays out of a match guard.
    #[allow(clippy::collapsible_match)]
    fn idle_subagent(&self, chat_id: &str, workdir: &str, patch: SubagentPatch) {
        match self.fold_subagent(chat_id, workdir, patch) {
            Some(RecordWrite::Update {
                message_id,
                key,
                text,
            }) => {
                if self
                    .inner
                    .store
                    .replace_message_content(chat_id, message_id, &text)
                    .is_ok()
                {
                    self.inner.events.publish(json!({
                        "event": "record",
                        "chat": chat_id,
                        "record": key,
                        "message_id": message_id,
                        "text": text,
                    }));
                }
            }
            Some(RecordWrite::New { key, text }) => {
                if let Ok(id) = self
                    .inner
                    .store
                    .append_turn_message(chat_id, "tool", &text, None)
                {
                    self.bind_record(chat_id, &key, id);
                    self.inner
                        .events
                        .publish(json!({"event": "changed", "chat": chat_id}));
                }
            }
            None => {}
        }
    }

    /// Passes on whatever of the streamed reply is ready to be seen, and says
    /// whether there was any.
    fn publish_streamed(
        &self,
        turn: &TurnSpec,
        turn_id: u64,
        progress: &TurnProgress,
        streamed_text: &str,
        visible_streamed_bytes: &mut usize,
    ) -> bool {
        let visible = ask::visible_bytes(streamed_text);
        if visible <= *visible_streamed_bytes {
            return false;
        }
        self.publish_sequenced(
            "text",
            turn,
            turn_id,
            progress,
            json!({"text": &streamed_text[*visible_streamed_bytes..visible]}),
        );
        *visible_streamed_bytes = visible;
        true
    }

    fn publish_sequenced(
        &self,
        name: &str,
        turn: &TurnSpec,
        turn_id: u64,
        progress: &TurnProgress,
        fields: Value,
    ) {
        let sequence = {
            let mut state = progress.state.lock().expect("turn progress lock");
            state.sequence += 1;
            if name == "text" {
                if let Some(text) = fields["text"].as_str() {
                    state.segment.push_str(text);
                }
            } else if name == "tool"
                && fields["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("todo_list\n"))
            {
                state.items.retain(|item| {
                    !item["text"]
                        .as_str()
                        .is_some_and(|text| text.starts_with("todo_list\n"))
                });
                state.items.push(json!({"tool":true,"text":fields["text"]}));
            } else if name == "tool" {
                if !state.segment.is_empty() {
                    let text = std::mem::take(&mut state.segment);
                    state.items.push(json!({"tool":false,"text":text}));
                }
                let mut item = json!({"tool":true,"text":fields["text"]});
                if let Some(record) = fields.get("record") {
                    item["record"] = record.clone();
                }
                if let Some(message_id) = fields.get("message_id") {
                    item["message_id"] = message_id.clone();
                }
                state.items.push(item);
            } else if name == "record" {
                // A client loading mid-turn gets the newest snapshot, in the
                // place the record first appeared.
                if let Some(item) = state
                    .items
                    .iter_mut()
                    .find(|item| item.get("record") == fields.get("record"))
                {
                    item["text"] = fields["text"].clone();
                }
            }
            state.sequence
        };
        let mut event = fields.as_object().cloned().unwrap_or_default();
        event.insert("event".into(), name.into());
        event.insert("chat".into(), turn.chat_id.clone().into());
        event.insert("turn_id".into(), turn_id.into());
        event.insert(
            "turn_epoch".into(),
            self.inner.interaction_epoch.clone().into(),
        );
        event.insert("turn_sequence".into(), sequence.into());
        self.inner.events.publish(Value::Object(event));
    }
}

/// Claude's answer to a permission prompt or a question, in the shape its
/// control channel expects.
/// Paths inside the chat's working directory, written relative to it: the
/// way people read them, and what fits on one line.
fn relative_paths(text: &str, workdir: &str) -> String {
    let root = workdir.trim_end_matches('/');
    if root.len() < 2 {
        return text.to_owned();
    }
    text.replace(&format!("{root}/"), "")
}

fn claude_answer(request: &PendingInteraction, response: &Value) -> Result<Value, String> {
    let claude = &request.params["claude"];
    let input = claude.get("input").cloned().unwrap_or_else(|| json!({}));
    if request.method == "item/tool/requestUserInput" {
        let answers = response
            .get("answers")
            .and_then(Value::as_object)
            .ok_or("Question needs answers")?;
        let mut by_question = serde_json::Map::new();
        for question in request.params["questions"].as_array().into_iter().flatten() {
            let id = question["id"].as_str().ok_or("Invalid question request")?;
            let chosen = answers
                .get(id)
                .and_then(|answer| answer.get("answers"))
                .and_then(Value::as_array)
                .filter(|chosen| !chosen.is_empty())
                .ok_or("Answer each requested question")?
                .iter()
                .map(|choice| choice.as_str().ok_or("Invalid question answers"))
                .collect::<Result<Vec<_>, _>>()?;
            let text = question["question"].as_str().unwrap_or(id);
            by_question.insert(text.to_owned(), Value::String(chosen.join(", ")));
        }
        let mut updated = input;
        updated["answers"] = Value::Object(by_question);
        return Ok(json!({"behavior": "allow", "updatedInput": updated}));
    }
    match response.get("decision").and_then(Value::as_str) {
        Some("accept") => Ok(json!({"behavior": "allow", "updatedInput": input})),
        Some("acceptForSession") => Ok(json!({
            "behavior": "allow",
            "updatedInput": input,
            "updatedPermissions": claude.get("suggestions").cloned().unwrap_or_else(|| json!([])),
        })),
        Some("decline") => Ok(json!({
            "behavior": "deny",
            "message": "The user declined this tool call.",
        })),
        Some("cancel") => Ok(json!({
            "behavior": "deny",
            "message": "The user stopped this turn.",
            "interrupt": true,
        })),
        _ => Err("Approval needs a decision".into()),
    }
}

fn todo_json(todos: &[TodoItem]) -> String {
    Value::Array(
        todos
            .iter()
            .map(|todo| {
                json!({
                    "id": todo.id,
                    "text": todo.text,
                    "status": match todo.status {
                        crate::agent::TodoStatus::Pending => "pending",
                        crate::agent::TodoStatus::InProgress => "in_progress",
                        crate::agent::TodoStatus::Completed => "completed",
                    },
                })
            })
            .collect(),
    )
    .to_string()
}

fn todos_from_json(json: &str) -> Option<Vec<TodoItem>> {
    serde_json::from_str::<Value>(json)
        .ok()?
        .as_array()?
        .iter()
        .map(|todo| {
            let status = match todo.get("status").and_then(Value::as_str)? {
                "pending" => crate::agent::TodoStatus::Pending,
                "in_progress" => crate::agent::TodoStatus::InProgress,
                "completed" => crate::agent::TodoStatus::Completed,
                _ => return None,
            };
            Some(TodoItem::new(
                todo.get("id").and_then(Value::as_str)?,
                todo.get("text").and_then(Value::as_str)?,
                status,
            ))
        })
        .collect()
}

fn terminate_agent(child: &mut Child) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        if let Ok(pid) = libc::pid_t::try_from(child.id())
            && unsafe { libc::kill(-pid, libc::SIGKILL) } == 0
        {
            return Ok(());
        }
    }
    child.kill()
}

/// Reads a process's stderr for as long as it lives, keeping the tail.
///
/// A kept process outlives every turn, and stderr that nobody reads fills its
/// pipe and blocks the agent behind it. The cap is on what is remembered, not
/// on what is read.
fn drain_stderr(stderr: impl Read + Send + 'static, into: Arc<Mutex<String>>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let Ok(mut said) = into.lock() else { return };
            said.push_str(&line);
            if said.len() > STDERR_KEPT_BYTES {
                let cut = said.len() - STDERR_KEPT_BYTES;
                let cut = (cut..said.len())
                    .find(|at| said.is_char_boundary(*at))
                    .unwrap_or(said.len());
                said.replace_range(..cut, "");
            }
        }
    });
}

fn usage_snapshot(
    input: u64,
    output: u64,
    reported_window: u64,
    configured_window: i64,
) -> Option<(u64, u64)> {
    let used = input.saturating_add(output);
    let window = if reported_window > 0 {
        reported_window
    } else {
        u64::try_from(configured_window).unwrap_or(0)
    };
    (used > 0 && window > 0).then_some((used, window))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_inside_the_workdir_read_relative() {
        let root = "/home/me/project";
        assert_eq!(
            relative_paths("$ cat -n /home/me/project/src/main.rs", root),
            "$ cat -n src/main.rs"
        );
        assert_eq!(
            relative_paths("Read  /home/me/project2/x.rs", root),
            "Read  /home/me/project2/x.rs"
        );
        assert_eq!(relative_paths("Read /etc/hosts", "/"), "Read /etc/hosts");
        assert_eq!(
            relative_paths("Read /home/me/project/a.rs", "/home/me/project/"),
            "Read a.rs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancel_kills_the_agent_process_group() {
        use std::{env, fs, os::unix::process::CommandExt, process::Command};

        let root = env::temp_dir().join(format!(
            "xd-runtime-process-group-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let descendant_file = root.join("descendant.pid");
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "sleep 30 & descendant=$!; printf '%s\\n' \"$descendant\" > \"$1\"; wait",
            "xd-process-group-test",
            descendant_file.to_str().unwrap(),
        ]);
        command.process_group(0);
        let child = command.spawn().unwrap();
        let group = child.id() as libc::pid_t;
        thread::sleep(Duration::from_millis(100));
        let descendant = fs::read_to_string(&descendant_file)
            .expect("descendant did not start")
            .trim()
            .parse::<libc::pid_t>()
            .unwrap();

        let store =
            Arc::new(StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap());
        let runtime = TurnRuntime::new(
            store,
            Arc::new(EventBus::default()),
            Arc::new(SecretsStore::new(None)),
        );
        runtime.inner.active.lock().unwrap().insert(
            "chat".to_owned(),
            ActiveProcess {
                message_boundary: 0,
                turn_id: 1,
                #[cfg(target_os = "linux")]
                pid: child.id(),
                child: Arc::new(Mutex::new(child)),
                cancelled: Arc::new(AtomicBool::new(false)),
                started: Instant::now(),
                label: "test".to_owned(),
                progress: Arc::new(TurnProgress::default()),
            },
        );

        assert!(runtime.cancel("chat"));
        let stopped = (0..100).any(|_| {
            let result = unsafe { libc::kill(descendant, 0) };
            if result == -1 {
                true
            } else {
                thread::sleep(Duration::from_millis(10));
                false
            }
        });
        if !stopped {
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
        assert!(stopped, "agent descendant survived cancellation");
        drop(runtime);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn turn_progress_numbers_every_published_event_and_holds_the_latest_segment() {
        let progress = TurnProgress::default();
        assert_eq!(progress.sequence(), 0);
        assert_eq!(
            {
                let mut state = progress.state.lock().unwrap();
                state.sequence += 1;
                state.sequence
            },
            1
        );
        assert_eq!(
            {
                let mut state = progress.state.lock().unwrap();
                state.sequence += 1;
                state.sequence
            },
            2
        );
        assert_eq!(progress.sequence(), 2);

        // The segment is the whole reply so far, not the last delta: a client
        // that loads mid-turn gets one snapshot and no chance to accumulate.
        progress.state.lock().unwrap().segment = "Half".into();
        progress.state.lock().unwrap().segment = "Half an ans".into();
        assert_eq!(progress.state.lock().unwrap().segment, "Half an ans");
    }

    #[test]
    fn usage_prefers_the_reported_window_and_falls_back_to_the_catalog() {
        assert_eq!(
            usage_snapshot(16_941, 7, 0, 272_000),
            Some((16_948, 272_000))
        );
        assert_eq!(
            usage_snapshot(21_328, 7, 1_000_000, 0),
            Some((21_335, 1_000_000))
        );
        assert_eq!(usage_snapshot(0, 0, 1_000_000, 0), None);
        assert_eq!(usage_snapshot(1, 0, 0, 0), None);
    }

    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        for _ in 0..200 {
            if ready() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {what}");
    }

    fn contents(store: &StateStore, chat: &str) -> Vec<(String, String)> {
        store.messages(&json!({"chat": chat})).unwrap()["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| {
                (
                    message["role"].as_str().unwrap().to_owned(),
                    message["content"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    /// An assistant names an image, streamed across deltas and in a block that
    /// arrives whole. The user gets the host's own copy, in the stored reply and
    /// in the live text alike, and that copy outlives the original.
    #[cfg(unix)]
    #[test]
    fn an_image_an_assistant_names_is_kept_and_shown_from_the_host_copy() {
        use std::{env, fs, os::unix::fs::PermissionsExt};

        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let root = env::temp_dir().join(format!(
            "xd-runtime-images-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("Workspaces")).unwrap();
        let original = root.join("shot.png");
        fs::write(&original, b"\x89PNG\r\n\x1a\npixels").unwrap();
        let agent = root.join("claude");
        let delta = |text: &str| {
            let event = json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}});
            format!("printf '%s\\n' '{event}'\n")
        };
        let path = original.to_string_lossy();
        let mut script = String::from(
            "#!/bin/sh\nread first\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}'\n",
        );
        script.push_str(&delta("Here it is:\n[ima"));
        script.push_str(&delta(&format!("ge: {path}")));
        script.push_str(&delta("]\nand again\n"));
        script.push_str(&delta(&format!("[image: {path}]")));
        script.push_str(&delta("\n[image: shot-missing.png]"));
        script.push_str("echo '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"done\"}'\nread second\n");
        fs::write(&agent, script).unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { env::set_var("XD_CLAUDE_EXECUTABLE", &agent) };

        let store =
            Arc::new(StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap());
        let folder = store.new_folder(&json!({"name": "project"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let chat = store
            .new_chat(&json!({"folder": folder, "backend": "claude"}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let events = Arc::new(EventBus::default());
        let (sender, live) = std::sync::mpsc::sync_channel(256);
        let (connection, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        events.subscribe(sender, connection).unwrap();
        let runtime = TurnRuntime::new(store.clone(), events, Arc::new(SecretsStore::new(None)));
        match store
            .prepare_send(&json!({"chat": chat, "text": "show me"}))
            .unwrap()
        {
            crate::storage::SendDisposition::Start { turn, .. } => runtime.start(turn).unwrap(),
            crate::storage::SendDisposition::Queued { .. } => panic!("the message was queued"),
        }
        wait_for("the reply", || {
            contents(&store, &chat)
                .iter()
                .any(|(role, _)| role == "assistant")
        });
        wait_for("the turn to end", || runtime.live_turn(&chat).is_none());

        let reply = contents(&store, &chat)
            .into_iter()
            .find(|(role, _)| role == "assistant")
            .unwrap()
            .1;
        assert!(!reply.contains(path.as_ref()), "{reply}");
        let lines = reply.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], "Here it is:");
        assert_eq!(lines[2], "and again");
        assert_eq!(
            lines[4],
            "Image unavailable: shot-missing.png (the file was not found)"
        );
        let kept = |line: &str| {
            line.strip_prefix("[image: ")
                .and_then(|line| line.strip_suffix(']'))
                .expect("a kept image")
                .to_owned()
        };
        let (first, second) = (kept(lines[1]), kept(lines[3]));
        // Named twice, stored once.
        assert_eq!(first, second);

        fs::remove_file(&original).unwrap();
        let read = store.image_read(&json!({"path": first})).unwrap();
        assert_eq!(read["mime"], "image/png");

        let mut streamed = String::new();
        while let Ok(event) = live.try_recv() {
            if event["event"] == "text" {
                streamed.push_str(event["text"].as_str().unwrap());
            }
        }
        assert_eq!(streamed, reply);
        let _ = fs::remove_dir_all(&root);
    }

    /// A background command reports nothing more once its process dies, so its
    /// card must stop rather than count up forever.
    #[cfg(unix)]
    #[test]
    fn agents_stop_when_the_process_reporting_on_them_dies() {
        use std::{env, fs, os::unix::fs::PermissionsExt};

        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let root = env::temp_dir().join(format!(
            "xd-runtime-orphans-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("Workspaces")).unwrap();
        let agent = root.join("claude");
        fs::write(
            &agent,
            r#"#!/bin/sh
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_shell","description":"Wait for checks","task_type":"local_bash","is_backgrounded":true}'
echo '{"type":"system","subtype":"task_started","task_id":"t2","tool_use_id":"toolu_done","description":"Print","task_type":"local_bash","is_backgrounded":true}'
echo '{"type":"system","subtype":"task_notification","task_id":"t2","tool_use_id":"toolu_done","status":"completed"}'
echo '{"type":"result","subtype":"success","result":"Started."}'
read second
"#,
        )
        .unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { env::set_var("XD_CLAUDE_EXECUTABLE", &agent) };

        let store =
            Arc::new(StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap());
        let folder = store.new_folder(&json!({"name": "project"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let chat = store
            .new_chat(&json!({"folder": folder, "backend": "claude"}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let runtime = TurnRuntime::new(
            store.clone(),
            Arc::new(EventBus::default()),
            Arc::new(SecretsStore::new(None)),
        );
        let turn = match store
            .prepare_send(&json!({"chat": chat, "text": "first"}))
            .unwrap()
        {
            crate::storage::SendDisposition::Start { turn, .. } => turn,
            crate::storage::SendDisposition::Queued { .. } => panic!("first was queued"),
        };
        runtime.start(turn).unwrap();
        wait_for("the turn to end", || {
            runtime.live_turn(&chat).is_none() && store.chat(&chat).unwrap()["working"] == false
        });
        let statuses = || {
            contents(&store, &chat)
                .iter()
                .filter_map(|(_, content)| crate::subagent::decode(content))
                .map(|record| {
                    (
                        record["key"].as_str().unwrap().to_owned(),
                        record["status"].as_str().unwrap().to_owned(),
                    )
                })
                .collect::<HashMap<_, _>>()
        };
        assert_eq!(statuses()["toolu_shell"], "running");

        // Dies between turns, with nothing reading on behalf of a turn.
        let child = runtime
            .inner
            .sessions
            .lock()
            .unwrap()
            .get(&chat)
            .unwrap()
            .child
            .clone();
        terminate_agent(&mut child.lock().unwrap()).unwrap();
        wait_for("the orphaned card to stop", || {
            statuses()["toolu_shell"] == "stopped"
        });
        assert_eq!(statuses()["toolu_done"], "completed");

        drop(runtime);
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { env::remove_var("XD_CLAUDE_EXECUTABLE") };
        let _ = fs::remove_dir_all(root);
    }

    struct SteerFixture {
        root: std::path::PathBuf,
        store: Arc<StateStore>,
        runtime: TurnRuntime,
        chat: String,
    }

    #[cfg(unix)]
    impl SteerFixture {
        fn start(script: &str) -> Self {
            use std::{env, fs, os::unix::fs::PermissionsExt};
            let root = env::temp_dir().join(format!(
                "xd-runtime-steer-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(root.join("Workspaces")).unwrap();
            let agent = root.join("claude");
            let script = script.replace("$HEARD", root.join("heard").to_str().unwrap());
            fs::write(&agent, script).unwrap();
            fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();
            // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
            unsafe { env::set_var("XD_CLAUDE_EXECUTABLE", &agent) };
            let store =
                Arc::new(StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap());
            let folder = store.new_folder(&json!({"name": "project"})).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let chat = store
                .new_chat(&json!({"folder": folder, "backend": "claude"}))
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let runtime = TurnRuntime::new(
                store.clone(),
                Arc::new(EventBus::default()),
                Arc::new(SecretsStore::new(None)),
            );
            match store
                .prepare_send(&json!({"chat": chat, "text": "first"}))
                .unwrap()
            {
                crate::storage::SendDisposition::Start { turn, .. } => runtime.start(turn).unwrap(),
                crate::storage::SendDisposition::Queued { .. } => panic!("first was queued"),
            }
            Self {
                root,
                store,
                runtime,
                chat,
            }
        }

        fn wait_for_text(&self, text: &str) {
            wait_for(text, || {
                contents(&self.store, &self.chat)
                    .iter()
                    .any(|(_, content)| content == text)
            });
        }

        fn wait_for_streamed(&self, text: &str) {
            wait_for(text, || {
                self.runtime
                    .live_turn(&self.chat)
                    .is_some_and(|turn| turn.segment == text)
            });
        }

        fn transcript(&self) -> Vec<(String, String)> {
            contents(&self.store, &self.chat)
                .into_iter()
                .filter(|(role, _)| role != "duration")
                .collect()
        }
    }

    #[cfg(unix)]
    impl Drop for SteerFixture {
        fn drop(&mut self) {
            self.runtime.discard_chat(&self.chat);
            // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
            unsafe { std::env::remove_var("XD_CLAUDE_EXECUTABLE") };
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn memory_guard_notice_reaches_the_running_agent() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Building."}]}}'
read notice
printf '%s\n' "$notice" > "$HEARD"
echo '{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_streaming"}'
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Reducing build jobs."}]}}'
echo '{"type":"result","subtype":"success","result":"Reducing build jobs."}'
read next
"#,
        );
        fixture.wait_for_streamed("Building.");
        fixture
            .runtime
            .notify_memory_guard(&fixture.chat, "xd stopped `rustc` (pid 21).");
        fixture.wait_for_text("Reducing build jobs.");
        let heard = std::fs::read_to_string(fixture.root.join("heard")).unwrap();
        let frame: Value = serde_json::from_str(&heard).unwrap();
        let prompt = frame["message"]["content"][0]["text"].as_str().unwrap();
        assert!(prompt.contains("[xd host notice: memory guard]"));
        assert!(prompt.contains("xd stopped `rustc` (pid 21)."));
        assert!(prompt.contains("Do not retry the same workload unchanged"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn memory_guard_notice_waits_when_no_turn_can_take_it() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"result","subtype":"success","result":"Done."}'
read next
"#,
        );
        wait_for("turn to end", || {
            fixture.runtime.live_turn(&fixture.chat).is_none()
        });
        fixture
            .runtime
            .notify_memory_guard(&fixture.chat, "xd stopped a background build.");
        let chat = fixture.store.chat(&fixture.chat).unwrap();
        let queue = chat["queue"].as_array().unwrap();
        assert_eq!(queue.len(), 1);
        assert!(
            queue[0]
                .as_str()
                .unwrap()
                .contains("xd stopped a background build.")
        );
    }

    /// The agent must not be killed: whatever it has running keeps running, and
    /// the same process answers the message sent into its turn.
    #[cfg(unix)]
    #[test]
    fn a_steer_reaches_the_running_agent_and_keeps_its_turn_and_process() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Working on it."}]}}'
read steer
printf '%s
' "$steer" > "$HEARD"
echo '{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_streaming"}'
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Redirected."}]}}'
echo '{"type":"result","subtype":"success","terminal_reason":"completed","result":"Redirected."}'
read next
"#,
        );
        fixture.wait_for_streamed("Working on it.");
        assert!(fixture.runtime.steer(&fixture.chat, "Do this instead"));
        fixture.wait_for_text("Redirected.");
        wait_for("the turn to end", || {
            fixture.runtime.live_turn(&fixture.chat).is_none()
        });

        assert_eq!(
            fixture.transcript(),
            [
                ("user", "first"),
                ("assistant", "Working on it."),
                ("user", "Do this instead"),
                ("assistant", "Redirected."),
            ]
            .map(|(role, text)| (role.to_owned(), text.to_owned()))
        );
        let heard: Value =
            serde_json::from_str(&std::fs::read_to_string(fixture.root.join("heard")).unwrap())
                .unwrap();
        assert_eq!(heard["priority"], "now");
        assert_eq!(heard["message"]["content"][0]["text"], "Do this instead");
        assert_eq!(fixture.store.chat(&fixture.chat).unwrap()["working"], false);
        assert!(
            fixture
                .runtime
                .inner
                .sessions
                .lock()
                .unwrap()
                .contains_key(&fixture.chat),
            "steering must leave the agent's process running"
        );
    }

    /// A client that opens the chat mid-turn sees the message where it was sent.
    #[cfg(unix)]
    #[test]
    fn a_steered_message_is_in_the_live_snapshot_with_its_marker() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Working on it."}]}}'
read steer
read never
"#,
        );
        fixture.wait_for_streamed("Working on it.");
        assert!(fixture.runtime.steer(&fixture.chat, "Do this instead"));
        wait_for("the live row", || {
            fixture
                .runtime
                .live_turn(&fixture.chat)
                .is_some_and(|turn| {
                    turn.items.iter().any(|item| {
                        item["tool"] == true && item["text"] == "steer\nDo this instead"
                    })
                })
        });
    }

    /// The turn ended just as the steers were written, and the agent never
    /// starts on them. The turn must end rather than wait forever, and they must
    /// still get answered, in order and only once, by a process that can't also
    /// read the first copy.
    #[cfg(unix)]
    #[test]
    fn steers_the_agent_never_read_are_sent_again_in_order() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
if [ -e "$HEARD" ]; then
  while read again; do
    echo '{"type":"system","subtype":"init","session_id":"s1"}'
    echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Answered again."}]}}'
    echo '{"type":"result","subtype":"success","terminal_reason":"completed","result":"Answered again."}'
  done
  exit 0
fi
: > "$HEARD"
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Working on it."}}}'
read steer
read another
echo '{"type":"result","subtype":"success","terminal_reason":"completed","result":"Working on it."}'
read next
"#,
        );
        fixture.wait_for_streamed("Working on it.");
        assert!(fixture.runtime.steer(&fixture.chat, "Do this instead"));
        assert!(fixture.runtime.steer(&fixture.chat, "And this"));
        wait_for("both to be answered", || {
            fixture.store.chat(&fixture.chat).unwrap()["working"] == false
                && fixture
                    .transcript()
                    .iter()
                    .filter(|(_, content)| content == "Answered again.")
                    .count()
                    == 2
        });
        assert_eq!(
            fixture.transcript(),
            [
                ("user", "first"),
                ("assistant", "Working on it."),
                ("user", "Do this instead"),
                ("assistant", "Answered again."),
                ("user", "And this"),
                ("assistant", "Answered again."),
            ]
            .map(|(role, text)| (role.to_owned(), text.to_owned()))
        );
    }

    /// Two steers with the same text: taking back the first must leave the second
    /// alone, in the live view as well as in the transcript.
    #[cfg(unix)]
    #[test]
    fn taking_back_a_steer_leaves_one_with_the_same_text_in_the_live_view() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
read first
read release
"#,
        );
        let store = &fixture.store;
        let chat = &fixture.chat;
        let row = |text: &str| store.append_turn_message(chat, "user", text, None).unwrap();
        let (first, second) = (row("Same text"), row("Same text"));
        let progress = TurnProgress::default();
        {
            let mut state = progress.state.lock().unwrap();
            for id in [first, second] {
                state
                    .items
                    .push(json!({"tool":true, "text":"steer\nSame text", "message_id":id}));
            }
        }
        let mut steered = HashMap::from([(
            1,
            SteeredMessage {
                prompt: "Same text".into(),
                row: Some(first),
            },
        )]);
        fixture
            .runtime
            .return_steer(chat, &progress, &mut steered, 1, &mut Vec::new());
        let state = progress.state.lock().unwrap();
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0]["message_id"], second);
        let remaining = fixture
            .transcript()
            .iter()
            .filter(|(_, content)| content == "Same text")
            .count();
        assert_eq!(remaining, 1);
        assert_eq!(store.chat(chat).unwrap()["queue"], json!(["Same text"]));
    }

    /// However they come back, and whatever the user has since dropped from the
    /// queue, returned steers wait in the order they were sent, ahead of the rest.
    #[cfg(unix)]
    #[test]
    fn returned_steers_keep_the_order_they_were_sent_in() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
read first
read release
"#,
        );
        let store = &fixture.store;
        let chat = &fixture.chat;
        let queue = || store.chat(chat).unwrap()["queue"].clone();
        store.requeue_front(chat, "queued").unwrap();
        let mut returned = Vec::new();
        for (steer, prompt) in [(3, "three"), (1, "one"), (4, "four"), (2, "two")] {
            fixture
                .runtime
                .requeue_steer(chat, steer, prompt, &mut returned);
        }
        assert_eq!(queue(), json!(["one", "two", "three", "four", "queued"]));

        store
            .take_queued(&json!({"chat": chat, "index": 0, "text": "one"}))
            .unwrap();
        store
            .take_queued(&json!({"chat": chat, "index": 0, "text": "two"}))
            .unwrap();
        store
            .take_queued(&json!({"chat": chat, "index": 0, "text": "three"}))
            .unwrap();
        store
            .take_queued(&json!({"chat": chat, "index": 0, "text": "four"}))
            .unwrap();
        fixture
            .runtime
            .requeue_steer(chat, 5, "five", &mut returned);
        assert_eq!(queue(), json!(["five", "queued"]));
    }

    /// Other output, such as a background agent's progress, must not put off the
    /// end of a turn that finished without reading a steer.
    #[cfg(unix)]
    #[test]
    fn output_from_elsewhere_does_not_extend_the_wait_for_a_new_run() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
if [ -e "$HEARD" ]; then
  while read again; do
    echo '{"type":"system","subtype":"init","session_id":"s1"}'
    echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Answered again."}]}}'
    echo '{"type":"result","subtype":"success","terminal_reason":"completed","result":"Answered again."}'
  done
  exit 0
fi
: > "$HEARD"
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Working on it."}}}'
read steer
echo '{"type":"result","subtype":"success","terminal_reason":"completed","result":"Working on it."}'
while :; do
  echo '{"type":"rate_limit_event"}'
  sleep 0.05
done
"#,
        );
        fixture.wait_for_streamed("Working on it.");
        assert!(fixture.runtime.steer(&fixture.chat, "Do this instead"));
        fixture.wait_for_text("Answered again.");
    }

    /// Two steers with the same text: the first is answered and the second is
    /// never read. The first must keep its row and the second alone is sent again.
    #[cfg(unix)]
    #[test]
    fn a_returned_steer_does_not_take_the_row_of_one_with_the_same_text() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
if [ -e "$HEARD" ]; then
  while read again; do
    echo '{"type":"system","subtype":"init","session_id":"s1"}'
    echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Second answer."}]}}'
    echo '{"type":"result","subtype":"success","terminal_reason":"completed","result":"Second answer."}'
  done
  exit 0
fi
: > "$HEARD"
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Working on it."}}}'
read steer
read same
echo '{"type":"result","is_error":true,"terminal_reason":"aborted_streaming"}'
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"First answer."}]}}'
echo '{"type":"result","subtype":"success","terminal_reason":"completed","result":"First answer."}'
read next
"#,
        );
        fixture.wait_for_streamed("Working on it.");
        assert!(fixture.runtime.steer(&fixture.chat, "Same text"));
        assert!(fixture.runtime.steer(&fixture.chat, "Same text"));
        wait_for("both to be answered", || {
            fixture.store.chat(&fixture.chat).unwrap()["working"] == false
                && fixture
                    .transcript()
                    .iter()
                    .any(|(_, content)| content == "Second answer.")
        });
        assert_eq!(
            fixture.transcript(),
            [
                ("user", "first"),
                ("assistant", "Working on it."),
                ("user", "Same text"),
                ("assistant", "First answer."),
                ("user", "Same text"),
                ("assistant", "Second answer."),
            ]
            .map(|(role, text)| (role.to_owned(), text.to_owned()))
        );
    }

    /// An agent that has stopped reading can't be told anything more. The turn
    /// must end even though something still holds its output open, and the message
    /// must still get an answer.
    #[cfg(unix)]
    #[test]
    fn a_steer_the_agent_cannot_be_sent_ends_its_process_and_is_sent_again() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
if [ -e "$HEARD" ]; then
  while read again; do
    echo '{"type":"system","subtype":"init","session_id":"s1"}'
    echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Answered again."}]}}'
    echo '{"type":"result","subtype":"success","terminal_reason":"completed","result":"Answered again."}'
  done
  exit 0
fi
: > "$HEARD"
read first
exec 0<&-
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Working on it."}}}'
sleep 30
"#,
        );
        fixture.wait_for_streamed("Working on it.");
        assert!(fixture.runtime.steer(&fixture.chat, "Do this instead"));
        fixture.wait_for_text("Answered again.");
        wait_for("the turn to end", || {
            fixture.store.chat(&fixture.chat).unwrap()["working"] == false
        });
        assert_eq!(
            fixture
                .transcript()
                .iter()
                .filter(|(_, content)| content == "Do this instead")
                .count(),
            1
        );
    }

    /// Before the agent has begun, a steer could be mistaken for the turn's own
    /// start, so the message goes back to the queue instead.
    #[cfg(unix)]
    #[test]
    fn a_steer_before_the_agent_has_begun_returns_to_the_queue() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
read first
read release
"#,
        );
        assert!(fixture.runtime.steer(&fixture.chat, "Do this instead"));
        wait_for("the message to return to the queue", || {
            fixture.store.chat(&fixture.chat).unwrap()["queue"] == json!(["Do this instead"])
        });
        assert!(
            !fixture
                .transcript()
                .iter()
                .any(|(_, content)| content == "Do this instead")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_steer_is_refused_with_no_running_turn_or_while_an_answer_is_awaited() {
        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let fixture = SteerFixture::start(
            r#"#!/bin/sh
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Working on it."}]}}'
read release
"#,
        );
        assert!(!fixture.runtime.steer("no-such-chat", "Do this instead"));
        fixture.wait_for_streamed("Working on it.");
        fixture
            .runtime
            .inner
            .interactions
            .lock()
            .unwrap()
            .entry(fixture.chat.clone())
            .or_default()
            .requests
            .insert(
                "request".into(),
                PendingInteraction {
                    id: json!(1),
                    method: "item/tool/requestUserInput".into(),
                    params: json!({}),
                    turn_id: 1,
                    claude: true,
                },
            );
        assert!(!fixture.runtime.steer(&fixture.chat, "Do this instead"));
    }

    /// A chat with a fake Claude behind it, cleaned up when the test ends.
    #[cfg(unix)]
    struct KeptAgent {
        root: std::path::PathBuf,
        store: Arc<StateStore>,
        runtime: TurnRuntime,
        chat: String,
        session: String,
        _environment: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(unix)]
    impl KeptAgent {
        const LONG: Duration = Duration::from_secs(60 * 60);
        const INIT: &str = r#"echo '{"type":"system","subtype":"init","session_id":"s1"}'"#;
        const ANSWER: &str = r#"echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Answer."}]}}'
echo '{"type":"result","subtype":"success","result":"Answer."}'"#;

        /// `script` receives `$ROOT`, a directory the test can read.
        fn new(script: &str) -> Self {
            use std::{env, fs, os::unix::fs::PermissionsExt};

            let environment = crate::AGENT_ENV_LOCK.lock().unwrap();
            let root = env::temp_dir().join(format!("xd-runtime-idle-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join("Workspaces")).unwrap();
            let agent = root.join("claude");
            fs::write(
                &agent,
                format!("#!/bin/sh\nROOT='{}'\n{script}\n", root.display()),
            )
            .unwrap();
            fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();
            // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
            unsafe { env::set_var("XD_CLAUDE_EXECUTABLE", &agent) };
            let store =
                Arc::new(StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap());
            let folder = store.new_folder(&json!({"name": "project"})).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let chat = store
                .new_chat(&json!({"folder": folder, "backend": "claude"}))
                .unwrap();
            let runtime = TurnRuntime::new(
                store.clone(),
                Arc::new(EventBus::default()),
                Arc::new(SecretsStore::new(None)),
            );
            Self {
                root,
                store,
                runtime,
                chat: chat["id"].as_str().unwrap().to_owned(),
                session: chat["session"].as_str().unwrap().to_owned(),
                _environment: environment,
            }
        }

        fn send(&self, text: &str) {
            match self
                .store
                .prepare_send(&json!({"chat": self.chat, "text": text}))
                .unwrap()
            {
                crate::storage::SendDisposition::Start { turn, .. } => {
                    self.runtime.start(turn).unwrap();
                }
                crate::storage::SendDisposition::Queued { .. } => panic!("{text} was queued"),
            }
        }

        fn send_and_finish(&self, text: &str) {
            self.send(text);
            self.wait_for_turn_end();
        }

        fn wait_for_turn_end(&self) {
            wait_for("the turn to end", || {
                self.runtime.live_turn(&self.chat).is_none()
                    && self.store.chat(&self.chat).unwrap()["working"] == false
            });
        }

        fn child(&self) -> Arc<Mutex<Child>> {
            self.runtime
                .inner
                .sessions
                .lock()
                .unwrap()
                .get(&self.chat)
                .expect("a kept process")
                .child
                .clone()
        }

        /// Writes a line to the kept process, for a script that is waiting to
        /// go on but must not be mistaken for answering a turn.
        fn poke(&self) {
            let mut sessions = self.runtime.inner.sessions.lock().unwrap();
            let session = sessions.get_mut(&self.chat).expect("a kept process");
            writeln!(session.stdin, "go").unwrap();
            session.stdin.flush().unwrap();
        }

        fn release_any_quiet(&self) -> usize {
            self.runtime
                .release_quiet_agents(Duration::ZERO, Duration::ZERO)
        }
    }

    #[cfg(unix)]
    impl Drop for KeptAgent {
        fn drop(&mut self) {
            self.runtime.shutdown();
            // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
            unsafe { std::env::remove_var("XD_CLAUDE_EXECUTABLE") };
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_idle_agent_is_stopped_and_the_next_message_resumes_its_conversation() {
        let agent = KeptAgent::new(&format!(
            "echo \"$1 $2\" >> \"$ROOT/started\"\nwhile read turn; do\n{}\n{}\ndone",
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        let runtime = &agent.runtime;

        assert_eq!(
            runtime.release_quiet_agents(KeptAgent::LONG, KeptAgent::LONG),
            0,
            "an agent that has only just gone quiet stays up"
        );
        let child = agent.child();
        assert_eq!(
            runtime.release_quiet_agents(Duration::ZERO, KeptAgent::LONG),
            1
        );
        assert!(child.lock().unwrap().try_wait().unwrap().is_some());
        assert!(
            !runtime
                .inner
                .sessions
                .lock()
                .unwrap()
                .contains_key(&agent.chat)
        );
        assert_eq!(agent.store.chat(&agent.chat).unwrap()["working"], false);

        agent.send_and_finish("second");
        let started = std::fs::read_to_string(agent.root.join("started")).unwrap();
        let started = started.lines().collect::<Vec<_>>();
        assert_eq!(started.len(), 2, "{started:?}");
        assert!(!started[0].contains("--resume"), "{started:?}");
        assert!(started[1].contains("--resume s1"), "{started:?}");
        let answers = contents(&agent.store, &agent.chat)
            .into_iter()
            .filter(|(role, _)| role == "assistant")
            .map(|(_, content)| content)
            .collect::<Vec<_>>();
        assert_eq!(answers, ["Answer.", "Answer."]);
    }

    #[cfg(unix)]
    #[test]
    fn a_settled_chat_is_stopped_sooner_than_an_active_one() {
        let agent = KeptAgent::new(&format!(
            "read turn\n{}\n{}\nread next",
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        let runtime = &agent.runtime;
        assert_eq!(
            runtime.release_quiet_agents(KeptAgent::LONG, Duration::ZERO),
            0
        );
        agent
            .store
            .settle_session(&json!({"session": agent.session}))
            .unwrap();
        assert_eq!(
            runtime.release_quiet_agents(KeptAgent::LONG, Duration::ZERO),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_is_kept_through_its_turn() {
        let agent = KeptAgent::new(&format!("read turn\n{}\nread never", KeptAgent::INIT));
        agent.send("first");
        wait_for("the turn to start", || {
            agent.runtime.live_turn(&agent.chat).is_some()
        });
        assert_eq!(agent.release_any_quiet(), 0);
        assert!(agent.runtime.cancel(&agent.chat));
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_is_kept_while_a_child_process_runs() {
        let agent = KeptAgent::new(&format!(
            "read turn\nsleep 300 >/dev/null 2>&1 &\necho $! > \"$ROOT/shell\"\n{}\n{}\nread next",
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        assert_eq!(agent.release_any_quiet(), 0);

        let shell = std::fs::read_to_string(agent.root.join("shell")).unwrap();
        let shell: libc::pid_t = shell.trim().parse().unwrap();
        // SAFETY: the pid names a child of the fake agent, still unreaped.
        assert_eq!(unsafe { libc::kill(shell, libc::SIGKILL) }, 0);
        wait_for("the idle agent to be stopped", || {
            agent.release_any_quiet() == 1
        });
    }

    /// The shell that started it has gone, so it is nobody's child, but stopping
    /// the agent would kill the whole group.
    #[cfg(unix)]
    #[test]
    fn an_agent_is_kept_while_something_it_left_runs_in_its_group() {
        let agent = KeptAgent::new(&format!(
            "read turn\nsh -c 'sleep 300 >/dev/null 2>&1 & echo $! > \"$0\"' \"$ROOT/left\"\n{}\n{}\nread next",
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        assert_eq!(agent.release_any_quiet(), 0);

        let left = std::fs::read_to_string(agent.root.join("left")).unwrap();
        let left: libc::pid_t = left.trim().parse().unwrap();
        // SAFETY: the pid names a process the fake agent's shell left running.
        assert_eq!(unsafe { libc::kill(left, libc::SIGKILL) }, 0);
        wait_for("the idle agent to be stopped", || {
            agent.release_any_quiet() == 1
        });
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_is_kept_while_a_background_task_runs() {
        let agent = KeptAgent::new(&format!(
            r#"read turn
{}
echo '{{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_shell","description":"Wait","task_type":"local_bash","is_backgrounded":true}}'
{}
read poke
echo '{{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_shell","status":"completed"}}'
read next"#,
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        assert_eq!(agent.release_any_quiet(), 0);

        agent.poke();
        wait_for("the idle agent to be stopped", || {
            agent.release_any_quiet() == 1
        });
    }

    /// An update restart stops every kept process, so a background task that
    /// outlived its turn has to hold the restart off until it finishes.
    #[cfg(unix)]
    #[test]
    fn a_background_task_between_turns_counts_as_work_a_restart_would_stop() {
        let agent = KeptAgent::new(&format!(
            r#"read turn
{}
echo '{{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_helper","description":"Look around","subagent_type":"Explore","is_backgrounded":true}}'
{}
read poke
echo '{{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_helper","status":"completed"}}'
read next"#,
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        assert!(!agent.runtime.has_active_turns());
        assert!(agent.runtime.has_background_tasks());

        agent.poke();
        wait_for("the task to finish", || {
            !agent.runtime.has_background_tasks()
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_background_task_ends_with_its_process() {
        let agent = KeptAgent::new(&format!(
            r#"read turn
{}
echo '{{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_shell","description":"Wait","task_type":"local_bash","is_backgrounded":true}}'
{}
read poke"#,
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        assert!(agent.runtime.has_background_tasks());

        agent.poke();
        wait_for("the process to exit", || {
            !agent.runtime.has_background_tasks()
        });
    }

    /// A chat waits on its background tasks between turns: clients hear the
    /// count each time it changes, and that it is zero once the process exits.
    #[cfg(unix)]
    #[test]
    fn clients_hear_how_many_background_tasks_a_chat_waits_on() {
        let agent = KeptAgent::new(&format!(
            r#"read turn
{}
echo '{{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_helper","description":"Look around","subagent_type":"Explore","is_backgrounded":true}}'
echo '{{"type":"system","subtype":"task_started","task_id":"t2","tool_use_id":"toolu_shell","description":"Wait","task_type":"local_bash","is_backgrounded":true}}'
echo '{{"type":"system","subtype":"task_progress","task_id":"t1","tool_use_id":"toolu_helper","description":"Reading"}}'
{}
read poke
echo '{{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_helper","status":"completed"}}'
read poke"#,
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        let (sender, events) = std::sync::mpsc::sync_channel(256);
        let (connection, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        agent
            .runtime
            .inner
            .events
            .subscribe(sender, connection)
            .unwrap();
        let mut counts = Vec::new();
        let heard = |counts: &mut Vec<u64>| {
            for event in events.try_iter() {
                if event["event"] == "background-tasks" {
                    assert_eq!(event["chat"], agent.chat.as_str());
                    counts.push(event["background_tasks"].as_u64().unwrap());
                }
            }
        };

        agent.send_and_finish("first");
        assert_eq!(agent.runtime.background_tasks(&agent.chat), 2);
        assert_eq!(
            agent.runtime.background_task_counts(),
            HashMap::from([(agent.chat.clone(), 2)])
        );
        heard(&mut counts);
        assert_eq!(counts, [1, 2]);

        agent.poke();
        wait_for("one task to finish", || {
            agent.runtime.background_tasks(&agent.chat) == 1
        });
        agent.poke();
        wait_for("the process to exit", || {
            heard(&mut counts);
            counts.last() == Some(&0)
        });
        assert_eq!(counts, [1, 2, 1, 0]);
        assert_eq!(agent.runtime.background_tasks(&agent.chat), 0);
        assert!(agent.runtime.background_task_counts().is_empty());
    }

    #[test]
    fn tasks_count_from_their_start_until_claude_reports_their_end() {
        let mut tasks = HashSet::new();
        for line in [
            r#"{"type":"system","subtype":"task_started","task_id":"agent","is_backgrounded":true}"#,
            r#"{"type":"system","subtype":"task_started","task_id":"shell","task_type":"local_bash"}"#,
            r#"{"type":"system","subtype":"task_started","task_id":"chore","skip_transcript":true}"#,
            r#"{"type":"system","subtype":"task_progress","task_id":"agent","description":"Reading"}"#,
            r#"{"type":"system","subtype":"task_updated","task_id":"agent","patch":{"is_backgrounded":true}}"#,
            r#"{"type":"system","subtype":"task_updated","task_id":"shell","patch":{"status":"running"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"task_notification"}]}}"#,
        ] {
            track_tasks(line, &mut tasks);
        }
        assert_eq!(
            tasks,
            HashSet::from(["agent".to_owned(), "shell".to_owned()])
        );

        track_tasks(
            r#"{"type":"system","subtype":"task_updated","task_id":"shell","patch":{"status":"killed"}}"#,
            &mut tasks,
        );
        assert_eq!(tasks, HashSet::from(["agent".to_owned()]));
        track_tasks(
            r#"{"type":"system","subtype":"task_notification","task_id":"agent","status":"failed"}"#,
            &mut tasks,
        );
        assert!(tasks.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_settled_chat_waits_longer_for_the_report_on_a_finished_task() {
        let agent = KeptAgent::new(&format!(
            r#"read turn
{}
echo '{{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_shell","description":"Wait","task_type":"local_bash","is_backgrounded":true}}'
{}
read poke
echo '{{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_shell","status":"completed"}}'
read next"#,
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        agent
            .store
            .settle_session(&json!({"session": agent.session}))
            .unwrap();
        agent.poke();
        wait_for("the task to finish", || {
            contents(&agent.store, &agent.chat)
                .iter()
                .filter_map(|(_, content)| crate::subagent::decode(content))
                .any(|record| record["status"] == "completed")
        });

        let runtime = &agent.runtime;
        assert_eq!(
            runtime.release_quiet_agents(KeptAgent::LONG, Duration::ZERO),
            0
        );
        assert_eq!(
            runtime.release_quiet_agents(Duration::ZERO, KeptAgent::LONG),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_task_finishing_as_a_turn_ends_still_waits_for_its_report() {
        let agent = KeptAgent::new(&format!(
            r#"read turn
{}
echo '{{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_shell","description":"Wait","task_type":"local_bash","is_backgrounded":true}}'
{}
echo '{{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_shell","status":"completed"}}'
read next"#,
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        agent
            .store
            .settle_session(&json!({"session": agent.session}))
            .unwrap();
        wait_for("the task to finish", || {
            contents(&agent.store, &agent.chat)
                .iter()
                .filter_map(|(_, content)| crate::subagent::decode(content))
                .any(|record| record["status"] == "completed")
        });
        assert_eq!(
            agent
                .runtime
                .release_quiet_agents(KeptAgent::LONG, Duration::ZERO),
            0
        );
    }

    #[test]
    fn a_report_is_due_from_a_task_finishing_until_a_turn_begins() {
        let finished = r#"{"type":"system","subtype":"task_notification","task_id":"t1"}"#;
        let begun = r#"{"type":"system","subtype":"init","session_id":"s1"}"#;
        let text = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"init"}]}}"#;
        assert!(report_due_after(finished, false));
        assert!(
            report_due_after(text, true),
            "only a turn's start clears it"
        );
        assert!(report_due_after("init", true));
        assert!(!report_due_after(begun, true));
        assert!(!report_due_after(text, false));
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_is_kept_once_it_has_set_a_timer() {
        let agent = KeptAgent::new(&format!(
            r#"read turn
{}
echo '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"c1","name":"ScheduleWakeup","input":{{"delaySeconds":1200}}}}]}}}}'
{}
read next"#,
            KeptAgent::INIT,
            KeptAgent::ANSWER
        ));
        agent.send_and_finish("first");
        assert_eq!(agent.release_any_quiet(), 0);
    }

    #[test]
    fn only_a_call_to_a_wakeup_tool_counts_as_setting_a_timer() {
        for line in [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"CronCreate"}]}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"ScheduleWakeup"}}}"#,
        ] {
            assert!(schedules_a_wakeup(line), "{line}");
        }
        for line in [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ScheduleWakeup"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"grep CronCreate"}}]}}"#,
            r#"{"type":"result","result":"Answer."}"#,
            "ScheduleWakeup",
        ] {
            assert!(!schedules_a_wakeup(line), "{line}");
        }
    }

    /// Claude ends a turn while a background agent still runs, then reports
    /// back on its own once the agent finishes. That report used to sit in
    /// the pipe and come out as the answer to the user's next message.
    #[cfg(unix)]
    #[test]
    fn a_background_report_becomes_its_own_turn_and_the_next_answer_is_not_shifted() {
        use std::{env, fs, os::unix::fs::PermissionsExt};

        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let root = env::temp_dir().join(format!(
            "xd-runtime-background-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("Workspaces")).unwrap();
        let agent = root.join("claude");
        fs::write(
            &agent,
            r#"#!/bin/sh
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_helper","name":"Agent"}}}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"subagent_type\":\"Explore\",\"description\":\"Look around\",\"prompt\":\"Look\",\"run_in_background\":true}"}}}'
echo '{"type":"stream_event","event":{"type":"content_block_stop","index":0}}'
echo '{"type":"system","subtype":"task_started","task_id":"task-1","tool_use_id":"toolu_helper","description":"Look around","subagent_type":"Explore","is_backgrounded":true}'
echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_helper","content":"Async agent launched successfully."}]}}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Started a helper."}]}}'
echo '{"type":"result","subtype":"success","result":"Started a helper."}'
sleep 0.3
echo '{"type":"assistant","parent_tool_use_id":"toolu_helper","message":{"content":[{"type":"tool_use","id":"c1","name":"Bash","input":{"command":"ls"}}]}}'
echo '{"type":"system","subtype":"task_progress","task_id":"task-1","tool_use_id":"toolu_helper","description":"Running ls","usage":{"total_tokens":900,"tool_uses":1,"duration_ms":400}}'
echo '{"type":"system","subtype":"task_notification","task_id":"task-1","tool_use_id":"toolu_helper","status":"completed","summary":"Found two files.","usage":{"total_tokens":1200,"tool_uses":1,"duration_ms":800}}'
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"The helper found two files."}]}}'
echo '{"type":"result","subtype":"success","result":"The helper found two files."}'
read second
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Answer two."}]}}'
echo '{"type":"result","subtype":"success","result":"Answer two."}'
read third
"#,
        )
        .unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { env::set_var("XD_CLAUDE_EXECUTABLE", &agent) };

        let store =
            Arc::new(StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap());
        let folder = store.new_folder(&json!({"name": "project"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let chat = store
            .new_chat(&json!({"folder": folder, "backend": "claude"}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let runtime = TurnRuntime::new(
            store.clone(),
            Arc::new(EventBus::default()),
            Arc::new(SecretsStore::new(None)),
        );
        let send = |text: &str| match store
            .prepare_send(&json!({"chat": chat, "text": text}))
            .unwrap()
        {
            crate::storage::SendDisposition::Start { turn, .. } => runtime.start(turn).unwrap(),
            crate::storage::SendDisposition::Queued { .. } => panic!("{text} was queued"),
        };

        send("first");
        wait_for("the background report", || {
            contents(&store, &chat)
                .iter()
                .any(|(_, content)| content == "The helper found two files.")
        });
        wait_for("the background turn to end", || {
            runtime.live_turn(&chat).is_none() && store.chat(&chat).unwrap()["working"] == false
        });

        send("second");
        wait_for("the second answer", || {
            contents(&store, &chat)
                .iter()
                .any(|(_, content)| content == "Answer two.")
        });
        wait_for("the second turn to end", || {
            runtime.live_turn(&chat).is_none()
        });

        let messages = contents(&store, &chat);
        let texts = messages
            .iter()
            .filter(|(role, _)| role == "user" || role == "assistant")
            .map(|(_, content)| content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            vec![
                "first",
                "Started a helper.",
                "The helper found two files.",
                "second",
                "Answer two."
            ]
        );
        // One card for the helper, finished, with its step and its report.
        let records = messages
            .iter()
            .filter_map(|(_, content)| crate::subagent::decode(content))
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 1, "{messages:?}");
        assert_eq!(records[0]["status"], "completed");
        assert_eq!(records[0]["agent"], "Explore");
        assert_eq!(records[0]["summary"], "Found two files.");
        assert_eq!(records[0]["steps"][0]["text"], "$ ls");
        assert_eq!(records[0]["background"], true);

        let idle_child = runtime
            .inner
            .sessions
            .lock()
            .unwrap()
            .get(&chat)
            .expect("the idle assistant process is retained between turns")
            .child
            .clone();
        runtime.discard_chat(&chat);
        assert!(!runtime.inner.sessions.lock().unwrap().contains_key(&chat));
        assert!(
            idle_child.lock().unwrap().try_wait().unwrap().is_some(),
            "deleting a conversation must stop its retained idle process"
        );
        drop(runtime);
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { env::remove_var("XD_CLAUDE_EXECUTABLE") };
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_turn_stopped_before_claude_recorded_it_is_passed_to_the_next_turn() {
        use std::{env, fs, os::unix::fs::PermissionsExt};

        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let root = env::temp_dir().join(format!("xd-cut-off-turn-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let agent = root.join("claude");
        let log = root.join("calls");
        // The first two runs behave like a forked skill: a task starts, carrying
        // the session id, but `init` never comes. The third answers, and the
        // fourth is an ordinary turn, stopped after its `init`.
        fs::write(
            &agent,
            format!(
                r#"#!/bin/sh
log='{}'
read first
printf 'args:%s\nprompt:%s\n' "$*" "$first" >> "$log"
runs=$(grep -c '^args:' "$log")
if [ "$runs" -le 2 ]; then
  echo '{{"type":"system","subtype":"task_started","task_id":"fork","skip_transcript":true,"session_id":"lost"}}'
  sleep 30
elif [ "$runs" = 3 ]; then
  echo '{{"type":"system","subtype":"init","session_id":"kept"}}'
  until [ -e "$log.go" ]; do sleep 0.05; done
  echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Answered."}}]}}}}'
  echo '{{"type":"result","subtype":"success","result":"Answered."}}'
  read second
else
  echo '{{"type":"system","subtype":"init","session_id":"kept"}}'
  sleep 30
fi
"#,
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { env::set_var("XD_CLAUDE_EXECUTABLE", &agent) };

        let store =
            Arc::new(StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap());
        let folder = store.new_folder(&json!({"name": "project"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let chat = store
            .new_chat(&json!({"folder": folder, "backend": "claude"}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let runtime = TurnRuntime::new(
            store.clone(),
            Arc::new(EventBus::default()),
            Arc::new(SecretsStore::new(None)),
        );
        let send = |text: &str| match store
            .prepare_send(&json!({"chat": chat, "text": text}))
            .unwrap()
        {
            crate::storage::SendDisposition::Start { turn, .. } => runtime.start(turn).unwrap(),
            crate::storage::SendDisposition::Queued { .. } => panic!("{text} was queued"),
        };
        let calls = || fs::read_to_string(&log).unwrap_or_default();
        let notices = || {
            contents(&store, &chat)
                .iter()
                .filter(|(role, content)| role == "error" && content == CUT_OFF_TURN)
                .count()
        };

        let run = |number: usize| {
            calls()
                .split("args:")
                .nth(number)
                .unwrap_or_default()
                .to_owned()
        };
        let stop = |what: &str| {
            wait_for(what, || {
                std::thread::sleep(Duration::from_millis(200));
                runtime.cancel(&chat)
            });
            wait_for("the stop to finish", || {
                runtime.live_turn(&chat).is_none() && store.chat(&chat).unwrap()["working"] == false
            });
        };

        send("/code-review the pull request");
        wait_for("the first fork", || calls().matches("args:").count() == 1);
        stop("the first stop");
        assert_eq!(notices(), 1, "{:?}", contents(&store, &chat));

        // Stopped again with the note in hand: nothing has reached the agent.
        send("try something else");
        wait_for("the second fork", || calls().matches("args:").count() == 2);
        assert!(
            run(2).contains("/code-review the pull request"),
            "{}",
            run(2)
        );
        stop("the second stop");
        assert_eq!(notices(), 2, "{:?}", contents(&store, &chat));

        send("are you ok?");
        // The note is delivered with `init`, not when the turn ends: a host that
        // dies in between must not repeat it.
        let pending_note = || {
            rusqlite::Connection::open(root.join("chats.db"))
                .unwrap()
                .query_row(
                    "SELECT cut_off_turn FROM chats WHERE id = ?",
                    [&chat],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap()
        };
        wait_for("the note to be delivered", || pending_note().is_none());
        assert!(runtime.live_turn(&chat).is_some());
        fs::write(format!("{}.go", log.display()), "").unwrap();
        wait_for("the answer", || {
            contents(&store, &chat)
                .iter()
                .any(|(_, content)| content == "Answered.")
        });
        wait_for("the turn to end", || runtime.live_turn(&chat).is_none());
        let third = run(3);
        assert!(!third.contains("--resume"), "{third}");
        assert!(third.contains("/code-review the pull request"), "{third}");
        assert!(third.contains("try something else"), "{third}");
        assert_eq!(third.matches("<xd-note>").count(), 1, "{third}");
        assert!(third.contains("are you ok?"), "{third}");
        let shown = contents(&store, &chat);
        assert!(
            shown
                .iter()
                .any(|(role, content)| role == "user" && content == "are you ok?"),
            "the user sees their own message, not the note: {shown:?}"
        );

        // The agent has the note now. A stop after `init` has its request in
        // the session already. Dropping the kept process makes this a new run,
        // and it resumes.
        runtime.discard_chat(&chat);
        send("another");
        wait_for("the fourth run", || calls().matches("args:").count() == 4);
        let fourth = run(4);
        assert!(fourth.contains("--resume kept"), "{fourth}");
        assert!(!fourth.contains("<xd-note>"), "{fourth}");
        stop("the last stop");
        assert_eq!(notices(), 2, "{:?}", contents(&store, &chat));

        drop(runtime);
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { env::remove_var("XD_CLAUDE_EXECUTABLE") };
        let _ = fs::remove_dir_all(root);
    }
}
