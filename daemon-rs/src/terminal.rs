use std::{
    collections::{HashMap, VecDeque},
    env,
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::raw::{c_char, c_int, c_void},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{EventBus, terminal_agent::TerminalAgent, terminal_query::TerminalQueryResponder};

const DEFAULT_COLUMNS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const MAX_GEOMETRY: u16 = 1_000;
const HISTORY_LIMIT: usize = 16 * 1024 * 1024;
const REPLAY_ITEM_LIMIT: usize = 65_536;
const INPUT_LIMIT: usize = 1024 * 1024;
const READ_SIZE: usize = 8_192;
const SIGHUP: c_int = 1;
const SIGKILL: c_int = 9;
const WNOHANG: c_int = 1;

#[repr(C)]
struct WinSize {
    rows: u16,
    columns: u16,
    x_pixels: u16,
    y_pixels: u16,
}

#[link(name = "util")]
unsafe extern "C" {
    fn forkpty(
        master: *mut c_int,
        name: *mut c_char,
        termios: *const c_void,
        size: *const WinSize,
    ) -> c_int;
    fn ioctl(fd: c_int, request: usize, ...) -> c_int;
    fn chdir(path: *const c_char) -> c_int;
    fn setenv(name: *const c_char, value: *const c_char, overwrite: c_int) -> c_int;
    fn execlp(file: *const c_char, argument: *const c_char, ...) -> c_int;
    fn _exit(status: c_int) -> !;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
}

pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    events: Arc<EventBus>,
}

struct TerminalSession {
    id: String,
    chat_id: String,
    title: String,
    agent: Option<TerminalAgent>,
    pid: c_int,
    writer: Mutex<Option<File>>,
    state: Mutex<TerminalState>,
}

struct TerminalState {
    columns: u16,
    rows: u16,
    replay: VecDeque<ReplayFrame>,
    replay_bytes: usize,
    sequence: u64,
    closing: bool,
}

enum ReplayFrame {
    Output(Vec<u8>),
    Resize { columns: u16, rows: u16 },
}

enum RecordOutcome {
    Accepted(u64),
    Unchanged,
    CapacityReached,
    Closing,
}

impl TerminalState {
    fn record_output_bounded(
        &mut self,
        data: Vec<u8>,
        byte_limit: usize,
        item_limit: usize,
    ) -> RecordOutcome {
        if self.closing {
            return RecordOutcome::Closing;
        }
        if self.replay.len() >= item_limit
            || data.len() > byte_limit.saturating_sub(self.replay_bytes)
        {
            return RecordOutcome::CapacityReached;
        }
        self.replay_bytes = self.replay_bytes.saturating_add(data.len());
        self.replay.push_back(ReplayFrame::Output(data));
        self.sequence = self.sequence.saturating_add(1);
        RecordOutcome::Accepted(self.sequence)
    }

    fn record_resize_bounded(
        &mut self,
        columns: u16,
        rows: u16,
        item_limit: usize,
    ) -> RecordOutcome {
        if self.closing {
            return RecordOutcome::Closing;
        }
        if self.columns == columns && self.rows == rows {
            return RecordOutcome::Unchanged;
        }
        if self.replay.len() >= item_limit {
            return RecordOutcome::CapacityReached;
        }
        self.columns = columns;
        self.rows = rows;
        self.replay.push_back(ReplayFrame::Resize { columns, rows });
        self.sequence = self.sequence.saturating_add(1);
        RecordOutcome::Accepted(self.sequence)
    }
}

impl TerminalManager {
    pub fn new(events: Arc<EventBus>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            events,
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
        let columns = geometry(request, "columns", DEFAULT_COLUMNS)?;
        let rows = geometry(request, "rows", DEFAULT_ROWS)?;
        let reuse = request
            .get("reuse")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if reuse
            && let Some(existing) = self
                .sessions
                .lock()
                .map_err(|_| "Terminal state is unavailable.".to_string())?
                .values()
                .find(|session| {
                    session.chat_id == chat_id && session.agent == agent && !session.is_closing()
                })
        {
            return Ok(json!({"ok": true, "id": existing.id}));
        }

        let (session, reader) =
            TerminalSession::spawn(chat_id, workdir, columns, rows, agent, environment)?;
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
            "sequence": 0,
        }));
        start_reader(session, reader, self.sessions.clone(), self.events.clone());
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

    pub fn resize(&self, request: &Value) -> Result<Value, String> {
        let session = self.session(text(request, "terminal", "A terminal id is required.")?)?;
        let columns = geometry(request, "columns", DEFAULT_COLUMNS)?;
        let rows = geometry(request, "rows", DEFAULT_ROWS)?;
        {
            let state = session
                .state
                .lock()
                .map_err(|_| "Terminal state is unavailable.".to_string())?;
            if state.closing {
                return Err("The terminal is closed.".into());
            }
            if state.columns == columns && state.rows == rows {
                return Ok(json!({"ok": true, "changed": false}));
            }
        }
        let writer = session
            .writer
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_string())?;
        let file = writer
            .as_ref()
            .ok_or_else(|| "The terminal is closed.".to_string())?;
        let size = WinSize {
            rows,
            columns,
            x_pixels: 0,
            y_pixels: 0,
        };
        // SAFETY: file owns a live PTY master and size points to initialized memory.
        if unsafe { ioctl(file.as_raw_fd(), libc::TIOCSWINSZ as usize, &size) } != 0 {
            return Err(format!(
                "Cannot resize terminal: {}.",
                std::io::Error::last_os_error()
            ));
        }
        drop(writer);
        let outcome = {
            let mut state = session
                .state
                .lock()
                .map_err(|_| "Terminal state is unavailable.".to_string())?;
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
            outcome
        };
        match outcome {
            RecordOutcome::Accepted(_) => Ok(json!({"ok": true, "changed": true})),
            RecordOutcome::Unchanged => Ok(json!({"ok": true, "changed": false})),
            RecordOutcome::Closing => Err("The terminal is closed.".into()),
            RecordOutcome::CapacityReached => {
                session.begin_close();
                Err("The terminal replay limit was reached, so it was closed safely.".into())
            }
        }
    }

    pub fn kill(&self, request: &Value) -> Result<Value, String> {
        let terminal = text(request, "terminal", "A terminal id is required.")?;
        if let Ok(sessions) = self.sessions.lock()
            && let Some(session) = sessions.get(terminal)
        {
            session.begin_close();
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
            session.begin_close();
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
            session.begin_close();
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
        environment: &[(String, String)],
    ) -> Result<(Arc<Self>, File), String> {
        if !workdir.is_dir() {
            return Err(format!("{} is not a directory.", workdir.display()));
        }
        let executable = agent.map(TerminalAgent::executable).unwrap_or_else(|| {
            env::var_os("SHELL")
                .map(PathBuf::from)
                .filter(|shell| shell.is_file())
                .unwrap_or_else(|| PathBuf::from("/bin/sh"))
        });
        if let Some(agent) = agent
            && !executable_available(&executable)
        {
            return Err(format!(
                "{} is not installed or is unavailable on the daemon machine.",
                agent.title()
            ));
        }
        let executable = CString::new(executable.as_os_str().as_encoded_bytes())
            .map_err(|_| "The terminal executable path is invalid.".to_string())?;
        let title = agent
            .map(TerminalAgent::title)
            .or_else(|| workdir.file_name().and_then(|name| name.to_str()))
            .unwrap_or("Terminal")
            .to_owned();
        let workdir = CString::new(workdir.as_os_str().as_encoded_bytes())
            .map_err(|_| "The terminal working directory is invalid.".to_string())?;
        let term = c"TERM";
        let term_value = c"xterm-256color";
        let colorterm = c"COLORTERM";
        let colorterm_value = c"truecolor";
        let environment = environment
            .iter()
            .map(|(name, value)| {
                Ok((
                    CString::new(name.as_bytes())
                        .map_err(|_| "A terminal environment name is invalid.".to_string())?,
                    CString::new(value.as_bytes())
                        .map_err(|_| "A terminal environment value is invalid.".to_string())?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let size = WinSize {
            rows,
            columns,
            x_pixels: 0,
            y_pixels: 0,
        };
        let mut master = -1;
        // SAFETY: all pointers are valid for the duration of forkpty. The child
        // performs only libc calls before exec and exits immediately on failure.
        let pid = unsafe { forkpty(&mut master, std::ptr::null_mut(), std::ptr::null(), &size) };
        if pid < 0 {
            return Err(format!(
                "Cannot open a terminal: {}.",
                std::io::Error::last_os_error()
            ));
        }
        if pid == 0 {
            // SAFETY: this is the post-fork child; arguments are NUL-terminated.
            unsafe {
                if chdir(workdir.as_ptr()) != 0 {
                    _exit(126);
                }
                setenv(term.as_ptr(), term_value.as_ptr(), 1);
                setenv(colorterm.as_ptr(), colorterm_value.as_ptr(), 1);
                for (name, value) in &environment {
                    setenv(name.as_ptr(), value.as_ptr(), 1);
                }
                execlp(
                    executable.as_ptr(),
                    executable.as_ptr(),
                    std::ptr::null::<c_char>(),
                );
                _exit(127);
            }
        }
        // SAFETY: forkpty returned exclusive ownership of this descriptor.
        let writer = unsafe { File::from_raw_fd(master) };
        let reader = writer
            .try_clone()
            .map_err(|error| format!("Cannot prepare terminal output: {error}."))?;
        let session = Arc::new(Self {
            id: Uuid::new_v4().to_string(),
            chat_id: chat_id.to_owned(),
            title,
            agent,
            pid,
            writer: Mutex::new(Some(writer)),
            state: Mutex::new(TerminalState {
                columns,
                rows,
                replay: VecDeque::from([ReplayFrame::Resize { columns, rows }]),
                replay_bytes: 0,
                sequence: 0,
                closing: false,
            }),
        });
        Ok((session, reader))
    }

    fn snapshot(&self) -> Option<Value> {
        let state = self.state.lock().ok()?;
        if state.closing {
            return None;
        }
        let replay = state
            .replay
            .iter()
            .map(|frame| match frame {
                ReplayFrame::Output(data) => json!({"data": STANDARD.encode(data)}),
                ReplayFrame::Resize { columns, rows } => {
                    json!({"columns": columns, "rows": rows})
                }
            })
            .collect::<Vec<_>>();
        Some(json!({
            "id": self.id,
            "title": self.title,
            "agent": self.agent.map(TerminalAgent::wire_name),
            "columns": state.columns,
            "rows": state.rows,
            "sequence": state.sequence,
            "replay": replay,
        }))
    }

    fn is_closing(&self) -> bool {
        self.state.lock().map(|state| state.closing).unwrap_or(true)
    }

    fn begin_close(&self) {
        let should_close = self
            .state
            .lock()
            .map(|mut state| {
                if !state.closing {
                    state.closing = true;
                    state.sequence = state.sequence.saturating_add(1);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if !should_close {
            return;
        }
        if let Ok(mut writer) = self.writer.lock() {
            writer.take();
        }
        let pid = self.pid;
        thread::spawn(move || {
            // SAFETY: pid is the child/session leader returned by forkpty.
            unsafe {
                kill(-pid, SIGHUP);
                kill(pid, SIGHUP);
            }
            for _ in 0..20 {
                // SAFETY: status is intentionally ignored and WNOHANG never blocks.
                let result = unsafe { waitpid(pid, std::ptr::null_mut(), WNOHANG) };
                if result == pid || result == -1 {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            // SAFETY: the child has not been reaped, so its pid still identifies it.
            unsafe {
                kill(-pid, SIGKILL);
                kill(pid, SIGKILL);
                waitpid(pid, std::ptr::null_mut(), 0);
            }
        });
    }
}

fn executable_available(executable: &Path) -> bool {
    let is_executable_file = |path: &Path| {
        path.metadata()
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if executable.is_absolute() || executable.components().count() > 1 {
        return is_executable_file(executable);
    }
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .any(|directory| is_executable_file(&directory.join(executable)))
}

fn start_reader(
    session: Arc<TerminalSession>,
    mut reader: File,
    sessions: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    events: Arc<EventBus>,
) {
    thread::Builder::new()
        .name(format!("xd-terminal-{}", session.id))
        .spawn(move || {
            let mut buffer = [0_u8; READ_SIZE];
            let mut queries = TerminalQueryResponder::new();
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                let data = buffer[..count].to_vec();
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
                    RecordOutcome::Accepted(_) => {}
                    RecordOutcome::Unchanged => {
                        unreachable!("terminal output always changes state")
                    }
                    RecordOutcome::CapacityReached | RecordOutcome::Closing => break,
                }
            }
            session.begin_close();
            let sequence = session
                .state
                .lock()
                .map(|state| state.sequence)
                .unwrap_or_default();
            if let Ok(mut sessions) = sessions.lock() {
                sessions.remove(&session.id);
            }
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

fn geometry(request: &Value, key: &str, default: u16) -> Result<u16, String> {
    let Some(value) = request.get(key) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .filter(|value| (1..=u64::from(MAX_GEOMETRY)).contains(value))
        .ok_or_else(|| format!("Terminal {key} must be between 1 and {MAX_GEOMETRY}."))?;
    Ok(value as u16)
}

fn error(message: impl Into<String>) -> Value {
    json!({"ok": false, "error": message.into()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn replay_capacity_closes_instead_of_replaying_an_invalid_suffix() {
        let mut state = TerminalState {
            columns: 80,
            rows: 24,
            replay: VecDeque::new(),
            replay_bytes: 0,
            sequence: 0,
            closing: false,
        };
        assert!(matches!(
            state.record_output_bounded(b"first".to_vec(), 8, 3),
            RecordOutcome::Accepted(1)
        ));
        assert!(matches!(
            state.record_output_bounded(b"second".to_vec(), 8, 3),
            RecordOutcome::CapacityReached
        ));

        assert!(state.replay_bytes <= 8);
        assert_eq!(state.sequence, 1);
        assert_eq!(state.replay.len(), 1);
        assert!(matches!(state.replay.back(), Some(ReplayFrame::Output(data)) if data == b"first"));
    }

    #[test]
    fn direct_cli_preflight_checks_paths_before_forking() {
        assert!(executable_available(Path::new("/bin/sh")));
        assert!(executable_available(Path::new("sh")));
        assert!(!executable_available(Path::new(
            "/definitely/missing/xd-agent-cli"
        )));
    }

    #[test]
    fn legacy_terminal_open_ignores_agent_fields() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let opened = manager
            .open(
                &json!({"chat": "shell-chat", "agent": "not-an-agent"}),
                Path::new("/tmp"),
            )
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();
        let snapshot = &manager.list("shell-chat")["terminals"][0];

        assert!(snapshot["agent"].is_null());
        manager.kill(&json!({"terminal": terminal})).unwrap();
    }

    #[test]
    fn pty_output_is_replayed_and_input_is_bounded() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let opened = manager
            .open_session(
                &json!({"chat": "chat-1", "columns": 92, "rows": 31}),
                Path::new("/tmp"),
                None,
                &[("XD_DIRECT_CLI_TEST".into(), "xd-pty-ready".into())],
            )
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();
        manager
            .input(&json!({
                "terminal": terminal,
                "data": STANDARD.encode(b"printf '%s\\n' \"$XD_DIRECT_CLI_TEST\"\n"),
            }))
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
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
            if String::from_utf8_lossy(&output).contains("xd-pty-ready") {
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
    fn output_and_resize_sequences_are_monotonic_snapshot_boundaries() {
        let mut state = TerminalState {
            columns: 80,
            rows: 24,
            replay: VecDeque::new(),
            replay_bytes: 0,
            sequence: 0,
            closing: false,
        };

        assert!(matches!(
            state.record_output_bounded(b"one".to_vec(), 64, 8),
            RecordOutcome::Accepted(1)
        ));
        assert!(matches!(
            state.record_resize_bounded(100, 30, 8),
            RecordOutcome::Accepted(2)
        ));
        assert!(matches!(
            state.record_output_bounded(b"two".to_vec(), 64, 8),
            RecordOutcome::Accepted(3)
        ));
        assert_eq!(state.sequence, 3);
    }

    #[test]
    fn pty_terminal_queries_are_answered_without_a_connected_ui() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let opened = manager
            .open(&json!({"chat": "query-chat"}), Path::new("/tmp"))
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();
        manager
            .input(&json!({
                "terminal": terminal,
                "data": STANDARD.encode(
                    b"stty raw -echo; printf '\\033[5n'; reply=$(dd bs=1 count=4 2>/dev/null); stty sane; printf '\\nxd-query-reply:%s\\n' \"$reply\"\n"
                ),
            }))
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let output = manager.list("query-chat")["terminals"]
                .as_array()
                .and_then(|terminals| terminals.first())
                .and_then(|terminal| terminal["replay"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|frame| frame["data"].as_str())
                .filter_map(|data| STANDARD.decode(data).ok())
                .flatten()
                .collect::<Vec<_>>();
            if output
                .windows(b"xd-query-reply:\x1b[0n".len())
                .any(|window| window == b"xd-query-reply:\x1b[0n")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the PTY child never received xd's terminal-query reply: {:?}",
                String::from_utf8_lossy(&output)
            );
            thread::sleep(Duration::from_millis(10));
        }
        manager.kill(&json!({"terminal": terminal})).unwrap();
    }

    #[test]
    fn deleting_a_chat_closes_all_of_its_terminals() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let opened = manager
            .open(&json!({"chat": "deleted-chat"}), Path::new("/tmp"))
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();

        manager.kill_chat("deleted-chat");

        assert!(
            manager
                .input(&json!({"terminal": terminal, "data": STANDARD.encode(b"x")}))
                .is_err()
        );
        assert!(
            manager.list("deleted-chat")["terminals"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn geometry_and_base64_are_validated_before_touching_a_session() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        assert!(
            manager
                .open(&json!({"chat": "chat-1", "columns": 0}), Path::new("/tmp"))
                .unwrap_err()
                .contains("columns")
        );
        assert!(
            manager
                .input(&json!({"terminal": "missing", "data": "not base64"}))
                .unwrap_err()
                .contains("No such terminal")
        );
    }

    #[test]
    fn resizing_to_the_current_geometry_is_a_no_op() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let opened = manager
            .open(
                &json!({"chat": "chat-1", "columns": 92, "rows": 31}),
                Path::new("/tmp"),
            )
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();

        let resized = manager
            .resize(&json!({"terminal": terminal, "columns": 92, "rows": 31}))
            .unwrap();

        assert_eq!(resized["changed"], false);
        manager.kill(&json!({"terminal": terminal})).unwrap();
    }
}
