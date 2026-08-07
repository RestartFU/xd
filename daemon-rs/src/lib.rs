use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread,
};

use serde_json::{Value, json};
use thiserror::Error;

pub mod agent;
mod ask;
mod auth;
mod claude_proxy;
mod cli_versions;
mod git_draft;
pub mod local_socket;
mod pairing;
mod private_fs;
mod repository_monitor;
mod runtime;
mod secrets;
mod self_update;
mod storage;
#[cfg(unix)]
mod terminal;
#[cfg(windows)]
#[path = "terminal_windows.rs"]
mod terminal;
mod voice;
mod workflow;
mod worktree_name;

use auth::AuthManager;
use cli_versions::CliVersions;
use git_draft::GitDraftService;
use local_socket::{UnixListener, UnixStream, make_private, path_is_socket};
use pairing::{PairingService, Transport, generate_token, token_hash};
use repository_monitor::RepositoryMonitor;
pub use runtime::TurnRuntime;
use secrets::SecretsStore;
use self_update::SelfUpdate;
pub use storage::{SendDisposition, StateStore, StorageError};
use storage::{clone_repository, normalize_device_name};
use terminal::TerminalManager;
use voice::VoiceService;
use workflow::WorkflowStatuses;

pub const FRAME_LIMIT: usize = 96 * 1024 * 1024;
pub const AUTH_FRAME_LIMIT: usize = 64 * 1024;
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
    remote_listener: UnixListener,
    remote_socket_path: PathBuf,
    engine: Arc<Engine>,
}

pub struct Engine {
    store: Option<Arc<StateStore>>,
    events: Arc<EventBus>,
    runtime: Option<TurnRuntime>,
    auth: AuthManager,
    cli_versions: CliVersions,
    workflows: WorkflowStatuses,
    terminals: TerminalManager,
    voice: VoiceService,
    secrets: Arc<SecretsStore>,
    git_drafts: GitDraftService,
    git_actions: Arc<Mutex<()>>,
    repository_monitor: RepositoryMonitor,
    self_update: SelfUpdate,
    pairing: PairingService,
    peer_listener: Mutex<Option<PeerListener>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerEndpoint {
    pub host: String,
    pub port: u16,
}

type PeerListener = Arc<dyn Fn(&str, u16) -> Result<PeerEndpoint, String> + Send + Sync>;

#[derive(Default)]
pub(crate) struct EventBus {
    next_event: AtomicU64,
    next_subscriber: AtomicU64,
    subscribers: Mutex<HashMap<u64, Subscriber>>,
}

struct Subscriber {
    sender: SyncSender<Value>,
    connection: UnixStream,
    authenticated: Arc<AtomicBool>,
}

impl Engine {
    pub fn transport_only() -> Self {
        let events = Arc::new(EventBus::default());
        let secrets = Arc::new(SecretsStore::new(None));
        let self_update = SelfUpdate::new(events.clone());
        Self {
            store: None,
            auth: AuthManager::new(events.clone()),
            cli_versions: CliVersions::new(events.clone()),
            workflows: WorkflowStatuses::new(events.clone()),
            terminals: TerminalManager::new(events.clone()),
            voice: VoiceService::new(events.clone(), None),
            secrets,
            git_drafts: GitDraftService::new(None, events.clone()),
            git_actions: Arc::new(Mutex::new(())),
            repository_monitor: RepositoryMonitor::disabled(),
            self_update,
            pairing: PairingService::default(),
            peer_listener: Mutex::new(None),
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
        let cli_versions = CliVersions::new(events.clone());
        let secrets = Arc::new(SecretsStore::new(data_directory.clone()));
        let git_drafts = GitDraftService::new(Some(store.clone()), events.clone());
        let self_update = SelfUpdate::new(events.clone());
        let repository_monitor = RepositoryMonitor::new(store.clone(), events.clone());
        let engine = Self {
            runtime: Some(TurnRuntime::new(
                store.clone(),
                events.clone(),
                secrets.clone(),
            )),
            store: Some(store),
            auth,
            cli_versions,
            workflows: WorkflowStatuses::new(events.clone()),
            terminals: TerminalManager::new(events.clone()),
            voice: VoiceService::new(events.clone(), data_directory),
            secrets,
            git_drafts,
            git_actions: Arc::new(Mutex::new(())),
            repository_monitor,
            self_update,
            pairing: PairingService::default(),
            peer_listener: Mutex::new(None),
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
        let operation = request.get("op").and_then(Value::as_str);
        let authorization = operation
            .map(|operation| self.pairing.authorize(owner, operation))
            .transpose();
        if let Err(error) = authorization {
            let mut reply = error_reply(error);
            if let (Some(request_id), Some(reply)) = (request_id, reply.as_object_mut()) {
                reply.insert(REQUEST_ID.into(), request_id);
            }
            return reply;
        }
        let mut reply = match operation {
            Some("ping") => json!({"ok": true}),
            Some("pair") => self.pair(owner, &request),
            Some("hello") => self.hello(owner, &request),
            Some("peer-pairing") => self.peer_pairing(owner, &request),
            Some("tree") => self.read(|store| store.tree()),
            Some("devices") => self.devices(),
            Some("rename-device") => self.read(|store| store.rename_device(&request)),
            Some("revoke-device") => self.revoke_device(&request),
            Some("chat") => match required_string(&request, "chat", "chat needs a chat id") {
                Ok(chat_id) => self.chat(chat_id),
                Err(error) => error_reply(error),
            },
            Some("messages") => self.read(|store| store.messages(&request)),
            Some("list-dir") => self.read(|store| store.list_directory(&request)),
            Some("file-browse") => self.read(|store| store.file_browse(&request)),
            Some("search") => self.read(|store| store.search(&request)),
            Some("image-read") => self.read(|store| store.image_read(&request)),
            Some("diff-read") => self.read(|store| store.diff_read(&request)),
            Some("git-status") => self.read(|store| store.git_status(&request)),
            Some("git-state") => self.git_state(&request),
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
            Some("git-commit") => self.git_commit(&request),
            Some("git-push") => self.git_push(&request),
            Some("git-action") => self.git_action(&request),
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
            Some("agent-clis") => self.cli_versions.snapshots(),
            Some("daemon-update") => self
                .self_update
                .perform(request.get("action").and_then(Value::as_str))
                .unwrap_or_else(error_reply),
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
            Some("workflow-status") => self.workflow_status(owner, &request),
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

    pub fn arm_pairing(&self, ttl: std::time::Duration) -> String {
        self.pairing.arm(ttl)
    }

    pub fn set_peer_listener(
        &self,
        listener: impl Fn(&str, u16) -> Result<PeerEndpoint, String> + Send + Sync + 'static,
    ) -> Result<(), String> {
        *self
            .peer_listener
            .lock()
            .map_err(|_| "daemon peer-listener state is unavailable".to_owned())? =
            Some(Arc::new(listener));
        Ok(())
    }

    fn peer_pairing(&self, owner: u64, request: &Value) -> Value {
        if !self.pairing.is_local(owner) {
            return error_reply("Pairing codes can only be created on the daemon machine.");
        }
        let bind = request.get("bind").and_then(Value::as_str).unwrap_or("::");
        if bind.is_empty() {
            return error_reply("Pairing bind address cannot be empty.");
        }
        let port = match request.get("port") {
            None => 4001,
            Some(Value::Number(port)) => {
                match port.as_u64().and_then(|port| u16::try_from(port).ok()) {
                    Some(port) => port,
                    None => return error_reply("Port must be from 0 to 65535."),
                }
            }
            Some(_) => return error_reply("Port must be from 0 to 65535."),
        };
        let listener = match self.peer_listener.lock() {
            Ok(listener) => listener.clone(),
            Err(_) => return error_reply("Daemon peer-listener state is unavailable."),
        };
        let Some(listener) = listener else {
            return error_reply("This daemon cannot accept remote devices.");
        };
        let endpoint = match listener(bind, port) {
            Ok(endpoint) => endpoint,
            Err(error) => return error_reply(format!("Cannot accept remote devices: {error}")),
        };
        if let Some(store) = self.store.as_ref()
            && let Err(error) = store.save_remote_listener(bind, endpoint.port)
        {
            return error_reply(format!("Cannot save the remote listener: {error}"));
        }
        let code = self.arm_pairing(std::time::Duration::from_secs(5 * 60));
        json!({
            "ok": true,
            "code": code,
            "host": endpoint.host,
            "port": endpoint.port,
            "expires_in": 300,
        })
    }

    fn pair(&self, owner: u64, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        let name = match required_string(request, "name", "pair needs a device name.") {
            Ok(name) => match normalize_device_name(name) {
                Ok(name) => name,
                Err(error) => return error_reply(error),
            },
            Err(error) => return error_reply(error),
        };
        let code = request
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !self.pairing.consume(code) {
            return error_reply("No such pairing code. Run the server with --pair.");
        }
        let token = generate_token();
        let hash = token_hash(&token);
        let name = match store.add_device(&hash, name) {
            Ok(name) => name,
            Err(error) => return error_reply(error),
        };
        if let Err(error) = self.pairing.authenticate(owner, hash) {
            return error_reply(error);
        }
        json!({"ok": true, "token": token, "device": name})
    }

    fn hello(&self, owner: u64, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        let token = match required_string(request, "token", "hello needs a token") {
            Ok(token) => token,
            Err(error) => return error_reply(error),
        };
        let hash = token_hash(token);
        let name = match store.authenticate_device(&hash) {
            Ok(Some(name)) => name,
            Ok(None) => return error_reply("Unknown device. Pair first."),
            Err(error) => return error_reply(error),
        };
        if let Err(error) = self.pairing.authenticate(owner, hash) {
            return error_reply(error);
        }
        json!({"ok": true, "device": name, "version": 1})
    }

    fn devices(&self) -> Value {
        let connected = self.pairing.connected_devices();
        self.read(|store| store.devices_with_connected(&connected))
    }

    fn revoke_device(&self, request: &Value) -> Value {
        let device = request
            .get("device")
            .or_else(|| request.get("id"))
            .and_then(Value::as_str)
            .filter(|device| !device.is_empty());
        let Some(device) = device else {
            return error_reply("revoke-device needs a device id.");
        };
        let reply = self.read(|store| store.revoke_device(request));
        if reply.get("ok").and_then(Value::as_bool) == Some(true) {
            for owner in self.pairing.connections_for_device(device) {
                self.events.disconnect(owner);
            }
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

    fn workflow_status(&self, owner: u64, request: &Value) -> Value {
        let text = match required_string(
            request,
            "text",
            "Workflow status needs the captured run marker.",
        ) {
            Ok(text) => text,
            Err(error) => return error_reply(error),
        };
        self.workflows
            .start(owner, text)
            .unwrap_or_else(error_reply)
    }

    fn git_state(&self, request: &Value) -> Value {
        let chat_id = match required_string(request, "chat", "git-state needs a chat id.") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        if let Err(error) = store.chat(chat_id) {
            return error_reply(error);
        }
        self.repository_monitor.watch(chat_id);
        let chat_id = chat_id.to_owned();
        let request_id = request
            .get("request")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let store = store.clone();
        let events = self.events.clone();
        thread::Builder::new()
            .name("xd-git-state".into())
            .spawn(move || {
                let mut event = store
                    .git_action_state(&chat_id)
                    .unwrap_or_else(|_| hidden_git_state());
                event["event"] = Value::String("git-state".into());
                event["chat"] = Value::String(chat_id);
                if let Some(request_id) = request_id {
                    event["request"] = Value::String(request_id);
                }
                events.publish(event);
            })
            .map(|_| json!({"ok": true}))
            .unwrap_or_else(|error| error_reply(format!("Cannot refresh Git state: {error}")))
    }

    fn git_action(&self, request: &Value) -> Value {
        let chat_id =
            match required_string(request, "chat", "git-action needs a chat id and action.") {
                Ok(chat_id) => chat_id,
                Err(error) => return error_reply(error),
            };
        let action =
            match required_string(request, "action", "git-action needs a chat id and action.") {
                Ok(action) if matches!(action, "commit" | "push" | "create-pr" | "view-pr") => {
                    action
                }
                Ok(_) => return error_reply("No such Git action."),
                Err(error) => return error_reply(error),
            };
        if action == "commit"
            && request
                .get("message")
                .and_then(Value::as_str)
                .is_none_or(|message| message.trim().is_empty())
        {
            return error_reply("Write a commit message first.");
        }
        if action == "create-pr"
            && request
                .get("title")
                .and_then(Value::as_str)
                .is_none_or(|title| title.trim().is_empty())
        {
            return error_reply("Write a pull request title first.");
        }
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        if let Err(error) = store.chat(chat_id) {
            return error_reply(error);
        }
        let request = request.clone();
        let chat_id = chat_id.to_owned();
        let action = action.to_owned();
        let request_id = request
            .get("request")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let store = store.clone();
        let events = self.events.clone();
        let action_lock = self.git_actions.clone();
        let repository_monitor = self.repository_monitor.handle();
        thread::Builder::new()
            .name("xd-git-action".into())
            .spawn(move || {
                let mut event = json!({
                    "event": "git-action-finished",
                    "chat": chat_id,
                    "action": action,
                    "success": false,
                });
                if let Some(request_id) = request_id {
                    event["request"] = Value::String(request_id);
                }
                let result = action_lock
                    .lock()
                    .map_err(|_| "Git action state is unavailable.".to_owned())
                    .and_then(|_guard| {
                        store
                            .perform_git_action(&request)
                            .map_err(|error| error.to_string())
                    });
                match result {
                    Ok(state) => {
                        repository_monitor.reset(&chat_id);
                        if let (Some(event), Some(state)) =
                            (event.as_object_mut(), state.as_object())
                        {
                            event.extend(state.clone());
                            event.insert("chat".into(), Value::String(chat_id));
                            event.insert("success".into(), Value::Bool(true));
                        }
                    }
                    Err(error) => event["error"] = Value::String(error),
                }
                events.publish(event);
            })
            .map(|_| json!({"ok": true}))
            .unwrap_or_else(|error| error_reply(format!("Cannot start Git action: {error}")))
    }

    fn git_commit(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        let chat_id = request
            .get("chat")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match store.git_commit(request) {
            Ok(reply) => {
                if let Some(chat_id) = chat_id {
                    self.repository_monitor.handle().reset(&chat_id);
                }
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    fn git_push(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust daemon state storage is not configured.");
        };
        let chat_id = request
            .get("chat")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match store.git_push(request) {
            Ok(reply) => {
                if let Some(chat_id) = chat_id {
                    self.repository_monitor.handle().reset(&chat_id);
                }
                reply
            }
            Err(error) => error_reply(error),
        }
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
                    Err(error) => eprintln!("xd: AI worktree naming failed: {error}"),
                },
                Ok(None) => {}
                Err(error) => eprintln!("xd: cannot prepare AI worktree naming: {error}"),
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

    fn subscribe_transport(
        &self,
        sender: SyncSender<Value>,
        connection: UnixStream,
        transport: Transport,
    ) -> Result<u64, String> {
        let authenticated = Arc::new(AtomicBool::new(transport == Transport::Local));
        let owner = self
            .events
            .subscribe_with_auth(sender, connection, authenticated.clone())?;
        if let Err(error) = self.pairing.register(owner, transport, authenticated) {
            self.events.unsubscribe(owner);
            return Err(error);
        }
        Ok(owner)
    }

    fn unsubscribe(&self, id: u64) {
        self.voice.cancel_owner(id);
        self.pairing.unregister(id);
        self.events.unsubscribe(id);
    }
}

impl EventBus {
    #[cfg(test)]
    fn subscribe(&self, sender: SyncSender<Value>, connection: UnixStream) -> Result<u64, String> {
        self.subscribe_with_auth(sender, connection, Arc::new(AtomicBool::new(true)))
    }

    fn subscribe_with_auth(
        &self,
        sender: SyncSender<Value>,
        connection: UnixStream,
        authenticated: Arc<AtomicBool>,
    ) -> Result<u64, String> {
        // Zero is reserved for direct/local dispatches that do not own a socket.
        let id = self.next_subscriber.fetch_add(1, Ordering::Relaxed) + 1;
        self.subscribers
            .lock()
            .map_err(|_| "daemon event state is unavailable".to_string())?
            .insert(
                id,
                Subscriber {
                    sender,
                    connection,
                    authenticated,
                },
            );
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
        subscribers.retain(|_, subscriber| {
            if !subscriber.authenticated.load(Ordering::Acquire) {
                true
            } else {
                match subscriber.sender.try_send(event.clone()) {
                    Ok(()) => true,
                    Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                        let _ = subscriber.connection.shutdown(std::net::Shutdown::Both);
                        false
                    }
                }
            }
        });
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

    fn disconnect(&self, subscriber_id: u64) {
        if let Ok(mut subscribers) = self.subscribers.lock()
            && let Some(subscriber) = subscribers.remove(&subscriber_id)
        {
            subscriber.authenticated.store(false, Ordering::Release);
            let _ = subscriber.connection.shutdown(std::net::Shutdown::Both);
        }
    }
}

pub fn remote_socket_path(local: &Path) -> PathBuf {
    let mut path = local.as_os_str().to_os_string();
    path.push(".remote");
    PathBuf::from(path)
}

fn bind_daemon_socket(path: &Path) -> Result<UnixListener, ServerError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ServerError::CreateDirectory {
            path: parent.to_owned(),
            source,
        })?;
    }
    match fs::symlink_metadata(path) {
        Ok(_) if path_is_socket(path) => {
            if UnixStream::connect(path).is_ok() {
                return Err(ServerError::AlreadyRunning(path.to_owned()));
            }
            fs::remove_file(path).map_err(|source| ServerError::RemoveSocket {
                path: path.to_owned(),
                source,
            })?;
        }
        Ok(_) => return Err(ServerError::UnsafeSocketPath(path.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(ServerError::Bind {
                path: path.to_owned(),
                source,
            });
        }
    }
    let listener = UnixListener::bind(path).map_err(|source| ServerError::Bind {
        path: path.to_owned(),
        source,
    })?;
    if let Err(source) = make_private(path) {
        drop(listener);
        let _ = fs::remove_file(path);
        return Err(ServerError::Bind {
            path: path.to_owned(),
            source,
        });
    }
    Ok(listener)
}

fn accept_connections(listener: UnixListener, engine: Arc<Engine>, transport: Transport) {
    for connection in listener.incoming() {
        let Ok(stream) = connection else {
            break;
        };
        let engine = engine.clone();
        let name = match transport {
            Transport::Local => "xd-rust-local-client",
            Transport::Remote => "xd-rust-remote-client",
        };
        if thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let _ = serve_connection_with_engine(stream, engine, transport);
            })
            .is_err()
        {
            break;
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
        let listener = bind_daemon_socket(&socket_path)?;
        let remote_socket_path = remote_socket_path(&socket_path);
        let remote_listener = match bind_daemon_socket(&remote_socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = fs::remove_file(&socket_path);
                return Err(error);
            }
        };
        Ok(Self {
            listener,
            socket_path,
            remote_listener,
            remote_socket_path,
            engine: Arc::new(engine),
        })
    }

    pub fn run(self) -> Result<(), ServerError> {
        let remote_listener = self
            .remote_listener
            .try_clone()
            .map_err(ServerError::Accept)?;
        let remote_engine = self.engine.clone();
        thread::Builder::new()
            .name("xd-rust-remote-ipc".into())
            .spawn(move || accept_connections(remote_listener, remote_engine, Transport::Remote))
            .map_err(ServerError::Accept)?;
        for connection in self.listener.incoming() {
            let stream = connection.map_err(ServerError::Accept)?;
            let engine = self.engine.clone();
            thread::Builder::new()
                .name("xd-rust-local-client".into())
                .spawn(move || {
                    let _ = serve_connection_with_engine(stream, engine, Transport::Local);
                })
                .map_err(ServerError::Accept)?;
        }
        Ok(())
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn remote_socket_path(&self) -> &Path {
        &self.remote_socket_path
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.remote_socket_path);
    }
}

pub fn serve_connection(stream: UnixStream) -> std::io::Result<()> {
    serve_connection_with_engine(stream, Arc::new(Engine::transport_only()), Transport::Local)
}

fn serve_connection_with_engine(
    stream: UnixStream,
    engine: Arc<Engine>,
    transport: Transport,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let writer_stream = stream.try_clone()?;
    let (outbound, frames) = sync_channel(256);
    let subscriber = engine
        .subscribe_transport(outbound.clone(), stream.try_clone()?, transport)
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
        let frame_limit = if engine.pairing.authenticated(subscriber) {
            FRAME_LIMIT
        } else {
            AUTH_FRAME_LIMIT
        };
        let mut line = Vec::new();
        let count = reader
            .by_ref()
            .take((frame_limit + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        if line.len() > frame_limit {
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

fn hidden_git_state() -> Value {
    json!({
        "visible": false,
        "action": "none",
        "label": "Up to date",
        "enabled": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::{
        env,
        io::{BufRead, BufReader},
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
    fn remote_sessions_pair_resume_and_are_revoked_immediately() {
        let root = test_directory();
        let store = StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap();
        let engine = Engine::with_store(store);

        let (sender, _receiver) = sync_channel(8);
        let (connection, peer) = UnixStream::pair().unwrap();
        let owner = engine
            .subscribe_transport(sender, connection, Transport::Remote)
            .unwrap();
        assert_eq!(
            engine.dispatch_for(owner, json!({"op": "tree"}))["error"],
            "Not authenticated. Say hello first."
        );

        let code = engine.arm_pairing(std::time::Duration::from_secs(60));
        let invalid =
            engine.dispatch_for(owner, json!({"op": "pair", "code": code, "name": "   "}));
        assert_eq!(invalid["ok"], false);
        let paired = engine.dispatch_for(
            owner,
            json!({"op": "pair", "code": code, "name": "  Phone  "}),
        );
        assert_eq!(paired["ok"], true);
        assert_eq!(paired["device"], "Phone");
        let token = paired["token"].as_str().unwrap().to_owned();
        assert_eq!(
            engine.dispatch_for(owner, json!({"op": "tree"}))["ok"],
            true
        );
        engine.unsubscribe(owner);
        drop(peer);

        let (sender, _receiver) = sync_channel(8);
        let (connection, mut peer) = UnixStream::pair().unwrap();
        let resumed = engine
            .subscribe_transport(sender, connection, Transport::Remote)
            .unwrap();
        let hello = engine.dispatch_for(resumed, json!({"op": "hello", "token": token}));
        assert_eq!(hello["ok"], true);
        assert_eq!(hello["device"], "Phone");
        assert_eq!(hello["version"], 1);
        assert_eq!(
            engine.dispatch_for(resumed, json!({"op": "devices"}))["error"],
            "Device management is only available on the daemon machine."
        );

        let devices = engine.dispatch(json!({"op": "devices"}));
        let device = devices["devices"][0]["id"].as_str().unwrap().to_owned();
        assert_eq!(devices["devices"][0]["connected"], true);
        assert_eq!(
            engine.dispatch(json!({"op": "revoke-device", "device": device}))["ok"],
            true
        );
        let mut buffer = [0_u8; 1];
        assert_eq!(peer.read(&mut buffer).unwrap(), 0);
        engine.unsubscribe(resumed);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unauthenticated_remote_sessions_do_not_receive_events() {
        let engine = Engine::transport_only();
        let (sender, receiver) = sync_channel(2);
        let (connection, peer) = UnixStream::pair().unwrap();
        let owner = engine
            .subscribe_transport(sender, connection, Transport::Remote)
            .unwrap();
        engine.events.publish(json!({"event": "tree"}));
        assert!(receiver.try_recv().is_err());
        engine.unsubscribe(owner);
        drop(peer);
    }

    #[test]
    fn unauthenticated_remote_frames_use_the_small_authentication_limit() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let engine = Arc::new(Engine::transport_only());
        let worker = thread::spawn(move || {
            serve_connection_with_engine(server, engine, Transport::Remote).unwrap()
        });
        let mut frame = vec![b'x'; AUTH_FRAME_LIMIT + 1];
        frame.push(b'\n');
        client.write_all(&frame).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(BufReader::new(client).lines().count(), 0);
        worker.join().unwrap();
    }

    #[test]
    fn local_pairing_requests_start_only_the_configured_remote_listener() {
        let engine = Engine::transport_only();
        engine
            .set_peer_listener(|bind, port| {
                assert_eq!(bind, "127.0.0.1");
                assert_eq!(port, 0);
                Ok(PeerEndpoint {
                    host: "192.168.1.10".into(),
                    port: 43125,
                })
            })
            .unwrap();
        let reply = engine.dispatch(json!({
            "op": "peer-pairing",
            "bind": "127.0.0.1",
            "port": 0,
        }));
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["host"], "192.168.1.10");
        assert_eq!(reply["port"], 43125);
        assert_eq!(reply["expires_in"], 300);
        assert_eq!(reply["code"].as_str().unwrap().len(), 9);

        let (sender, _receiver) = sync_channel(1);
        let (connection, peer) = UnixStream::pair().unwrap();
        let remote = engine
            .subscribe_transport(sender, connection, Transport::Remote)
            .unwrap();
        engine
            .pairing
            .authenticate(remote, "device".into())
            .unwrap();
        assert_eq!(
            engine.dispatch_for(remote, json!({"op": "peer-pairing"}))["error"],
            "Pairing codes can only be created on the daemon machine."
        );
        engine.unsubscribe(remote);
        drop(peer);
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
        let remote = server.remote_socket_path().to_owned();
        assert!(remote.exists());
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&remote).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(server);
        assert!(!path.exists());
        assert!(!remote.exists());
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
