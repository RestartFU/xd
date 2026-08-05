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
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::model::Attachment;
use crate::protocol::{AUTHENTICATED_FRAME_LIMIT, Frame, ProtocolCodec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestKind {
    Tree,
    AgentCatalog,
    AgentAuth,
    AgentAuthMutation,
    Search {
        query: String,
    },
    DiffRead {
        chat_id: String,
        read: String,
        generation: u64,
    },
    GitStatus {
        chat_id: String,
        generation: u64,
    },
    RepositoryFiles {
        chat_id: String,
        generation: u64,
    },
    RepositoryFile {
        chat_id: String,
        path: String,
        generation: u64,
    },
    GitCommit {
        chat_id: String,
        message: String,
        generation: u64,
    },
    GitPush {
        chat_id: String,
        generation: u64,
    },
    TerminalOpen {
        chat_id: String,
    },
    TerminalList {
        chat_id: String,
    },
    TerminalInput {
        terminal_id: String,
    },
    TerminalResize {
        terminal_id: String,
    },
    TerminalKill {
        terminal_id: String,
    },
    Shortcuts {
        folder_id: String,
    },
    FolderContext {
        folder_id: String,
    },
    SetFolderContext {
        folder_id: String,
        context: Option<String>,
    },
    FolderSettings {
        folder_id: String,
    },
    SetFolderSettings {
        folder_id: String,
    },
    NewFolder {
        name: String,
        repo: Option<String>,
        repo_url: Option<String>,
    },
    NewChat {
        folder_id: String,
        title: String,
    },
    RenameFolder {
        folder_id: String,
        name: String,
    },
    MoveFolder {
        folder_id: String,
        parent_id: Option<String>,
    },
    TrashFolder {
        folder_id: String,
    },
    RenameChat {
        chat_id: String,
        title: String,
    },
    MoveChat {
        chat_id: String,
        folder_id: String,
    },
    DeleteChat {
        chat_id: String,
    },
    Chat {
        chat_id: String,
    },
    Messages {
        chat_id: String,
    },
    Send {
        chat_id: String,
        text: String,
    },
    QueueMutation {
        chat_id: String,
    },
    EditQueue {
        chat_id: String,
        index: usize,
        old_text: String,
        new_text: String,
    },
    Cancel {
        chat_id: String,
    },
    SetOption {
        chat_id: String,
    },
    SetShortcuts,
    RemoveWorktree {
        chat_id: String,
    },
    SetDraft {
        chat_id: String,
        text: String,
        attachment_generation: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DaemonUpdate {
    Connected {
        path: PathBuf,
    },
    Reply {
        kind: RequestKind,
        body: Map<String, Value>,
        attachments: Option<Vec<Attachment>>,
    },
    Event {
        name: String,
        body: Map<String, Value>,
        attachments: Option<Vec<Attachment>>,
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
    #[error("could not start the xd-dev Rust daemon ({0})")]
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
                .args(["serve", "--socket", &socket])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    failures.push(format!("{} is not installed", launcher.display()));
                    continue;
                }
                Err(error) => {
                    failures.push(format!("cannot launch {}: {error}", launcher.display()));
                    continue;
                }
            };

            for _ in 0..50 {
                if let Ok((daemon, updates)) = Self::connect(path.clone()) {
                    return Ok((daemon, updates, Some(StartedDaemon { child })));
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        failures.push(format!("{} exited with {status}", launcher.display()));
                        break;
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(100)),
                    Err(error) => {
                        failures.push(format!("cannot inspect {}: {error}", launcher.display()));
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
                "{} did not open {} within five seconds",
                launcher.display(),
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

    pub fn agent_catalog(&self) -> Result<(), String> {
        self.send(RequestKind::AgentCatalog, json!({"op": "agent-catalog"}))
    }

    pub fn agent_auth(&self) -> Result<(), String> {
        self.send(RequestKind::AgentAuth, json!({"op": "agent-auth"}))
    }

    pub fn agent_auth_action(
        &self,
        operation: &str,
        provider: &str,
        input: Option<&str>,
    ) -> Result<(), String> {
        let mut body = json!({"op": operation, "provider": provider});
        if let Some(input) = input {
            body["input"] = Value::String(input.to_owned());
        }
        self.send(RequestKind::AgentAuthMutation, body)
    }

    pub fn shortcuts(&self, folder_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::Shortcuts {
                folder_id: folder_id.to_owned(),
            },
            json!({"op": "shortcuts", "folder": folder_id}),
        )
    }

    pub fn folder_context(&self, folder_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::FolderContext {
                folder_id: folder_id.to_owned(),
            },
            json!({"op": "folder-context", "folder": folder_id}),
        )
    }

    pub fn set_folder_context(&self, folder_id: &str, context: Option<&str>) -> Result<(), String> {
        self.send(
            RequestKind::SetFolderContext {
                folder_id: folder_id.to_owned(),
                context: context.map(str::to_owned),
            },
            json!({
                "op": "set-folder-context",
                "folder": folder_id,
                "context": context,
            }),
        )
    }

    pub fn folder_settings(&self, folder_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::FolderSettings {
                folder_id: folder_id.to_owned(),
            },
            json!({"op": "folder-settings", "folder": folder_id}),
        )
    }

    pub fn set_folder_settings(
        &self,
        folder_id: &str,
        backend: Option<&str>,
        model: Option<&str>,
        workdir: Option<&str>,
        repo: Option<&str>,
    ) -> Result<(), String> {
        self.send(
            RequestKind::SetFolderSettings {
                folder_id: folder_id.to_owned(),
            },
            json!({
                "op": "set-folder-settings",
                "folder": folder_id,
                "backend": backend,
                "model": model,
                "workdir": workdir,
                "repo": repo,
            }),
        )
    }

    pub fn set_shortcuts(
        &self,
        folder_id: Option<&str>,
        shortcuts: &[String],
    ) -> Result<(), String> {
        let mut body = json!({"op": "set-shortcuts", "shortcuts": shortcuts});
        if let Some(folder_id) = folder_id {
            body["folder"] = Value::String(folder_id.to_owned());
        }
        self.send(RequestKind::SetShortcuts, body)
    }

    pub fn new_folder(
        &self,
        name: &str,
        repo: Option<&str>,
        repo_url: Option<&str>,
    ) -> Result<(), String> {
        let mut body = json!({"op": "new-folder", "name": name});
        if let Some(repo) = repo {
            body["repo"] = Value::String(repo.to_owned());
        }
        if let Some(repo_url) = repo_url {
            body["repo_url"] = Value::String(repo_url.to_owned());
        }
        self.send(
            RequestKind::NewFolder {
                name: name.to_owned(),
                repo: repo.map(str::to_owned),
                repo_url: repo_url.map(str::to_owned),
            },
            body,
        )
    }

    pub fn new_chat(&self, folder_id: &str, title: &str) -> Result<(), String> {
        self.send(
            RequestKind::NewChat {
                folder_id: folder_id.to_owned(),
                title: title.to_owned(),
            },
            json!({"op": "new-chat", "folder": folder_id, "title": title}),
        )
    }

    pub fn rename_folder(&self, folder_id: &str, name: &str) -> Result<(), String> {
        self.send(
            RequestKind::RenameFolder {
                folder_id: folder_id.to_owned(),
                name: name.to_owned(),
            },
            json!({"op": "rename-folder", "folder": folder_id, "name": name}),
        )
    }

    pub fn move_folder(&self, folder_id: &str, parent_id: Option<&str>) -> Result<(), String> {
        let mut body = json!({"op": "move-folder", "folder": folder_id});
        if let Some(parent_id) = parent_id {
            body["parent"] = Value::String(parent_id.to_owned());
        }
        self.send(
            RequestKind::MoveFolder {
                folder_id: folder_id.to_owned(),
                parent_id: parent_id.map(str::to_owned),
            },
            body,
        )
    }

    pub fn trash_folder(&self, folder_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::TrashFolder {
                folder_id: folder_id.to_owned(),
            },
            json!({"op": "trash-folder", "folder": folder_id}),
        )
    }

    pub fn rename_chat(&self, chat_id: &str, title: &str) -> Result<(), String> {
        self.send(
            RequestKind::RenameChat {
                chat_id: chat_id.to_owned(),
                title: title.to_owned(),
            },
            json!({"op": "rename-chat", "chat": chat_id, "title": title}),
        )
    }

    pub fn move_chat(&self, chat_id: &str, folder_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::MoveChat {
                chat_id: chat_id.to_owned(),
                folder_id: folder_id.to_owned(),
            },
            json!({"op": "move-chat", "chat": chat_id, "folder": folder_id}),
        )
    }

    pub fn delete_chat(&self, chat_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::DeleteChat {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "delete-chat", "chat": chat_id}),
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

    pub fn search(&self, query: &str) -> Result<(), String> {
        self.send(
            RequestKind::Search {
                query: query.to_owned(),
            },
            json!({"op": "search", "query": query}),
        )
    }

    pub fn diff_read(
        &self,
        chat_id: &str,
        read: &str,
        base: Option<&str>,
        generation: u64,
    ) -> Result<(), String> {
        let mut body = json!({"op": "diff-read", "chat": chat_id, "read": read});
        if let Some(base) = base {
            body["base"] = Value::String(base.to_owned());
        }
        self.send(
            RequestKind::DiffRead {
                chat_id: chat_id.to_owned(),
                read: read.to_owned(),
                generation,
            },
            body,
        )
    }

    pub fn git_status(&self, chat_id: &str, generation: u64) -> Result<(), String> {
        self.send(
            RequestKind::GitStatus {
                chat_id: chat_id.to_owned(),
                generation,
            },
            json!({"op": "git-status", "chat": chat_id}),
        )
    }

    pub fn repository_files(&self, chat_id: &str, generation: u64) -> Result<(), String> {
        self.send(
            RequestKind::RepositoryFiles {
                chat_id: chat_id.to_owned(),
                generation,
            },
            json!({"op": "repository-files", "chat": chat_id}),
        )
    }

    pub fn repository_file(
        &self,
        chat_id: &str,
        path: &str,
        generation: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::RepositoryFile {
                chat_id: chat_id.to_owned(),
                path: path.to_owned(),
                generation,
            },
            json!({"op": "repository-file", "chat": chat_id, "path": path}),
        )
    }

    pub fn git_commit(&self, chat_id: &str, message: &str, generation: u64) -> Result<(), String> {
        self.send(
            RequestKind::GitCommit {
                chat_id: chat_id.to_owned(),
                message: message.to_owned(),
                generation,
            },
            json!({"op": "git-commit", "chat": chat_id, "message": message}),
        )
    }

    pub fn git_push(&self, chat_id: &str, generation: u64) -> Result<(), String> {
        self.send(
            RequestKind::GitPush {
                chat_id: chat_id.to_owned(),
                generation,
            },
            json!({"op": "git-push", "chat": chat_id}),
        )
    }

    pub fn terminal_open(&self, chat_id: &str, columns: usize, rows: usize) -> Result<(), String> {
        self.send(
            RequestKind::TerminalOpen { chat_id: chat_id.to_owned() },
            json!({"op": "terminal-open", "chat": chat_id, "columns": columns, "rows": rows, "reuse": true}),
        )
    }

    pub fn terminal_list(&self, chat_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::TerminalList {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "terminal-list", "chat": chat_id}),
        )
    }

    pub fn terminal_input(&self, terminal_id: &str, data: &[u8]) -> Result<(), String> {
        self.send(
            RequestKind::TerminalInput {
                terminal_id: terminal_id.to_owned(),
            },
            json!({"op": "terminal-input", "terminal": terminal_id, "data": STANDARD.encode(data)}),
        )
    }

    pub fn terminal_resize(
        &self,
        terminal_id: &str,
        columns: usize,
        rows: usize,
    ) -> Result<(), String> {
        self.send(
            RequestKind::TerminalResize { terminal_id: terminal_id.to_owned() },
            json!({"op": "terminal-resize", "terminal": terminal_id, "columns": columns, "rows": rows}),
        )
    }

    pub fn terminal_kill(&self, terminal_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::TerminalKill {
                terminal_id: terminal_id.to_owned(),
            },
            json!({"op": "terminal-kill", "terminal": terminal_id}),
        )
    }

    pub fn send_message(
        &self,
        chat_id: &str,
        text: &str,
        attachments: &[Attachment],
    ) -> Result<(), String> {
        let attachments = attachments
            .iter()
            .map(|attachment| {
                json!({
                    "name": attachment.name,
                    "mime": attachment.mime,
                    "data": attachment.data,
                })
            })
            .collect::<Vec<_>>();
        let mut body = json!({"op": "send", "chat": chat_id, "text": text});
        if !attachments.is_empty() {
            body["attachments"] = Value::Array(attachments);
        }
        self.send(
            RequestKind::Send {
                chat_id: chat_id.to_owned(),
                text: text.to_owned(),
            },
            body,
        )
    }

    pub fn drop_queue(&self, chat_id: &str, index: usize) -> Result<(), String> {
        self.send(
            RequestKind::QueueMutation {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "drop-queue", "chat": chat_id, "index": index}),
        )
    }

    pub fn edit_queue(
        &self,
        chat_id: &str,
        index: usize,
        old_text: &str,
        new_text: &str,
    ) -> Result<(), String> {
        self.send(
            RequestKind::EditQueue {
                chat_id: chat_id.to_owned(),
                index,
                old_text: old_text.to_owned(),
                new_text: new_text.to_owned(),
            },
            json!({
                "op": "edit-queue",
                "chat": chat_id,
                "index": index,
                "old_text": old_text,
                "text": new_text,
            }),
        )
    }

    pub fn steer_queue(&self, chat_id: &str, index: usize, text: &str) -> Result<(), String> {
        self.send(
            RequestKind::QueueMutation {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "steer-queue", "chat": chat_id, "index": index, "text": text}),
        )
    }

    pub fn cancel(&self, chat_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::Cancel {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "cancel", "chat": chat_id}),
        )
    }

    pub fn set_draft(
        &self,
        chat_id: &str,
        text: &str,
        attachments: Option<&[Attachment]>,
        attachment_generation: Option<u64>,
    ) -> Result<(), String> {
        let mut body = json!({"op": "set-draft", "chat": chat_id, "text": text});
        if let Some(attachments) = attachments {
            body["attachments"] = Value::Array(
                attachments
                    .iter()
                    .map(|attachment| {
                        json!({
                            "name": attachment.name,
                            "mime": attachment.mime,
                            "data": attachment.data,
                        })
                    })
                    .collect(),
            );
        }
        self.send(
            RequestKind::SetDraft {
                chat_id: chat_id.to_owned(),
                text: text.to_owned(),
                attachment_generation,
            },
            body,
        )
    }

    pub fn set_new_worktree(&self, chat_id: &str, enabled: bool) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "new-worktree",
                "value": if enabled { "true" } else { "false" },
            }),
        )
    }

    pub fn set_model(&self, chat_id: &str, backend: &str, model: &str) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "model",
                "backend": backend,
                "value": model,
            }),
        )
    }

    pub fn set_effort(&self, chat_id: &str, effort: &str) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "effort",
                "value": effort,
            }),
        )
    }

    pub fn set_access(&self, chat_id: &str, access: &str) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "access",
                "value": access,
            }),
        )
    }

    pub fn set_plan(&self, chat_id: &str, enabled: bool) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "plan",
                "value": if enabled { "true" } else { "false" },
            }),
        )
    }

    pub fn set_workspace(&self, chat_id: &str, path: &str) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "workspace",
                "value": path,
            }),
        )
    }

    pub fn remove_worktree(&self, chat_id: &str, path: &str) -> Result<(), String> {
        self.send(
            RequestKind::RemoveWorktree {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "remove-worktree", "chat": chat_id, "worktree": path}),
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
                    Frame::Event { name, mut body } => {
                        let attachments = take_draft_attachments(&mut body);
                        DaemonUpdate::Event {
                            name,
                            body,
                            attachments,
                        }
                    }
                    Frame::Reply {
                        request_id: Some(request_id),
                        mut body,
                    } => {
                        let kind = pending
                            .lock()
                            .ok()
                            .and_then(|mut requests| requests.remove(&request_id));
                        let Some(kind) = kind else {
                            continue;
                        };
                        let attachments = take_draft_attachments(&mut body);
                        DaemonUpdate::Reply {
                            kind,
                            body,
                            attachments,
                        }
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

fn take_draft_attachments(body: &mut Map<String, Value>) -> Option<Vec<Attachment>> {
    let value = body.remove("draft_attachments")?;
    let values = value.as_array()?;
    Some(values.iter().filter_map(Attachment::from_value).collect())
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

    socket_candidates_for(
        data_home,
        None,
        env::var_os("XD_DATA_NAME").filter(|name| !name.is_empty()),
    )
}

fn socket_candidates_for(
    data_home: PathBuf,
    explicit_socket: Option<std::ffi::OsString>,
    data_name: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    if let Some(socket) = explicit_socket.filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(socket)];
    }
    let data_name = data_name.unwrap_or_else(|| "xd-dev".into());
    vec![data_home.join(data_name).join("daemon.sock")]
}

fn startup_candidates() -> Vec<(PathBuf, PathBuf)> {
    let launchers = launcher_candidates();
    socket_candidates()
        .into_iter()
        .flat_map(|path| {
            launchers
                .iter()
                .cloned()
                .map(move |launcher| (path.clone(), launcher))
        })
        .collect()
}

fn launcher_candidates() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("XD_DAEMON_EXECUTABLE").filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(path)];
    }
    let mut candidates = Vec::new();
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        let sibling = parent.join("xd-daemon-dev");
        if sibling.is_file() {
            candidates.push(sibling);
        }
    }
    candidates.push(PathBuf::from("xd-daemon-dev"));
    candidates
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

    #[test]
    fn edits_queued_messages_with_the_original_text_guard() {
        let directory = env::temp_dir().join(format!("xd-dev-edit-queue-{}", std::process::id()));
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
            assert_eq!(request["op"], "edit-queue");
            assert_eq!(request["chat"], "chat-1");
            assert_eq!(request["index"], 2);
            assert_eq!(request["old_text"], "before");
            assert_eq!(request["text"], "after");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon.edit_queue("chat-1", 2, "before", "after").unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::EditQueue {
                    chat_id,
                    index: 2,
                    old_text,
                    new_text,
                },
                ..
            } if chat_id == "chat-1" && old_text == "before" && new_text == "after"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn dev_socket_is_isolated_from_production_state() {
        assert_eq!(
            socket_candidates_for(PathBuf::from("/data"), None, None),
            vec![PathBuf::from("/data/xd-dev/daemon.sock")]
        );
        assert_eq!(
            socket_candidates_for(
                PathBuf::from("/data"),
                Some("/run/custom.sock".into()),
                None,
            ),
            vec![PathBuf::from("/run/custom.sock")]
        );
        assert_eq!(
            socket_candidates_for(PathBuf::from("/data"), None, Some("preview".into())),
            vec![PathBuf::from("/data/preview/daemon.sock")]
        );
    }

    #[test]
    fn decodes_synchronized_previews_before_the_ui_thread() {
        let mut body = json!({
            "draft": "look",
            "draft_attachments": [{
                "name": "screen.png",
                "mime": "image/png",
                "data": "iVBORw0KGgo="
            }]
        })
        .as_object()
        .unwrap()
        .clone();

        let attachments = take_draft_attachments(&mut body).unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].preview.bytes, b"\x89PNG\r\n\x1a\n");
        assert!(!body.contains_key("draft_attachments"));
    }
}
