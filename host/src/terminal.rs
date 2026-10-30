use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::raw::{c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    EventBus,
    terminal_activity::TerminalActivityParser,
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
    fn execvp(file: *const c_char, arguments: *const *const c_char) -> c_int;
    fn _exit(status: c_int) -> !;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
}

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
    pid: c_int,
    writer: Mutex<Option<File>>,
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
        let chat_id = text(request, "chat", "terminal-open needs a chat id")?;
        let columns = geometry(request, "columns", DEFAULT_COLUMNS, MAX_COLUMNS)?;
        let rows = geometry(request, "rows", DEFAULT_ROWS, MAX_ROWS)?;
        let reuse = request
            .get("reuse")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // Reuse is a check-then-create operation shared by every connected
        // client. Serialize that short path so a desktop and phone restoring
        // the same chat cannot both observe an empty map and fork duplicate
        // shells. This gate is separate from `sessions` to preserve the close
        // path's state -> sessions lock order.
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
                .find(|session| session.chat_id == chat_id && !session.is_closing())
        {
            return Ok(json!({"ok": true, "id": existing.id}));
        }
        let queries = TerminalQueryResponder::from_request(request);

        let (session, reader) =
            TerminalSession::spawn(chat_id, workdir, columns, rows, self.activity.clone())?;
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
            "agent": null,
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

    pub(crate) fn has_sessions(&self) -> bool {
        self.sessions
            .lock()
            .map_or(true, |sessions| !sessions.is_empty())
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
        activity: Arc<TerminalActivityState>,
    ) -> Result<(Arc<Self>, File), String> {
        if !workdir.is_dir() {
            return Err(format!("{} is not a directory.", workdir.display()));
        }
        let executable = {
            env::var_os("SHELL")
                .map(PathBuf::from)
                .filter(|shell| shell.is_file())
                .unwrap_or_else(|| PathBuf::from("/bin/sh"))
        };
        let executable = CString::new(executable.as_os_str().as_encoded_bytes())
            .map_err(|_| "The terminal executable path is invalid.".to_string())?;
        let argument_pointers = [executable.as_ptr(), std::ptr::null()];
        let title = "Terminal".to_owned();
        let workdir = CString::new(workdir.as_os_str().as_encoded_bytes())
            .map_err(|_| "The terminal working directory is invalid.".to_string())?;
        let term = c"TERM";
        let term_value = c"xterm-256color";
        let colorterm = c"COLORTERM";
        let colorterm_value = c"truecolor";
        let environment = crate::user_environment::values()
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_encoded_bytes()))
            .map(|(name, value)| {
                Ok((
                    CString::new(name.as_bytes())
                        .map_err(|_| "A terminal environment name is invalid.".to_string())?,
                    CString::new(value)
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
                execvp(executable.as_ptr(), argument_pointers.as_ptr());
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
            pid,
            writer: Mutex::new(Some(writer)),
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
            "agent": null,
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
        false
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
    mut reader: File,
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
    use std::{
        sync::mpsc::{Receiver, sync_channel},
        time::Instant,
    };

    use crate::local_socket::UnixStream;

    fn wait_for_activity(manager: &TerminalManager, chat_id: &str, working: bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let listed = manager.list(chat_id);
            let activity = listed["terminals"]
                .as_array()
                .and_then(|terminals| terminals.first())
                .and_then(|terminal| terminal["working"].as_bool());
            if activity == Some(working) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "terminal activity never became {working}: {listed}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn next_activity(receiver: &Receiver<Value>, terminal: &str, working: bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let event = next_terminal_activity(receiver, deadline);
            if event["event"] == "terminal-activity"
                && event["terminal"] == terminal
                && event["working"] == working
            {
                return event;
            }
        }
    }

    fn next_terminal_activity(receiver: &Receiver<Value>, deadline: Instant) -> Value {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = receiver
                .recv_timeout(remaining)
                .expect("terminal activity event was not published");
            if event["event"] == "terminal-activity" {
                return event;
            }
        }
    }

    #[test]
    fn concurrent_reuse_opens_only_one_terminal() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let workdir = env::temp_dir();
        let ids = thread::scope(|scope| {
            let workers = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        manager
                            .open(&json!({"chat": "shared-chat", "reuse": true}), &workdir)
                            .unwrap()["id"]
                            .as_str()
                            .unwrap()
                            .to_owned()
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });

        assert!(ids.iter().all(|id| id == &ids[0]));
        assert_eq!(
            manager.list("shared-chat")["terminals"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        manager.kill(&json!({"terminal": ids[0]})).unwrap();
    }

    #[test]
    fn codex_terminal_titles_drive_activity_snapshots() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let opened = manager
            .open(&json!({"chat": "codex-title"}), Path::new("/tmp"))
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();

        manager
            .input(&json!({
                "terminal": terminal,
                "data": STANDARD.encode(b"printf '\\033]0;Working\\007'\n")
            }))
            .unwrap();
        wait_for_activity(&manager, "codex-title", true);

        manager
            .input(&json!({
                "terminal": terminal,
                "data": STANDARD.encode(b"printf '\\033]0;Ready\\007'\n")
            }))
            .unwrap();
        wait_for_activity(&manager, "codex-title", false);
        manager.kill(&json!({"terminal": terminal})).unwrap();
    }

    #[test]
    fn claude_progress_sequences_drive_activity_snapshots() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let opened = manager
            .open(&json!({"chat": "claude-progress"}), Path::new("/tmp"))
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();

        manager
            .input(&json!({
                "terminal": terminal,
                "data": STANDARD.encode(b"printf '\\033]9;4;3;\\007'\n")
            }))
            .unwrap();
        wait_for_activity(&manager, "claude-progress", true);

        manager
            .input(&json!({
                "terminal": terminal,
                "data": STANDARD.encode(b"printf '\\033]9;4;0;\\007'\n")
            }))
            .unwrap();
        wait_for_activity(&manager, "claude-progress", false);
        manager.kill(&json!({"terminal": terminal})).unwrap();
    }

    #[test]
    fn activity_events_include_shell_excluding_aggregate_and_close_state() {
        let events = Arc::new(EventBus::default());
        let (sender, receiver) = sync_channel(32);
        let (connection, peer) = UnixStream::pair().unwrap();
        let subscriber = events.subscribe(sender, connection).unwrap();
        let manager = TerminalManager::new(events.clone());
        let opened = manager
            .open(&json!({"chat": "shell-activity"}), Path::new("/tmp"))
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();

        manager
            .input(&json!({
                "terminal": terminal,
                "data": STANDARD.encode(b"printf '\\033]0;Working\\007'\n")
            }))
            .unwrap();
        let active = next_activity(&receiver, terminal, true);
        assert_eq!(active["chat"], "shell-activity");
        assert_eq!(active["terminal_working"], false);
        let epoch = active["terminal_activity_epoch"]
            .as_str()
            .unwrap()
            .to_owned();
        let revision = active["terminal_activity_revision"].as_u64().unwrap();
        assert!(manager.working_chats().is_empty());

        manager.kill(&json!({"terminal": terminal})).unwrap();
        let closed = next_activity(&receiver, terminal, false);
        assert_eq!(closed["terminal_working"], false);
        assert_eq!(closed["terminal_activity_epoch"], epoch);
        assert!(closed["terminal_activity_revision"].as_u64().unwrap() > revision);
        events.unsubscribe(subscriber);
        drop(peer);
    }

    #[test]
    fn terminal_activity_epochs_are_per_manager_and_start_at_revision_zero() {
        let first = TerminalManager::new(Arc::new(EventBus::default())).activity_snapshot();
        let second = TerminalManager::new(Arc::new(EventBus::default())).activity_snapshot();

        assert!(!first.epoch.is_empty());
        assert_ne!(first.epoch, second.epoch);
        assert_eq!(first.revision, 0);
        assert_eq!(second.revision, 0);
        assert!(first.working_chats.is_empty());
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
        assert_eq!(snapshot["title"], "Terminal");
        manager.kill(&json!({"terminal": terminal})).unwrap();
    }

    #[test]
    fn pty_output_is_replayed_and_input_is_bounded() {
        let manager = TerminalManager::new(Arc::new(EventBus::default()));
        let opened = manager
            .open(
                &json!({"chat": "chat-1", "columns": 92, "rows": 31}),
                Path::new("/tmp"),
            )
            .unwrap();
        let terminal = opened["id"].as_str().unwrap();
        manager
            .input(&json!({
                "terminal": terminal,
                "data": STANDARD.encode(b"printf 'xd-pty-ready\\n'\n"),
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
        let mut state = TerminalState::new(80, 24);

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
                .open(
                    &json!({"chat": "chat-1", "columns": MAX_COLUMNS + 1}),
                    Path::new("/tmp"),
                )
                .unwrap_err()
                .contains("columns")
        );
        assert!(
            manager
                .open(
                    &json!({"chat": "chat-1", "rows": MAX_ROWS + 1}),
                    Path::new("/tmp"),
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
