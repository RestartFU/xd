use std::{
    collections::HashMap,
    env,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::EventBus;

const DEFAULT_COLUMNS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const MAX_GEOMETRY: u16 = 1_000;
const HISTORY_LIMIT: usize = 16 * 1024 * 1024;
const REPLAY_ITEM_LIMIT: usize = 65_536;
const INPUT_LIMIT: usize = 1024 * 1024;
const READ_SIZE: usize = 8_192;
const LIMIT_NOTICE: &[u8] = b"\r\n[xd: terminal closed after exceeding its replay limit]\r\n";

pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    events: Arc<EventBus>,
}

struct TerminalSession {
    id: String,
    chat_id: String,
    title: String,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Option<Box<dyn Child + Send + Sync>>>,
    state: Mutex<TerminalState>,
}

struct TerminalState {
    columns: u16,
    rows: u16,
    replay: Vec<ReplayFrame>,
    replay_bytes: usize,
    closing: bool,
}

enum ReplayFrame {
    Output(Vec<u8>),
    Resize { columns: u16, rows: u16 },
}

enum RecordOutcome {
    Accepted,
    Closing,
    Full,
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
                .find(|session| session.chat_id == chat_id && !session.is_closing())
        {
            return Ok(json!({"ok": true, "id": existing.id}));
        }

        let (session, reader) = TerminalSession::spawn(chat_id, workdir, columns, rows)?;
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
            "columns": columns,
            "rows": rows,
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
        session
            .master
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_string())?
            .as_ref()
            .ok_or_else(|| "The terminal is closed.".to_string())?
            .resize(pty_size(columns, rows))
            .map_err(|error| format!("Cannot resize terminal: {error}."))?;
        {
            let mut state = session
                .state
                .lock()
                .map_err(|_| "Terminal state is unavailable.".to_string())?;
            if state.closing {
                return Err("The terminal is closed.".into());
            }
            if state.replay.len() >= REPLAY_ITEM_LIMIT {
                return Err("The terminal replay is full.".into());
            }
            state.columns = columns;
            state.rows = rows;
            if !matches!(
                state.replay.last(),
                Some(ReplayFrame::Resize { columns: old_columns, rows: old_rows })
                    if *old_columns == columns && *old_rows == rows
            ) {
                state.replay.push(ReplayFrame::Resize { columns, rows });
            }
        }
        self.events.publish(json!({
            "event": "terminal-resized",
            "chat": session.chat_id,
            "terminal": session.id,
            "columns": columns,
            "rows": rows,
        }));
        Ok(json!({"ok": true}))
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
    ) -> Result<(Arc<Self>, Box<dyn Read + Send>), String> {
        if !workdir.is_dir() {
            return Err(format!("{} is not a directory.", workdir.display()));
        }
        let pair = native_pty_system()
            .openpty(pty_size(columns, rows))
            .map_err(|error| format!("Cannot open a terminal: {error}."))?;
        let mut command = terminal_command();
        command.cwd(workdir.as_os_str());
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
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

        let title = workdir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Terminal")
            .to_owned();
        let session = Arc::new(Self {
            id: Uuid::new_v4().to_string(),
            chat_id: chat_id.to_owned(),
            title,
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(writer)),
            child: Mutex::new(Some(child)),
            state: Mutex::new(TerminalState {
                columns,
                rows,
                replay: vec![ReplayFrame::Resize { columns, rows }],
                replay_bytes: 0,
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
            "columns": state.columns,
            "rows": state.rows,
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
                if state.closing {
                    false
                } else {
                    state.closing = true;
                    true
                }
            })
            .unwrap_or(false);
        if !should_close {
            return;
        }
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

fn start_reader(
    session: Arc<TerminalSession>,
    mut reader: Box<dyn Read + Send>,
    sessions: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    events: Arc<EventBus>,
) {
    thread::Builder::new()
        .name(format!("xd-terminal-{}", session.id))
        .spawn(move || {
            let mut buffer = [0_u8; READ_SIZE];
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                let data = buffer[..count].to_vec();
                let outcome = session
                    .state
                    .lock()
                    .map(|mut state| {
                        if state.closing {
                            RecordOutcome::Closing
                        } else if data.len() > HISTORY_LIMIT.saturating_sub(state.replay_bytes)
                            || state.replay.len() >= REPLAY_ITEM_LIMIT
                        {
                            RecordOutcome::Full
                        } else {
                            state.replay_bytes += data.len();
                            state.replay.push(ReplayFrame::Output(data.clone()));
                            RecordOutcome::Accepted
                        }
                    })
                    .unwrap_or(RecordOutcome::Closing);
                match outcome {
                    RecordOutcome::Closing => break,
                    RecordOutcome::Full => {
                        events.publish(json!({
                            "event": "terminal-output",
                            "chat": session.chat_id,
                            "terminal": session.id,
                            "data": STANDARD.encode(LIMIT_NOTICE),
                        }));
                        session.begin_close();
                        break;
                    }
                    RecordOutcome::Accepted => {}
                }
                events.publish(json!({
                    "event": "terminal-output",
                    "chat": session.chat_id,
                    "terminal": session.id,
                    "data": STANDARD.encode(&data),
                }));
            }
            session.begin_close();
            if let Ok(mut sessions) = sessions.lock() {
                sessions.remove(&session.id);
            }
            events.publish(json!({
                "event": "terminal-closed",
                "chat": session.chat_id,
                "terminal": session.id,
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
    use std::time::{Duration, Instant};

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
                .input(&json!({"terminal": "missing", "data": "not base64"}))
                .unwrap_err()
                .contains("No such terminal")
        );
    }
}
