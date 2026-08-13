use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};

use async_channel::{Receiver, Sender};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::session_host::ProcessSpec;

const MAX_COLUMNS: usize = 500;
const MAX_ROWS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
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

#[derive(Clone)]
pub struct SessionRuntime {
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    events: Sender<SessionEvent>,
}

struct Session {
    chat_id: String,
    terminal_id: String,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Option<Box<dyn Child + Send + Sync>>>,
}

impl SessionRuntime {
    pub fn new() -> (Self, Receiver<SessionEvent>) {
        let (events, updates) = async_channel::unbounded();
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
            chat_id: chat_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(writer)),
            child: Mutex::new(Some(child)),
        });
        sessions.insert(terminal_id.to_owned(), session.clone());
        drop(sessions);

        self.events
            .send_blocking(SessionEvent::Opened {
                chat_id: chat_id.to_owned(),
                terminal_id: terminal_id.to_owned(),
                title: title.to_owned(),
                agent: agent.map(str::to_owned),
                columns,
                rows,
            })
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
        let _ = self.events.send_blocking(SessionEvent::Resized {
            chat_id: session.chat_id.clone(),
            terminal_id: session.terminal_id.clone(),
            columns,
            rows,
        });
        Ok(())
    }

    pub fn kill(&self, terminal_id: &str) -> Result<(), String> {
        let session = match self.session(terminal_id) {
            Ok(session) => session,
            Err(_) => return Ok(()),
        };
        session.close();
        Ok(())
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
    events: Sender<SessionEvent>,
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
                    .send_blocking(SessionEvent::Output {
                        chat_id: session.chat_id.clone(),
                        terminal_id: session.terminal_id.clone(),
                        data: buffer[..count].to_vec(),
                    })
                    .is_err()
                {
                    break;
                }
                for working in activity.feed(&buffer[..count]) {
                    let _ = events.send_blocking(SessionEvent::Activity {
                        chat_id: session.chat_id.clone(),
                        terminal_id: session.terminal_id.clone(),
                        working,
                    });
                }
            }
            session.close();
            if let Ok(mut sessions) = sessions.lock() {
                sessions.remove(&session.terminal_id);
            }
            let _ = events.send_blocking(SessionEvent::Closed {
                chat_id: session.chat_id.clone(),
                terminal_id: session.terminal_id.clone(),
            });
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

    use super::{ActivityParser, SessionEvent, SessionRuntime};

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
                80,
                24,
            )
            .unwrap();

        let opened = events.recv_blocking().unwrap();
        assert!(matches!(opened, SessionEvent::Opened { .. }));
        assert!(recv_output_containing(&events, "ready"));

        runtime.input("terminal-one", b"hello\n").unwrap();
        assert!(recv_output_containing(&events, ":hello"));

        let closed = events.recv_blocking().unwrap();
        assert!(matches!(closed, SessionEvent::Closed { .. }));
        assert!(runtime.wait_until_empty(Duration::from_secs(1)));
    }

    fn recv_output_containing(
        events: &async_channel::Receiver<SessionEvent>,
        expected: &str,
    ) -> bool {
        for _ in 0..8 {
            match events.recv_blocking().unwrap() {
                SessionEvent::Output { data, .. }
                    if String::from_utf8_lossy(&data).contains(expected) =>
                {
                    return true;
                }
                SessionEvent::Closed { .. } => return false,
                _ => {}
            }
        }
        false
    }
}
