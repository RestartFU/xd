use std::{
    collections::{HashMap, HashSet},
    env,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    EventBus,
    terminal_activity::TerminalActivityParser,
    terminal_agent::TerminalAgent,
    terminal_query::TerminalQueryResponder,
    terminal_replay::{
        HISTORY_LIMIT, REPLAY_ITEM_LIMIT, RecordOutcome, ReplayFrame, TerminalState,
        pasted_text_bytes,
    },
};

const DEFAULT_COLUMNS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const MAX_COLUMNS: u16 = 500;
const MAX_ROWS: u16 = 200;
const INPUT_LIMIT: usize = 1024 * 1024;
const READ_SIZE: usize = 8_192;

pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    opening: Mutex<()>,
    events: Arc<EventBus>,
    activity: Arc<TerminalActivityState>,
}

struct TerminalActivityState {
    epoch: String,
    revision: AtomicU64,
    gate: Mutex<()>,
}

pub(crate) struct TerminalActivitySnapshot {
    pub(crate) epoch: String,
    pub(crate) revision: u64,
    pub(crate) working_chats: HashSet<String>,
}

struct TerminalSession {
    id: String,
    chat_id: String,
    title: String,
    agent: Option<TerminalAgent>,
    allow_all_permissions: bool,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Option<Box<dyn Child + Send + Sync>>>,
    state: Mutex<TerminalState>,
    activity: Arc<TerminalActivityState>,
}

impl TerminalManager {
    pub fn new(events: Arc<EventBus>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            opening: Mutex::new(()),
            events,
            activity: Arc::new(TerminalActivityState {
                epoch: Uuid::new_v4().to_string(),
                revision: AtomicU64::new(0),
                gate: Mutex::new(()),
            }),
        }
    }

    pub fn list(&self, chat_id: &str) -> Value {
        let sessions = match self.sessions.lock() {
            Ok(sessions) => sessions
                .values()
                .filter(|session| session.chat_id == chat_id)
                .cloned()
                .collect::<Vec<_>>(),
            Err(_) => return error("Terminal state is unavailable."),
        };
        let terminals = sessions
            .iter()
            .filter_map(|session| session.snapshot())
            .collect::<Vec<_>>();
        json!({"ok": true, "terminals": terminals})
    }

    pub fn open(&self, request: &Value, workdir: &Path) -> Result<Value, String> {
        self.open_session(request, workdir, None, &[])
    }

    pub fn open_agent(
        &self,
        request: &Value,
        workdir: &Path,
        agent: TerminalAgent,
        environment: &[(String, String)],
    ) -> Result<Value, String> {
        self.open_session(request, workdir, Some(agent), environment)
    }

    fn open_session(
        &self,
        request: &Value,
        workdir: &Path,
        agent: Option<TerminalAgent>,
        environment: &[(String, String)],
    ) -> Result<Value, String> {
        let chat_id = text(request, "chat", "terminal-open needs a chat id")?;
        let columns = geometry(request, "columns", DEFAULT_COLUMNS, MAX_COLUMNS)?;
        let rows = geometry(request, "rows", DEFAULT_ROWS, MAX_ROWS)?;
        let reuse = request
            .get("reuse")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let allow_all_permissions = agent.is_some()
            && request
                .get("allow_all_permissions")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        // Keep the reusable check and process creation single-flight across
        // clients without reversing the session close path's lock order.
        let _opening = self
            .opening
            .lock()
            .map_err(|_| "Terminal opening state is unavailable.".to_string())?;
        if reuse
            && let Some(existing) = self
                .sessions
                .lock()
                .map_err(|_| "Terminal state is unavailable.".to_string())?
                .values()
                .find(|session| {
                    session.chat_id == chat_id
                        && session.agent == agent
                        && session.allow_all_permissions == allow_all_permissions
                        && !session.is_closing()
                })
        {
            return Ok(json!({"ok": true, "id": existing.id}));
        }
        let queries = TerminalQueryResponder::from_request(request);

        let (session, reader) = TerminalSession::spawn(
            chat_id,
            workdir,
            columns,
            rows,
            agent,
            allow_all_permissions,
            environment,
            self.activity.clone(),
        )?;
        let id = session.id.clone();
        self.sessions
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_string())?
            .insert(id.clone(), session.clone());
        self.events.publish(json!({
            "event": "terminal-opened",
            "chat": chat_id,
            "terminal": id,
            "title": session.title,
            "agent": session.agent.map(TerminalAgent::wire_name),
            "columns": columns,
            "rows": rows,
            "working": false,
            "sequence": 0,
        }));
        start_reader(
            session,
            reader,
            self.sessions.clone(),
            self.events.clone(),
            queries,
        );
        Ok(json!({"ok": true, "id": id}))
    }

    pub fn input(&self, request: &Value) -> Result<Value, String> {
        let session = self.session(text(request, "terminal", "A terminal id is required.")?)?;
        let encoded = text(request, "data", "terminal-input needs data.")?;
        if encoded.len() > INPUT_LIMIT.saturating_mul(4).div_ceil(3) + 4 {
            return Err("Terminal input is too large.".into());
        }
        let data = STANDARD
            .decode(encoded)
            .map_err(|_| "terminal-input needs valid base64 data.".to_string())?;
        if data.len() > INPUT_LIMIT {
            return Err("Terminal input is too large.".into());
        }
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_string())?;
        writer
            .as_mut()
            .ok_or_else(|| "The terminal is closed.".to_string())?
            .write_all(&data)
            .map_err(|error| format!("Cannot write terminal: {error}."))?;
        Ok(json!({"ok": true}))
    }

    pub fn paste_image(&self, request: &Value, path: &Path) -> Result<Value, String> {
        let session = self.session(text(request, "terminal", "A terminal id is required.")?)?;
        let path = path
            .to_str()
            .ok_or_else(|| "The pasted image path is not valid UTF-8.".to_owned())?;
        let bracketed = session
            .state
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_owned())?
            .bracketed_paste();
        let data = pasted_text_bytes(path, bracketed);
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_owned())?;
        writer
            .as_mut()
            .ok_or_else(|| "The terminal is closed.".to_owned())?
            .write_all(&data)
            .map_err(|error| format!("Cannot write terminal: {error}."))?;
        Ok(json!({"ok": true, "path": path}))
    }

    pub fn resize(&self, request: &Value) -> Result<Value, String> {
        let session = self.session(text(request, "terminal", "A terminal id is required.")?)?;
        let columns = geometry(request, "columns", DEFAULT_COLUMNS, MAX_COLUMNS)?;
        let rows = geometry(request, "rows", DEFAULT_ROWS, MAX_ROWS)?;
        let mut state = session
            .state
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_string())?;
        if state.closing {
            return Err("The terminal is closed.".into());
        }
        if state.columns == columns && state.rows == rows {
            return Ok(json!({"ok": true, "changed": false}));
        }
        session
            .master
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_string())?
            .as_ref()
            .ok_or_else(|| "The terminal is closed.".to_string())?
            .resize(pty_size(columns, rows))
            .map_err(|error| format!("Cannot resize terminal: {error}."))?;
        let outcome = state.record_resize_bounded(columns, rows, REPLAY_ITEM_LIMIT);
        if let RecordOutcome::Accepted(sequence) = outcome {
            self.events.publish(json!({
                "event": "terminal-resized",
                "chat": session.chat_id,
                "terminal": session.id,
                "columns": columns,
                "rows": rows,
                "sequence": sequence,
            }));
        }
        match outcome {
            RecordOutcome::Accepted(_) => Ok(json!({"ok": true, "changed": true})),
            RecordOutcome::Unchanged => Ok(json!({"ok": true, "changed": false})),
            RecordOutcome::Closing => Err("The terminal is closed.".into()),
        }
    }

    pub fn kill(&self, request: &Value) -> Result<Value, String> {
        let terminal = text(request, "terminal", "A terminal id is required.")?;
        let session = self
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(terminal).cloned());
        if let Some(session) = session {
            close_session(&session, &self.sessions, &self.events);
        }
        Ok(json!({"ok": true}))
    }

    pub fn kill_chat(&self, chat_id: &str) {
        let sessions = self
            .sessions
            .lock()
            .map(|sessions| {
                sessions
                    .values()
                    .filter(|session| session.chat_id == chat_id)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for session in sessions {
            close_session(&session, &self.sessions, &self.events);
        }
    }

    pub fn has_agent_session(&self, chat_id: &str) -> bool {
        self.sessions
            .lock()
            .map(|sessions| {
                sessions.values().any(|session| {
                    session.chat_id == chat_id && session.agent.is_some() && !session.is_closing()
                })
            })
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub fn working_chats(&self) -> HashSet<String> {
        self.activity_snapshot().working_chats
    }

    pub(crate) fn activity_snapshot(&self) -> TerminalActivitySnapshot {
        let _activity = self
            .activity
            .gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            let before = self.activity.revision.load(Ordering::Acquire);
            let working_chats = working_chats(&self.sessions);
            let after = self.activity.revision.load(Ordering::Acquire);
            if before == after {
                return TerminalActivitySnapshot {
                    epoch: self.activity.epoch.clone(),
                    revision: after,
                    working_chats,
                };
            }
        }
    }

    fn session(&self, id: &str) -> Result<Arc<TerminalSession>, String> {
        self.sessions
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_string())?
            .get(id)
            .cloned()
            .ok_or_else(|| "No such terminal.".to_string())
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        let sessions = self
            .sessions
            .lock()
            .map(|sessions| sessions.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for session in sessions {
            close_session(&session, &self.sessions, &self.events);
        }
    }
}

impl TerminalSession {
    fn spawn(
        chat_id: &str,
        workdir: &Path,
        columns: u16,
        rows: u16,
        agent: Option<TerminalAgent>,
        allow_all_permissions: bool,
        environment: &[(String, String)],
        activity: Arc<TerminalActivityState>,
    ) -> Result<(Arc<Self>, Box<dyn Read + Send>), String> {
        if !workdir.is_dir() {
            return Err(format!("{} is not a directory.", workdir.display()));
        }
        let pair = native_pty_system()
            .openpty(pty_size(columns, rows))
            .map_err(|error| format!("Cannot open a terminal: {error}."))?;
        let mut command = agent
            .map(|agent| CommandBuilder::new(agent.executable()))
            .unwrap_or_else(terminal_command);
        command.cwd(workdir.as_os_str());
        configure_command(&mut command, agent, allow_all_permissions, environment);
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("Cannot start the terminal shell: {error}."))?;
        let reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                return Err(format!("Cannot prepare terminal output: {error}."));
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                let _ = child.kill();
                return Err(format!("Cannot prepare terminal input: {error}."));
            }
        };
        drop(pair.slave);

        let title = agent
            .map(TerminalAgent::title)
            .unwrap_or("Terminal")
            .to_owned();
        let session = Arc::new(Self {
            id: Uuid::new_v4().to_string(),
            chat_id: chat_id.to_owned(),
            title,
            agent,
            allow_all_permissions,
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(writer)),
            child: Mutex::new(Some(child)),
            activity,
            state: Mutex::new(TerminalState::new(columns, rows)),
        });
        Ok((session, reader))
    }

    fn snapshot(&self) -> Option<Value> {
        let mut state = self.state.lock().ok()?;
        if state.closing {
            return None;
        }
        state.compact_for_transfer();
        let replay = state
            .replay
            .iter()
            .map(|frame| match frame {
                ReplayFrame::Output(data) => json!({"data": STANDARD.encode(data)}),
                ReplayFrame::Resize { columns, rows } => {
                    json!({"columns": columns, "rows": rows})
                }
                ReplayFrame::Checkpoint { exact, fallback } => json!({
                    "checkpoint": STANDARD.encode(exact),
                    "data": STANDARD.encode(fallback),
                }),
            })
            .collect::<Vec<_>>();
        Some(json!({
            "id": self.id,
            "title": self.title,
            "agent": self.agent.map(TerminalAgent::wire_name),
            "columns": state.columns,
            "rows": state.rows,
            "working": state.working,
            "sequence": state.sequence,
            "replay": replay,
        }))
    }

    fn is_closing(&self) -> bool {
        self.state.lock().map(|state| state.closing).unwrap_or(true)
    }

    fn contributes_working(&self) -> bool {
        self.agent.is_some()
            && self
                .state
                .lock()
                .map(|state| state.working && !state.closing)
                .unwrap_or(false)
    }

    fn set_working_state(&self, working: bool) -> bool {
        self.state
            .lock()
            .map(|mut state| {
                if state.closing || state.working == working {
                    false
                } else {
                    state.working = working;
                    true
                }
            })
            .unwrap_or(false)
    }

    fn begin_close_state(&self) -> bool {
        self.state
            .lock()
            .map(|mut state| {
                if !state.closing {
                    state.closing = true;
                    state.working = false;
                    state.sequence = state.sequence.saturating_add(1);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    fn close_resources(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            writer.take();
        }
        if let Ok(mut master) = self.master.lock() {
            master.take();
        }
        let child = self.child.lock().ok().and_then(|mut child| child.take());
        if let Some(mut child) = child {
            thread::spawn(move || {
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

fn configure_command(
    command: &mut CommandBuilder,
    agent: Option<TerminalAgent>,
    allow_all_permissions: bool,
    environment: &[(String, String)],
) {
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    for (name, value) in environment {
        command.env(name, value);
    }
    match agent {
        Some(TerminalAgent::Codex) => {
            command.arg("--no-alt-screen");
            command.arg("-c");
            command.arg("tui.terminal_title=[\"run-state\"]");
            command.arg("-c");
            command.arg("tui.terminal_resize_reflow_max_rows=5000");
            if allow_all_permissions {
                command.arg("--dangerously-bypass-approvals-and-sandbox");
            }
        }
        Some(TerminalAgent::Claude) => {
            for name in [
                "WT_SESSION",
                "TMUX",
                "TMUX_PANE",
                "STY",
                "ZELLIJ",
                "ZELLIJ_SESSION_NAME",
                "TERM_PROGRAM",
                "TERM_PROGRAM_VERSION",
            ] {
                command.env_remove(name);
            }
            command.env("ConEmuANSI", "ON");
            if allow_all_permissions {
                command.arg("--dangerously-skip-permissions");
            }
        }
        None => {}
    }
}

fn terminal_command() -> CommandBuilder {
    if let Some(shell) = env::var_os("XD_TERMINAL_SHELL").filter(|shell| !shell.is_empty()) {
        return CommandBuilder::new(shell);
    }
    if let Some(program_files) = env::var_os("ProgramFiles") {
        let pwsh = PathBuf::from(program_files).join("PowerShell/7/pwsh.exe");
        if pwsh.is_file() {
            let mut command = CommandBuilder::new(pwsh);
            command.arg("-NoLogo");
            return command;
        }
    }
    if let Some(system_root) = env::var_os("SystemRoot") {
        let powershell =
            PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        if powershell.is_file() {
            let mut command = CommandBuilder::new(powershell);
            command.arg("-NoLogo");
            return command;
        }
    }
    CommandBuilder::new_default_prog()
}

fn pty_size(columns: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols: columns,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn working_chats(sessions: &Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>) -> HashSet<String> {
    sessions
        .lock()
        .map(|sessions| {
            sessions
                .values()
                .filter(|session| session.contributes_working())
                .map(|session| session.chat_id.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn chat_terminal_working(
    sessions: &Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    chat_id: &str,
) -> bool {
    sessions
        .lock()
        .map(|sessions| {
            sessions
                .values()
                .any(|session| session.chat_id == chat_id && session.contributes_working())
        })
        .unwrap_or(false)
}

fn publish_activity_locked(
    events: &EventBus,
    sessions: &Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    session: &TerminalSession,
    working: bool,
    revision: u64,
) {
    let terminal_working = chat_terminal_working(sessions, &session.chat_id);
    events.publish(json!({
        "event": "terminal-activity",
        "chat": session.chat_id,
        "terminal": session.id,
        "working": working,
        "terminal_working": terminal_working,
        "terminal_activity_epoch": session.activity.epoch,
        "terminal_activity_revision": revision,
    }));
}

fn transition_activity(
    events: &EventBus,
    sessions: &Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    session: &TerminalSession,
    working: bool,
) {
    let _activity = session
        .activity
        .gate
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !session.set_working_state(working) {
        return;
    }
    let revision = session.activity.revision.fetch_add(1, Ordering::AcqRel) + 1;
    publish_activity_locked(events, sessions, session, working, revision);
}

fn close_session(
    session: &TerminalSession,
    sessions: &Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    events: &EventBus,
) {
    let closed = {
        let _activity = session
            .activity
            .gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !session.begin_close_state() {
            false
        } else {
            if let Ok(mut sessions) = sessions.lock() {
                sessions.remove(&session.id);
            }
            let revision = session.activity.revision.fetch_add(1, Ordering::AcqRel) + 1;
            publish_activity_locked(events, sessions, session, false, revision);
            true
        }
    };
    if closed {
        session.close_resources();
    }
}

fn start_reader(
    session: Arc<TerminalSession>,
    mut reader: Box<dyn Read + Send>,
    sessions: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    events: Arc<EventBus>,
    mut queries: TerminalQueryResponder,
) {
    thread::Builder::new()
        .name(format!("xd-terminal-{}", session.id))
        .spawn(move || {
            let mut buffer = [0_u8; READ_SIZE];
            let mut activity = TerminalActivityParser::default();
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                let data = buffer[..count].to_vec();
                let activity_updates = activity.feed(&data);
                let replies = queries.feed(&data);
                if !replies.is_empty()
                    && let Ok(mut writer) = session.writer.lock()
                    && let Some(writer) = writer.as_mut()
                {
                    let _ = writer.write_all(&replies);
                    let _ = writer.flush();
                }
                let outcome = session
                    .state
                    .lock()
                    .map_or(RecordOutcome::Closing, |mut state| {
                        let outcome = state.record_output_bounded(
                            data.clone(),
                            HISTORY_LIMIT,
                            REPLAY_ITEM_LIMIT,
                        );
                        if let RecordOutcome::Accepted(sequence) = outcome {
                            events.publish(json!({
                                "event": "terminal-output",
                                "chat": session.chat_id,
                                "terminal": session.id,
                                "data": STANDARD.encode(&data),
                                "sequence": sequence,
                            }));
                        }
                        outcome
                    });
                match outcome {
                    RecordOutcome::Accepted(_) => {
                        for working in activity_updates {
                            transition_activity(&events, &sessions, &session, working);
                        }
                    }
                    RecordOutcome::Unchanged => {
                        unreachable!("terminal output always changes state")
                    }
                    RecordOutcome::Closing => break,
                }
            }
            close_session(&session, &sessions, &events);
            let sequence = session
                .state
                .lock()
                .map(|state| state.sequence)
                .unwrap_or_default();
            events.publish(json!({
                "event": "terminal-closed",
                "chat": session.chat_id,
                "terminal": session.id,
                "sequence": sequence,
            }));
        })
        .expect("terminal reader thread should start");
}

fn text<'a>(request: &'a Value, key: &str, message: &str) -> Result<&'a str, String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| message.to_owned())
}

fn geometry(request: &Value, key: &str, default: u16, maximum: u16) -> Result<u16, String> {
    let Some(value) = request.get(key) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .filter(|value| (1..=u64::from(maximum)).contains(value))
        .ok_or_else(|| format!("Terminal {key} must be between 1 and {maximum}."))?;
    Ok(value as u16)
}

fn error(message: impl Into<String>) -> Value {
    json!({"ok": false, "error": message.into()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn direct_agent_commands_apply_activity_configuration_after_user_environment() {
        let topology = [
            ("WT_SESSION".to_owned(), "xd-wt".to_owned()),
            ("TMUX".to_owned(), "xd-tmux".to_owned()),
            ("TMUX_PANE".to_owned(), "xd-pane".to_owned()),
            ("STY".to_owned(), "xd-screen".to_owned()),
            ("ZELLIJ".to_owned(), "xd-zellij".to_owned()),
            ("TERM_PROGRAM".to_owned(), "xd-terminal".to_owned()),
            ("ConEmuANSI".to_owned(), "OFF".to_owned()),
        ];
        let mut codex = CommandBuilder::new("codex.exe");
        configure_command(&mut codex, Some(TerminalAgent::Codex), true, &topology);
        let arguments = codex
            .get_argv()
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                "codex.exe",
                "--no-alt-screen",
                "-c",
                "tui.terminal_title=[\"run-state\"]",
                "-c",
                "tui.terminal_resize_reflow_max_rows=5000",
                "--dangerously-bypass-approvals-and-sandbox"
            ]
        );

        let mut claude = CommandBuilder::new("claude.exe");
        configure_command(&mut claude, Some(TerminalAgent::Claude), true, &topology);
        assert!(
            claude
                .get_argv()
                .iter()
                .any(|argument| argument == "--dangerously-skip-permissions")
        );
        for name in [
            "WT_SESSION",
            "TMUX",
            "TMUX_PANE",
            "STY",
            "ZELLIJ",
            "TERM_PROGRAM",
        ] {
            assert!(claude.get_env(name).is_none(), "{name} was not removed");
        }
        assert_eq!(claude.get_env("ConEmuANSI").unwrap(), "ON");
    }

    #[test]
    fn conpty_output_is_replayed_and_input_is_bounded() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let opened = manager
            .open(
                &json!({"chat": "chat-1", "columns": 92, "rows": 31}),
                &env::temp_dir(),
            )
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();
        manager
            .input(&json!({
                "terminal": terminal,
                "data": STANDARD.encode(b"echo xd-conpty-ready\r\n"),
            }))
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let listed = manager.list("chat-1");
            let output = listed["terminals"]
                .as_array()
                .and_then(|terminals| terminals.first())
                .and_then(|terminal| terminal["replay"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|frame| frame["data"].as_str())
                .filter_map(|data| STANDARD.decode(data).ok())
                .flatten()
                .collect::<Vec<_>>();
            if String::from_utf8_lossy(&output).contains("xd-conpty-ready") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "terminal output was not replayed"
            );
            thread::sleep(Duration::from_millis(10));
        }
        manager.kill(&json!({"terminal": terminal})).unwrap();
        assert!(
            manager
                .input(&json!({"terminal": terminal, "data": STANDARD.encode(b"x")}))
                .is_err()
        );
    }

    #[test]
    fn geometry_and_base64_are_validated_before_touching_a_session() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        assert!(
            manager
                .open(&json!({"chat": "chat-1", "columns": 0}), &env::temp_dir(),)
                .unwrap_err()
                .contains("columns")
        );
        assert!(
            manager
                .open(
                    &json!({"chat": "chat-1", "columns": MAX_COLUMNS + 1}),
                    &env::temp_dir(),
                )
                .unwrap_err()
                .contains("columns")
        );
        assert!(
            manager
                .open(
                    &json!({"chat": "chat-1", "rows": MAX_ROWS + 1}),
                    &env::temp_dir(),
                )
                .unwrap_err()
                .contains("rows")
        );
        assert!(
            manager
                .input(&json!({"terminal": "missing", "data": "not base64"}))
                .unwrap_err()
                .contains("No such terminal")
        );
    }
}
