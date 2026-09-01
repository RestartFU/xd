use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::session_host::ProcessSpec;

const MAX_COLUMNS: usize = 500;
const MAX_ROWS: usize = 200;
const MAX_BUFFERED_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_BUFFERED_EVENTS: usize = 2_048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndpoint {
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEvent {
    pub endpoint: SessionEndpoint,
    pub kind: SessionEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEventKind {
    Opened {
        chat_id: String,
        terminal_id: String,
        title: String,
        agent: Option<String>,
        columns: usize,
        rows: usize,
    },
    Output {
        chat_id: String,
        terminal_id: String,
        data: Vec<u8>,
    },
    Resized {
        chat_id: String,
        terminal_id: String,
        columns: usize,
        rows: usize,
    },
    Activity {
        chat_id: String,
        terminal_id: String,
        working: bool,
    },
    Closed {
        chat_id: String,
        terminal_id: String,
    },
}

impl SessionEvent {
    fn new(endpoint: SessionEndpoint, kind: SessionEventKind) -> Self {
        Self { endpoint, kind }
    }

    pub fn endpoint(&self) -> SessionEndpoint {
        self.endpoint
    }

    fn terminal_id(&self) -> &str {
        match &self.kind {
            SessionEventKind::Opened { terminal_id, .. }
            | SessionEventKind::Output { terminal_id, .. }
            | SessionEventKind::Resized { terminal_id, .. }
            | SessionEventKind::Activity { terminal_id, .. }
            | SessionEventKind::Closed { terminal_id, .. } => terminal_id,
        }
    }
}

#[derive(Clone)]
pub struct SessionRuntime {
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    events: SessionEventSender,
}

struct SessionEventQueue {
    events: VecDeque<SessionEvent>,
    output_bytes: usize,
}

#[derive(Clone)]
struct SessionEventSender {
    queue: Arc<Mutex<SessionEventQueue>>,
    wake: async_channel::Sender<()>,
}

pub struct SessionEventReceiver {
    queue: Arc<Mutex<SessionEventQueue>>,
    wake: async_channel::Receiver<()>,
}

fn session_event_channel() -> (SessionEventSender, SessionEventReceiver) {
    let queue = Arc::new(Mutex::new(SessionEventQueue {
        events: VecDeque::new(),
        output_bytes: 0,
    }));
    let (wake, awakened) = async_channel::bounded(1);
    (
        SessionEventSender {
            queue: queue.clone(),
            wake,
        },
        SessionEventReceiver {
            queue,
            wake: awakened,
        },
    )
}

impl SessionEventSender {
    fn send(&self, event: SessionEvent) -> Result<(), ()> {
        let mut queue = self.queue.lock().map_err(|_| ())?;
        if matches!(
            &event.kind,
            SessionEventKind::Activity { .. } | SessionEventKind::Resized { .. }
        ) && let Some(index) = queue.events.iter().position(|queued| {
            std::mem::discriminant(&queued.kind) == std::mem::discriminant(&event.kind)
                && queued.endpoint() == event.endpoint()
                && queued.terminal_id() == event.terminal_id()
        }) {
            queue.events[index] = event;
            drop(queue);
            let _ = self.wake.try_send(());
            return Ok(());
        }
        if let SessionEventKind::Output { data, .. } = &event.kind {
            while queue.output_bytes.saturating_add(data.len()) > MAX_BUFFERED_OUTPUT_BYTES {
                let Some(index) = queue
                    .events
                    .iter()
                    .position(|queued| matches!(&queued.kind, SessionEventKind::Output { .. }))
                else {
                    break;
                };
                if let Some(SessionEvent {
                    kind: SessionEventKind::Output { data, .. },
                    ..
                }) = queue.events.remove(index)
                {
                    queue.output_bytes = queue.output_bytes.saturating_sub(data.len());
                }
            }
            if data.len() > MAX_BUFFERED_OUTPUT_BYTES {
                return Ok(());
            }
            queue.output_bytes += data.len();
        }
        while queue.events.len() >= MAX_BUFFERED_EVENTS {
            let Some(index) = queue
                .events
                .iter()
                .position(|queued| !matches!(&queued.kind, SessionEventKind::Closed { .. }))
            else {
                if !matches!(&event.kind, SessionEventKind::Closed { .. }) {
                    return Ok(());
                }
                break;
            };
            if let Some(SessionEvent {
                kind: SessionEventKind::Output { data, .. },
                ..
            }) = queue.events.remove(index)
            {
                queue.output_bytes = queue.output_bytes.saturating_sub(data.len());
            }
        }
        queue.events.push_back(event);
        drop(queue);
        let _ = self.wake.try_send(());
        Ok(())
    }
}

impl SessionEventReceiver {
    pub async fn recv(&self) -> Result<SessionEvent, async_channel::RecvError> {
        loop {
            if let Ok(event) = self.try_recv() {
                return Ok(event);
            }
            self.wake.recv().await?;
        }
    }

    pub fn recv_blocking(&self) -> Result<SessionEvent, async_channel::RecvError> {
        loop {
            if let Ok(event) = self.try_recv() {
                return Ok(event);
            }
            self.wake.recv_blocking()?;
        }
    }

    pub fn try_recv(&self) -> Result<SessionEvent, async_channel::TryRecvError> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| async_channel::TryRecvError::Closed)?;
        let event = queue
            .events
            .pop_front()
            .ok_or(async_channel::TryRecvError::Empty)?;
        if let SessionEventKind::Output { data, .. } = &event.kind {
            queue.output_bytes = queue.output_bytes.saturating_sub(data.len());
        }
        Ok(event)
    }

    #[cfg(test)]
    fn buffered_output_bytes(&self) -> usize {
        self.queue.lock().map_or(0, |queue| queue.output_bytes)
    }
}

struct Session {
    endpoint: SessionEndpoint,
    chat_id: String,
    terminal_id: String,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Option<Box<dyn Child + Send + Sync>>>,
    cleanup: Option<ProcessSpec>,
}

#[cfg(not(test))]
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const CLEANUP_TIMEOUT: Duration = Duration::from_millis(100);

impl SessionRuntime {
    pub fn new() -> (Self, SessionEventReceiver) {
        let (events, updates) = session_event_channel();
        (
            Self {
                sessions: Arc::new(Mutex::new(HashMap::new())),
                events,
            },
            updates,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &self,
        chat_id: &str,
        terminal_id: &str,
        title: &str,
        agent: Option<&str>,
        spec: ProcessSpec,
        cleanup: Option<ProcessSpec>,
        columns: usize,
        rows: usize,
    ) -> Result<(), String> {
        self.open_for(
            SessionEndpoint::Local,
            chat_id,
            terminal_id,
            title,
            agent,
            spec,
            cleanup,
            columns,
            rows,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_for(
        &self,
        endpoint: SessionEndpoint,
        chat_id: &str,
        terminal_id: &str,
        title: &str,
        agent: Option<&str>,
        spec: ProcessSpec,
        cleanup: Option<ProcessSpec>,
        columns: usize,
        rows: usize,
    ) -> Result<(), String> {
        let columns = columns.clamp(1, MAX_COLUMNS);
        let rows = rows.clamp(1, MAX_ROWS);
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_owned())?;
        if sessions.contains_key(terminal_id) {
            return Ok(());
        }

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows as u16,
                cols: columns as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Cannot open a terminal: {error}."))?;
        let mut command = CommandBuilder::new(&spec.program);
        command.args(spec.arguments);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        restore_host_environment(&mut command);
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("Cannot start terminal: {error}."))?;
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("Cannot read terminal output: {error}."))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("Cannot prepare terminal input: {error}."))?;
        let session = Arc::new(Session {
            endpoint,
            chat_id: chat_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(writer)),
            child: Mutex::new(Some(child)),
            cleanup,
        });
        sessions.insert(terminal_id.to_owned(), session.clone());
        drop(sessions);

        self.events
            .send(SessionEvent::new(
                endpoint,
                SessionEventKind::Opened {
                    chat_id: chat_id.to_owned(),
                    terminal_id: terminal_id.to_owned(),
                    title: title.to_owned(),
                    agent: agent.map(str::to_owned),
                    columns,
                    rows,
                },
            ))
            .map_err(|_| "Terminal event receiver is closed.".to_owned())?;
        start_reader(session, reader, self.sessions.clone(), self.events.clone());
        Ok(())
    }

    pub fn input(&self, terminal_id: &str, data: &[u8]) -> Result<(), String> {
        let session = self.session(terminal_id)?;
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| "Terminal input is unavailable.".to_owned())?;
        let writer = writer
            .as_mut()
            .ok_or_else(|| "The terminal is closed.".to_owned())?;
        writer
            .write_all(data)
            .and_then(|_| writer.flush())
            .map_err(|error| format!("Cannot write terminal: {error}."))
    }

    pub fn resize(&self, terminal_id: &str, columns: usize, rows: usize) -> Result<(), String> {
        let session = self.session(terminal_id)?;
        let columns = columns.clamp(1, MAX_COLUMNS);
        let rows = rows.clamp(1, MAX_ROWS);
        session
            .master
            .lock()
            .map_err(|_| "Terminal geometry is unavailable.".to_owned())?
            .as_ref()
            .ok_or_else(|| "The terminal is closed.".to_owned())?
            .resize(PtySize {
                rows: rows as u16,
                cols: columns as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Cannot resize terminal: {error}."))?;
        let _ = self.events.send(SessionEvent::new(
            session.endpoint,
            SessionEventKind::Resized {
                chat_id: session.chat_id.clone(),
                terminal_id: session.terminal_id.clone(),
                columns,
                rows,
            },
        ));
        Ok(())
    }

    pub fn kill(&self, terminal_id: &str) -> Result<(), String> {
        let session = match self
            .sessions
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_owned())?
            .remove(terminal_id)
        {
            Some(session) => session,
            None => return Ok(()),
        };
        let _ = self.events.send(SessionEvent::new(
            session.endpoint,
            SessionEventKind::Closed {
                chat_id: session.chat_id.clone(),
                terminal_id: session.terminal_id.clone(),
            },
        ));
        let _ = thread::Builder::new()
            .name("xd-terminal-cleanup".into())
            .spawn(move || {
                session.close();
                if let Some(cleanup) = session.cleanup.clone() {
                    run_cleanup(cleanup);
                }
            });
        Ok(())
    }

    pub fn kill_chats(
        &self,
        endpoint: SessionEndpoint,
        chat_ids: &HashSet<String>,
    ) -> Result<(), String> {
        let terminal_ids = self
            .sessions
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_owned())?
            .values()
            .filter(|session| session.endpoint == endpoint && chat_ids.contains(&session.chat_id))
            .map(|session| session.terminal_id.clone())
            .collect::<Vec<_>>();
        for terminal_id in terminal_ids {
            self.kill(&terminal_id)?;
        }
        Ok(())
    }

    pub fn reconcile_chats(
        &self,
        endpoint: SessionEndpoint,
        live_chats: &HashSet<String>,
    ) -> Result<(), String> {
        let removed = self
            .sessions
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_owned())?
            .values()
            .filter(|session| {
                session.endpoint == endpoint
                    && !session.chat_id.starts_with("global:")
                    && !live_chats.contains(&session.chat_id)
            })
            .map(|session| session.chat_id.clone())
            .collect::<HashSet<_>>();
        self.kill_chats(endpoint, &removed)
    }

    fn session(&self, terminal_id: &str) -> Result<Arc<Session>, String> {
        self.sessions
            .lock()
            .map_err(|_| "Terminal state is unavailable.".to_owned())?
            .get(terminal_id)
            .cloned()
            .ok_or_else(|| "No such terminal.".to_owned())
    }

    #[cfg(test)]
    fn wait_until_empty(&self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self
                .sessions
                .lock()
                .is_ok_and(|sessions| sessions.is_empty())
            {
                return true;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }
}

fn run_cleanup(spec: ProcessSpec) {
    let mut child = match Command::new(spec.program)
        .args(spec.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return,
    };
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

impl Session {
    fn close(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            writer.take();
        }
        if let Ok(mut child) = self.child.lock()
            && let Some(mut child) = child.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Ok(mut master) = self.master.lock() {
            master.take();
        }
    }
}

fn start_reader(
    session: Arc<Session>,
    mut reader: Box<dyn Read + Send>,
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    events: SessionEventSender,
) {
    thread::Builder::new()
        .name(format!("xd-terminal-{}", session.terminal_id))
        .spawn(move || {
            let mut buffer = [0_u8; 8_192];
            let mut activity = ActivityParser::default();
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                if events
                    .send(SessionEvent::new(
                        session.endpoint,
                        SessionEventKind::Output {
                            chat_id: session.chat_id.clone(),
                            terminal_id: session.terminal_id.clone(),
                            data: buffer[..count].to_vec(),
                        },
                    ))
                    .is_err()
                {
                    break;
                }
                for working in activity.feed(&buffer[..count]) {
                    let _ = events.send(SessionEvent::new(
                        session.endpoint,
                        SessionEventKind::Activity {
                            chat_id: session.chat_id.clone(),
                            terminal_id: session.terminal_id.clone(),
                            working,
                        },
                    ));
                }
            }
            session.close();
            let should_close = sessions.lock().is_ok_and(|mut sessions| {
                let is_current = sessions
                    .get(&session.terminal_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &session));
                if is_current {
                    sessions.remove(&session.terminal_id);
                }
                is_current
            });
            if should_close {
                let _ = events.send(SessionEvent::new(
                    session.endpoint,
                    SessionEventKind::Closed {
                        chat_id: session.chat_id.clone(),
                        terminal_id: session.terminal_id.clone(),
                    },
                ));
            }
        })
        .expect("terminal reader thread should start");
}

#[derive(Clone, Copy, Default)]
enum ActivityState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
    Discard,
    DiscardEscape,
}

#[derive(Default)]
struct ActivityParser {
    state: ActivityState,
    payload: Vec<u8>,
    working: Option<bool>,
}

impl ActivityParser {
    fn feed(&mut self, data: &[u8]) -> Vec<bool> {
        let mut updates = Vec::new();
        for &byte in data {
            match self.state {
                ActivityState::Ground if byte == 0x1b => self.state = ActivityState::Escape,
                ActivityState::Ground => {}
                ActivityState::Escape if byte == b']' => {
                    self.payload.clear();
                    self.state = ActivityState::Osc;
                }
                ActivityState::Escape if byte == 0x1b => {}
                ActivityState::Escape => self.state = ActivityState::Ground,
                ActivityState::Osc if byte == 0x07 => self.finish(&mut updates),
                ActivityState::Osc if byte == 0x1b => self.state = ActivityState::OscEscape,
                ActivityState::Osc if self.payload.len() < 256 => self.payload.push(byte),
                ActivityState::Osc => self.discard(),
                ActivityState::OscEscape if matches!(byte, b'\\' | 0x07) => {
                    self.finish(&mut updates)
                }
                ActivityState::OscEscape => self.discard(),
                ActivityState::Discard if byte == 0x07 => self.state = ActivityState::Ground,
                ActivityState::Discard if byte == 0x1b => self.state = ActivityState::DiscardEscape,
                ActivityState::Discard => {}
                ActivityState::DiscardEscape if matches!(byte, b'\\' | 0x07) => {
                    self.state = ActivityState::Ground
                }
                ActivityState::DiscardEscape => self.state = ActivityState::Discard,
            }
        }
        updates
    }

    fn finish(&mut self, updates: &mut Vec<bool>) {
        let update = self
            .payload
            .strip_prefix(b"0;")
            .and_then(|title| match title {
                b"Ready" => Some(false),
                title if title.starts_with("✳ ".as_bytes()) => Some(false),
                b"Working" | b"Thinking" | b"Waiting" | b"Starting" => Some(true),
                title
                    if title.starts_with("◐ ".as_bytes()) || title.starts_with("◑ ".as_bytes()) =>
                {
                    Some(true)
                }
                _ => None,
            })
            .or_else(|| {
                let mut fields = self.payload.split(|byte| *byte == b';');
                (fields.next() == Some(b"9".as_slice()) && fields.next() == Some(b"4".as_slice()))
                    .then(|| match fields.next() {
                        Some(b"0") => Some(false),
                        Some(b"1" | b"2" | b"3" | b"4") => Some(true),
                        _ => None,
                    })
                    .flatten()
            });
        if let Some(update) = update
            && self.working != Some(update)
        {
            self.working = Some(update);
            updates.push(update);
        }
        self.payload.clear();
        self.state = ActivityState::Ground;
    }

    fn discard(&mut self) {
        self.payload.clear();
        self.state = ActivityState::Discard;
    }
}

fn restore_host_environment(command: &mut CommandBuilder) {
    for name in ["PATH", "LANG", "LC_ALL", "LOCPATH"] {
        let saved = format!("XD_HOST_{name}");
        if let Some(value) = std::env::var_os(&saved) {
            if value.is_empty() {
                command.env_remove(name);
            } else {
                command.env(name, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use crate::session_host::ProcessSpec;

    use super::{ActivityParser, SessionEndpoint, SessionEvent, SessionEventKind, SessionRuntime};

    #[test]
    fn chat_reconciliation_is_scoped_to_the_sessions_origin_endpoint() {
        let (runtime, events) = SessionRuntime::new();
        for (endpoint, terminal_id) in [
            (SessionEndpoint::Local, "terminal-local"),
            (SessionEndpoint::Remote, "terminal-remote"),
        ] {
            runtime
                .open_for(
                    endpoint,
                    "same-chat",
                    terminal_id,
                    "Terminal",
                    None,
                    ProcessSpec::new("/bin/sh", ["-c", "sleep 30"]),
                    None,
                    80,
                    24,
                )
                .unwrap();
            let opened = events.recv_blocking().unwrap();
            assert_eq!(opened.endpoint(), endpoint);
        }

        runtime
            .reconcile_chats(SessionEndpoint::Local, &std::collections::HashSet::new())
            .unwrap();

        assert!(runtime.input("terminal-local", b"x").is_err());
        assert!(runtime.input("terminal-remote", b"x").is_ok());
        runtime.kill("terminal-remote").unwrap();
    }

    #[test]
    fn terminal_output_queue_is_bounded_and_never_drops_closure() {
        let (events, updates) = super::session_event_channel();
        for _ in 0..(super::MAX_BUFFERED_OUTPUT_BYTES / 8_192 + 32) {
            events
                .send(SessionEvent::new(
                    super::SessionEndpoint::Local,
                    SessionEventKind::Output {
                        chat_id: "chat".into(),
                        terminal_id: "terminal".into(),
                        data: vec![b'x'; 8_192],
                    },
                ))
                .unwrap();
        }
        events
            .send(SessionEvent::new(
                super::SessionEndpoint::Local,
                SessionEventKind::Closed {
                    chat_id: "chat".into(),
                    terminal_id: "terminal".into(),
                },
            ))
            .unwrap();

        assert!(updates.buffered_output_bytes() <= super::MAX_BUFFERED_OUTPUT_BYTES);
        let mut saw_closed = false;
        while let Ok(event) = updates.try_recv() {
            saw_closed |= matches!(event.kind, SessionEventKind::Closed { .. });
        }
        assert!(saw_closed);
    }

    #[test]
    fn terminal_activity_flood_is_coalesced_without_dropping_closure() {
        let (events, updates) = super::session_event_channel();
        for index in 0..10_000 {
            events
                .send(SessionEvent::new(
                    super::SessionEndpoint::Local,
                    SessionEventKind::Activity {
                        chat_id: "chat".into(),
                        terminal_id: "terminal".into(),
                        working: index % 2 == 0,
                    },
                ))
                .unwrap();
        }
        events
            .send(SessionEvent::new(
                super::SessionEndpoint::Local,
                SessionEventKind::Closed {
                    chat_id: "chat".into(),
                    terminal_id: "terminal".into(),
                },
            ))
            .unwrap();

        let mut count = 0;
        let mut saw_closed = false;
        while let Ok(event) = updates.try_recv() {
            count += 1;
            saw_closed |= matches!(event.kind, SessionEventKind::Closed { .. });
        }
        assert!(count <= 64, "activity events were not bounded: {count}");
        assert!(saw_closed);
    }

    #[test]
    fn cli_terminal_titles_drive_activity_without_host_events() {
        let mut parser = ActivityParser::default();
        assert_eq!(parser.feed(b"\x1b]0;Working\x07"), vec![true]);
        assert_eq!(parser.feed(b"\x1b]0;Ready\x1b\\"), vec![false]);
        assert_eq!(parser.feed(b"\x1b]9;4;3;42\x07"), vec![true]);
        assert_eq!(parser.feed(b"\x1b]9;4;0;\x07"), vec![false]);
    }

    #[test]
    fn claude_background_agent_spinner_titles_are_working() {
        let mut parser = ActivityParser::default();
        assert_eq!(
            parser.feed("\x1b]0;✳ Claude Code\x07".as_bytes()),
            vec![false]
        );
        assert_eq!(
            parser.feed("\x1b]0;◐ Claude Code\x07".as_bytes()),
            vec![true]
        );
        assert_eq!(
            parser.feed("\x1b]0;◑ Claude Code\x07".as_bytes()),
            Vec::<bool>::new()
        );
        assert_eq!(
            parser.feed("\x1b]0;✳ Claude Code\x07".as_bytes()),
            vec![false]
        );
    }

    #[test]
    fn claude_task_titles_preserve_spinner_activity_semantics() {
        let mut parser = ActivityParser::default();

        assert_eq!(
            parser.feed("\x1b]0;◐ Fix confirmed bugs\x07".as_bytes()),
            vec![true]
        );
        assert_eq!(
            parser.feed("\x1b]0;✳ Fix confirmed bugs\x07".as_bytes()),
            vec![false]
        );
    }

    #[test]
    fn a_pty_session_streams_output_accepts_input_and_closes() {
        let (runtime, events) = SessionRuntime::new();
        runtime
            .open(
                "chat-one",
                "terminal-one",
                "Terminal",
                None,
                ProcessSpec::new(
                    PathBuf::from("/bin/sh"),
                    ["-c", "printf ready; read line; printf ':%s' \"$line\""],
                ),
                None,
                80,
                24,
            )
            .unwrap();

        let opened = events.recv_blocking().unwrap();
        assert!(matches!(opened.kind, SessionEventKind::Opened { .. }));
        assert!(recv_output_containing(&events, "ready"));

        runtime.input("terminal-one", b"hello\n").unwrap();
        assert!(recv_output_containing(&events, ":hello"));

        let closed = events.recv_blocking().unwrap();
        assert!(matches!(closed.kind, SessionEventKind::Closed { .. }));
        assert!(runtime.wait_until_empty(Duration::from_secs(1)));
    }

    #[test]
    fn explicit_close_runs_persistent_cleanup_without_blocking_the_caller() {
        let directory =
            std::env::temp_dir().join(format!("xd-terminal-cleanup-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let marker = directory.join("closed");
        let cleanup = ProcessSpec::new(
            "/bin/sh",
            [
                "-c".to_owned(),
                format!("printf closed > {}", marker.to_string_lossy()),
            ],
        );
        let (runtime, events) = SessionRuntime::new();
        runtime
            .open(
                "chat-one",
                "terminal-cleanup",
                "Terminal",
                None,
                ProcessSpec::new("/bin/sh", ["-c", "sleep 30"]),
                Some(cleanup),
                80,
                24,
            )
            .unwrap();
        assert!(matches!(
            events.recv_blocking().unwrap().kind,
            SessionEventKind::Opened { .. }
        ));

        let started = std::time::Instant::now();
        runtime.kill("terminal-cleanup").unwrap();
        assert!(started.elapsed() < Duration::from_millis(50));
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !marker.exists() {
            assert!(std::time::Instant::now() < deadline, "cleanup did not run");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn explicit_close_allows_immediate_reopen_with_the_same_terminal_id() {
        let (runtime, events) = SessionRuntime::new();
        runtime
            .open(
                "chat-one",
                "stable-agent",
                "Claude",
                Some("claude"),
                ProcessSpec::new("/bin/sh", ["-c", "sleep 30"]),
                None,
                80,
                24,
            )
            .unwrap();
        assert!(matches!(
            events.recv_blocking().unwrap().kind,
            SessionEventKind::Opened { .. }
        ));
        runtime.kill("stable-agent").unwrap();
        assert!((0..16).any(|_| matches!(
            events.recv_blocking().unwrap().kind,
            SessionEventKind::Closed { .. }
        )));

        runtime
            .open(
                "chat-one",
                "stable-agent",
                "Claude",
                Some("claude"),
                ProcessSpec::new("/bin/sh", ["-c", "printf reopened; sleep 30"]),
                None,
                80,
                24,
            )
            .unwrap();
        assert!((0..16).any(|_| matches!(
            events.recv_blocking().unwrap().kind,
            SessionEventKind::Opened { .. }
        )));
        assert!(recv_output_containing(&events, "reopened"));
        runtime.kill("stable-agent").unwrap();
    }

    fn recv_output_containing(events: &super::SessionEventReceiver, expected: &str) -> bool {
        for _ in 0..8 {
            match events.recv_blocking().unwrap().kind {
                SessionEventKind::Output { data, .. }
                    if String::from_utf8_lossy(&data).contains(expected) =>
                {
                    return true;
                }
                SessionEventKind::Closed { .. } => return false,
                _ => {}
            }
        }
        false
    }
}
