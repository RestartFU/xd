use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde_json::{Value, json};

mod agent;
mod ask;
mod auth;
mod background_process;
mod cli_versions;
mod codex_app;
mod git_draft;
mod local_socket;
mod pairing;
mod private_fs;
mod repository_monitor;
mod runtime;
mod secrets;
mod self_update;
mod storage;
mod subagent;

/// Tests that point an agent executable at a stand-in take this first, since
/// the override is process-wide.
#[cfg(test)]
pub(crate) static AGENT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
mod terminal;
mod terminal_activity;
mod terminal_query;
mod terminal_replay;
mod tool_diff;
mod user_environment;
mod workflow;
mod worktree_name;

use auth::AuthManager;
use cli_versions::CliVersions;
use git_draft::GitDraftService;
use local_socket::UnixStream;
use pairing::{PairingService, Transport, generate_token, token_hash};
use repository_monitor::RepositoryMonitor;
use runtime::{LiveTurn, TurnRuntime};
use secrets::SecretsStore;
use self_update::SelfUpdate;
use storage::SendDisposition;
pub use storage::{StateStore, StorageError};
use storage::{clone_repository, normalize_device_name};
use terminal::TerminalManager;
use workflow::WorkflowStatuses;

pub const FRAME_LIMIT: usize = 96 * 1024 * 1024;
pub const AUTH_FRAME_LIMIT: usize = 64 * 1024;
const REQUEST_ID: &str = "_xd_request";

pub struct Engine {
    shutdown: AtomicBool,
    store: Option<Arc<StateStore>>,
    events: Arc<EventBus>,
    runtime: Option<TurnRuntime>,
    auth: AuthManager,
    cli_versions: CliVersions,
    workflows: WorkflowStatuses,
    terminals: TerminalManager,
    chat_execution: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    session_resources: Mutex<()>,
    secrets: Arc<SecretsStore>,
    git_drafts: GitDraftService,
    git_actions: Arc<Mutex<()>>,
    repository_monitor: RepositoryMonitor,
    self_update: SelfUpdate,
    pairing: PairingService,
    peer_listener: Mutex<Option<PeerListener>>,
    _shared_event_monitor: Option<SharedEventMonitor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerEndpoint {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
}

type PeerListener = Arc<dyn Fn(&str, u16) -> Result<PeerEndpoint, String> + Send + Sync>;

#[derive(Default)]
pub(crate) struct EventBus {
    next_event: AtomicU64,
    next_subscriber: AtomicU64,
    subscribers: Mutex<HashMap<u64, Subscriber>>,
    shared: Option<SharedEventPublisher>,
}

struct SharedEventPublisher {
    store: Arc<StateStore>,
    source: String,
}

struct SharedEventMonitor {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

struct Subscriber {
    sender: SyncSender<Value>,
    connection: Option<UnixStream>,
    authenticated: Arc<AtomicBool>,
    events_ready: Option<Arc<AtomicBool>>,
}

impl Engine {
    #[cfg(test)]
    pub fn transport_only() -> Self {
        let events = Arc::new(EventBus::default());
        let secrets = Arc::new(SecretsStore::new(None));
        let self_update = SelfUpdate::new(events.clone());
        Self {
            shutdown: AtomicBool::new(false),
            store: None,
            auth: AuthManager::new(events.clone()),
            cli_versions: CliVersions::new(events.clone()),
            workflows: WorkflowStatuses::new(events.clone()),
            terminals: TerminalManager::new(events.clone()),
            chat_execution: Mutex::new(HashMap::new()),
            session_resources: Mutex::new(()),
            secrets,
            git_drafts: GitDraftService::new(None, events.clone()),
            git_actions: Arc::new(Mutex::new(())),
            repository_monitor: RepositoryMonitor::disabled(),
            self_update,
            pairing: PairingService::default(),
            peer_listener: Mutex::new(None),
            _shared_event_monitor: None,
            events,
            runtime: None,
        }
    }

    #[cfg(test)]
    pub fn with_store(store: StateStore) -> Self {
        Self::with_store_and_data(store, None)
    }

    pub fn with_store_and_data(store: StateStore, data_directory: Option<PathBuf>) -> Self {
        let store = Arc::new(store);
        let events = Arc::new(EventBus::with_store(store.clone()));
        let auth = AuthManager::new(events.clone());
        let cli_versions = CliVersions::new(events.clone());
        let secrets = Arc::new(SecretsStore::new(data_directory.clone()));
        let git_drafts = GitDraftService::new(Some(store.clone()), events.clone());
        let self_update = SelfUpdate::new(events.clone());
        let repository_monitor = RepositoryMonitor::new(store.clone(), events.clone());
        let shared_event_monitor = Some(SharedEventMonitor::start(events.clone()));
        let engine = Self {
            shutdown: AtomicBool::new(false),
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
            chat_execution: Mutex::new(HashMap::new()),
            session_resources: Mutex::new(()),
            secrets,
            git_drafts,
            git_actions: Arc::new(Mutex::new(())),
            repository_monitor,
            self_update,
            pairing: PairingService::default(),
            peer_listener: Mutex::new(None),
            _shared_event_monitor: shared_event_monitor,
            events,
        };
        engine.auth.refresh_all();
        engine
    }

    #[cfg(test)]
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
            Some("daemon-info") => {
                json!({"ok": true, "version": env!("CARGO_PKG_VERSION"), "protocol": 2, "build": option_env!("XD_COMMIT").unwrap_or("development"), "pid": std::process::id(), "workspaces": self.store.as_ref().and_then(|store| fs::canonicalize(store.workspace_path()).ok())})
            }
            Some("daemon-stop") if self.pairing.is_local(owner) => {
                self.shutdown.store(true, Ordering::Release);
                json!({"ok": true})
            }
            Some("pair") => self.pair(owner, &request),
            Some("hello") => self.hello(owner, &request),
            Some("peer-pairing") => self.peer_pairing(owner, &request),
            Some("tree") => self.tree(),
            Some("devices") => self.devices(),
            Some("rename-device") => self.read(|store| store.rename_device(&request)),
            Some("revoke-device") => self.revoke_device(&request),
            Some("chat") => match required_string(&request, "chat", "chat needs a chat id") {
                Ok(chat_id) => self.chat(chat_id),
                Err(error) => error_reply(error),
            },
            Some("messages") => self.messages(&request),
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
            // Protocol v1 desktop builds used these names before file-browse
            // became the shared file API. Keep only the wire aliases so an
            // independently updated remote host does not strand them.
            Some("repository-files") => self.read(|store| store.compat_repository_files(&request)),
            Some("repository-file") => self.compat_repository_file(&request, "read"),
            Some("repository-file-write") => self.compat_repository_file(&request, "write"),
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
            Some("agent-auth") => {
                self.auth.refresh_all();
                self.auth.snapshots()
            }
            Some("agent-clis") => self.cli_versions.snapshots(),
            Some("host-update") => {
                let action = request.get("action").and_then(Value::as_str);
                if action == Some("restart")
                    && (self
                        .runtime
                        .as_ref()
                        .is_some_and(TurnRuntime::has_active_turns)
                        || self.terminals.has_sessions())
                {
                    error_reply(
                        "Close terminal tabs and wait for active chats to finish before restarting for an update.",
                    )
                } else {
                    let reply = self.self_update.perform(action).unwrap_or_else(error_reply);
                    if action == Some("restart") && reply["ok"] == true {
                        if let Some(runtime) = &self.runtime {
                            runtime.shutdown();
                        }
                    }
                    reply
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
            Some("new-chat-sources") => self.read(|store| store.new_chat_sources(&request)),
            Some("set-draft") => self.event_mutation(|store| store.set_draft(&request)),
            Some("set-shortcuts") => self.event_mutation(|store| store.set_shortcuts(&request)),
            Some("set-option") => self.event_mutation(|store| store.set_option(&request)),
            Some("send") => self.send_message(&request),
            Some("respond") => {
                match (
                    request.get("chat").and_then(Value::as_str),
                    request.get("request_id").and_then(Value::as_str),
                    request.get("response"),
                    &self.runtime,
                ) {
                    (Some(chat), Some(id), Some(response), Some(runtime)) => runtime
                        .respond(chat, id, response)
                        .map(|()| json!({"ok":true}))
                        .unwrap_or_else(error_reply),
                    _ => error_reply("respond needs chat, request_id, and response"),
                }
            }
            Some("subagent") => {
                match (
                    request.get("chat").and_then(Value::as_str),
                    request.get("key").and_then(Value::as_str),
                    request.get("action").and_then(Value::as_str),
                    &self.runtime,
                ) {
                    (Some(chat), Some(key), Some(action), Some(runtime)) => runtime
                        .control_subagent(chat, key, action)
                        .map(|()| json!({"ok":true}))
                        .unwrap_or_else(error_reply),
                    _ => error_reply("subagent needs chat, key, and action"),
                }
            }
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
            Some("new-chat") => {
                let _resources = self
                    .session_resources
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                self.tree_mutation(|store| store.new_chat(&request))
            }
            Some("rename-session") => self.tree_mutation(|store| store.rename_session(&request)),
            Some("move-session") => self.tree_mutation(|store| store.move_session(&request)),
            Some("delete-session") => self.delete_session(&request),
            Some("rename-chat") => self.tree_mutation(|store| store.rename_chat(&request)),
            Some("move-chat") => self.tree_mutation(|store| store.move_chat(&request)),
            Some("delete-chat") => self.delete_chat(&request),
            Some("remove-worktree") => self.remove_worktree(&request),
            Some("queue") => self.event_mutation(|store| store.queue(&request)),
            Some("drop-queue") => self.event_mutation(|store| store.drop_queue(&request)),
            Some("edit-queue") => self.event_mutation(|store| store.edit_queue(&request)),
            Some("reorder-queue") => self.event_mutation(|store| store.reorder_queue(&request)),
            Some("steer-queue") => self.steer_queue(&request),
            Some("workflow-status") => self.workflow_status(owner, &request),
            Some("terminal-list") => self.terminal_list(&request),
            Some("terminal-open") => self.terminal_open(&request),
            Some("terminal-input") => self.terminals.input(&request).unwrap_or_else(error_reply),
            Some("terminal-materialize-image") => self.terminal_materialize_image(&request),
            Some("terminal-paste-image") => self.terminal_paste_image(&request),
            Some("terminal-resize") => self.terminals.resize(&request).unwrap_or_else(error_reply),
            Some("terminal-kill") => self.terminals.kill(&request).unwrap_or_else(error_reply),
            Some(operation) => json!({
                "ok": false,
                "error": format!("Operation {operation} is not implemented by the Rust host yet.")
            }),
            None => json!({"ok": false, "error": "Request must include a string op."}),
        };
        if let (Some(request_id), Some(reply)) = (request_id, reply.as_object_mut()) {
            reply.insert(REQUEST_ID.into(), request_id);
        }
        reply
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    pub fn shutdown(&self) {
        if let Some(runtime) = &self.runtime {
            runtime.shutdown();
        }
        if let Ok(mut subscribers) = self.events.subscribers.lock() {
            for subscriber in subscribers.values() {
                if let Some(connection) = &subscriber.connection {
                    let _ = connection.shutdown(std::net::Shutdown::Both);
                }
            }
            subscribers.clear();
        }
    }

    pub fn remote_listener(&self) -> Option<(String, u16)> {
        self.store.as_ref()?.remote_listener().ok().flatten()
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
            .map_err(|_| "host peer-listener state is unavailable".to_owned())? =
            Some(Arc::new(listener));
        Ok(())
    }

    fn peer_pairing(&self, owner: u64, request: &Value) -> Value {
        if !self.pairing.is_local(owner) {
            return error_reply("Pairing codes can only be created on the host machine.");
        }
        let bind = request
            .get("bind")
            .and_then(Value::as_str)
            .unwrap_or("0.0.0.0");
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
            Err(_) => return error_reply("Host peer-listener state is unavailable."),
        };
        let Some(listener) = listener else {
            return error_reply("This host cannot accept remote devices.");
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
            "fingerprint": endpoint.fingerprint,
        })
    }

    fn pair(&self, owner: u64, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
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
            return error_reply(
                "Pairing code expired or was already used. Create a new code on the host.",
            );
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
        self.clear_authentication_deadline(owner);
        json!({"ok": true, "token": token, "device": name})
    }

    fn hello(&self, owner: u64, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
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
        self.clear_authentication_deadline(owner);
        json!({"ok": true, "device": name, "version": 1})
    }

    fn clear_authentication_deadline(&self, owner: u64) {
        if let Ok(subscribers) = self.events.subscribers.lock() {
            if let Some(connection) = subscribers
                .get(&owner)
                .and_then(|subscriber| subscriber.connection.as_ref())
            {
                let _ = connection.set_read_timeout(None);
            }
        }
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
            None => error_reply("Rust host state storage is not configured."),
        }
    }

    fn tree(&self) -> Value {
        if let Err(error) = self.terminal_owner("") {
            return error_reply(error);
        }
        let mut tree = self.read(StateStore::tree);
        if tree.get("ok").and_then(Value::as_bool) == Some(true) {
            let activity = self.terminals.activity_snapshot();
            overlay_terminal_working(&mut tree, &activity.working_chats);
            tree["terminal_activity_epoch"] = Value::String(activity.epoch);
            tree["terminal_activity_revision"] = activity.revision.into();
        }
        tree
    }

    fn compat_repository_file(&self, request: &Value, action: &str) -> Value {
        let mut response = self.read(|store| store.compat_repository_file(request, action));
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return response;
        }
        if let Some(path) = request.get("path").cloned() {
            response["path"] = path;
        }
        if action == "read" {
            if response.get("truncated").is_none() {
                response["truncated"] = Value::Bool(false);
            }
        } else if let Some(content) = request.get("content").cloned() {
            response["content"] = content;
        }
        response
    }

    fn messages(&self, request: &Value) -> Value {
        let mut request = request.clone();
        request
            .as_object_mut()
            .map(|request| request.remove("_through_id"));
        if let (Some(runtime), Some(chat)) =
            (&self.runtime, request.get("chat").and_then(Value::as_str))
        {
            if let Some(live) = runtime.live_turn(chat) {
                request["_through_id"] = json!(live.message_boundary);
            }
        }
        self.read(|store| store.messages(&request))
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
            let interactions = runtime.interactions(chat_id);
            response["interactions"] = interactions["interactions"].clone();
            response["interaction_revision"] = interactions["interaction_revision"].clone();
            response["interaction_epoch"] = interactions["interaction_epoch"].clone();
            response["commands"] = Value::Array(
                runtime
                    .commands(chat_id, &provider)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            );
            if let Some(live) = runtime.live_turn(chat_id) {
                merge_live_turn(&mut response, live);
            }
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
            .ok_or_else(|| "Rust host state storage is not configured.".to_string())?;
        store
            .folder_path(folder)
            .map_err(|error| error.to_string())?;
        Ok(Some(folder.to_owned()))
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
            return error_reply("Rust host state storage is not configured.");
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
            return error_reply("Rust host state storage is not configured.");
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
            return error_reply("Rust host state storage is not configured.");
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
            return error_reply("Rust host state storage is not configured.");
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

    fn terminal_owner(&self, owner: &str) -> Result<String, String> {
        let Some(store) = self.store.as_ref() else {
            return Ok(owner.to_owned());
        };
        let aliases = store
            .terminal_owner_aliases()
            .map_err(|error| error.to_string())?;
        self.terminals.adopt_chat_owners(&aliases)?;
        Ok(aliases
            .get(owner)
            .cloned()
            .unwrap_or_else(|| owner.to_owned()))
    }

    fn terminal_list(&self, request: &Value) -> Value {
        let chat_id = match required_string(request, "chat", "terminal-list needs a chat id") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        match self.terminal_owner(chat_id) {
            Ok(owner) => match self.terminals.register_chat_alias(&owner, chat_id) {
                Ok(()) => self.terminals.list(&owner),
                Err(error) => error_reply(error),
            },
            Err(error) => error_reply(error),
        }
    }

    fn terminal_paste_image(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let path = match store.materialize_terminal_image(request) {
            Ok(path) => path,
            Err(error) => return error_reply(error),
        };
        match self.terminals.paste_image(request, &path) {
            Ok(reply) => reply,
            Err(error) => {
                let _ = fs::remove_file(path);
                error_reply(error)
            }
        }
    }

    fn terminal_materialize_image(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        match store.materialize_terminal_image(request) {
            Ok(path) => json!({"ok": true, "path": path}),
            Err(error) => error_reply(error),
        }
    }

    fn delete_chat(&self, request: &Value) -> Value {
        let chat_id = match required_string(request, "chat", "delete-chat needs a chat id") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        let gate = match self.chat_execution_gate(chat_id) {
            Ok(gate) => gate,
            Err(error) => return error_reply(error),
        };
        let _execution = match gate.lock() {
            Ok(execution) => execution,
            Err(_) => return error_reply("This session's agent state is unavailable."),
        };
        let _resources = match self.session_resources.lock() {
            Ok(resources) => resources,
            Err(_) => return error_reply("Session resources are unavailable."),
        };
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        if let Err(error) = self.terminal_owner(chat_id) {
            return error_reply(error);
        }
        match store.delete_chat(request) {
            Ok(reply) => {
                if let Some(runtime) = &self.runtime {
                    runtime.discard_chat(chat_id);
                }
                if reply["session_deleted"] == true {
                    if let Some(session) = reply["session"].as_str() {
                        self.terminals.kill_chat(&format!("session:{session}"));
                    }
                }
                self.events.publish(json!({"event": "tree"}));
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    fn delete_session(&self, request: &Value) -> Value {
        let session =
            match required_string(request, "session", "delete-session needs a session id.") {
                Ok(session) => session,
                Err(error) => return error_reply(error),
            };
        let _resources = match self.session_resources.lock() {
            Ok(resources) => resources,
            Err(_) => return error_reply("Session resources are unavailable."),
        };
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let chats = match store.session_chats(session) {
            Ok(chats) => chats,
            Err(error) => return error_reply(error),
        };
        if let Err(error) = self.terminal_owner(session) {
            return error_reply(error);
        }
        match store.delete_session(request) {
            Ok(reply) => {
                for chat in chats {
                    if let Some(runtime) = &self.runtime {
                        runtime.discard_chat(&chat);
                    }
                }
                self.terminals.kill_chat(&format!("session:{session}"));
                self.events.publish(json!({"event": "tree"}));
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    fn terminal_open(&self, request: &Value) -> Value {
        let chat_id = match required_string(request, "chat", "terminal-open needs a chat id") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        let _resources = match self.session_resources.lock() {
            Ok(resources) => resources,
            Err(_) => return error_reply("Session resources are unavailable."),
        };
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let owner = match self.terminal_owner(chat_id) {
            Ok(owner) => owner,
            Err(error) => return error_reply(error),
        };
        let mut canonical_request = request.clone();
        canonical_request["chat"] = json!(owner);
        canonical_request["_terminal_alias"] = if chat_id != owner {
            json!(chat_id)
        } else {
            Value::Null
        };
        match store.terminal_workdir(&owner) {
            Ok(workdir) => self
                .terminals
                .open(&canonical_request, &workdir)
                .unwrap_or_else(error_reply),
            Err(error) => error_reply(error),
        }
    }

    fn chat_execution_gate(&self, chat_id: &str) -> Result<Arc<Mutex<()>>, String> {
        let mut chats = self
            .chat_execution
            .lock()
            .map_err(|_| "Session agent state is unavailable.".to_string())?;
        Ok(chats
            .entry(chat_id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone())
    }

    fn tree_mutation(
        &self,
        operation: impl FnOnce(&StateStore) -> Result<Value, StorageError>,
    ) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
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
            return error_reply("Rust host state storage is not configured.");
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
            return error_reply("Rust host state storage is not configured.");
        };
        match operation(store) {
            Ok((reply, event)) => {
                if let Some(chats) = event.get("workspace_chats").and_then(Value::as_array) {
                    for chat in chats {
                        if chat != &event["chat"] {
                            self.events
                                .publish(json!({"event": "changed", "chat": chat}));
                        }
                    }
                    self.events.publish(json!({"event": "tree"}));
                    self.events.publish(json!({"event": "worktrees-changed"}));
                }
                self.events.publish(event);
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    fn send_message(&self, request: &Value) -> Value {
        let chat_id = match required_string(request, "chat", "send needs a chat id") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        let gate = match self.chat_execution_gate(chat_id) {
            Ok(gate) => gate,
            Err(error) => return error_reply(error),
        };
        let _execution = match gate.lock() {
            Ok(execution) => execution,
            Err(_) => return error_reply("This session's agent state is unavailable."),
        };
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return error_reply("Rust host state storage is not configured.");
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
        let _resources = match self.session_resources.lock() {
            Ok(resources) => resources,
            Err(_) => return error_reply("Session resources are unavailable."),
        };
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
        let killed = self
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.cancel(chat_id));
        // Nothing to kill but the chat still reads as working: the turn died
        // with a previous host. Stop should still get the chat back.
        if !killed
            && let Some(store) = self.store.as_ref()
            && store.clear_interrupted_turn(chat_id).unwrap_or(false)
        {
            self.events
                .publish(json!({"event": "changed", "chat": chat_id}));
        }
        json!({"ok": true})
    }

    fn steer_queue(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
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
            return error_reply("Rust host state storage is not configured.");
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
        let owner =
            self.events
                .subscribe_with_auth(sender, Some(connection), authenticated.clone())?;
        if let Err(error) = self.pairing.register(owner, transport, authenticated) {
            self.events.unsubscribe(owner);
            return Err(error);
        }
        Ok(owner)
    }

    fn subscribe_stdio(&self, sender: SyncSender<Value>) -> Result<u64, String> {
        let authenticated = Arc::new(AtomicBool::new(true));
        let owner = self
            .events
            .subscribe_with_auth(sender, None, authenticated.clone())?;
        if let Err(error) = self
            .pairing
            .register(owner, Transport::Local, authenticated)
        {
            self.events.unsubscribe(owner);
            return Err(error);
        }
        Ok(owner)
    }

    fn unsubscribe(&self, id: u64) {
        self.pairing.unregister(id);
        self.events.unsubscribe(id);
    }
}

impl EventBus {
    fn with_store(store: Arc<StateStore>) -> Self {
        Self {
            shared: Some(SharedEventPublisher {
                store,
                source: uuid::Uuid::new_v4().to_string(),
            }),
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn subscribe(&self, sender: SyncSender<Value>, connection: UnixStream) -> Result<u64, String> {
        self.subscribe_with_auth(sender, Some(connection), Arc::new(AtomicBool::new(true)))
    }

    fn subscribe_with_auth(
        &self,
        sender: SyncSender<Value>,
        connection: Option<UnixStream>,
        authenticated: Arc<AtomicBool>,
    ) -> Result<u64, String> {
        // Zero is reserved for direct/local dispatches that do not own a socket.
        let id = self.next_subscriber.fetch_add(1, Ordering::Relaxed) + 1;
        self.subscribers
            .lock()
            .map_err(|_| "host event state is unavailable".to_string())?
            .insert(
                id,
                Subscriber {
                    sender,
                    connection,
                    authenticated,
                    events_ready: None,
                },
            );
        Ok(id)
    }

    fn unsubscribe(&self, id: u64) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.remove(&id);
        }
    }

    fn publish(&self, event: Value) {
        if shared_event(&event)
            && let Some(shared) = &self.shared
            && let Err(error) = shared.store.append_shared_event(&shared.source, &event)
        {
            eprintln!("xd-host: cannot relay chat event: {error}");
        }
        self.publish_local(event);
    }

    fn publish_local(&self, mut event: Value) {
        let id = self.next_event.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(event) = event.as_object_mut() {
            event.insert("id".into(), id.into());
        }
        let Ok(mut subscribers) = self.subscribers.lock() else {
            return;
        };
        subscribers.retain(|_, subscriber| {
            if !subscriber.authenticated.load(Ordering::Acquire)
                || subscriber
                    .events_ready
                    .as_ref()
                    .is_some_and(|ready| !ready.load(Ordering::Acquire))
            {
                true
            } else {
                match subscriber.sender.try_send(event.clone()) {
                    Ok(()) => true,
                    Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                        if let Some(connection) = &subscriber.connection {
                            let _ = connection.shutdown(std::net::Shutdown::Both);
                        }
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
                    if let Some(connection) = &subscriber.connection {
                        let _ = connection.shutdown(std::net::Shutdown::Both);
                    }
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
            if let Some(connection) = &subscriber.connection {
                let _ = connection.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    fn subscribed(&self, subscriber_id: u64) -> bool {
        self.subscribers
            .lock()
            .is_ok_and(|subscribers| subscribers.contains_key(&subscriber_id))
    }
}

impl SharedEventMonitor {
    fn start(events: Arc<EventBus>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let initial_cursor = events
            .shared
            .as_ref()
            .and_then(|shared| shared.store.latest_shared_event_id().ok())
            .unwrap_or(0);
        let worker = thread::Builder::new()
            .name("xd-shared-events".into())
            .spawn(move || {
                let Some(shared) = events.shared.as_ref() else {
                    return;
                };
                let mut cursor = initial_cursor;
                while !worker_stop.load(Ordering::Acquire) {
                    thread::park_timeout(Duration::from_millis(50));
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(pending) = shared.store.shared_events_after(cursor) else {
                        continue;
                    };
                    for (id, source, event) in pending {
                        cursor = id;
                        if source != shared.source {
                            events.publish_local(event);
                        }
                    }
                }
            })
            .ok();
        Self { stop, worker }
    }
}

impl Drop for SharedEventMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn shared_event(event: &Value) -> bool {
    matches!(
        event.get("event").and_then(Value::as_str),
        Some(
            "tree"
                | "changed"
                | "repository-changed"
                | "turn-started"
                | "text"
                | "tool"
                | "turn-finished"
                | "commands"
                | "queued"
                | "draft"
                | "shortcuts-changed"
                | "terminal-activity"
        )
    )
}

#[cfg(test)]
pub fn serve_connection(stream: UnixStream) -> std::io::Result<()> {
    serve_connection_with_engine(stream, Arc::new(Engine::transport_only()), Transport::Local)
}

#[cfg(test)]
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
    if result.is_err() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    drop(outbound);
    let writer_result = writer
        .join()
        .map_err(|_| std::io::Error::other("host writer panicked"))?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    result.and(writer_result)
}

/// Serve an authenticated connection to the shared daemon.
pub fn serve_daemon_connection(
    stream: UnixStream,
    engine: Arc<Engine>,
    remote: bool,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let (outbound, frames) = sync_channel(256);
    let subscriber = engine
        .subscribe_transport(
            outbound.clone(),
            stream.try_clone()?,
            if remote {
                Transport::Remote
            } else {
                Transport::Local
            },
        )
        .map_err(std::io::Error::other)?;
    let events_ready = Arc::new(AtomicBool::new(!remote));
    if let Ok(mut subscribers) = engine.events.subscribers.lock() {
        if let Some(client) = subscribers.get_mut(&subscriber) {
            client.events_ready = Some(events_ready.clone());
        }
    }
    let writer_stream = stream.try_clone()?;
    let writer = thread::spawn(move || write_frames(writer_stream, frames));
    let (commands, queued) = sync_channel::<Value>(128);
    let worker_engine = engine.clone();
    let replies = outbound.clone();
    let worker_stream = stream.try_clone()?;
    let worker = thread::spawn(move || {
        while let Ok(request) = queued.recv() {
            if !worker_engine.events.subscribed(subscriber) {
                break;
            }
            let reply = worker_engine.dispatch_for(subscriber, request);
            if replies.try_send(reply).is_err() {
                let _ = worker_stream.shutdown(std::net::Shutdown::Both);
                break;
            }
        }
    });
    let result = (|| -> std::io::Result<()> {
        loop {
            let authenticated = engine.pairing.authenticated(subscriber);
            let limit = if authenticated {
                FRAME_LIMIT
            } else {
                AUTH_FRAME_LIMIT
            };
            let mut line = Vec::new();
            if reader
                .by_ref()
                .take((limit + 1) as u64)
                .read_until(b'\n', &mut line)?
                == 0
            {
                return Ok(());
            }
            if line.len() > limit {
                return Err(std::io::Error::other("Frame exceeds protocol limit"));
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let request = match serde_json::from_slice::<Value>(&line) {
                Ok(request) => request,
                Err(_) => {
                    outbound
                        .try_send(error_reply("Invalid JSON request"))
                        .map_err(std::io::Error::other)?;
                    continue;
                }
            };
            let control = !authenticated
                || matches!(
                    request["op"].as_str(),
                    Some(
                        "ping"
                            | "cancel"
                            | "respond"
                            | "subagent"
                            | "agent-auth-cancel"
                            | "daemon-stop"
                    )
                );
            if control {
                let authenticating = matches!(request["op"].as_str(), Some("pair" | "hello"));
                let reply = engine.dispatch_for(subscriber, request);
                let authenticated = authenticating && reply["ok"] == true;
                outbound.try_send(reply).map_err(std::io::Error::other)?;
                if authenticated {
                    events_ready.store(true, Ordering::Release);
                }
            } else {
                commands
                    .try_send(request)
                    .map_err(|_| std::io::Error::other("Client command queue overflowed"))?;
            }
        }
    })();
    drop(commands);
    if result.is_err() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    // A half-close requests a final drain; a disconnected client cannot block
    // this forever because daemon sockets have bounded write deadlines.
    let _ = worker.join();
    engine.unsubscribe(subscriber);
    drop(outbound);
    let writer_result = writer
        .join()
        .map_err(|_| std::io::Error::other("Daemon writer panicked"))?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    result.and(writer_result)
}

/// Serve an isolated stdio host (used by transport tests).
pub fn serve_stdio(
    engine: Engine,
    reader: impl Read,
    writer: impl Write + Send + 'static,
) -> std::io::Result<()> {
    let engine = Arc::new(engine);
    let mut reader = BufReader::new(reader);
    let (outbound, frames) = sync_channel(256);
    let subscriber = engine
        .subscribe_stdio(outbound.clone())
        .map_err(std::io::Error::other)?;
    let writer = thread::Builder::new()
        .name("xd-rust-stdio-writer".into())
        .spawn(move || write_frames(writer, frames))?;
    let result = read_requests(&mut reader, &engine, subscriber, &outbound);
    engine.unsubscribe(subscriber);
    drop(outbound);
    if result.is_err() {
        // A full event queue usually means stdout itself is blocked. Waiting
        // for that writer here would keep a dead stdio transport alive forever;
        // returning lets the dedicated host process exit and the client resync.
        return result;
    }
    let writer_result = writer
        .join()
        .map_err(|_| std::io::Error::other("host writer panicked"))?;
    result.and(writer_result)
}

fn read_requests(
    reader: &mut impl BufRead,
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
        if !engine.events.subscribed(subscriber) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "client event queue overflowed",
            ));
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
        outbound.try_send(reply).map_err(|error| match error {
            TrySendError::Full(_) => std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "client output queue overflowed",
            ),
            TrySendError::Disconnected(_) => {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client closed")
            }
        })?;
    }
}

fn write_frames(
    mut stream: impl Write,
    frames: std::sync::mpsc::Receiver<Value>,
) -> std::io::Result<()> {
    while let Ok(frame) = frames.recv() {
        let mut encoded = serde_json::to_vec(&frame)?;
        encoded.push(b'\n');
        stream.write_all(&encoded)?;
        stream.flush()?;
    }
    Ok(())
}

#[cfg(test)]
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

fn overlay_terminal_working(tree: &mut Value, working_chats: &HashSet<String>) {
    let Some(chats) = tree.get_mut("chats").and_then(Value::as_array_mut) else {
        return;
    };
    for chat in chats {
        let working = chat
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|chat_id| working_chats.contains(chat_id))
            || chat
                .get("session")
                .and_then(Value::as_str)
                .is_some_and(|session| working_chats.contains(&format!("session:{session}")));
        chat["terminal_working"] = Value::Bool(working);
    }
    let active_sessions = tree
        .get("chats")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|chat| chat["terminal_working"] == true)
        .filter_map(|chat| chat["session"].as_str().map(str::to_owned))
        .collect::<HashSet<_>>();
    if let Some(sessions) = tree.get_mut("sessions").and_then(Value::as_array_mut) {
        for session in sessions {
            session["terminal_working"] = json!(
                session["id"]
                    .as_str()
                    .is_some_and(|id| active_sessions.contains(id))
            );
        }
    }
}

/// Tell a client that just loaded a chat where its live turn already is.
///
/// A chat row only records *that* a turn is running. Without the rest, a client
/// counts the turn's age from the moment it looked, and shows none of the reply
/// streamed before then. `segment` carries only text that is not a message yet:
/// blocks and tool calls are stored as they arrive and come back in `messages`.
fn merge_live_turn(response: &mut Value, live: LiveTurn) {
    response["turn_id"] = live.turn_id.into();
    response["turn_epoch"] = live.epoch.into();
    response["items"] = Value::Array(live.items);
    response["turn_sequence"] = live.sequence.into();
    response["working_for"] = live.working_for.into();
    response["label"] = Value::String(live.label);
    response["segment"] = Value::String(live.segment);
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
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::{
        env,
        io::{BufRead, BufReader},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn outgoing_json_frames_are_written_as_complete_lines() {
        #[derive(Default)]
        struct RecordingWriter {
            writes: Vec<Vec<u8>>,
            flushes: usize,
        }
        impl Write for &mut RecordingWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.writes.push(bytes.to_vec());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushes += 1;
                Ok(())
            }
        }
        let frames = [
            json!({"event":"tree","sessions":[{"id":"session","chats":["one","two"]}]}),
            json!({"ok":true,"text":"escaped \"quotes\" and \nnewlines"}),
        ];
        let (sender, receiver) = std::sync::mpsc::channel();
        for frame in &frames {
            sender.send(frame.clone()).unwrap();
        }
        drop(sender);
        let mut writer = RecordingWriter::default();
        write_frames(&mut writer, receiver).unwrap();
        assert_eq!(writer.flushes, frames.len());
        assert_eq!(writer.writes.len(), frames.len());
        for (bytes, expected) in writer.writes.iter().zip(frames) {
            assert_eq!(bytes.last(), Some(&b'\n'));
            assert_eq!(serde_json::from_slice::<Value>(bytes).unwrap(), expected);
        }
    }

    #[test]
    fn a_loaded_chat_carries_where_its_live_turn_already_is() {
        let mut response = json!({"ok": true, "working": true, "label": null});
        merge_live_turn(
            &mut response,
            LiveTurn {
                epoch: "epoch-1".into(),
                message_boundary: 42,
                items: Vec::new(),
                turn_id: 7,
                sequence: 12,
                label: "Codex".into(),
                working_for: 41,
                segment: "Half an ans".into(),
            },
        );
        // A client counts the turn's age from this, so it has to be the age of
        // the turn and not of the request.
        assert_eq!(response["working_for"], 41);
        assert_eq!(response["turn_id"], 7);
        assert_eq!(response["turn_sequence"], 12);
        assert_eq!(response["label"], "Codex");
        assert_eq!(response["segment"], "Half an ans");
    }

    #[test]
    fn ping_echoes_the_private_request_id() {
        assert_eq!(
            dispatch(json!({"op": "ping", "_xd_request": 42})),
            json!({"ok": true, "_xd_request": 42})
        );
    }

    #[test]
    fn tree_activity_overlay_is_transient_and_total_for_visible_chats() {
        let mut tree = json!({
            "ok": true,
            "chats": [
                {"id": "idle", "working": false},
                {"id": "direct", "working": false},
                {"id": "managed", "working": true}
            ]
        });

        overlay_terminal_working(
            &mut tree,
            &std::collections::HashSet::from(["direct".to_owned(), "hidden".to_owned()]),
        );

        assert_eq!(tree["chats"][0]["terminal_working"], false);
        assert_eq!(tree["chats"][1]["terminal_working"], true);
        assert_eq!(tree["chats"][2]["terminal_working"], false);
        assert_eq!(tree["chats"][1]["working"], false);
    }

    #[test]
    fn tree_replies_include_the_terminal_activity_epoch_and_revision() {
        let root = test_directory();
        let store = StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap();
        let engine = Engine::with_store(store);

        let tree = engine.dispatch(json!({"op": "tree"}));

        assert_eq!(tree["ok"], true);
        assert!(!tree["terminal_activity_epoch"].as_str().unwrap().is_empty());
        assert_eq!(tree["terminal_activity_revision"], 0);
        drop(engine);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unknown_operations_are_explicit_and_correlated() {
        let reply = dispatch(json!({"op": "terminal-open", "_xd_request": 7}));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["_xd_request"], 7);
        assert!(reply["error"].as_str().unwrap().contains("terminal-open"));
    }

    #[test]
    fn host_does_not_expose_the_orphaned_auth_refresh_operation() {
        let operation = "agent-auth-refresh";
        let reply = dispatch(json!({"op": operation}));
        assert_eq!(reply["ok"], false);
        assert_eq!(
            reply["error"],
            format!("Operation {operation} is not implemented by the Rust host yet.")
        );
    }

    #[test]
    fn legacy_repository_operations_are_compatibility_aliases() {
        use std::process::Command;

        let root = test_directory();
        let workspaces = root.join("Workspaces");
        let store = StateStore::open(root.join("chats.db"), workspaces.clone()).unwrap();
        let engine = Engine::with_store(store);
        let folder = engine.dispatch(json!({"op": "new-folder", "name": "Project"}))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let repository = workspaces.join("Project");
        fs::create_dir_all(repository.join("src")).unwrap();
        fs::write(repository.join("src/main.rs"), "before\n").unwrap();
        assert!(
            Command::new("git")
                .arg("init")
                .arg(&repository)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["-C", repository.to_str().unwrap(), "add", "src/main.rs"])
                .status()
                .unwrap()
                .success()
        );
        let chat = engine.dispatch(json!({
            "op": "new-chat",
            "folder": folder,
            "backend": "codex",
            "workdir": repository.join("src"),
        }))["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let files = engine.dispatch(json!({"op": "repository-files", "chat": chat}));
        assert_eq!(files["files"], json!(["src/main.rs"]));
        let read = engine.dispatch(json!({
            "op": "repository-file",
            "chat": chat,
            "path": "src/main.rs",
            "content": "ignored request member",
        }));
        assert_eq!(read["content"], "before\n");
        let write = engine.dispatch(json!({
            "op": "repository-file-write",
            "chat": chat,
            "path": "src/main.rs",
            "original": "before\n",
            "content": "after\n",
        }));
        assert_eq!(write["content"], "after\n");
        assert_eq!(
            fs::read_to_string(repository.join("src/main.rs")).unwrap(),
            "after\n"
        );

        fs::write(repository.join("large.txt"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        let preview = engine.dispatch(json!({
            "op": "repository-file",
            "chat": chat,
            "path": "large.txt",
        }));
        assert_eq!(preview["ok"], true);
        assert_eq!(preview["content"].as_str().unwrap().len(), 128 * 1024);
        assert_eq!(preview["truncated"], true);

        drop(engine);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn terminal_agent_chats_are_no_longer_supported() {
        let reply = dispatch(json!({
            "op": "terminal-open-agent", "chat": "legacy", "agent": "codex"
        }));
        assert_eq!(reply["ok"], false);
    }

    #[test]
    fn terminal_image_paste_materializes_the_image_on_the_host_machine() {
        let root = test_directory();
        let workspaces = root.join("Workspaces");
        let store = StateStore::open(root.join("chats.db"), workspaces).unwrap();
        let engine = Engine::with_store(store);
        let folder = engine.dispatch(json!({
            "op": "new-folder",
            "name": "Project",
        }))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let chat = engine.dispatch(json!({
            "op": "new-chat",
            "folder": folder,
            "backend": "codex",
        }))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let opened = engine.dispatch(json!({
            "op": "terminal-open",
            "chat": chat,
            "columns": 80,
            "rows": 24,
            "reuse": false,
        }));
        assert_eq!(opened["ok"], true, "{opened}");
        let terminal = opened["id"].as_str().unwrap().to_owned();
        let png = b"\x89PNG\r\n\x1a\n";

        let pasted = engine.dispatch(json!({
            "op": "terminal-paste-image",
            "terminal": terminal,
            "attachments": [{
                "name": "screenshot.png",
                "mime": "image/png",
                "data": STANDARD.encode(png),
            }],
        }));
        let _ = engine.dispatch(json!({"op": "terminal-kill", "terminal": terminal}));

        assert_eq!(pasted["ok"], true, "{pasted}");
        let path = PathBuf::from(pasted["path"].as_str().unwrap());
        assert!(path.starts_with(root.join("remote-pasted")));
        assert_eq!(fs::read(path).unwrap(), png);
        drop(engine);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn global_terminal_opens_without_a_chat() {
        let root = test_directory();
        let store = StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap();
        let engine = Engine::with_store(store);

        let opened = engine.dispatch(json!({
            "op": "terminal-open",
            "chat": "global:mobile",
            "columns": 80,
            "rows": 24,
            "reuse": true,
        }));

        assert_eq!(opened["ok"], true, "{opened}");
        let terminal = opened["id"].as_str().expect("global terminal id");
        let _ = engine.dispatch(json!({"op": "terminal-kill", "terminal": terminal}));
        drop(engine);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn shared_session_terminals_survive_initial_tab_deletion_and_close_with_the_session() {
        let root = test_directory();
        let store = StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap();
        let engine = Engine::with_store(store);
        let folder = engine.dispatch(json!({"op": "new-folder", "name": "Project"}))["id"].clone();
        let first = engine.dispatch(json!({"op": "new-chat", "folder": folder}));
        let resource = first["terminal_chat"].clone();
        let opened = engine
            .terminals
            .open(
                &json!({"chat":first["id"]}),
                &root.join("Workspaces/Project"),
            )
            .unwrap();
        let adopted = engine.dispatch(json!({"op":"terminal-list","chat":resource}));
        assert_eq!(adopted["terminals"].as_array().unwrap().len(), 1);
        assert_eq!(adopted["terminals"][0]["id"], opened["id"]);
        assert_eq!(
            engine.dispatch(json!({"op":"terminal-list","chat":first["id"]}))["terminals"][0]["id"],
            opened["id"]
        );
        let (sender, terminal_events) = sync_channel(64);
        let subscriber = engine
            .events
            .subscribe_with_auth(sender, None, Arc::new(AtomicBool::new(true)))
            .unwrap();
        engine.terminals.input(&json!({"terminal":opened["id"],"data":STANDARD.encode(b"printf 'ownership-probe\\n'\n")})).unwrap();
        let mut owners = HashSet::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while owners.len() < 2 {
            let event = terminal_events
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap();
            if event["event"] == "terminal-output"
                && event["terminal"] == opened["id"]
                && event["data"]
                    .as_str()
                    .and_then(|data| STANDARD.decode(data).ok())
                    .is_some_and(|data| String::from_utf8_lossy(&data).contains("ownership-probe"))
            {
                owners.insert(event["chat"].as_str().unwrap().to_owned());
            }
        }
        assert_eq!(
            owners,
            HashSet::from([
                first["id"].as_str().unwrap().to_owned(),
                resource.as_str().unwrap().to_owned()
            ])
        );
        engine.events.unsubscribe(subscriber);
        let replay = engine.dispatch(json!({"op":"terminal-list","chat":resource}))["terminals"][0]
            ["replay"]
            .clone();
        assert!(
            replay
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|frame| frame["data"].as_str())
                .filter_map(|data| STANDARD.decode(data).ok())
                .any(|data| String::from_utf8_lossy(&data).contains("ownership-probe"))
        );
        let second =
            engine.dispatch(json!({"op":"new-chat","session":first["session"],"backend":"claude"}));
        assert_eq!(second["ok"], true, "{second}");
        let deleted = engine.dispatch(json!({"op": "delete-chat", "chat": first["id"]}));
        assert_eq!(deleted["session_deleted"], false);
        let reused =
            engine.dispatch(json!({"op": "terminal-open", "chat": resource, "reuse": true}));
        assert_eq!(reused["id"], opened["id"]);
        let deleted = engine.dispatch(json!({"op": "delete-chat", "chat": second["id"]}));
        assert_eq!(deleted["session_deleted"], true);
        assert_eq!(
            engine.dispatch(json!({"op": "terminal-list", "chat": resource}))["terminals"],
            json!([])
        );
        assert_eq!(
            engine.dispatch(json!({"op": "terminal-open", "chat": resource}))["ok"],
            false
        );
        let first = engine.dispatch(json!({"op": "new-chat", "folder": folder}));
        let second = engine.dispatch(json!({"op": "new-chat", "session": first["session"]}));
        let resource = first["terminal_chat"].clone();
        assert_eq!(
            engine.dispatch(json!({"op": "terminal-open", "chat": resource}))["ok"],
            true
        );
        assert_eq!(
            engine.dispatch(json!({"op": "delete-session", "session": first["session"]}))["ok"],
            true
        );
        assert_eq!(
            engine.dispatch(json!({"op": "terminal-list", "chat": resource}))["terminals"],
            json!([])
        );
        assert_eq!(
            engine.dispatch(json!({"op": "chat", "chat": second["id"]}))["ok"],
            false
        );
        drop(engine);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn deleting_a_chat_waits_for_its_agent_execution_gate() {
        let engine = Arc::new(Engine::transport_only());
        let gate = engine.chat_execution_gate("missing").unwrap();
        let gate = gate.lock().unwrap();
        let (finished, reply) = std::sync::mpsc::channel();
        let worker = engine.clone();

        thread::spawn(move || {
            finished
                .send(worker.dispatch(json!({"op": "delete-chat", "chat": "missing"})))
                .unwrap();
        });

        assert!(matches!(
            reply.recv_timeout(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        drop(gate);
        assert_eq!(
            reply
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()["ok"],
            false
        );
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
    fn chat_events_cross_host_process_boundaries() {
        let root = test_directory();
        let database = root.join("chats.db");
        let workspaces = root.join("Workspaces");
        let first =
            Engine::with_store(StateStore::open(&database, &workspaces).expect("first host store"));
        let second = Engine::with_store(
            StateStore::open(&database, &workspaces).expect("second host store"),
        );
        let (sender, receiver) = sync_channel(8);
        let (connection, peer) = UnixStream::pair().unwrap();
        let subscriber = second.events.subscribe(sender, connection).unwrap();
        while receiver.try_recv().is_ok() {}

        first.events.publish(json!({
            "event": "text",
            "chat": "chat-1",
            "text": "live from desktop",
            "turn_id": 7,
            "turn_sequence": 3,
        }));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let event = loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let event = receiver
                .recv_timeout(remaining)
                .expect("the other host process should relay the live chat event");
            if event["event"] == "text" {
                break event;
            }
        };
        assert_eq!(event["event"], "text");
        assert_eq!(event["chat"], "chat-1");
        assert_eq!(event["text"], "live from desktop");

        second.events.unsubscribe(subscriber);
        drop(peer);
    }

    #[test]
    fn shared_chat_events_do_not_echo_or_relay_terminal_output() {
        let root = test_directory();
        let database = root.join("chats.db");
        let workspaces = root.join("Workspaces");
        let first =
            Engine::with_store(StateStore::open(&database, &workspaces).expect("first host store"));
        let second = Engine::with_store(
            StateStore::open(&database, &workspaces).expect("second host store"),
        );
        let (first_sender, first_receiver) = sync_channel(8);
        let (first_connection, first_peer) = UnixStream::pair().unwrap();
        let first_subscriber = first
            .events
            .subscribe(first_sender, first_connection)
            .unwrap();
        let (second_sender, second_receiver) = sync_channel(8);
        let (second_connection, second_peer) = UnixStream::pair().unwrap();
        let second_subscriber = second
            .events
            .subscribe(second_sender, second_connection)
            .unwrap();

        first
            .events
            .publish(json!({"event": "text", "chat": "chat-1", "text": "once"}));
        assert_eq!(next_event(&first_receiver, "text")["text"], "once");
        assert_eq!(next_event(&second_receiver, "text")["text"], "once");
        first.events.publish(json!({
            "event": "terminal-output",
            "chat": "chat-1",
            "terminal": "terminal-1",
            "data": "not shared",
        }));
        assert_eq!(
            next_event(&first_receiver, "terminal-output")["data"],
            "not shared"
        );
        assert!(!receives_event(
            &second_receiver,
            "terminal-output",
            std::time::Duration::from_millis(150),
        ));
        assert!(!receives_event(
            &first_receiver,
            "text",
            std::time::Duration::from_millis(150),
        ));

        first.events.unsubscribe(first_subscriber);
        second.events.unsubscribe(second_subscriber);
        drop(first_peer);
        drop(second_peer);
    }

    fn next_event(receiver: &std::sync::mpsc::Receiver<Value>, name: &str) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let event = receiver
                .recv_timeout(remaining)
                .expect("expected host event");
            if event["event"] == name {
                return event;
            }
        }
    }

    fn receives_event(
        receiver: &std::sync::mpsc::Receiver<Value>,
        name: &str,
        timeout: std::time::Duration,
    ) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(event) if event["event"] == name => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
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
    fn overflowed_stdio_subscription_rejects_later_requests() {
        let engine = Engine::transport_only();
        let (events, frames) = sync_channel(1);
        let subscriber = engine.subscribe_stdio(events.clone()).unwrap();
        engine.events.publish(json!({"event": "tree"}));
        engine.events.publish(json!({"event": "tree"}));
        assert!(engine.events.subscribers.lock().unwrap().is_empty());

        let _queued = frames.recv().unwrap();
        let mut requests = BufReader::new(b"{\"op\":\"ping\",\"_xd_request\":1}\n".as_slice());
        let error = read_requests(&mut requests, &engine, subscriber, &events).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert!(
            frames.try_recv().is_err(),
            "overflowed transport replied after unsubscribe"
        );
    }

    #[test]
    fn reply_only_stdio_backpressure_disconnects_instead_of_blocking() {
        let engine = Engine::transport_only();
        let (outbound, frames) = sync_channel(1);
        let subscriber = engine.subscribe_stdio(outbound.clone()).unwrap();
        outbound.send(json!({"ok": true, "queued": true})).unwrap();
        let (done, result) = sync_channel(1);
        let worker = thread::spawn(move || {
            let mut requests = BufReader::new(b"{\"op\":\"ping\"}\n".as_slice());
            done.send(read_requests(&mut requests, &engine, subscriber, &outbound))
                .unwrap();
        });

        let returned = result.recv_timeout(Duration::from_millis(250));
        if returned.is_err() {
            let _ = frames.recv();
        }
        worker.join().unwrap();

        let error = returned
            .expect("reply enqueue blocked behind the full stdio queue")
            .expect_err("full stdio queue should disconnect the transport");
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    struct OverflowingStdioReader {
        events: Arc<EventBus>,
        request: std::io::Cursor<Vec<u8>>,
        published: bool,
    }

    impl Read for OverflowingStdioReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.published {
                self.published = true;
                for _ in 0..512 {
                    self.events.publish(json!({"event": "tree"}));
                }
            }
            self.request.read(buffer)
        }
    }

    struct BlockedStdioWriter {
        released: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }

    impl Write for BlockedStdioWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            let (released, wake) = &*self.released;
            let mut released = released.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn overflowed_stdio_returns_without_waiting_for_a_blocked_writer() {
        let engine = Engine::transport_only();
        let events = engine.events.clone();
        let released = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let writer = BlockedStdioWriter {
            released: released.clone(),
        };
        let reader = OverflowingStdioReader {
            events,
            request: std::io::Cursor::new(b"{\"op\":\"ping\"}\n".to_vec()),
            published: false,
        };
        let (done, result) = sync_channel(1);
        let worker = thread::spawn(move || done.send(serve_stdio(engine, reader, writer)).unwrap());

        let returned = result.recv_timeout(Duration::from_millis(250)).is_ok();
        let (released, wake) = &*released;
        *released.lock().unwrap() = true;
        wake.notify_all();
        worker.join().unwrap();

        assert!(returned, "stdio waited forever for its black-holed writer");
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
            "Device management is only available on the host machine."
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
                    fingerprint: "ab".repeat(32),
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
            "Pairing codes can only be created on the host machine."
        );
        engine.unsubscribe(remote);
        drop(peer);
    }

    fn test_directory() -> PathBuf {
        env::temp_dir().join(format!(
            "xd-rust-host-test-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
