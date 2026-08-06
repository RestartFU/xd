use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::FileTypeExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread,
};

use serde_json::{Value, json};
use thiserror::Error;

pub mod agent;
mod ask;
mod auth;
mod git_draft;
mod runtime;
mod secrets;
mod storage;
mod terminal;
mod voice;
mod workflow;
mod worktree_name;

use auth::AuthManager;
use git_draft::GitDraftService;
pub use runtime::TurnRuntime;
use secrets::SecretsStore;
use storage::clone_repository;
pub use storage::{SendDisposition, StateStore, StorageError};
use terminal::TerminalManager;
use voice::VoiceService;
use workflow::WorkflowStatuses;

pub const FRAME_LIMIT: usize = 96 * 1024 * 1024;
const REQUEST_ID: &str = "_xd_request";

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("cannot prepare daemon socket directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("refusing to replace non-socket path {0}")]
    UnsafeSocketPath(PathBuf),
    #[error("an xd daemon is already listening on {0}")]
    AlreadyRunning(PathBuf),
    #[error("cannot remove stale daemon socket {path}: {source}")]
    RemoveSocket {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot bind daemon socket {path}: {source}")]
    Bind {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("daemon socket failed: {0}")]
    Accept(std::io::Error),
}

pub struct LocalServer {
    listener: UnixListener,
    socket_path: PathBuf,
    engine: Arc<Engine>,
}

pub struct Engine {
    store: Option<Arc<StateStore>>,
    events: Arc<EventBus>,
    runtime: Option<TurnRuntime>,
    auth: AuthManager,
    workflows: WorkflowStatuses,
    terminals: TerminalManager,
    voice: VoiceService,
    secrets: Arc<SecretsStore>,
    git_drafts: GitDraftService,
}

#[derive(Default)]
pub(crate) struct EventBus {
    next_event: AtomicU64,
    next_subscriber: AtomicU64,
    subscribers: Mutex<HashMap<u64, Subscriber>>,
}

struct Subscriber {
    sender: SyncSender<Value>,
    connection: UnixStream,
}

impl Engine {
    pub fn transport_only() -> Self {
        let events = Arc::new(EventBus::default());
        let secrets = Arc::new(SecretsStore::new(None));
        Self {
            store: None,
            auth: AuthManager::new(events.clone()),
            workflows: WorkflowStatuses::new(),
            terminals: TerminalManager::new(events.clone()),
            voice: VoiceService::new(events.clone(), None),
            secrets,
            git_drafts: GitDraftService::new(None, events.clone()),
            events,
            runtime: None,
        }
    }

    pub fn with_store(store: StateStore) -> Self {
        Self::with_store_and_data(store, None)
    }

    pub fn with_store_and_data(store: StateStore, data_directory: Option<PathBuf>) -> Self {
        let store = Arc::new(store);
        let events = Arc::new(EventBus::default());
        let auth = AuthManager::new(events.clone());
        let secrets = Arc::new(SecretsStore::new(data_directory.clone()));
        let git_drafts = GitDraftService::new(Some(store.clone()), events.clone());
        let engine = Self {
            runtime: Some(TurnRuntime::new(
                store.clone(),
                events.clone(),
                secrets.clone(),
            )),
            store: Some(store),
            auth,
            workflows: WorkflowStatuses::new(),
            terminals: TerminalManager::new(events.clone()),
            voice: VoiceService::new(events.clone(), data_directory),
            secrets,
            git_drafts,
            events,
        };
        engine.auth.refresh_all();
        engine
    }

    pub fn dispatch(&self, request: Value) -> Value {
        self.dispatch_for(0, request)
    }

    fn dispatch_for(&self, owner: u64, request: Value) -> Value {
        let request_id = request.get(REQUEST_ID).cloned();
        let mut reply = match request.get("op").and_then(Value::as_str) {
            Some("ping") => json!({"ok": true}),
            Some("tree") => self.read(|store| store.tree()),
            Some("chat") => match required_string(&request, "chat", "chat needs a chat id") {
                Ok(chat_id) => self.chat(chat_id),
                Err(error) => error_reply(error),
            },
            Some("messages") => self.read(|store| store.messages(&request)),
            Some("search") => self.read(|store| store.search(&request)),
            Some("diff-read") => self.read(|store| store.diff_read(&request)),
            Some("git-status") => self.read(|store| store.git_status(&request)),
            Some("git-pr-status") => self.read(|store| store.git_pull_request_status(&request)),
            Some("git-pr-create") => self.read(|store| store.git_create_pull_request(&request)),
            Some("git-draft") => self
                .git_drafts
                .start(&request)
                .map(|()| json!({"ok": true}))
                .unwrap_or_else(error_reply),
            Some("repository-files") => self.read(|store| store.repository_files(&request)),
            Some("repository-file") => self.read(|store| store.repository_file(&request)),
            Some("repository-file-write") => {
                self.read(|store| store.write_repository_file(&request))
            }
            Some("git-commit") => self.read(|store| store.git_commit(&request)),
            Some("git-push") => self.read(|store| store.git_push(&request)),
            Some("agent-catalog") => self.read(|store| store.agent_catalog()),
            Some("agent-secrets") => match self.secret_folder(&request) {
                Ok(folder) => self
                    .secrets
                    .names(folder.as_deref())
                    .map(|names| json!({"ok": true, "names": names}))
                    .unwrap_or_else(error_reply),
                Err(error) => error_reply(error),
            },
            Some("set-agent-secrets") => {
                match (self.secret_folder(&request), request.get("entries")) {
                    (Ok(folder), Some(entries)) => self
                        .secrets
                        .set(folder.as_deref(), entries)
                        .map(|()| json!({"ok": true}))
                        .unwrap_or_else(error_reply),
                    (Err(error), _) => error_reply(error),
                    (_, None) => error_reply("set-agent-secrets needs an entries array."),
                }
            }
            Some("agent-auth") => self.auth.snapshots(),
            Some("agent-auth-refresh") => {
                let provider = request.get("provider").and_then(Value::as_str);
                match provider {
                    Some(provider) if self.auth.refresh(provider) => json!({"ok": true}),
                    Some(_) => error_reply("No such assistant."),
                    None => {
                        self.auth.refresh_all();
                        json!({"ok": true})
                    }
                }
            }
            Some("agent-auth-start") => {
                self.auth_mutation(&request, |auth, provider, _| auth.login(provider))
            }
            Some("agent-auth-input") => self.auth_mutation(&request, |auth, provider, request| {
                let input = required_string(request, "input", "agent-auth-input needs text.")?;
                auth.input(provider, input)
            }),
            Some("agent-auth-cancel") => {
                self.auth_mutation(&request, |auth, provider, _| auth.cancel(provider))
            }
            Some("agent-auth-logout") => {
                self.auth_mutation(&request, |auth, provider, _| auth.logout(provider))
            }
            Some("shortcuts") => self.read(|store| store.shortcuts(&request)),
            Some("folder-context") => self.read(|store| store.folder_context(&request)),
            Some("folder-settings") => self.read(|store| store.folder_settings(&request)),
            Some("set-draft") => self.event_mutation(|store| store.set_draft(&request)),
            Some("set-shortcuts") => self.event_mutation(|store| store.set_shortcuts(&request)),
            Some("set-option") => self.event_mutation(|store| store.set_option(&request)),
            Some("send") => self.send_message(&request),
            Some("cancel") => self.cancel(&request),
            Some("new-folder") => self.new_folder(&request),
            Some("set-folder-context") => {
                self.tree_mutation(|store| store.set_folder_context(&request))
            }
            Some("set-folder-settings") => {
                self.tree_mutation(|store| store.set_folder_settings(&request))
            }
            Some("rename-folder") => self.tree_mutation(|store| store.rename_folder(&request)),
            Some("move-folder") => self.tree_mutation(|store| store.move_folder(&request)),
            Some("trash-folder") => self.tree_mutation(|store| store.trash_folder(&request)),
            Some("new-chat") => self.tree_mutation(|store| store.new_chat(&request)),
            Some("rename-chat") => self.tree_mutation(|store| store.rename_chat(&request)),
            Some("move-chat") => self.tree_mutation(|store| store.move_chat(&request)),
            Some("delete-chat") => self.tree_mutation(|store| store.delete_chat(&request)),
            Some("remove-worktree") => self.remove_worktree(&request),
            Some("queue") => self.event_mutation(|store| store.queue(&request)),
            Some("drop-queue") => self.event_mutation(|store| store.drop_queue(&request)),
            Some("edit-queue") => self.event_mutation(|store| store.edit_queue(&request)),
            Some("steer-queue") => self.steer_queue(&request),
            Some("workflow-status") => self.workflow_status(&request),
            Some("terminal-list") => self.terminal_list(&request),
            Some("terminal-open") => self.terminal_open(&request),
            Some("terminal-input") => self.terminals.input(&request).unwrap_or_else(error_reply),
            Some("terminal-resize") => self.terminals.resize(&request).unwrap_or_else(error_reply),
            Some("terminal-kill") => self.terminals.kill(&request).unwrap_or_else(error_reply),
            Some("voice-model") => self.voice_request(&request, "voice-model", || {
                self.voice.model_available(&request)
            }),
            Some("voice-model-download") => {
                self.voice_request(&request, "voice-model-download", || {
                    self.voice.download(owner, &request)
                })
            }
            Some("voice-stream-start") => {
                self.voice_request(&request, "voice-stream-start", || {
                    self.voice.start_stream(owner, &request)
                })
            }
            Some("voice-stream-chunk") => {
                self.voice_request(&request, "voice-stream-chunk", || {
                    self.voice.append_stream(owner, &request)
                })
            }
            Some("voice-stream-finish") => {
                self.voice_request(&request, "voice-stream-finish", || {
                    self.voice.finish_stream(owner, &request)
                })
            }
            Some("voice-transcribe") => self.voice_request(&request, "voice-transcribe", || {
                self.voice.transcribe(owner, &request)
            }),
            Some("voice-cancel") => self.voice.cancel(owner, &request),
            Some(operation) => json!({
                "ok": false,
                "error": format!("Operation {operation} is not implemented by the Rust daemon yet.")
            }),
            None => json!({"ok": false, "error": "Request must include a string op."}),
        };
        if let (Some(request_id), Some(reply)) = (request_id, reply.as_object_mut()) {
            reply.insert(REQUEST_ID.into(), request_id);
        }
        reply
    }

    fn read(&self, operation: impl FnOnce(&StateStore) -> Result<Value, StorageError>) -> Value {
        match self.store.as_ref() {
            Some(store) => operation(store).unwrap_or_else(error_reply),
            None => error_reply("Rust daemon state storage is not configured."),
        }
    }

    fn chat(&self, chat_id: &str) -> Value {
        let mut response = self.read(|store| store.chat(chat_id));
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return response;
        }
        let provider = response
            .get("backend")
            .and_then(Value::as_str)
            .unwrap_or("codex")
            .to_owned();
        response["auth_state"] = Value::String(self.auth.state(&provider));
        if let Some(runtime) = &self.runtime {
            response["commands"] = Value::Array(
                runtime
                    .commands(chat_id, &provider)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            );
        }
        response
    }

    fn auth_mutation(
        &self,
        request: &Value,
        operation: impl FnOnce(&AuthManager, &str, &Value) -> Result<(), String>,
    ) -> Value {
        let provider = match required_string(
            request,
            "provider",
            "Agent authentication needs a provider.",
        ) {
            Ok(provider) => provider,
            Err(error) => return error_reply(error),
        };
        operation(&self.auth, provider, request)
            .map(|()| json!({"ok": true}))
            .unwrap_or_else(error_reply)
    }

    fn secret_folder(&self, request: &Value) -> Result<Option<String>, String> {
        let Some(folder) = request.get("folder") else {
            return Ok(None);
        };
        let folder = folder
            .as_str()
            .filter(|folder| !folder.is_empty())
            .ok_or_else(|| "Agent secrets need a valid folder id.".to_string())?;
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| "Rust daemon state storage is not configured.".to_string())?;
        store
            .folder_path(folder)
            .map_err(|error| error.to_string())?;
        Ok(Some(folder.to_owned()))
    }

    fn voice_request(
        &self,
        request: &Value,
        operation: &str,
        run: impl FnOnce() -> Value,
    ) -> Value {
        let chat_id =
            match required_string(request, "chat", &format!("{operation} needs a chat id.")) {
                Ok(chat_id) => chat_id,
                Err(error) => return error_reply(error),
            };
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        match store.chat(chat_id) {
            Ok(_) => run(),
            Err(error) => error_reply(error),
        }
    }

    fn workflow_status(&self, request: &Value) -> Value {
        let text = match required_string(
            request,
            "text",
            "Workflow status needs the captured run marker.",
        ) {
            Ok(text) => text,
            Err(error) => return error_reply(error),
        };
        self.workflows.fetch(text).unwrap_or_else(error_reply)
    }

    fn terminal_list(&self, request: &Value) -> Value {
        let chat_id = match required_string(request, "chat", "terminal-list needs a chat id") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        self.terminals.list(chat_id)
    }

    fn terminal_open(&self, request: &Value) -> Value {
        let chat_id = match required_string(request, "chat", "terminal-open needs a chat id") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        match store.terminal_workdir(chat_id) {
            Ok(workdir) => self
                .terminals
                .open(request, &workdir)
                .unwrap_or_else(error_reply),
            Err(error) => error_reply(error),
        }
    }

    fn tree_mutation(
        &self,
        operation: impl FnOnce(&StateStore) -> Result<Value, StorageError>,
    ) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        match operation(store) {
            Ok(reply) => {
                self.events.publish(json!({"event": "tree"}));
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    fn new_folder(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        let reply = match store.new_folder(request) {
            Ok(reply) => reply,
            Err(error) => return error_reply(error),
        };
        self.events.publish(json!({"event": "tree"}));
        let (Some(folder_id), Some(url)) = (
            reply.get("id").and_then(Value::as_str),
            reply.get("cloning").and_then(Value::as_str),
        ) else {
            return reply;
        };
        let destination = match store.folder_path(folder_id) {
            Ok(destination) => destination,
            Err(error) => return error_reply(error),
        };
        let folder_id = folder_id.to_owned();
        let url = url.to_owned();
        let store = store.clone();
        let events = self.events.clone();
        events.publish(json!({
            "event": "folder-clone",
            "folder": folder_id.clone(),
            "url": url.clone(),
            "state": "cloning",
        }));
        thread::spawn(move || {
            let result = clone_repository(&url, &destination)
                .and_then(|_| store.finish_folder_clone(&folder_id, &destination));
            let mut event = json!({
                "event": "folder-clone",
                "folder": folder_id,
                "url": url,
                "state": if result.is_ok() { "ready" } else { "failed" },
            });
            if let Err(error) = result {
                event["error"] = Value::String(error.to_string());
            }
            events.publish(event);
            events.publish(json!({"event": "tree"}));
        });
        reply
    }

    fn event_mutation(
        &self,
        operation: impl FnOnce(&StateStore) -> Result<(Value, Value), StorageError>,
    ) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        match operation(store) {
            Ok((reply, event)) => {
                self.events.publish(event);
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    fn send_message(&self, request: &Value) -> Value {
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        let mut request = request.clone();
        if request.get("worktree_name").is_none() {
            match store.prepare_worktree_name(&request) {
                Ok(Some(spec)) => match worktree_name::generate(spec) {
                    Ok(name) => request["worktree_name"] = Value::String(name),
                    Err(error) => eprintln!("xd-dev: AI worktree naming failed: {error}"),
                },
                Ok(None) => {}
                Err(error) => eprintln!("xd-dev: cannot prepare AI worktree naming: {error}"),
            }
        }
        match store.prepare_send(&request) {
            Ok(SendDisposition::Queued { reply, event }) => {
                self.events.publish(event);
                reply
            }
            Ok(SendDisposition::Start { reply, turn }) => match runtime.start(turn.clone()) {
                Ok(()) => reply,
                Err(error) => {
                    let _ = store.abort_turn_start(&turn.chat_id, &error);
                    error_reply(error)
                }
            },
            Err(error) => error_reply(error),
        }
    }

    fn cancel(&self, request: &Value) -> Value {
        let chat_id = match required_string(request, "chat", "cancel needs a chat id") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        if let Some(runtime) = &self.runtime {
            runtime.cancel(chat_id);
        }
        json!({"ok": true})
    }

    fn steer_queue(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        match store.steer_queue(request) {
            Ok((reply, event)) => {
                self.events.publish(event);
                if let Some(chat_id) = request.get("chat").and_then(Value::as_str)
                    && let Some(runtime) = &self.runtime
                {
                    runtime.cancel(chat_id);
                }
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    fn remove_worktree(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        match store.remove_worktree(request) {
            Ok(reply) => {
                if let Some(chat_id) = request.get("chat").and_then(Value::as_str) {
                    self.events
                        .publish(json!({"event": "changed", "chat": chat_id}));
                }
                self.events.publish(json!({"event": "worktrees-changed"}));
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    fn subscribe(&self, sender: SyncSender<Value>, connection: UnixStream) -> Result<u64, String> {
        self.events.subscribe(sender, connection)
    }

    fn unsubscribe(&self, id: u64) {
        self.voice.cancel_owner(id);
        self.events.unsubscribe(id);
    }
}

impl EventBus {
    fn subscribe(&self, sender: SyncSender<Value>, connection: UnixStream) -> Result<u64, String> {
        let id = self.next_subscriber.fetch_add(1, Ordering::Relaxed);
        self.subscribers
            .lock()
            .map_err(|_| "daemon event state is unavailable".to_string())?
            .insert(id, Subscriber { sender, connection });
        Ok(id)
    }

    fn unsubscribe(&self, id: u64) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.remove(&id);
        }
    }

    fn publish(&self, mut event: Value) {
        let id = self.next_event.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(event) = event.as_object_mut() {
            event.insert("id".into(), id.into());
        }
        let Ok(mut subscribers) = self.subscribers.lock() else {
            return;
        };
        subscribers.retain(
            |_, subscriber| match subscriber.sender.try_send(event.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    let _ = subscriber.connection.shutdown(std::net::Shutdown::Both);
                    false
                }
            },
        );
    }

    fn publish_to(&self, subscriber_id: u64, mut event: Value) {
        let id = self.next_event.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(event) = event.as_object_mut() {
            event.insert("id".into(), id.into());
        }
        let Ok(mut subscribers) = self.subscribers.lock() else {
            return;
        };
        let remove = subscribers
            .get_mut(&subscriber_id)
            .is_some_and(|subscriber| match subscriber.sender.try_send(event) {
                Ok(()) => false,
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    let _ = subscriber.connection.shutdown(std::net::Shutdown::Both);
                    true
                }
            });
        if remove {
            subscribers.remove(&subscriber_id);
        }
    }
}

impl LocalServer {
    pub fn bind(socket_path: impl Into<PathBuf>) -> Result<Self, ServerError> {
        Self::bind_with_engine(socket_path, Engine::transport_only())
    }

    pub fn bind_with_engine(
        socket_path: impl Into<PathBuf>,
        engine: Engine,
    ) -> Result<Self, ServerError> {
        let socket_path = socket_path.into();
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ServerError::CreateDirectory {
                path: parent.to_owned(),
                source,
            })?;
        }
        match fs::symlink_metadata(&socket_path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                if UnixStream::connect(&socket_path).is_ok() {
                    return Err(ServerError::AlreadyRunning(socket_path));
                }
                fs::remove_file(&socket_path).map_err(|source| ServerError::RemoveSocket {
                    path: socket_path.clone(),
                    source,
                })?;
            }
            Ok(_) => return Err(ServerError::UnsafeSocketPath(socket_path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ServerError::Bind {
                    path: socket_path,
                    source,
                });
            }
        }
        let listener = UnixListener::bind(&socket_path).map_err(|source| ServerError::Bind {
            path: socket_path.clone(),
            source,
        })?;
        Ok(Self {
            listener,
            socket_path,
            engine: Arc::new(engine),
        })
    }

    pub fn run(self) -> Result<(), ServerError> {
        for connection in self.listener.incoming() {
            let stream = connection.map_err(ServerError::Accept)?;
            let engine = self.engine.clone();
            thread::Builder::new()
                .name("xd-rust-local-client".into())
                .spawn(move || {
                    let _ = serve_connection_with_engine(stream, engine);
                })
                .map_err(ServerError::Accept)?;
        }
        Ok(())
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

pub fn serve_connection(stream: UnixStream) -> std::io::Result<()> {
    serve_connection_with_engine(stream, Arc::new(Engine::transport_only()))
}

fn serve_connection_with_engine(stream: UnixStream, engine: Arc<Engine>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let writer_stream = stream.try_clone()?;
    let (outbound, frames) = sync_channel(256);
    let subscriber = engine
        .subscribe(outbound.clone(), stream.try_clone()?)
        .map_err(std::io::Error::other)?;
    let writer = thread::Builder::new()
        .name("xd-rust-local-writer".into())
        .spawn(move || write_frames(writer_stream, frames))?;
    let result = read_requests(&mut reader, &engine, subscriber, &outbound);
    engine.unsubscribe(subscriber);
    drop(outbound);
    let writer_result = writer
        .join()
        .map_err(|_| std::io::Error::other("daemon writer panicked"))?;
    result.and(writer_result)
}

fn read_requests(
    reader: &mut BufReader<UnixStream>,
    engine: &Engine,
    subscriber: u64,
    outbound: &SyncSender<Value>,
) -> std::io::Result<()> {
    loop {
        let mut line = Vec::new();
        let count = reader
            .by_ref()
            .take((FRAME_LIMIT + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        if line.len() > FRAME_LIMIT {
            return Ok(());
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }

        let reply = match serde_json::from_slice::<Value>(&line) {
            Ok(request) => engine.dispatch_for(subscriber, request),
            Err(error) => json!({"ok": false, "error": format!("Invalid JSON request: {error}")}),
        };
        outbound
            .send(reply)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client closed"))?;
    }
}

fn write_frames(
    mut stream: UnixStream,
    frames: std::sync::mpsc::Receiver<Value>,
) -> std::io::Result<()> {
    while let Ok(frame) = frames.recv() {
        serde_json::to_writer(&mut stream, &frame)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }
    Ok(())
}

pub fn dispatch(request: Value) -> Value {
    Engine::transport_only().dispatch(request)
}

fn required_string<'a>(request: &'a Value, key: &str, message: &str) -> Result<&'a str, String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| message.into())
}

fn error_reply(error: impl std::fmt::Display) -> Value {
    json!({"ok": false, "error": error.to_string()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        io::{BufRead, BufReader},
        os::unix::net::UnixStream,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn ping_echoes_the_private_request_id() {
        assert_eq!(
            dispatch(json!({"op": "ping", "_xd_request": 42})),
            json!({"ok": true, "_xd_request": 42})
        );
    }

    #[test]
    fn unknown_operations_are_explicit_and_correlated() {
        let reply = dispatch(json!({"op": "terminal-open", "_xd_request": 7}));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["_xd_request"], 7);
        assert!(reply["error"].as_str().unwrap().contains("terminal-open"));
    }

    #[test]
    fn workflow_status_is_dispatched_and_validates_markers_before_networking() {
        let reply = dispatch(json!({
            "op": "workflow-status",
            "text": "not a captured run",
            "_xd_request": 8
        }));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["_xd_request"], 8);
        assert_eq!(reply["error"], "Invalid workflow run marker.");
    }

    #[test]
    fn one_connection_handles_multiple_json_lines() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || serve_connection(server).unwrap());
        client
            .write_all(
                b"\n{\"op\":\"ping\",\"_xd_request\":1}\n{\"op\":\"ping\",\"_xd_request\":2}\n",
            )
            .unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let replies = BufReader::new(client)
            .lines()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(replies.len(), 2);
        assert_eq!(
            serde_json::from_str::<Value>(&replies[0]).unwrap()[REQUEST_ID],
            1
        );
        assert_eq!(
            serde_json::from_str::<Value>(&replies[1]).unwrap()[REQUEST_ID],
            2
        );
        worker.join().unwrap();
    }

    #[test]
    fn event_bus_broadcasts_monotonic_events() {
        let bus = EventBus::default();
        let (sender, receiver) = sync_channel(2);
        let (connection, peer) = UnixStream::pair().unwrap();
        let subscriber = bus.subscribe(sender, connection).unwrap();
        bus.publish(json!({"event": "draft", "chat": "chat-1"}));
        bus.publish(json!({"event": "tree"}));
        assert_eq!(receiver.recv().unwrap()["id"], 1);
        assert_eq!(receiver.recv().unwrap()["id"], 2);
        bus.unsubscribe(subscriber);
        drop(peer);
    }

    #[test]
    fn event_bus_disconnects_a_client_that_stops_draining() {
        let bus = EventBus::default();
        let (sender, _receiver) = sync_channel(1);
        let (connection, mut peer) = UnixStream::pair().unwrap();
        bus.subscribe(sender, connection).unwrap();
        bus.publish(json!({"event": "tree"}));
        bus.publish(json!({"event": "tree"}));
        assert!(bus.subscribers.lock().unwrap().is_empty());
        assert_eq!(
            peer.write(b"probe").unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn targeted_events_reach_only_the_requesting_connection() {
        let bus = EventBus::default();
        let (first_sender, first_receiver) = sync_channel(2);
        let (first_connection, first_peer) = UnixStream::pair().unwrap();
        let first = bus.subscribe(first_sender, first_connection).unwrap();
        let (second_sender, second_receiver) = sync_channel(2);
        let (second_connection, second_peer) = UnixStream::pair().unwrap();
        let second = bus.subscribe(second_sender, second_connection).unwrap();

        bus.publish_to(second, json!({"event": "voice", "request": "recording"}));

        assert_eq!(second_receiver.recv().unwrap()["request"], "recording");
        assert!(first_receiver.try_recv().is_err());
        bus.unsubscribe(first);
        bus.unsubscribe(second);
        drop((first_peer, second_peer));
    }

    #[test]
    fn voice_operations_require_a_chat_before_starting_work() {
        let reply = dispatch(json!({
            "op": "voice-model",
            "_xd_request": 19,
        }));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["_xd_request"], 19);
        assert_eq!(reply["error"], "voice-model needs a chat id.");
    }

    #[test]
    fn refuses_to_replace_a_regular_file_with_a_socket() {
        let root = test_directory();
        fs::create_dir_all(&root).unwrap();
        let path = root.join("daemon.sock");
        fs::write(&path, "keep me").unwrap();
        assert!(matches!(
            LocalServer::bind(&path),
            Err(ServerError::UnsafeSocketPath(found)) if found == path
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep me");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_unlink_a_live_daemon_socket() {
        let root = test_directory();
        fs::create_dir_all(&root).unwrap();
        let path = root.join("daemon.sock");
        let listener = UnixListener::bind(&path).unwrap();
        assert!(matches!(
            LocalServer::bind(&path),
            Err(ServerError::AlreadyRunning(found)) if found == path
        ));
        assert!(path.exists());
        drop(listener);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replaces_a_stale_socket() {
        let root = test_directory();
        fs::create_dir_all(&root).unwrap();
        let path = root.join("daemon.sock");
        drop(UnixListener::bind(&path).unwrap());
        let server = LocalServer::bind(&path).unwrap();
        assert!(path.exists());
        drop(server);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn socket_is_removed_when_the_server_is_dropped() {
        let root = test_directory();
        let path = root.join("daemon.sock");
        let server = LocalServer::bind(&path).unwrap();
        assert_eq!(server.socket_path(), path);
        assert!(path.exists());
        drop(server);
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn test_directory() -> PathBuf {
        env::temp_dir().join(format!(
            "xd-rust-daemon-test-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
