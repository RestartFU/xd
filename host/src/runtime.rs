use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    /// Lent to the turn being answered, and handed back when it ends. Absent
    /// while a turn holds it.
    stdout: Option<ChildStdout>,
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
struct TurnProgress {
    sequence: AtomicU64,
    segment: Mutex<String>,
}

impl TurnProgress {
    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Relaxed)
    }

    fn set_segment(&self, text: &str) {
        if let Ok(mut segment) = self.segment.lock() {
            segment.clear();
            segment.push_str(text);
        }
    }
}

/// Where a live turn has got to, for a client that just loaded the chat.
pub struct LiveTurn {
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
            }),
        }
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
        let keeps_process = AgentCommand::keeps_its_process(&command_backend);
        // Everything baked into argv when a process starts. A turn that wants
        // any of it different cannot be answered by the process already up.
        let fingerprint = format!(
            "{command_backend}\x01{command_model}\x01{command_effort}\x01{}\x01{}\x01{}\x01{:?}",
            turn.access,
            turn.workdir,
            turn.system_prompt.as_deref().unwrap_or_default(),
            turn.environment,
        );
        let mut command = specification.build();
        let (child, stdout, complaint) = if keeps_process {
            self.session_for(
                &turn.chat_id,
                &fingerprint,
                &turn.prompt,
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
            (Arc::new(Mutex::new(child)), stdout, complaint)
        };
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
                if let Ok(mut process) = child.lock() {
                    let _ = terminate_agent(&mut process);
                }
                return Err("That chat already has a running turn.".into());
            }
            active.insert(
                turn.chat_id.clone(),
                ActiveProcess {
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
                        stdout,
                        complaint,
                        keeps_process,
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
        self.inner.events.publish(json!({
            "event": "turn-started",
            "chat": chat_id,
            "label": label,
            "turn_id": turn_id,
            "turn_sequence": 0,
        }));
        Ok(())
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
        Some(LiveTurn {
            turn_id: process.turn_id,
            sequence: process.progress.sequence(),
            label: process.label.clone(),
            working_for: process.started.elapsed().as_secs(),
            segment: process
                .progress
                .segment
                .lock()
                .map(|segment| segment.clone())
                .unwrap_or_default(),
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
        prompt: &str,
        command: &mut std::process::Command,
        backend: &str,
    ) -> Result<(Arc<Mutex<Child>>, ChildStdout, Arc<Mutex<String>>), String> {
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
            let usable = !dead && session.fingerprint == fingerprint && session.stdout.is_some();
            if usable {
                // Writing the turn is also the test of whether the pipe is
                // still there; a broken one falls through to a fresh process.
                let written = writeln!(session.stdin, "{}", AgentCommand::encode_turn(prompt))
                    .and_then(|()| session.stdin.flush())
                    .is_ok();
                if written {
                    let stdout = session.stdout.take().expect("checked above");
                    return Ok((session.child.clone(), stdout, session.complaint.clone()));
                }
            }
            if let Some(mut gone) = sessions.remove(chat_id)
                && let Ok(mut child) = gone.child.lock()
            {
                let _ = terminate_agent(&mut child);
                gone.stdout.take();
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
        writeln!(stdin, "{}", AgentCommand::encode_turn(prompt))
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("Cannot send the turn to {backend}: {error}"))?;
        let complaint = Arc::new(Mutex::new(String::new()));
        drain_stderr(stderr, complaint.clone());
        let child = Arc::new(Mutex::new(child));
        sessions.insert(
            chat_id.to_owned(),
            AgentSession {
                child: child.clone(),
                stdin,
                stdout: None,
                fingerprint: fingerprint.to_owned(),
                complaint: complaint.clone(),
            },
        );
        Ok((child, stdout, complaint))
    }

    /// Hands a kept process's output back, so the next turn can have it.
    fn return_stdout(&self, chat_id: &str, stdout: ChildStdout) {
        if let Ok(mut sessions) = self.inner.sessions.lock()
            && let Some(session) = sessions.get_mut(chat_id)
        {
            session.stdout = Some(stdout);
        }
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
        stdout: ChildStdout,
        complaint: Arc<Mutex<String>>,
        keeps_process: bool,
    ) {
        let started = Instant::now();
        let mut parser = match AgentParser::new(&turn.backend) {
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

        // Owned back after the loop so a kept process can be lent out again.
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                // End of output. For a process that answers once this is the
                // end of the turn; for a kept one it means it died.
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => {
                    latest_error = Some(format!("Cannot read {} output.", turn.backend));
                    break;
                }
            }
            let line = line.trim_end_matches(['\r', '\n']).to_owned();
            for event in parser.feed(&line) {
                match event {
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
                            progress.set_segment(&streamed_text[..visible_streamed_bytes]);
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
                    AgentEvent::Tool(text) => {
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
                self.return_stdout(&turn.chat_id, reader.into_inner());
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
        let asked = ask::parse(&assistant_text);
        self.finish(
            turn,
            turn_id,
            progress.sequence(),
            success,
            error,
            started.elapsed().as_secs(),
            !had_activity && !was_cancelled,
            asked,
            context_usage,
        );
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

    fn publish_sequenced(
        &self,
        name: &str,
        turn: &TurnSpec,
        turn_id: u64,
        progress: &TurnProgress,
        fields: Value,
    ) {
        let sequence = progress.next_sequence();
        let mut event = fields.as_object().cloned().unwrap_or_default();
        event.insert("event".into(), name.into());
        event.insert("chat".into(), turn.chat_id.clone().into());
        event.insert("turn_id".into(), turn_id.into());
        event.insert("turn_sequence".into(), sequence.into());
        self.inner.events.publish(Value::Object(event));
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
        assert_eq!(progress.next_sequence(), 1);
        assert_eq!(progress.next_sequence(), 2);
        assert_eq!(progress.sequence(), 2);

        // The segment is the whole reply so far, not the last delta: a client
        // that loads mid-turn gets one snapshot and no chance to accumulate.
        progress.set_segment("Half");
        progress.set_segment("Half an ans");
        assert_eq!(*progress.segment.lock().unwrap(), "Half an ans");
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
}
