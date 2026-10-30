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
    secrets::SecretsStore,
    storage::TurnSpec,
    subagent::{SubagentPatch, SubagentRun, SubagentStatus},
};

/// How often a streamed reply is parked on the chat so an interrupted turn
/// keeps most of its text without turning every token into a database write.
const PARK_STREAMED_REPLY_EVERY: Duration = Duration::from_millis(500);

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
}

/// Where a turn reads its agent's output from.
enum LineSource {
    /// A process that answers one turn and exits.
    Pipe(BufReader<ChildStdout>),
    /// A kept process, through its router.
    Feed {
        lines: mpsc::Receiver<Feed>,
        background: bool,
    },
}

enum NextLine {
    Line,
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
            Self::Feed { lines, background } => {
                let received = if *background {
                    match lines.recv_timeout(BACKGROUND_QUIET_LIMIT) {
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
                    Feed::End => NextLine::End,
                }
            }
        }
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

#[derive(Default)]
struct PendingInteractions {
    revision: u64,
    requests: HashMap<String, PendingInteraction>,
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
/// is still running when the next one arrives.
struct AgentSession {
    child: Arc<Mutex<Child>>,
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

    pub(crate) fn shutdown(&self) {
        if let Ok(active) = self.inner.active.lock() {
            for process in active.values() {
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
            if run.provider != "Claude" {
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
            "{command_backend}\x01{command_model}\x01{command_effort}\x01{}\x01{}\x01{}\x01{:?}",
            turn.access,
            turn.workdir,
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
                if starting {
                    if let Ok(mut child) = child.lock() {
                        let _ = terminate_agent(&mut child);
                    }
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
                    let Ok(mut router) = router.lock() else {
                        return;
                    };
                    router.backlog.push_back(text);
                    runtime.pump(&chat_id, &mut router);
                }
                if let Ok(mut router) = router.lock()
                    && let Some(sink) = router.sink.take()
                {
                    let _ = sink.send(Feed::End);
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
    fn detach_feed(&self, chat_id: &str, source: LineSource) {
        let LineSource::Feed { lines, .. } = source else {
            return;
        };
        let Some(router) = self.router(chat_id) else {
            return;
        };
        let Ok(mut router) = router.lock() else {
            return;
        };
        router.sink = None;
        let mut unread = Vec::new();
        while let Ok(Feed::Line(line)) = lines.try_recv() {
            unread.push(line);
        }
        for line in unread.into_iter().rev() {
            router.backlog.push_front(line);
        }
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
        }

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Cannot start {backend}: {error}"))?;
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
    }

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
        let mut had_activity = false;
        let mut streamed_text = String::new();
        let mut assistant_text = String::new();
        let mut visible_streamed_bytes = 0;
        let mut published_text = false;
        let mut context_usage = None;
        let mut parked = Instant::now();

        let mut line = String::new();
        // Stores the reply streamed so far as its own message, which a tool
        // card that follows it must come after.
        macro_rules! close_segment {
            () => {
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
            match source.next(&mut line) {
                NextLine::Line => {}
                // End of output. For a process that answers once this is the
                // end of the turn; for a kept one it means it died.
                NextLine::End => break,
                NextLine::Quiet => {
                    completed = true;
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
                        if let Ok(mut interactions) = self.inner.interactions.lock() {
                            if let Some(pending) = interactions.get_mut(&turn.chat_id) {
                                pending.requests.remove(&request_id);
                                pending.revision =
                                    self.inner.next_interaction.fetch_add(1, Ordering::Relaxed);
                                self.inner.events.publish(json!({"event":"interaction-resolved", "chat":turn.chat_id, "request_id":request_id,"interaction_epoch":self.inner.interaction_epoch,"interaction_revision":pending.revision}));
                            }
                        }
                    }
                    AgentEvent::Session(session) => {
                        if let Err(error) = self.inner.store.set_session(
                            &turn.chat_id,
                            &turn.session_backend,
                            &session,
                        ) {
                            latest_error = Some(error.to_string());
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
                        streamed_text.push_str(&text);
                        assistant_text.push_str(&text);
                        let visible = ask::visible_bytes(&streamed_text);
                        if visible > visible_streamed_bytes {
                            self.publish_sequenced(
                                "text",
                                &turn,
                                turn_id,
                                &progress,
                                json!({"text": &streamed_text[visible_streamed_bytes..visible]}),
                            );
                            visible_streamed_bytes = visible;
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
                    AgentEvent::Error(error) => latest_error = Some(error),
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

        let was_cancelled = cancelled.load(Ordering::Acquire);
        let success = if keeps_process {
            // Nothing to wait for: it is still up, ready for the next turn.
            // Whether the turn worked is what it said, not how it exited.
            let alive = child
                .lock()
                .ok()
                .and_then(|mut child| child.try_wait().ok().flatten())
                .is_none();
            if alive && completed && !was_cancelled {
                self.detach_feed(&turn.chat_id, source);
            } else {
                self.drop_session(&turn.chat_id);
            }
            was_cancelled || completed
        } else {
            let status = child.lock().ok().and_then(|mut child| child.wait().ok());
            was_cancelled || (completed && status.is_some_and(|status| status.success()))
        };
        let stderr = complaint
            .lock()
            .map(|said| said.clone())
            .unwrap_or_default();
        let error = if success {
            None
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
                    item["message_id"] = fields["message_id"].clone();
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
            runtime.live_turn(&chat).is_none()
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

        drop(runtime);
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { env::remove_var("XD_CLAUDE_EXECUTABLE") };
        let _ = fs::remove_dir_all(root);
    }
}
