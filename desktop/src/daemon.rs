use std::{
    collections::HashMap,
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::protocol::{AUTHENTICATED_FRAME_LIMIT, Frame, ProtocolCodec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestKind {
    Tree,
    NewFolder,
    NewChat { folder_id: String },
    Chat { chat_id: String },
    Messages { chat_id: String },
    Send { chat_id: String, text: String },
    SetDraft { chat_id: String, text: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DaemonUpdate {
    Connected {
        path: PathBuf,
    },
    Reply {
        kind: RequestKind,
        body: Map<String, Value>,
    },
    Event {
        name: String,
        body: Map<String, Value>,
    },
    Disconnected {
        message: String,
    },
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("no xd daemon socket was found (looked in {0})")]
    NotFound(String),
    #[error("could not connect to xd daemon at {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not start an installed xd daemon ({0})")]
    Start(String),
}

struct Command {
    kind: RequestKind,
    body: Value,
}

#[derive(Clone)]
pub struct DaemonHandle {
    commands: mpsc::Sender<Command>,
}

pub struct StartedDaemon {
    child: Child,
}

impl Drop for StartedDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl DaemonHandle {
    pub fn connect_or_start()
    -> Result<(Self, Receiver<DaemonUpdate>, Option<StartedDaemon>), ConnectError> {
        if let Ok((daemon, updates)) = Self::connect_discovered() {
            return Ok((daemon, updates, None));
        }

        let mut failures = Vec::new();
        for (path, launcher) in startup_candidates() {
            let socket = path.to_string_lossy().into_owned();
            let mut child = match ProcessCommand::new(&launcher)
                .args(["serve", "--port", "0", "--socket", &socket])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    failures.push(format!("{launcher} is not installed"));
                    continue;
                }
                Err(error) => {
                    failures.push(format!("cannot launch {launcher}: {error}"));
                    continue;
                }
            };

            for _ in 0..50 {
                if let Ok((daemon, updates)) = Self::connect(path.clone()) {
                    return Ok((daemon, updates, Some(StartedDaemon { child })));
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        failures.push(format!("{launcher} exited with {status}"));
                        break;
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(100)),
                    Err(error) => {
                        failures.push(format!("cannot inspect {launcher}: {error}"));
                        break;
                    }
                }
            }

            if let Ok((daemon, updates)) = Self::connect(path.clone()) {
                return Ok((daemon, updates, Some(StartedDaemon { child })));
            }
            let _ = child.kill();
            let _ = child.wait();
            failures.push(format!(
                "{launcher} did not open {} within five seconds",
                path.display()
            ));
        }

        Err(ConnectError::Start(failures.join("; ")))
    }

    pub fn connect_discovered() -> Result<(Self, Receiver<DaemonUpdate>), ConnectError> {
        let candidates = socket_candidates();
        let mut last_error = None;
        for path in &candidates {
            if !is_socket(path) {
                continue;
            }
            match Self::connect(path.clone()) {
                Ok(connection) => return Ok(connection),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            ConnectError::NotFound(
                candidates
                    .iter()
                    .map(|candidate| candidate.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        }))
    }

    pub fn connect(path: PathBuf) -> Result<(Self, Receiver<DaemonUpdate>), ConnectError> {
        let stream = UnixStream::connect(&path).map_err(|source| ConnectError::Connect {
            path: path.clone(),
            source,
        })?;
        let reader = stream.try_clone().map_err(|source| ConnectError::Connect {
            path: path.clone(),
            source,
        })?;
        let (command_tx, command_rx) = mpsc::channel();
        let (update_tx, update_rx) = async_channel::bounded(1024);
        let pending = Arc::new(Mutex::new(HashMap::new()));

        spawn_writer(stream, command_rx, update_tx.clone(), pending.clone());
        spawn_reader(reader, update_tx.clone(), pending);
        let _ = update_tx.try_send(DaemonUpdate::Connected { path });

        Ok((
            Self {
                commands: command_tx,
            },
            update_rx,
        ))
    }

    pub fn tree(&self) -> Result<(), String> {
        self.send(RequestKind::Tree, json!({"op": "tree"}))
    }

    pub fn new_folder(&self, name: &str) -> Result<(), String> {
        self.send(
            RequestKind::NewFolder,
            json!({"op": "new-folder", "name": name}),
        )
    }

    pub fn new_chat(&self, folder_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::NewChat {
                folder_id: folder_id.to_owned(),
            },
            json!({"op": "new-chat", "folder": folder_id}),
        )
    }

    pub fn chat(&self, chat_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::Chat {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "chat", "chat": chat_id}),
        )
    }

    pub fn messages(&self, chat_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::Messages {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "messages", "chat": chat_id, "limit": 400}),
        )
    }

    pub fn send_message(&self, chat_id: &str, text: &str) -> Result<(), String> {
        self.send(
            RequestKind::Send {
                chat_id: chat_id.to_owned(),
                text: text.to_owned(),
            },
            json!({"op": "send", "chat": chat_id, "text": text}),
        )
    }

    pub fn set_draft(&self, chat_id: &str, text: &str) -> Result<(), String> {
        self.send(
            RequestKind::SetDraft {
                chat_id: chat_id.to_owned(),
                text: text.to_owned(),
            },
            json!({"op": "set-draft", "chat": chat_id, "text": text}),
        )
    }

    fn send(&self, kind: RequestKind, body: Value) -> Result<(), String> {
        self.commands
            .send(Command { kind, body })
            .map_err(|_| "the xd daemon connection is closed".to_owned())
    }
}

fn spawn_writer(
    mut stream: UnixStream,
    commands: mpsc::Receiver<Command>,
    updates: Sender<DaemonUpdate>,
    pending: Arc<Mutex<HashMap<u64, RequestKind>>>,
) {
    thread::Builder::new()
        .name("xd-dev-daemon-writer".into())
        .spawn(move || {
            let mut codec = ProtocolCodec::new();
            while let Ok(command) = commands.recv() {
                let (request_id, encoded) = match codec.encode_request(command.body) {
                    Ok(encoded) => encoded,
                    Err(error) => {
                        disconnect(&updates, error.to_string());
                        return;
                    }
                };
                if let Ok(mut requests) = pending.lock() {
                    requests.insert(request_id, command.kind);
                } else {
                    disconnect(&updates, "daemon request state is unavailable".into());
                    return;
                }
                if let Err(error) = stream.write_all(&encoded) {
                    if let Ok(mut requests) = pending.lock() {
                        requests.remove(&request_id);
                    }
                    disconnect(&updates, format!("could not write to xd daemon: {error}"));
                    return;
                }
            }
        })
        .expect("spawn xd daemon writer");
}

fn spawn_reader(
    stream: UnixStream,
    updates: Sender<DaemonUpdate>,
    pending: Arc<Mutex<HashMap<u64, RequestKind>>>,
) {
    thread::Builder::new()
        .name("xd-dev-daemon-reader".into())
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = Vec::new();
                let read = reader
                    .by_ref()
                    .take((AUTHENTICATED_FRAME_LIMIT + 1) as u64)
                    .read_until(b'\n', &mut line);
                match read {
                    Ok(0) => {
                        disconnect(&updates, "xd daemon closed the connection".into());
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        disconnect(&updates, format!("could not read from xd daemon: {error}"));
                        return;
                    }
                }

                let frame = match ProtocolCodec::decode_line(&line, AUTHENTICATED_FRAME_LIMIT) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => continue,
                    Err(error) => {
                        disconnect(&updates, error.to_string());
                        return;
                    }
                };
                let update = match frame {
                    Frame::Event { name, body } => DaemonUpdate::Event { name, body },
                    Frame::Reply {
                        request_id: Some(request_id),
                        body,
                    } => {
                        let kind = pending
                            .lock()
                            .ok()
                            .and_then(|mut requests| requests.remove(&request_id));
                        let Some(kind) = kind else {
                            continue;
                        };
                        DaemonUpdate::Reply { kind, body }
                    }
                    Frame::Reply {
                        request_id: None, ..
                    } => continue,
                };
                if updates.send_blocking(update).is_err() {
                    return;
                }
            }
        })
        .expect("spawn xd daemon reader");
}

fn disconnect(updates: &Sender<DaemonUpdate>, message: String) {
    let _ = updates.send_blocking(DaemonUpdate::Disconnected { message });
}

fn is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false)
}

pub fn socket_candidates() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("XD_SOCKET").filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(path)];
    }

    let data_home = env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(|home| PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from(".local/share"));

    if let Some(data_name) = env::var_os("XD_DATA_NAME").filter(|name| !name.is_empty()) {
        return vec![data_home.join(data_name).join("daemon.sock")];
    }

    vec![
        data_home.join("xd-nightly/daemon.sock"),
        data_home.join("xd/daemon.sock"),
    ]
}

fn startup_candidates() -> Vec<(PathBuf, String)> {
    let sockets = socket_candidates();
    if env::var_os("XD_SOCKET").is_some() || env::var_os("XD_DATA_NAME").is_some() {
        return sockets
            .into_iter()
            .flat_map(|path| {
                ["xd-nightly", "xd"]
                    .into_iter()
                    .map(move |launcher| (path.clone(), launcher.to_owned()))
            })
            .collect();
    }

    sockets
        .into_iter()
        .zip(["xd-nightly".to_owned(), "xd".to_owned()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn correlates_replies_and_continues_delivering_events() {
        let directory = env::temp_dir().join(format!("xd-dev-daemon-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(
                stream,
                "{{\"ok\":true,\"_xd_request\":{request_id},\"folders\":[],\"chats\":[]}}"
            )
            .unwrap();
            writeln!(stream, "{{\"event\":\"tree\"}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon.tree().unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::Tree,
                ..
            }
        ));
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Event { name, .. } if name == "tree"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }
}
