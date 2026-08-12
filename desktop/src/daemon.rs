use std::{
    collections::HashMap,
    env,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use async_channel::{Receiver, Sender};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::channel;
use crate::local_socket::{UnixStream, path_is_socket};
use crate::model::Attachment;
use crate::protocol::{AUTHENTICATED_FRAME_LIMIT, Frame, ProtocolCodec};

const MESSAGE_PAGE_SIZE: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageCursor {
    Tail,
    Before(i64),
    After(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewSessionWorktree {
    New,
    Existing(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestKind {
    Tree,
    AgentCatalog,
    AgentAuth,
    AgentAuthMutation,
    AgentClis,
    DaemonUpdate {
        action: String,
    },
    AgentSecrets {
        folder_id: Option<String>,
    },
    SetAgentSecrets {
        folder_id: Option<String>,
    },
    Devices,
    PeerPairing,
    PairRemote,
    HelloRemote,
    RenameDevice {
        device_id: String,
    },
    RevokeDevice {
        device_id: String,
    },
    VoiceModel {
        chat_id: String,
    },
    VoiceMutation {
        chat_id: String,
        token: String,
        operation: String,
    },
    Search {
        query: String,
    },
    ListDirectory {
        path: Option<String>,
        generation: u64,
    },
    ImageRead {
        path: String,
    },
    WorkflowStatus {
        marker: String,
    },
    DiffRead {
        chat_id: String,
        read: String,
        path: Option<String>,
        generation: u64,
    },
    GitStatus {
        chat_id: String,
        generation: u64,
    },
    GitState {
        chat_id: String,
    },
    GitDraft {
        chat_id: String,
        kind: String,
        request: String,
        generation: u64,
    },
    GitPullRequestStatus {
        chat_id: String,
        generation: u64,
    },
    GitPullRequestCreate {
        chat_id: String,
        generation: u64,
    },
    FileBrowseList {
        chat_id: String,
        path: String,
        generation: u64,
    },
    FileBrowseRead {
        chat_id: String,
        path: String,
        generation: u64,
    },
    FileBrowseWrite {
        chat_id: String,
        path: String,
        content: String,
        generation: u64,
    },
    // The tree and its tabs run on their own generation. They outlive the diff
    // panel being opened and closed, and sharing its counter would have every
    // toggle of that panel discard listings the tree still wants.
    FileTreeList {
        chat_id: String,
        path: String,
        generation: u64,
    },
    FileTabRead {
        chat_id: String,
        path: String,
        generation: u64,
    },
    FileTabWrite {
        chat_id: String,
        path: String,
        content: String,
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
        reuse: bool,
        agent: Option<String>,
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
        folder_id: Option<String>,
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
        workdir: Option<String>,
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
        folder_id: Option<String>,
    },
    DeleteChat {
        chat_id: String,
    },
    Chat {
        chat_id: String,
    },
    Messages {
        chat_id: String,
        cursor: MessageCursor,
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
    SetShortcuts {
        folder_id: Option<String>,
    },
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
    #[error("no legacy xd socket was found (looked in {0})")]
    NotFound(String),
    #[error("could not connect to legacy xd socket at {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not start the xd state host ({0})")]
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

pub struct StartedHost {
    child: Child,
}

impl Drop for StartedHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl DaemonHandle {
    pub fn start_local() -> Result<(Self, Receiver<DaemonUpdate>, StartedHost), ConnectError> {
        let mut failures = Vec::new();
        let data = data_root();
        for launcher in launcher_candidates() {
            let mut command = ProcessCommand::new(&launcher);
            command
                .arg("stdio")
                .arg("--data")
                .arg(&data)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            channel::configure_host(&mut command, &launcher);
            match Self::connect_command(command, data.clone()) {
                Ok(connection) => return Ok(connection),
                Err(error) => failures.push(format!("{}: {error}", launcher.display())),
            }
        }

        Err(ConnectError::Start(failures.join("; ")))
    }

    pub fn connect_command(
        mut command: ProcessCommand,
        identity: PathBuf,
    ) -> Result<(Self, Receiver<DaemonUpdate>, StartedHost), ConnectError> {
        let mut child = command
            .spawn()
            .map_err(|error| ConnectError::Start(error.to_string()))?;
        let writer = child
            .stdin
            .take()
            .ok_or_else(|| ConnectError::Start("host stdin is unavailable".into()))?;
        let reader = child
            .stdout
            .take()
            .ok_or_else(|| ConnectError::Start("host stdout is unavailable".into()))?;
        let (handle, updates) = Self::connect_io(reader, writer, identity);
        Ok((handle, updates, StartedHost { child }))
    }

    fn connect_io(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        identity: PathBuf,
    ) -> (Self, Receiver<DaemonUpdate>) {
        let (command_tx, command_rx) = mpsc::channel();
        let (update_tx, update_rx) = async_channel::bounded(1024);
        let pending = Arc::new(Mutex::new(HashMap::new()));

        spawn_writer(writer, command_rx, update_tx.clone(), pending.clone());
        spawn_reader(reader, update_tx.clone(), pending);
        let _ = update_tx.try_send(DaemonUpdate::Connected { path: identity });

        (
            Self {
                commands: command_tx,
            },
            update_rx,
        )
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
        Ok(Self::connect_io(reader, stream, path))
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

    pub fn agent_clis(&self) -> Result<(), String> {
        self.send(RequestKind::AgentClis, json!({"op": "agent-clis"}))
    }

    pub fn daemon_update(&self, action: &str) -> Result<(), String> {
        self.send(
            RequestKind::DaemonUpdate {
                action: action.to_owned(),
            },
            json!({"op": "daemon-update", "action": action}),
        )
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

    pub fn agent_secrets(&self, folder_id: Option<&str>) -> Result<(), String> {
        let mut body = json!({"op": "agent-secrets"});
        if let Some(folder_id) = folder_id {
            body["folder"] = Value::String(folder_id.to_owned());
        }
        self.send(
            RequestKind::AgentSecrets {
                folder_id: folder_id.map(str::to_owned),
            },
            body,
        )
    }

    pub fn set_agent_secrets(
        &self,
        folder_id: Option<&str>,
        entries: &[(String, Option<String>)],
    ) -> Result<(), String> {
        let entries = entries
            .iter()
            .map(|(name, value)| match value {
                Some(value) => json!({"name": name, "value": value}),
                None => json!({"name": name}),
            })
            .collect::<Vec<_>>();
        let mut body = json!({"op": "set-agent-secrets", "entries": entries});
        if let Some(folder_id) = folder_id {
            body["folder"] = Value::String(folder_id.to_owned());
        }
        self.send(
            RequestKind::SetAgentSecrets {
                folder_id: folder_id.map(str::to_owned),
            },
            body,
        )
    }

    pub fn devices(&self) -> Result<(), String> {
        self.send(RequestKind::Devices, json!({"op": "devices"}))
    }

    pub fn peer_pairing(&self) -> Result<(), String> {
        self.send(RequestKind::PeerPairing, json!({"op": "peer-pairing"}))
    }

    pub fn pair_remote(&self, code: &str, name: &str) -> Result<(), String> {
        self.send(
            RequestKind::PairRemote,
            json!({"op": "pair", "code": code, "name": name}),
        )
    }

    pub fn hello_remote(&self, token: &str) -> Result<(), String> {
        self.send(
            RequestKind::HelloRemote,
            json!({"op": "hello", "token": token}),
        )
    }

    pub fn rename_device(&self, device_id: &str, name: &str) -> Result<(), String> {
        self.send(
            RequestKind::RenameDevice {
                device_id: device_id.to_owned(),
            },
            json!({"op": "rename-device", "device": device_id, "name": name}),
        )
    }

    pub fn revoke_device(&self, device_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::RevokeDevice {
                device_id: device_id.to_owned(),
            },
            json!({"op": "revoke-device", "device": device_id}),
        )
    }

    pub fn voice_model(&self, chat_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::VoiceModel {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "voice-model", "chat": chat_id}),
        )
    }

    pub fn voice_action(
        &self,
        operation: &str,
        chat_id: &str,
        token: &str,
        audio: Option<&[u8]>,
    ) -> Result<(), String> {
        let mut body = json!({"op": operation, "request": token});
        if operation != "voice-cancel" {
            body["chat"] = Value::String(chat_id.to_owned());
        }
        if let Some(audio) = audio {
            body["audio"] = Value::String(STANDARD.encode(audio));
        }
        self.send(
            RequestKind::VoiceMutation {
                chat_id: chat_id.to_owned(),
                token: token.to_owned(),
                operation: operation.to_owned(),
            },
            body,
        )
    }

    pub fn shortcuts(&self, folder_id: Option<&str>) -> Result<(), String> {
        let mut body = json!({"op": "shortcuts"});
        if let Some(folder_id) = folder_id {
            body["folder"] = Value::String(folder_id.to_owned());
        }
        self.send(
            RequestKind::Shortcuts {
                folder_id: folder_id.map(str::to_owned),
            },
            body,
        )
    }

    pub fn workflow_status(&self, marker: &str) -> Result<(), String> {
        self.send(
            RequestKind::WorkflowStatus {
                marker: marker.to_owned(),
            },
            json!({"op": "workflow-status", "text": marker}),
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
        self.send(
            RequestKind::SetShortcuts {
                folder_id: folder_id.map(str::to_owned),
            },
            body,
        )
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

    pub fn new_chat(
        &self,
        folder_id: &str,
        title: &str,
        workdir: Option<&str>,
    ) -> Result<(), String> {
        self.send_new_chat(folder_id, title, workdir, None, None)
    }

    pub fn new_chat_with_backend(
        &self,
        folder_id: &str,
        title: &str,
        workdir: Option<&str>,
        backend: &str,
    ) -> Result<(), String> {
        self.send_new_chat(folder_id, title, workdir, Some(backend), None)
    }

    pub fn new_chat_with_backend_in_worktree(
        &self,
        folder_id: &str,
        title: &str,
        backend: &str,
        worktree: &NewSessionWorktree,
    ) -> Result<(), String> {
        self.send_new_chat(folder_id, title, None, Some(backend), Some(worktree))
    }

    fn send_new_chat(
        &self,
        folder_id: &str,
        title: &str,
        workdir: Option<&str>,
        backend: Option<&str>,
        worktree: Option<&NewSessionWorktree>,
    ) -> Result<(), String> {
        let mut body = json!({"op": "new-chat", "folder": folder_id, "title": title});
        if let Some(workdir) = workdir {
            body["workdir"] = Value::String(workdir.to_owned());
        }
        if let Some(backend) = backend {
            body["backend"] = Value::String(backend.to_owned());
        }
        if let Some(worktree) = worktree {
            body["worktree"] = match worktree {
                NewSessionWorktree::New => json!({"kind": "new"}),
                NewSessionWorktree::Existing(path) => {
                    json!({"kind": "existing", "path": path})
                }
            };
        }
        self.send(
            RequestKind::NewChat {
                folder_id: folder_id.to_owned(),
                title: title.to_owned(),
                workdir: workdir.map(str::to_owned),
            },
            body,
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

    pub fn reorder_folder(
        &self,
        folder_id: &str,
        anchor_id: &str,
        after: bool,
    ) -> Result<(), String> {
        let mut body = json!({"op": "move-folder", "folder": folder_id});
        body[if after { "after" } else { "before" }] = Value::String(anchor_id.to_owned());
        self.send(
            RequestKind::MoveFolder {
                folder_id: folder_id.to_owned(),
                parent_id: None,
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
                folder_id: Some(folder_id.to_owned()),
            },
            json!({"op": "move-chat", "chat": chat_id, "folder": folder_id}),
        )
    }

    pub fn reorder_chat(&self, chat_id: &str, anchor_id: &str, after: bool) -> Result<(), String> {
        let mut body = json!({"op": "move-chat", "chat": chat_id});
        body[if after { "after" } else { "before" }] = Value::String(anchor_id.to_owned());
        self.send(
            RequestKind::MoveChat {
                chat_id: chat_id.to_owned(),
                folder_id: None,
            },
            body,
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

    pub fn messages(&self, chat_id: &str, cursor: MessageCursor) -> Result<(), String> {
        let mut body = json!({
            "op": "messages",
            "chat": chat_id,
            "limit": MESSAGE_PAGE_SIZE,
        });
        match cursor {
            MessageCursor::Tail => {}
            MessageCursor::Before(id) => body["before"] = Value::from(id),
            MessageCursor::After(id) => body["after"] = Value::from(id),
        }
        self.send(
            RequestKind::Messages {
                chat_id: chat_id.to_owned(),
                cursor,
            },
            body,
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

    pub fn list_directory(&self, path: Option<&str>, generation: u64) -> Result<(), String> {
        let mut body = json!({"op": "list-dir"});
        if let Some(path) = path {
            body["path"] = Value::String(path.to_owned());
        }
        self.send(
            RequestKind::ListDirectory {
                path: path.map(str::to_owned),
                generation,
            },
            body,
        )
    }

    pub fn image_read(&self, path: &str) -> Result<(), String> {
        self.send(
            RequestKind::ImageRead {
                path: path.to_owned(),
            },
            json!({"op": "image-read", "path": path, "preview": true}),
        )
    }

    pub fn diff_read(
        &self,
        chat_id: &str,
        read: &str,
        base: Option<&str>,
        path: Option<&str>,
        generation: u64,
    ) -> Result<(), String> {
        let mut body = json!({"op": "diff-read", "chat": chat_id, "read": read});
        if let Some(base) = base {
            body["base"] = Value::String(base.to_owned());
        }
        if let Some(path) = path {
            body["path"] = Value::String(path.to_owned());
        }
        self.send(
            RequestKind::DiffRead {
                chat_id: chat_id.to_owned(),
                read: read.to_owned(),
                path: path.map(str::to_owned),
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

    pub fn git_state(&self, chat_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::GitState {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "git-state", "chat": chat_id}),
        )
    }

    pub fn git_draft(
        &self,
        chat_id: &str,
        kind: &str,
        request: &str,
        backend: Option<&str>,
        model: Option<&str>,
        generation: u64,
    ) -> Result<(), String> {
        let mut body = json!({
            "op": "git-draft",
            "chat": chat_id,
            "kind": kind,
            "request": request,
        });
        if let Some(backend) = backend {
            body["backend"] = Value::String(backend.to_owned());
        }
        if let Some(model) = model {
            body["model"] = Value::String(model.to_owned());
        }
        self.send(
            RequestKind::GitDraft {
                chat_id: chat_id.to_owned(),
                kind: kind.to_owned(),
                request: request.to_owned(),
                generation,
            },
            body,
        )
    }

    pub fn git_pull_request_status(&self, chat_id: &str, generation: u64) -> Result<(), String> {
        self.send(
            RequestKind::GitPullRequestStatus {
                chat_id: chat_id.to_owned(),
                generation,
            },
            json!({"op": "git-pr-status", "chat": chat_id}),
        )
    }

    pub fn git_create_pull_request(
        &self,
        chat_id: &str,
        title: &str,
        body: &str,
        generation: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::GitPullRequestCreate {
                chat_id: chat_id.to_owned(),
                generation,
            },
            json!({
                "op": "git-pr-create",
                "chat": chat_id,
                "title": title,
                "body": body,
            }),
        )
    }

    pub fn file_browse_list(
        &self,
        chat_id: &str,
        path: &str,
        generation: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::FileBrowseList {
                chat_id: chat_id.to_owned(),
                path: path.to_owned(),
                generation,
            },
            json!({"op": "file-browse", "chat": chat_id, "action": "list", "path": path}),
        )
    }

    pub fn file_browse_read(
        &self,
        chat_id: &str,
        path: &str,
        generation: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::FileBrowseRead {
                chat_id: chat_id.to_owned(),
                path: path.to_owned(),
                generation,
            },
            json!({"op": "file-browse", "chat": chat_id, "action": "read", "path": path}),
        )
    }

    pub fn file_tree_list(&self, chat_id: &str, path: &str, generation: u64) -> Result<(), String> {
        self.send(
            RequestKind::FileTreeList {
                chat_id: chat_id.to_owned(),
                path: path.to_owned(),
                generation,
            },
            json!({"op": "file-browse", "chat": chat_id, "action": "list", "path": path}),
        )
    }

    pub fn file_tab_read(&self, chat_id: &str, path: &str, generation: u64) -> Result<(), String> {
        self.send(
            RequestKind::FileTabRead {
                chat_id: chat_id.to_owned(),
                path: path.to_owned(),
                generation,
            },
            json!({"op": "file-browse", "chat": chat_id, "action": "read", "path": path}),
        )
    }

    pub fn file_tab_write(
        &self,
        chat_id: &str,
        path: &str,
        original: &str,
        content: &str,
        generation: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::FileTabWrite {
                chat_id: chat_id.to_owned(),
                path: path.to_owned(),
                content: content.to_owned(),
                generation,
            },
            json!({
                "op": "file-browse",
                "chat": chat_id,
                "action": "write",
                "path": path,
                "original": original,
                "content": content,
            }),
        )
    }

    pub fn file_browse_write(
        &self,
        chat_id: &str,
        path: &str,
        original: &str,
        content: &str,
        generation: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::FileBrowseWrite {
                chat_id: chat_id.to_owned(),
                path: path.to_owned(),
                content: content.to_owned(),
                generation,
            },
            json!({
                "op": "file-browse",
                "chat": chat_id,
                "action": "write",
                "path": path,
                "original": original,
                "content": content,
            }),
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

    pub fn terminal_open(
        &self,
        chat_id: &str,
        columns: usize,
        rows: usize,
        reuse: bool,
        foreground: u32,
        background: u32,
    ) -> Result<(), String> {
        self.send(
            RequestKind::TerminalOpen {
                chat_id: chat_id.to_owned(),
                reuse,
                agent: None,
            },
            json!({
                "op": "terminal-open",
                "chat": chat_id,
                "columns": columns,
                "rows": rows,
                "reuse": reuse,
                "foreground": foreground,
                "background": background,
            }),
        )
    }

    pub fn terminal_open_agent(
        &self,
        chat_id: &str,
        columns: usize,
        rows: usize,
        reuse: bool,
        agent: &str,
        allow_all_permissions: bool,
        foreground: u32,
        background: u32,
    ) -> Result<(), String> {
        self.send(
            RequestKind::TerminalOpen {
                chat_id: chat_id.to_owned(),
                reuse,
                agent: Some(agent.to_owned()),
            },
            json!({
                "op": "terminal-open-agent",
                "chat": chat_id,
                "columns": columns,
                "rows": rows,
                "reuse": reuse,
                "agent": agent,
                "allow_all_permissions": allow_all_permissions,
                "foreground": foreground,
                "background": background,
            }),
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

    pub fn terminal_paste_image(
        &self,
        terminal_id: &str,
        attachment: &Attachment,
    ) -> Result<(), String> {
        self.send(
            RequestKind::TerminalInput {
                terminal_id: terminal_id.to_owned(),
            },
            json!({
                "op": "terminal-paste-image",
                "terminal": terminal_id,
                "attachments": [{
                    "name": attachment.name,
                    "mime": attachment.mime,
                    "data": attachment.data,
                }],
            }),
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
        worktree_backend: Option<&str>,
        worktree_model: Option<&str>,
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
        let mut body = json!({
            "op": "send",
            "chat": chat_id,
            "text": text,
            "generate_worktree_name": true,
        });
        if !attachments.is_empty() {
            body["attachments"] = Value::Array(attachments);
        }
        if let Some(backend) = worktree_backend {
            body["worktree_backend"] = Value::String(backend.to_owned());
        }
        if let Some(model) = worktree_model {
            body["worktree_model"] = Value::String(model.to_owned());
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

    pub fn queue_message(&self, chat_id: &str, text: &str) -> Result<(), String> {
        self.send(
            RequestKind::QueueMutation {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "queue", "chat": chat_id, "text": text}),
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
                // The daemon guards the edit against this key, hyphenated.
                "old-text": old_text,
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

    pub fn reorder_queue(
        &self,
        chat_id: &str,
        source: usize,
        source_text: &str,
        anchor: usize,
        anchor_text: &str,
        after: bool,
    ) -> Result<(), String> {
        self.send(
            RequestKind::QueueMutation {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "reorder-queue",
                "chat": chat_id,
                "source": source,
                "source-text": source_text,
                "anchor": anchor,
                "anchor-text": anchor_text,
                "after": after,
            }),
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

    pub fn set_fast(&self, chat_id: &str, enabled: bool) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "fast",
                "value": if enabled { "true" } else { "false" },
            }),
        )
    }

    pub fn set_claude_mode(&self, chat_id: &str, enabled: bool) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "claude-mode",
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
            .map_err(|_| "the xd state connection is closed".to_owned())
    }
}

fn spawn_writer(
    mut stream: impl Write + Send + 'static,
    commands: mpsc::Receiver<Command>,
    updates: Sender<DaemonUpdate>,
    pending: Arc<Mutex<HashMap<u64, RequestKind>>>,
) {
    thread::Builder::new()
        .name("xd-host-writer".into())
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
                    disconnect(&updates, "host request state is unavailable".into());
                    return;
                }
                if let Err(error) = stream.write_all(&encoded) {
                    if let Ok(mut requests) = pending.lock() {
                        requests.remove(&request_id);
                    }
                    disconnect(&updates, format!("could not write to xd host: {error}"));
                    return;
                }
            }
        })
        .expect("spawn xd host writer");
}

fn spawn_reader(
    stream: impl Read + Send + 'static,
    updates: Sender<DaemonUpdate>,
    pending: Arc<Mutex<HashMap<u64, RequestKind>>>,
) {
    thread::Builder::new()
        .name("xd-host-reader".into())
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
                        disconnect(&updates, "xd host closed the connection".into());
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        disconnect(&updates, format!("could not read from xd host: {error}"));
                        return;
                    }
                }

                if line.len() <= AUTHENTICATED_FRAME_LIMIT && line.last() != Some(&b'\n') {
                    disconnect(&updates, "xd host closed while sending a response".into());
                    return;
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
                        let attachments = match &kind {
                            RequestKind::ImageRead { path } => {
                                take_image(&mut body, path).map(|attachment| vec![attachment])
                            }
                            _ => take_draft_attachments(&mut body),
                        };
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
        .expect("spawn xd host reader");
}

fn take_draft_attachments(body: &mut Map<String, Value>) -> Option<Vec<Attachment>> {
    let value = body.remove("draft_attachments")?;
    let values = value.as_array()?;
    Some(values.iter().filter_map(Attachment::from_value).collect())
}

fn take_image(body: &mut Map<String, Value>, path: &str) -> Option<Attachment> {
    let value = json!({
        "name": Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image.png"),
        "mime": body.remove("mime")?,
        "data": body.remove("data")?,
    });
    Attachment::from_value(&value)
}

fn disconnect(updates: &Sender<DaemonUpdate>, message: String) {
    let _ = updates.send_blocking(DaemonUpdate::Disconnected { message });
}

fn is_socket(path: &Path) -> bool {
    path_is_socket(path)
}

pub fn socket_candidates() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("XD_SOCKET").filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(path)];
    }

    #[cfg(unix)]
    let data_home = env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(|home| PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    #[cfg(windows)]
    let data_home = env::var_os("LOCALAPPDATA")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    socket_candidates_for(data_home, None, Some(channel::data_name()))
}

fn socket_candidates_for(
    data_home: PathBuf,
    explicit_socket: Option<std::ffi::OsString>,
    data_name: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    if let Some(socket) = explicit_socket.filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(socket)];
    }
    let data_name = data_name.unwrap_or_else(|| "xd".into());
    vec![data_home.join(data_name).join("daemon.sock")]
}

fn data_root() -> PathBuf {
    socket_candidates()
        .into_iter()
        .next()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn launcher_candidates() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("XD_HOST_EXECUTABLE").filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(path)];
    }
    let mut candidates = Vec::new();
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        let sibling = parent.join(if cfg!(windows) {
            "xd-host.exe"
        } else {
            "xd-host"
        });
        if sibling.is_file() {
            candidates.push(sibling);
        }
    }
    candidates.push(PathBuf::from(if cfg!(windows) {
        "xd-host.exe"
    } else {
        "xd-host"
    }));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_socket::UnixListener;
    use std::fs;

    #[test]
    fn direct_cli_terminals_name_the_requested_agent_on_the_wire() {
        let directory =
            env::temp_dir().join(format!("xd-direct-cli-terminal-{}", std::process::id()));
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
            assert_eq!(request["op"], "terminal-open-agent");
            assert_eq!(request["chat"], "chat-1");
            assert_eq!(request["agent"], "claude");
            assert_eq!(request["reuse"], true);
            assert_eq!(request["allow_all_permissions"], true);
            assert_eq!(request["foreground"], 0x202020);
            assert_eq!(request["background"], 0xfafafa);
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon
            .terminal_open_agent("chat-1", 120, 32, true, "claude", true, 0x202020, 0xfafafa)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::TerminalOpen { chat_id, reuse, agent },
                ..
            } if chat_id == "chat-1" && reuse && agent.as_deref() == Some("claude")
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn shell_terminals_send_the_selected_palette_on_the_wire() {
        let directory =
            env::temp_dir().join(format!("xd-shell-terminal-palette-{}", std::process::id()));
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
            assert_eq!(request["op"], "terminal-open");
            assert_eq!(request["foreground"], 0xf1f1f1);
            assert_eq!(request["background"], 0x202020);
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon
            .terminal_open("chat-1", 120, 32, false, 0xf1f1f1, 0x202020)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::TerminalOpen { chat_id, reuse, agent },
                ..
            } if chat_id == "chat-1" && !reuse && agent.is_none()
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn correlates_replies_and_continues_delivering_events() {
        let directory = env::temp_dir().join(format!("xd-daemon-{}", std::process::id()));
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
    fn reports_a_connection_closed_mid_frame_without_a_json_parser_error() {
        let directory = env::temp_dir().join(format!("xd-partial-frame-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(br#"{"event":"text","text":"unfinished"#)
                .unwrap();
        });

        let (_daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Disconnected { message }
                if message == "xd host closed while sending a response"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn sends_stable_bidirectional_message_cursors() {
        let directory = env::temp_dir().join(format!("xd-message-cursors-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            for (key, id) in [("before", 42), ("after", 84)] {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["op"], "messages");
                assert_eq!(request["chat"], "chat-1");
                assert_eq!(request["limit"], MESSAGE_PAGE_SIZE);
                assert_eq!(request[key], id);
                let request_id = request["_xd_request"].as_u64().unwrap();
                writeln!(
                    stream,
                    "{{\"ok\":true,\"_xd_request\":{request_id},\"messages\":[]}}"
                )
                .unwrap();
            }
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon
            .messages("chat-1", MessageCursor::Before(42))
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::Messages {
                    cursor: MessageCursor::Before(42),
                    ..
                },
                ..
            }
        ));
        daemon.messages("chat-1", MessageCursor::After(84)).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::Messages {
                    cursor: MessageCursor::After(84),
                    ..
                },
                ..
            }
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn sends_remote_pair_and_resume_authentication_before_other_requests() {
        let directory = env::temp_dir().join(format!("xd-remote-auth-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["op"], "pair");
            assert_eq!(request["code"], "ABCD2345");
            assert_eq!(request["name"], "Laptop");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(
                stream,
                "{{\"ok\":true,\"_xd_request\":{request_id},\"token\":\"private\"}}"
            )
            .unwrap();

            line.clear();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["op"], "hello");
            assert_eq!(request["token"], "private");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon.pair_remote("ABCD2345", "Laptop").unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::PairRemote,
                ..
            }
        ));
        daemon.hello_remote("private").unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::HelloRemote,
                ..
            }
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn edits_queued_messages_with_the_original_text_guard() {
        let directory = env::temp_dir().join(format!("xd-edit-queue-{}", std::process::id()));
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
            assert_eq!(request["old-text"], "before");
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
    fn queues_and_reorders_messages_without_starting_a_turn() {
        let directory = env::temp_dir().join(format!("xd-queue-message-{}", std::process::id()));
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
            assert_eq!(request["op"], "queue");
            assert_eq!(request["chat"], "chat-2");
            assert_eq!(request["text"], "shared context");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();

            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["op"], "reorder-queue");
            assert_eq!(request["chat"], "chat-2");
            assert_eq!(request["source"], 2);
            assert_eq!(request["source-text"], "third");
            assert_eq!(request["anchor"], 0);
            assert_eq!(request["anchor-text"], "first");
            assert_eq!(request["after"], false);
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon.queue_message("chat-2", "shared context").unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::QueueMutation { chat_id },
                ..
            } if chat_id == "chat-2"
        ));
        daemon
            .reorder_queue("chat-2", 2, "third", 0, "first", false)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::QueueMutation { chat_id },
                ..
            } if chat_id == "chat-2"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn reorders_chats_before_and_after_an_anchor() {
        let directory = env::temp_dir().join(format!("xd-reorder-chat-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            for (field, after) in [("before", false), ("after", true)] {
                let mut request = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                let request: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(request["op"], "move-chat");
                assert_eq!(request["chat"], "chat-1");
                assert_eq!(request[field], "chat-2");
                assert_eq!(request.get(if after { "before" } else { "after" }), None);
                let request_id = request["_xd_request"].as_u64().unwrap();
                writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
            }
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        for after in [false, true] {
            daemon.reorder_chat("chat-1", "chat-2", after).unwrap();
            assert!(matches!(
                updates.recv_blocking().unwrap(),
                DaemonUpdate::Reply {
                    kind: RequestKind::MoveChat { chat_id, .. },
                    ..
                } if chat_id == "chat-1"
            ));
        }

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn sends_the_configured_git_writer_for_worktree_naming() {
        let directory = env::temp_dir().join(format!("xd-worktree-name-{}", std::process::id()));
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
            assert_eq!(request["op"], "send");
            assert_eq!(request["chat"], "chat-1");
            assert_eq!(request["text"], "fix the queue");
            assert_eq!(request["generate_worktree_name"], true);
            assert_eq!(request["worktree_backend"], "claude");
            assert_eq!(request["worktree_model"], "claude-sonnet-5");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon
            .send_message(
                "chat-1",
                "fix the queue",
                &[],
                Some("claude"),
                Some("claude-sonnet-5"),
            )
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::Send { chat_id, text },
                ..
            } if chat_id == "chat-1" && text == "fix the queue"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn sends_conflict_guarded_workspace_file_writes() {
        let directory = env::temp_dir().join(format!("xd-file-write-{}", std::process::id()));
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
            assert_eq!(request["op"], "file-browse");
            assert_eq!(request["action"], "write");
            assert_eq!(request["chat"], "chat-1");
            assert_eq!(request["path"], "src/main.rs");
            assert_eq!(request["original"], "before\n");
            assert_eq!(request["content"], "after\n");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon
            .file_browse_write("chat-1", "src/main.rs", "before\n", "after\n", 12)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::FileBrowseWrite {
                    chat_id,
                    path,
                    content,
                    generation: 12,
                },
                ..
            } if chat_id == "chat-1" && path == "src/main.rs" && content == "after\n"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn requests_daemon_side_directories_for_remote_safe_browsing() {
        let directory = env::temp_dir().join(format!("xd-list-directory-{}", std::process::id()));
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
            assert_eq!(request["op"], "list-dir");
            assert_eq!(request["path"], "/srv/workspaces");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon.list_directory(Some("/srv/workspaces"), 14).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::ListDirectory {
                    path: Some(path),
                    generation: 14,
                },
                ..
            } if path == "/srv/workspaces"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn new_chats_send_selected_workdirs_and_omit_workspace_defaults() {
        let directory = env::temp_dir().join(format!("xd-new-chat-workdir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            for (title, workdir, backend, worktree) in [
                (
                    "Selected directory",
                    Some("/srv/workspaces/project"),
                    None,
                    None,
                ),
                ("Workspace default", None, None, None),
                ("Direct Claude", None, Some("claude"), None),
                (
                    "Fresh worktree",
                    None,
                    Some("codex"),
                    Some(json!({"kind": "new"})),
                ),
                (
                    "Existing worktree",
                    None,
                    Some("claude"),
                    Some(json!({
                        "kind": "existing",
                        "path": "/srv/workspaces/feature",
                    })),
                ),
            ] {
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let request: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(request["op"], "new-chat");
                assert_eq!(request["folder"], "folder-1");
                assert_eq!(request["title"], title);
                match workdir {
                    Some(workdir) => assert_eq!(request["workdir"], workdir),
                    None => assert!(request.get("workdir").is_none()),
                }
                match backend {
                    Some(backend) => assert_eq!(request["backend"], backend),
                    None => assert!(request.get("backend").is_none()),
                }
                match worktree {
                    Some(worktree) => assert_eq!(request["worktree"], worktree),
                    None => assert!(request.get("worktree").is_none()),
                }
                let request_id = request["_xd_request"].as_u64().unwrap();
                writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
            }
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon
            .new_chat(
                "folder-1",
                "Selected directory",
                Some("/srv/workspaces/project"),
            )
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: Some(workdir),
                },
                ..
            } if folder_id == "folder-1"
                && title == "Selected directory"
                && workdir == "/srv/workspaces/project"
        ));
        daemon
            .new_chat("folder-1", "Workspace default", None)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: None,
                },
                ..
            } if folder_id == "folder-1" && title == "Workspace default"
        ));
        daemon
            .new_chat_with_backend("folder-1", "Direct Claude", None, "claude")
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: None,
                },
                ..
            } if folder_id == "folder-1" && title == "Direct Claude"
        ));
        daemon
            .new_chat_with_backend_in_worktree(
                "folder-1",
                "Fresh worktree",
                "codex",
                &NewSessionWorktree::New,
            )
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: None,
                },
                ..
            } if folder_id == "folder-1" && title == "Fresh worktree"
        ));
        daemon
            .new_chat_with_backend_in_worktree(
                "folder-1",
                "Existing worktree",
                "claude",
                &NewSessionWorktree::Existing("/srv/workspaces/feature".into()),
            )
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: None,
                },
                ..
            } if folder_id == "folder-1" && title == "Existing worktree"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn streams_voice_chunks_with_chat_and_request_identity() {
        let directory = env::temp_dir().join(format!("xd-voice-{}", std::process::id()));
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
            assert_eq!(request["op"], "voice-stream-chunk");
            assert_eq!(request["chat"], "chat-1");
            assert_eq!(request["request"], "recording-1");
            assert_eq!(request["audio"], "AAEC");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon
            .voice_action(
                "voice-stream-chunk",
                "chat-1",
                "recording-1",
                Some(&[0, 1, 2]),
            )
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::VoiceMutation { token, operation, .. },
                ..
            } if token == "recording-1" && operation == "voice-stream-chunk"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn sends_scoped_secret_updates_without_placeholder_values() {
        let directory = env::temp_dir().join(format!("xd-secrets-{}", std::process::id()));
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
            assert_eq!(request["op"], "set-agent-secrets");
            assert_eq!(request["folder"], "folder-1");
            assert_eq!(request["entries"][0], json!({"name": "EXISTING"}));
            assert_eq!(
                request["entries"][1],
                json!({"name": "NEW_TOKEN", "value": "private"})
            );
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (daemon, updates) = DaemonHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Connected { .. }
        ));
        daemon
            .set_agent_secrets(
                Some("folder-1"),
                &[
                    ("EXISTING".into(), None),
                    ("NEW_TOKEN".into(), Some("private".into())),
                ],
            )
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            DaemonUpdate::Reply {
                kind: RequestKind::SetAgentSecrets { folder_id },
                ..
            } if folder_id.as_deref() == Some("folder-1")
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn default_socket_uses_the_production_data_root() {
        assert_eq!(
            socket_candidates_for(PathBuf::from("/data"), None, None),
            vec![PathBuf::from("/data/xd/daemon.sock")]
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

    #[test]
    fn decodes_persisted_image_replies_before_the_ui_thread() {
        let mut body = json!({
            "ok": true,
            "mime": "image/png",
            "data": "iVBORw0KGgo="
        })
        .as_object()
        .unwrap()
        .clone();

        let image = take_image(&mut body, "/private/paste.png").unwrap();
        assert_eq!(image.name, "paste.png");
        assert_eq!(image.preview.bytes, b"\x89PNG\r\n\x1a\n");
        assert!(!body.contains_key("data"));
    }
}
