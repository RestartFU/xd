use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

mod accounts;
mod agent;
mod agent_text;
mod agent_tools;
mod ask;
mod auth;
mod background_process;
mod background_tasks;
mod chat_title;
mod child_agents;
mod cli_updates;
mod cli_versions;
mod codex_app;
mod database;
mod delegation;
mod deletion;
mod git_draft;
mod local_socket;
#[cfg(target_os = "linux")]
mod memory_guard;
mod model_catalog;
mod owner_icon;
mod pairing;
mod private_fs;
#[cfg(target_os = "linux")]
mod process_delegation;
mod process_tree;
mod pull_requests;
mod quota;
mod reply_images;
mod repository_monitor;
mod runtime;
mod secrets;
mod self_update;
mod settlement;
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
mod usage;
mod user_environment;
mod workflow;
mod worktree_name;

pub use agent_tools::{AGENT_TOOL_TIMEOUT_SECONDS, answer_mcp, offer_link_tool};
use auth::AuthManager;
use chat_title::ChatTitleService;
use cli_updates::CliUpdates;
use cli_versions::CliVersions;
use git_draft::GitDraftService;
use local_socket::UnixStream;
use owner_icon::OwnerIcons;
use pairing::{PairingService, Transport, generate_token, token_hash};
use pull_requests::LinkSource;
use repository_monitor::RepositoryMonitor;
use runtime::{LiveTurn, TurnRuntime};
use secrets::SecretsStore;
use self_update::SelfUpdate;
pub use self_update::installed_host_executable;
use settlement::AutoSettler;
pub use storage::ChatToResume;
use storage::{Continuation, SendDisposition};
pub use storage::{StateStore, StorageError};
use storage::{clone_repository, normalize_device_name};
use terminal::TerminalManager;
use usage::UsageService;
use workflow::WorkflowStatuses;

// Authenticated file transfers have no configured byte limit. Leave room
// for the reader's extra byte; allocation capacity is the platform limit.
pub const FRAME_LIMIT: usize = isize::MAX as usize - 1;
pub const AUTH_FRAME_LIMIT: usize = 64 * 1024;
/// New chats one connection may be setting up beside its other requests.
/// Making a worktree can take a while, and the client should not have to
/// wait for it to open another chat. Past this many they wait in a line of
/// their own, as long as the connection's command queue.
const NEW_CHATS_ASIDE: usize = 4;
const NEW_CHATS_QUEUED: usize = 128;

/// The new chats a connection is setting up beside its request worker.
#[derive(Default)]
struct NewChatsAside {
    state: Mutex<NewChatsAsideState>,
    idle: std::sync::Condvar,
}

#[derive(Default)]
struct NewChatsAsideState {
    running: usize,
    queued: std::collections::VecDeque<Value>,
}

impl NewChatsAside {
    fn state(&self) -> std::sync::MutexGuard<'_, NewChatsAsideState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// The request back to start on a thread of its own, or none when it
    /// waits for one of the running ones.
    fn take_or_queue(&self, request: Value) -> std::io::Result<Option<Value>> {
        let mut state = self.state();
        if state.running < NEW_CHATS_ASIDE {
            state.running += 1;
            return Ok(Some(request));
        }
        if state.queued.len() >= NEW_CHATS_QUEUED {
            return Err(std::io::Error::other("Client command queue overflowed"));
        }
        state.queued.push_back(request);
        Ok(None)
    }

    /// The next waiting request for a thread that finished one, or none
    /// when it should stop. A connection that stopped answering drops the
    /// rest.
    fn next(&self, answered: bool) -> Option<Value> {
        let mut state = self.state();
        if !answered {
            state.queued.clear();
        }
        let next = state.queued.pop_front();
        if next.is_none() {
            state.running -= 1;
            if state.running == 0 {
                self.idle.notify_all();
            }
        }
        next
    }

    /// Waits until every accepted new chat has been answered.
    fn wait_idle(&self) {
        let mut state = self.state();
        while state.running > 0 {
            state = self
                .idle
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}
const REQUEST_ID: &str = "_xd_request";

struct AppClients {
    open: HashSet<u64>,
    /// When the last one closed, or when the engine started.
    last_seen: Instant,
}

impl Default for AppClients {
    fn default() -> Self {
        Self {
            open: HashSet::new(),
            last_seen: Instant::now(),
        }
    }
}

pub struct Engine {
    shutdown: AtomicBool,
    /// Local connections the xd app's own bridge opened. A daemon the app
    /// started stops once none has been open for a while.
    app_clients: Mutex<AppClients>,
    /// Whether this daemon stops once the xd app that started it closes.
    stops_with_app: AtomicBool,
    store: Option<Arc<StateStore>>,
    events: Arc<EventBus>,
    runtime: Option<TurnRuntime>,
    auth: AuthManager,
    cli_updates: CliUpdates,
    cli_versions: CliVersions,
    workflows: WorkflowStatuses,
    usage: UsageService,
    owner_icons: Arc<OwnerIcons>,
    terminals: TerminalManager,
    chat_execution: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    session_resources: Arc<Mutex<()>>,
    secrets: Arc<SecretsStore>,
    git_drafts: GitDraftService,
    chat_titles: ChatTitleService,
    worktree_names: worktree_name::WorktreeNameService,
    git_actions: Arc<Mutex<()>>,
    repository_monitor: RepositoryMonitor,
    self_update: SelfUpdate,
    pairing: PairingService,
    peer_listener: Mutex<Option<PeerListener>>,
    auto_settler: Mutex<Option<AutoSettler>>,
    quota_meter: Mutex<Option<quota::QuotaMeter>>,
    /// Held from saving the default chat quota until it is broadcast, so the
    /// last broadcast always carries the saved value.
    quota_settings: Mutex<()>,
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
            app_clients: Mutex::default(),
            stops_with_app: AtomicBool::new(false),
            store: None,
            auth: AuthManager::new(events.clone(), accounts::Accounts::new(None)),
            cli_updates: CliUpdates::new(events.clone()),
            cli_versions: CliVersions::new(events.clone()),
            workflows: WorkflowStatuses::new(events.clone()),
            usage: UsageService::new(events.clone(), None),
            owner_icons: Arc::new(OwnerIcons::new(events.clone())),
            terminals: TerminalManager::new(events.clone()),
            chat_execution: Mutex::new(HashMap::new()),
            session_resources: Arc::new(Mutex::new(())),
            secrets,
            git_drafts: GitDraftService::new(None, events.clone()),
            chat_titles: ChatTitleService::new(None, events.clone()),
            worktree_names: worktree_name::WorktreeNameService::new(
                None,
                events.clone(),
                Arc::new(Mutex::new(())),
            ),
            git_actions: Arc::new(Mutex::new(())),
            repository_monitor: RepositoryMonitor::disabled(),
            self_update,
            pairing: PairingService::default(),
            peer_listener: Mutex::new(None),
            auto_settler: Mutex::new(None),
            quota_meter: Mutex::new(None),
            quota_settings: Mutex::new(()),
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
        let events = Arc::new(EventBus::default());
        // Unit tests keep to the user's own sign-in; a stray data directory
        // must not give them accounts of their own.
        let accounts =
            accounts::Accounts::new(data_directory.as_deref().filter(|_| cfg!(not(test))));
        let auth = AuthManager::new(events.clone(), accounts.clone());
        let cli_updates = CliUpdates::new(events.clone());
        let cli_versions = CliVersions::new(events.clone());
        let secrets = Arc::new(SecretsStore::new(data_directory.clone()));
        let runtime =
            TurnRuntime::with_auth(store.clone(), events.clone(), secrets.clone(), auth.clone());
        auth.set_releaser(runtime.releaser());
        let git_drafts = GitDraftService::new(Some(store.clone()), events.clone());
        // Unit tests send messages on hosts with no fake assistant behind them, and
        // must not reach a real CLI. The daemon tests cover titles end to end.
        let chat_titles =
            ChatTitleService::new(cfg!(not(test)).then(|| store.clone()), events.clone());
        // Renames wait their turn with what creates, deletes, or settles a
        // chat's worktree, as everything that changes one does.
        let session_resources = Arc::new(Mutex::new(()));
        let worktree_names = worktree_name::WorktreeNameService::new(
            cfg!(not(test)).then(|| store.clone()),
            events.clone(),
            session_resources.clone(),
        );
        let self_update = SelfUpdate::new(events.clone());
        let repository_monitor = RepositoryMonitor::new(store.clone(), events.clone());
        let engine = Self {
            shutdown: AtomicBool::new(false),
            app_clients: Mutex::default(),
            stops_with_app: AtomicBool::new(false),
            runtime: Some(runtime),
            store: Some(store),
            auth,
            cli_updates,
            cli_versions,
            workflows: WorkflowStatuses::new(events.clone()),
            usage: UsageService::new(events.clone(), data_directory.clone())
                .with_accounts(accounts),
            owner_icons: Arc::new(OwnerIcons::new(events.clone())),
            terminals: TerminalManager::new(events.clone()),
            chat_execution: Mutex::new(HashMap::new()),
            session_resources,
            secrets,
            git_drafts,
            chat_titles,
            worktree_names,
            git_actions: Arc::new(Mutex::new(())),
            repository_monitor,
            self_update,
            pairing: PairingService::default(),
            peer_listener: Mutex::new(None),
            auto_settler: Mutex::new(None),
            quota_meter: Mutex::new(None),
            quota_settings: Mutex::new(()),
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
                json!({"ok": true, "version": env!("CARGO_PKG_VERSION"), "protocol": 2, "build": option_env!("XD_COMMIT").unwrap_or("development"), "pid": std::process::id(), "workspaces": self.store.as_ref().and_then(|store| fs::canonicalize(store.workspace_path()).ok()), "executable": self.self_update.host_executable().and_then(std::path::Path::to_str), "stops_with_app": self.stops_with_app.load(Ordering::Acquire)})
            }
            Some("resume-interrupted") => self.resume_interrupted(&request),
            Some("daemon-stop") if self.pairing.is_local(owner) => {
                self.shutdown.store(true, Ordering::Release);
                json!({"ok": true})
            }
            Some("app-client") if self.pairing.is_local(owner) => {
                if let Ok(mut clients) = self.app_clients.lock() {
                    clients.open.insert(owner);
                }
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
            Some("background-task-output") => {
                match required_string(&request, "chat", "background-task-output needs a chat id") {
                    Ok(chat) => {
                        let exists = self.chat(chat);
                        if exists["ok"] != true {
                            exists
                        } else {
                            self.runtime.as_ref().map_or_else(
                                || json!({"ok":true,"chat":chat,"tasks":[]}),
                                |runtime| {
                                    runtime.background_task_output(chat, request["task"].as_str())
                                },
                            )
                        }
                    }
                    Err(error) => error_reply(error),
                }
            }
            Some("messages") => self.messages(&request),
            Some("list-dir") => self.read(|store| store.list_directory(&request)),
            Some("file-browse") => self.read(|store| store.file_browse(&request)),
            Some("search") => self.read(|store| store.search(&request)),
            Some("image-read") => self.read(|store| store.image_read(&request)),
            Some("diff-read") => self.read(|store| store.diff_read(&request)),
            Some("git-status") => self.read(|store| store.git_status(&request)),
            Some("git-state") => self.git_state(&request),
            Some("git-pr-status") => self.git_pull_request_status(&request),
            Some("git-pr-create") => self.git_create_pull_request(&request),
            Some("link-pull-requests") => self.link_pull_requests(&request),
            Some("unlink-pull-request") => self.unlink_pull_request(&request),
            Some("start-agent") => self.start_agent(&request),
            Some("list-agents") => self.list_agents(&request),
            Some("message-agent") => self.message_agent(&request),
            Some("stop-agent") => self.stop_agent(&request),
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
            Some("agent-cli-updates") => self
                .cli_updates
                .perform(
                    request.get("action").and_then(Value::as_str),
                    request.get("provider").and_then(Value::as_str),
                )
                .unwrap_or_else(error_reply),
            Some("host-update") => {
                let action = request.get("action").and_then(Value::as_str);
                if action == Some("restart") && self.cli_updates.is_updating() {
                    error_reply("Wait for the assistant updates to finish before restarting.")
                } else if action == Some("restart")
                    && (self.runtime.as_ref().is_some_and(|runtime| {
                        runtime.has_active_turns() || runtime.has_background_tasks()
                    }) || self.terminals.has_sessions())
                {
                    // A restart stops every agent process, and with it any
                    // background agent or shell that outlived its turn.
                    error_reply(
                        "Close terminal tabs and wait for active chats and their background agents and shells to finish before restarting for an update.",
                    )
                } else {
                    let reply = self.self_update.perform(action).unwrap_or_else(error_reply);
                    if action == Some("restart")
                        && reply["ok"] == true
                        && let Some(runtime) = &self.runtime
                    {
                        runtime.shutdown();
                    }
                    reply
                }
            }
            Some("agent-auth-start") => self.auth_mutation(&request, |auth, provider, request| {
                auth.login(provider, request.get("account").and_then(Value::as_str))
            }),
            Some("agent-auth-add") => {
                self.auth_mutation(&request, |auth, provider, _| auth.add(provider))
            }
            Some("agent-accounts-order") => {
                self.auth_mutation(&request, |auth, provider, request| {
                    let order = request
                        .get("accounts")
                        .and_then(Value::as_array)
                        .ok_or("agent-accounts-order needs an accounts array.")?
                        .iter()
                        .map(|id| id.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                        .ok_or("Account ids are strings.")?;
                    auth.reorder(provider, &order)
                })
            }
            Some("agent-accounts-auto-switch") => {
                self.auth_mutation(&request, |auth, provider, request| {
                    let enabled = request
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .ok_or("agent-accounts-auto-switch needs enabled.")?;
                    auth.set_auto_switch(provider, enabled)
                })
            }
            Some("agent-auth-input") => self.auth_mutation(&request, |auth, provider, request| {
                let input = required_string(request, "input", "agent-auth-input needs text.")?;
                auth.input(provider, input)
            }),
            Some("agent-auth-cancel") => {
                self.auth_mutation(&request, |auth, provider, _| auth.cancel(provider))
            }
            Some("agent-auth-logout") => self.auth_mutation(&request, |auth, provider, request| {
                auth.logout(provider, request.get("account").and_then(Value::as_str))
            }),
            Some("shortcuts") => self.read(|store| store.shortcuts(&request)),
            Some("chat-title-settings") => self.read(StateStore::chat_title_settings),
            Some("set-chat-title-settings") => {
                self.read(|store| store.set_chat_title_settings(&request))
            }
            Some("folder-context") => self.read(|store| store.folder_context(&request)),
            Some("folder-settings") => self.read(|store| store.folder_settings(&request)),
            Some("new-chat-sources") => self.read(|store| store.new_chat_sources(&request)),
            Some("set-draft") => self.event_mutation(|store| store.set_draft(&request)),
            Some("set-shortcuts") => self.event_mutation(|store| store.set_shortcuts(&request)),
            Some("set-option") => self.event_mutation(|store| store.set_option(&request)),
            Some("send") => self.send_message(&request),
            Some("resume-turn") => self.resume_turn(&request),
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
            Some("pause-all") => self.pause_all(&request),
            Some("resume-all") => self.resume_all(&request),
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
                let reply = {
                    let _resources = self
                        .session_resources
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    self.tree_mutation(|store| store.new_chat(&request))
                };
                // Named afterwards, so creating the chat never waits on a model.
                if reply.get("ok").and_then(Value::as_bool) == Some(true)
                    && let Some(chat_id) = reply.get("id").and_then(Value::as_str)
                {
                    self.worktree_names
                        .start(chat_id.to_owned(), request.clone());
                }
                reply
            }
            Some("rename-session") => self.tree_mutation(|store| store.rename_session(&request)),
            Some("move-session") => self.tree_mutation(|store| store.move_session(&request)),
            Some("delete-session") => self.delete_session(&request),
            Some("settle-session") => self.settle_session(&request),
            Some("unsettle-session") => {
                self.tree_mutation(|store| store.unsettle_session(&request))
            }
            Some("set-session-auto-settle") => {
                self.tree_mutation(|store| store.set_session_auto_settle(&request))
            }
            Some("auto-settle-settings") => self.read(|store| {
                store
                    .auto_settle_settings()
                    .map(|settings| settings.reply())
            }),
            Some("set-auto-settle-settings") => self.set_auto_settle_settings(&request),
            Some("auto-delete-settings") => self.read(|store| {
                store
                    .auto_delete_settings()
                    .map(|settings| settings.reply())
            }),
            Some("set-auto-delete-settings") => self.set_auto_delete_settings(&request),
            Some("chat-quota-settings") => self.read(|store| {
                store
                    .chat_quota_settings()
                    .map(|percent| json!({"ok": true, "percent": percent}))
            }),
            Some("set-chat-quota-settings") => self.set_chat_quota_settings(&request),
            Some("set-chat-quota") => self.set_chat_quota(&request),
            Some("rename-chat") => self.tree_mutation(|store| store.rename_chat(&request)),
            Some("move-chat") => self.tree_mutation(|store| store.move_chat(&request)),
            Some("delete-chat") => self.delete_chat(&request),
            Some("remove-worktree") => self.remove_worktree(&request),
            Some("queue") => self.event_mutation(|store| store.queue(&request)),
            Some("drop-queue") => self.event_mutation(|store| store.drop_queue(&request)),
            Some("edit-queue") => self.event_mutation(|store| store.edit_queue(&request)),
            Some("reorder-queue") => self.event_mutation(|store| store.reorder_queue(&request)),
            Some("steer-queue") => self.steer_queued(&request, false),
            Some("workflow-status") => self.workflow_status(owner, &request),
            Some("usage-summary") => self
                .usage
                .summary(owner, &request)
                .unwrap_or_else(error_reply),
            Some("usage-limits") => self
                .usage
                .limits(owner, &request)
                .unwrap_or_else(error_reply),
            Some("usage-consume-reset-credit") => self
                .usage
                .consume_reset_credit(owner, &request)
                .unwrap_or_else(error_reply),
            Some("usage-prices") => self.usage.prices().unwrap_or_else(error_reply),
            Some("set-usage-prices") => self.usage.set_prices(&request).unwrap_or_else(error_reply),
            Some("owner-icon") => self
                .owner_icons
                .icon(owner, &request)
                .unwrap_or_else(error_reply),
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

    /// Answers for every chat a restart has cut off since the last answer, so a
    /// client reconnecting later, or turning the setting on later, does not
    /// restart work from an old interruption.
    fn resume_interrupted(&self, request: &Value) -> Value {
        let chats = self
            .store
            .as_ref()
            .and_then(|store| store.take_chats_to_resume().ok())
            .unwrap_or_default();
        let mut resumed = Vec::new();
        if request.get("resume").and_then(Value::as_bool) == Some(true) {
            for chat in chats {
                let prompt = chat.prompt();
                let ChatToResume {
                    chat_id,
                    last_message,
                    ..
                } = chat;
                let reply = self.send_message_inner(
                    &json!({"op": "send", "chat": chat_id, "text": prompt}),
                    Some(Continuation::AfterRestart(last_message)),
                );
                if reply["ok"] == true {
                    self.events
                        .publish(json!({"event": "changed", "chat": chat_id}));
                    resumed.push(chat_id);
                } else {
                    eprintln!("xd-host: not resuming chat {chat_id}: {reply}");
                }
            }
        }
        json!({"ok": true, "resumed": resumed})
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    /// Records that this daemon stops once the xd app that started it closes.
    pub fn stop_with_app(&self) {
        self.stops_with_app.store(true, Ordering::Release);
    }

    /// How long no connection from the xd app has been open, counted from the
    /// last one closing or the engine starting. `None` while one is open.
    pub fn app_absent_for(&self) -> Option<Duration> {
        let clients = self.app_clients.lock().ok()?;
        clients.open.is_empty().then(|| clients.last_seen.elapsed())
    }

    /// Whether an app update is being installed, which a stop must not cut off.
    pub fn installing_update(&self) -> bool {
        self.self_update.installing()
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
        if let Ok(subscribers) = self.events.subscribers.lock()
            && let Some(connection) = subscribers
                .get(&owner)
                .and_then(|subscriber| subscriber.connection.as_ref())
        {
            let _ = connection.set_read_timeout(None);
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
            if let Some(runtime) = &self.runtime {
                overlay_background_tasks(&mut tree, &runtime.background_task_counts());
            }
            tree["terminal_activity_epoch"] = Value::String(activity.epoch);
            tree["terminal_activity_revision"] = activity.revision.into();
            self.overlay_folder_owners(&mut tree);
        }
        tree
    }

    /// Names the GitHub account behind each workspace, so a client can show
    /// its avatar in place of a folder icon.
    fn overlay_folder_owners(&self, tree: &mut Value) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let Some(folders) = tree.get_mut("folders").and_then(Value::as_array_mut) else {
            return;
        };
        for folder in folders {
            // A folder of projects has no repository; an organization's folder
            // already names its account.
            if folder.get("group").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let Some(directory) = folder
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| store.folder_workdir(id).ok())
            else {
                continue;
            };
            if let Some(owner) = self.owner_icons.owner(Path::new(&directory)) {
                folder["owner"] = Value::String(owner);
            }
        }
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
            && let Some(live) = runtime.live_turn(chat)
        {
            request["_through_id"] = json!(live.message_boundary);
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
            response["background_tasks"] = runtime.background_tasks(chat_id).into();
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
        let with_pull_request = request.get("pull_request").and_then(Value::as_bool) == Some(true);
        let store = store.clone();
        let events = self.events.clone();
        thread::Builder::new()
            .name("xd-git-state".into())
            .spawn(move || {
                let mut event = store
                    .git_action_state(&chat_id)
                    .unwrap_or_else(|_| hidden_git_state());
                if with_pull_request && let Some(pull_request) = store.chat_pull_request(&chat_id) {
                    if let Some(url) = pull_request
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|url| !url.is_empty())
                    {
                        link_pull_requests(
                            &store,
                            &events,
                            &chat_id,
                            &[url.to_owned()],
                            LinkSource::Branch,
                        );
                    }
                    event["pull_request"] = pull_request;
                }
                event["event"] = Value::String("git-state".into());
                event["chat"] = Value::String(chat_id.clone());
                if let Some(request_id) = request_id {
                    event["request"] = Value::String(request_id);
                }
                events.publish(event);
                if with_pull_request {
                    refresh_pull_request_states(&store, &events, &chat_id);
                }
            })
            .map(|_| json!({"ok": true}))
            .unwrap_or_else(|error| error_reply(format!("Cannot refresh Git state: {error}")))
    }

    /// Links the branch's pull request, when the lookup found one, and asks
    /// GitHub again about the chat's other links.
    fn git_pull_request_status(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let reply = match store.git_pull_request_status(request) {
            Ok(reply) => reply,
            Err(error) => return error_reply(error),
        };
        if let Some(chat_id) = request.get("chat").and_then(Value::as_str) {
            let url = reply
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
                .map(str::to_owned);
            let chat_id = chat_id.to_owned();
            let store = store.clone();
            let events = self.events.clone();
            let _ = thread::Builder::new()
                .name("xd-pull-requests".into())
                .spawn(move || {
                    if let Some(url) = url {
                        link_pull_requests(&store, &events, &chat_id, &[url], LinkSource::Branch);
                    }
                    refresh_pull_request_states(&store, &events, &chat_id);
                });
        }
        reply
    }

    fn git_create_pull_request(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        match store.git_create_pull_request(request) {
            Ok(reply) => {
                if let (Some(chat_id), Some(url)) = (
                    request.get("chat").and_then(Value::as_str),
                    reply.get("url").and_then(Value::as_str),
                ) {
                    link_pull_requests(
                        store,
                        &self.events,
                        chat_id,
                        &[url.to_owned()],
                        LinkSource::Branch,
                    );
                }
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    /// Links pull requests to a chat, for the user from a client, or for the
    /// agent through its link tool when `source` says so.
    fn link_pull_requests(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let chat_id = match required_string(request, "chat", "Linking needs a chat.") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        let Some(urls) = request.get("urls").and_then(Value::as_array) else {
            return error_reply("Name the pull requests to link.");
        };
        let urls = urls
            .iter()
            .map(|url| url.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>();
        let Some(urls) = urls.filter(|urls| !urls.is_empty()) else {
            return error_reply("Name the pull requests to link.");
        };
        let source = match request.get("source").and_then(Value::as_str) {
            None | Some("user") => LinkSource::User,
            Some("agent") => LinkSource::Agent,
            Some(_) => return error_reply("No such link source."),
        };
        match store.link_pull_requests(chat_id, &urls, source) {
            Ok(outcome) => {
                if outcome.changed {
                    self.events.publish(json!({"event": "tree"}));
                }
                if !outcome.linked.is_empty() {
                    let chat_id = chat_id.to_owned();
                    let store = store.clone();
                    let events = self.events.clone();
                    let _ = thread::Builder::new()
                        .name("xd-pull-requests".into())
                        .spawn(move || refresh_pull_request_states(&store, &events, &chat_id));
                }
                json!({"ok": true, "linked": outcome.linked, "invalid": outcome.invalid})
            }
            Err(error) => error_reply(error),
        }
    }

    fn unlink_pull_request(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let (chat_id, url) = match (
            required_string(
                request,
                "chat",
                "Unlinking needs a chat and a pull request.",
            ),
            required_string(request, "url", "Unlinking needs a chat and a pull request."),
        ) {
            (Ok(chat_id), Ok(url)) => (chat_id, url),
            (Err(error), _) | (_, Err(error)) => return error_reply(error),
        };
        match store.unlink_pull_request(chat_id, url) {
            Ok(changed) => {
                if changed {
                    self.events.publish(json!({"event": "tree"}));
                }
                json!({"ok": true, "changed": changed})
            }
            Err(error) => error_reply(error),
        }
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
                        if action == "create-pr"
                            && let Some(url) = state.get("url").and_then(Value::as_str)
                        {
                            link_pull_requests(
                                &store,
                                &events,
                                &chat_id,
                                &[url.to_owned()],
                                LinkSource::Branch,
                            );
                        }
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
        // A worker's card leaves its parent with it; the parent reloads.
        let parent = store.chat_parent(chat_id).ok().flatten();
        match store.delete_chat(request) {
            Ok(reply) => {
                if let Some(runtime) = &self.runtime {
                    runtime.discard_chat(chat_id);
                }
                if let Some((parent, _)) = parent {
                    self.events
                        .publish(json!({"event": "changed", "chat": parent}));
                }
                if reply["session_deleted"] == true
                    && let Some(session) = reply["session"].as_str()
                {
                    self.terminals.kill_chat(&format!("session:{session}"));
                }
                self.events.publish(json!({"event": "tree"}));
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    /// The chats outside `chats` that started any of them, whose transcripts
    /// lose those workers' cards when they are deleted.
    fn worker_parents(&self, chats: &[String]) -> Vec<String> {
        let Some(store) = self.store.as_ref() else {
            return Vec::new();
        };
        let mut parents = chats
            .iter()
            .filter_map(|chat| store.chat_parent(chat).ok().flatten())
            .map(|(parent, _)| parent)
            .filter(|parent| !chats.contains(parent))
            .collect::<Vec<_>>();
        parents.sort();
        parents.dedup();
        parents
    }

    /// Tells clients the transcripts of `parents` changed.
    fn reload_parents(&self, parents: Vec<String>) {
        for parent in parents {
            self.events
                .publish(json!({"event": "changed", "chat": parent}));
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
        let parents = self.worker_parents(&chats);
        match store.delete_session(request) {
            Ok(reply) => {
                for chat in chats {
                    if let Some(runtime) = &self.runtime {
                        runtime.discard_chat(&chat);
                    }
                }
                self.terminals.kill_chat(&format!("session:{session}"));
                self.events.publish(json!({"event": "tree"}));
                self.reload_parents(parents);
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    /// Settling waits for what the session still owes the user. The locks keep
    /// a message from starting a turn between this check and the update.
    fn settle_session(&self, request: &Value) -> Value {
        let session =
            match required_string(request, "session", "settle-session needs a session id.") {
                Ok(session) => session,
                Err(error) => return error_reply(error),
            };
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let _resources = match self.session_resources.lock() {
            Ok(resources) => resources,
            Err(_) => return error_reply("Session resources are unavailable."),
        };
        let chats = match store.session_chats(session) {
            Ok(chats) => chats,
            Err(error) => return error_reply(error),
        };
        if chats.iter().any(|chat| {
            self.runtime
                .as_ref()
                .is_some_and(|runtime| runtime.has_interactions(chat))
        }) {
            return error_reply(
                "This session is waiting on an approval or question. Answer it before settling.",
            );
        }
        self.tree_mutation(|store| store.settle_session(request))
    }

    fn set_auto_settle_settings(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        match store.set_auto_settle_settings(request) {
            Ok(settings) => {
                let reply = settings.reply();
                let mut event = reply.clone();
                event["event"] = json!("auto-settle-settings");
                self.events.publish(event);
                if let Ok(settler) = self.auto_settler.lock()
                    && let Some(settler) = settler.as_ref()
                {
                    settler.wake();
                }
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    /// Starts settling sessions on the host's own schedule. Only the daemon
    /// does this, since it is what outlives the clients.
    ///
    /// The same schedule deletes sessions once the user has chosen a number of
    /// days for that.
    pub fn start_auto_settle(self: &Arc<Self>) {
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return;
        };
        if let Ok(mut settler) = self.auto_settler.lock() {
            settler.get_or_insert_with(|| {
                // Weak, since the engine owns the thread that holds this.
                let engine = Arc::downgrade(self);
                let mut reported = HashSet::new();
                AutoSettler::start(
                    store.clone(),
                    runtime.clone(),
                    self.events.clone(),
                    Box::new(move || {
                        if let Some(engine) = engine.upgrade() {
                            engine.auto_delete_sweep(settlement::now_seconds(), &mut reported);
                        }
                    }),
                )
            });
        }
    }

    /// Lets a chat whose account ran out continue on the next one, and keeps
    /// reading every account's limits while an assistant has several. Only the
    /// daemon does this, since it is what outlives the clients.
    pub fn start_account_switching(self: &Arc<Self>) {
        self.auth.start_watch();
        // Codex sessions from before accounts were kept are filed under the
        // login Codex has as the host starts, so a later change of login is
        // caught for them too rather than resumed across.
        // Off the startup path, so a slow Codex cannot hold up the socket; a
        // turn about to resume one of them waits for this instead. When the
        // login cannot be read they are filed as an unknown login: then a
        // later login never takes them for its own.
        if let (Some(store), Some(runtime)) = (self.store.clone(), self.runtime.clone()) {
            runtime.filing();
            let auth = self.auth.clone();
            let spawned = thread::Builder::new()
                .name("xd-file-sessions".into())
                .spawn({
                    let runtime = runtime.clone();
                    move || {
                        let tag = auth
                            .read_own_codex_login()
                            .unwrap_or_else(|| format!("{}#?", accounts::DEFAULT_ACCOUNT));
                        if let Err(error) = store.claim_unfiled_sessions("codex", &tag) {
                            eprintln!("xd-host: cannot file earlier Codex sessions: {error}");
                        }
                        runtime.filed();
                    }
                });
            if spawned.is_err() {
                runtime.filed();
            }
        }
        let Some(runtime) = self.runtime.as_ref() else {
            return;
        };
        // Weak, since the engine owns the runtime that holds this.
        let engine = Arc::downgrade(self);
        runtime.set_account_switch_handler(Box::new(move |chat_id, last_message| {
            let engine = engine.clone();
            // Off the turn's thread: continuing takes the chat's own locks.
            let _ = thread::Builder::new()
                .name("xd-account-continue".into())
                .spawn(move || {
                    let Some(engine) = engine.upgrade() else {
                        return;
                    };
                    let reply = engine.send_message_inner(
                        &json!({"op": "send", "chat": chat_id, "text": storage::CONTINUE_PROMPT}),
                        Some(Continuation::AfterLimit(last_message)),
                    );
                    if reply["ok"] == true {
                        engine
                            .events
                            .publish(json!({"event": "changed", "chat": chat_id}));
                    } else {
                        eprintln!(
                            "xd-host: not continuing chat {chat_id} on another account: {reply}"
                        );
                    }
                });
        }));
    }

    /// Starts the memory guard; see [`memory_guard`]. It does nothing while the
    /// daemon runs in no cgroup that running out of memory kills as a whole,
    /// but looks again now and then, since a container's limits can change.
    #[cfg(target_os = "linux")]
    pub fn start_memory_guard(self: &Arc<Self>) {
        let setting = std::env::var(memory_guard::PERCENT_VARIABLE).ok();
        let Some(percent) = memory_guard::threshold_percent(setting.as_deref()) else {
            return;
        };
        // Weak, since the engine owns nothing that would stop this thread.
        let engine = Arc::downgrade(self);
        let spawned = thread::Builder::new()
            .name("xd-memory-guard".into())
            .spawn(move || {
                let mut guarded = Vec::new();
                let mut discovered: Option<std::time::Instant> = None;
                while engine.strong_count() > 0 {
                    if discovered.is_none_or(|at| at.elapsed() >= memory_guard::DISCOVER_EVERY) {
                        guarded = memory_guard::retarget(
                            guarded,
                            memory_guard::Cgroup::discover(),
                            percent,
                        );
                        discovered = Some(std::time::Instant::now());
                    }
                    for memory_guard::Guarded { cgroup, trigger } in &mut guarded {
                        let Some(usage) = cgroup.unreclaimable() else {
                            continue;
                        };
                        let now = std::time::Instant::now();
                        if !trigger.due(usage, now) {
                            continue;
                        }
                        trigger.fired(now);
                        let Some(engine) = engine.upgrade() else {
                            return;
                        };
                        engine.relieve_memory(cgroup, usage);
                    }
                    thread::sleep(memory_guard::SAMPLE_EVERY);
                }
            });
        if let Err(error) = spawned {
            eprintln!("xd-host: cannot start the memory guard: {error}");
        }
    }

    /// Stops the largest process tree xd may stop, and says so in the host log
    /// and in the chat whose agent or terminal it ran under.
    #[cfg(target_os = "linux")]
    fn relieve_memory(&self, cgroup: &memory_guard::Cgroup, usage: u64) {
        let mut chats: HashMap<u32, String> =
            self.terminals.session_leaders().into_iter().collect();
        if let Some(runtime) = &self.runtime {
            chats.extend(runtime.agent_processes());
        }
        let owners = chats.keys().copied().collect();
        let scope = memory_guard::Scope {
            me: std::process::id(),
            // SAFETY: getuid has no preconditions.
            uid: unsafe { libc::getuid() },
            cgroup: cgroup.path(),
            owners: &owners,
        };
        let Some(victim) = memory_guard::choose_victim(&memory_guard::read_processes(), &scope)
        else {
            eprintln!(
                "xd-host: memory is approaching the available limit, but nothing is running that xd may stop."
            );
            return;
        };
        let message = memory_guard::notice(&victim, usage, cgroup.max());
        eprintln!("xd-host: {message}");
        memory_guard::stop(&victim, &scope);
        if let Some(chat) = victim.owner.and_then(|owner| chats.get(&owner)) {
            if let Some(store) = &self.store
                && store.chat(chat).is_ok()
                && store
                    .append_turn_message(chat, "error", &message, None)
                    .is_ok()
            {
                self.events
                    .publish(json!({"event": "changed", "chat": chat}));
            }
            if let Some(runtime) = &self.runtime {
                runtime.notify_memory_guard(chat, &message);
            }
        }
    }

    fn set_auto_delete_settings(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        match store.set_auto_delete_settings(request) {
            Ok(settings) => {
                let reply = settings.reply();
                let mut event = reply.clone();
                event["event"] = json!("auto-delete-settings");
                self.events.publish(event);
                if let Ok(settler) = self.auto_settler.lock()
                    && let Some(settler) = settler.as_ref()
                {
                    settler.wake();
                }
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    fn set_chat_quota_settings(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let _settings = self
            .quota_settings
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match store.set_chat_quota_settings(request) {
            Ok(percent) => {
                self.events
                    .publish(json!({"event": "chat-quota-settings", "percent": percent}));
                // A lower default can have stopped idle chats.
                self.events.publish(json!({"event": "tree"}));
                self.wake_quota_meter();
                json!({"ok": true, "percent": percent})
            }
            Err(error) => error_reply(error),
        }
    }

    /// Gives a chat a quota of its own, or hands it back to the default, and
    /// continues it if it had stopped at its quota.
    fn set_chat_quota(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let chat_id = match required_string(request, "chat", "set-chat-quota needs a chat id") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        // Under the chat's gate, so the meter cannot stop the chat between
        // the new quota and the turn it continues, at the old count.
        let reached = {
            let gate = match self.chat_execution_gate(chat_id) {
                Ok(gate) => gate,
                Err(error) => return error_reply(error),
            };
            let Ok(_execution) = gate.lock() else {
                return error_reply("This session's agent state is unavailable.");
            };
            match store.set_chat_quota(request) {
                Ok(reached) => reached,
                Err(error) => return error_reply(error),
            }
        };
        self.events
            .publish(json!({"event": "changed", "chat": chat_id}));
        self.events.publish(json!({"event": "tree"}));
        self.wake_quota_meter();
        let resumed = reached && self.resume_paused_chat(chat_id, &|_| true);
        json!({"ok": true, "resumed": resumed})
    }

    fn wake_quota_meter(&self) {
        if let Ok(meter) = self.quota_meter.lock()
            && let Some(meter) = meter.as_ref()
        {
            meter.wake();
        }
    }

    /// Stops a chat that has used its quota, as `pause-all` stops a chat,
    /// and marks it so only a new quota picks it back up.
    fn stop_for_quota(&self, chat_id: &str) {
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return;
        };
        let Ok(gate) = self.chat_execution_gate(chat_id) else {
            return;
        };
        let Ok(_execution) = gate.lock() else {
            return;
        };
        // Asked again under the gate: a new quota may have come meanwhile.
        if !store
            .chat_quota(chat_id)
            .is_ok_and(|quota| quota.reached || quota.spent())
        {
            return;
        }
        let stopped = runtime.stop_chat_if(chat_id, |background| {
            store
                .stop_for_quota(chat_id, background)
                .unwrap_or_else(|error| {
                    eprintln!("xd-host: cannot stop chat {chat_id} at its quota: {error}");
                    false
                })
        });
        match stopped {
            None => return,
            // As with stop: a turn that died with a previous host still ends.
            Some(false) => {
                let _ = store.clear_interrupted_turn(chat_id);
            }
            Some(true) => {}
        }
        self.events
            .publish(json!({"event": "changed", "chat": chat_id}));
        self.events.publish(json!({"event": "tree"}));
    }

    /// Holds chats to their quotas. Only the daemon does this, since it is
    /// what outlives the clients.
    pub fn start_chat_quotas(self: &Arc<Self>) {
        let (Some(store), Some(runtime)) = (self.store.clone(), self.runtime.clone()) else {
            return;
        };
        let auth = self.auth.clone();
        let reader: quota::Reader = Box::new(move |provider, account| {
            if !auth.signed_in(provider, account) {
                return None;
            }
            let asked = accounts::now_ms();
            usage::check_account(auth.accounts(), provider, account)?;
            let (windows, checked_at) = auth.accounts().limits(provider, account)?;
            // A reading from before this one says nothing of the turn.
            if checked_at < asked {
                return None;
            }
            // Which login was read, so the meter never measures one login's
            // usage against another's.
            let login = auth.accounts().session_tag(provider, account);
            Some((quota::quota_window(&windows)?, login))
        });
        // Weak, since the engine owns the meter that holds this.
        let engine = Arc::downgrade(self);
        // Off the meter's thread: stopping waits for any message being sent
        // to the chat, which must not hold up readings for other chats.
        // One stop at a time per chat, however often the meter looks while
        // a slow send holds the chat's gate.
        let stopping = Arc::new(Mutex::new(HashSet::<String>::new()));
        let stopper: quota::Stopper = Box::new(move |chat_id| {
            let fresh = stopping
                .lock()
                .map(|mut stopping| stopping.insert(chat_id.to_owned()))
                .unwrap_or(false);
            if !fresh {
                return;
            }
            let engine = engine.clone();
            let stopping = stopping.clone();
            let chat_id = chat_id.to_owned();
            let spawned = thread::Builder::new().name("xd-quota-stop".into()).spawn({
                let stopping = stopping.clone();
                let chat_id = chat_id.clone();
                move || {
                    if let Some(engine) = engine.upgrade() {
                        engine.stop_for_quota(&chat_id);
                    }
                    if let Ok(mut stopping) = stopping.lock() {
                        stopping.remove(&chat_id);
                    }
                }
            });
            if spawned.is_err()
                && let Ok(mut stopping) = stopping.lock()
            {
                stopping.remove(&chat_id);
            }
        });
        if let Ok(mut meter) = self.quota_meter.lock() {
            meter.get_or_insert_with(|| {
                quota::QuotaMeter::start(store, runtime, self.events.clone(), reader, stopper)
            });
        }
    }

    /// One pass of the deletion rule. A session waits while it has an approval
    /// or question open, and typing in or output from its terminal counts as
    /// activity, even after the terminal closes. Its terminals close only once its worktrees have passed their
    /// checks, and refuse input from then on. `reported` holds sessions
    /// already logged as kept, so a worktree with uncommitted work is
    /// mentioned once rather than every sweep. Returns how many sessions it
    /// deleted.
    fn auto_delete_sweep(&self, now: i64, reported: &mut HashSet<String>) -> usize {
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return 0;
        };
        // Kept in the store, even while the rule is off, so terminal use still
        // counts after the terminal closes or the host restarts. Until it is,
        // nothing is deleted.
        let usage = self.terminals.usage();
        match store.record_terminal_use(&usage) {
            Ok(()) => self.terminals.forget_closed_usage(&usage),
            Err(error) => {
                eprintln!("xd: automatic deletion skipped: cannot record terminal use: {error}");
                return 0;
            }
        }
        let chosen = store
            .auto_delete_settings()
            .and_then(|settings| Ok((settings, store.deletion_candidates(now, &settings)?)));
        let (settings, candidates) = match chosen {
            Ok(chosen) => chosen,
            Err(error) => {
                eprintln!("xd: automatic deletion skipped: {error}");
                return 0;
            }
        };
        let Some(days) = settings.after_days else {
            return 0;
        };
        let idle_since = now - days * settlement::DAY_SECONDS;
        let mut deleted = 0;
        let mut worktrees_changed = false;
        for candidate in candidates {
            let terminal = format!("session:{}", candidate.session);
            let owners = candidate
                .chats
                .iter()
                .cloned()
                .chain([terminal.clone()])
                .collect::<HashSet<_>>();
            if candidate
                .chats
                .iter()
                .any(|chat| runtime.has_interactions(chat))
                || self
                    .terminals
                    .last_used(&owners)
                    .is_some_and(|used| used > idle_since)
            {
                continue;
            }
            // Opening a terminal for the session waits on this.
            let Ok(_resources) = self.session_resources.lock() else {
                return deleted;
            };
            if let Err(error) = self.terminal_owner(&candidate.session) {
                eprintln!("xd: automatic deletion skipped: {error}");
                return deleted;
            }
            let plan = match store.plan_auto_delete(&candidate, &settings) {
                Ok(Ok(plan)) => plan,
                Ok(Err(deletion::AutoDeletion::Kept(reason))) => {
                    if reported.insert(candidate.session.clone()) {
                        eprintln!(
                            "xd: keeping idle session {} instead of deleting it: {reason}",
                            candidate.session
                        );
                    }
                    continue;
                }
                Ok(Err(_)) => continue,
                Err(error) => {
                    eprintln!("xd: automatic deletion skipped: {error}");
                    continue;
                }
            };
            // Closing publishes events the store records, so it happens
            // between the two steps rather than under the store's lock.
            if !self.terminals.close_unused(&owners, idle_since) {
                continue;
            }
            let still_idle = || !self.terminals.closed_since(&owners, idle_since);
            let parents = store
                .session_chats(&candidate.session)
                .map(|chats| self.worker_parents(&chats))
                .unwrap_or_default();
            match store.finish_auto_delete(&candidate, &settings, plan, &still_idle) {
                Ok(deletion::AutoDeletion::Deleted {
                    chats,
                    worktrees,
                    left,
                }) => {
                    for chat in &chats {
                        runtime.discard_chat(chat);
                    }
                    self.reload_parents(parents);
                    for reason in left {
                        eprintln!(
                            "xd: deleted idle session {} but kept its worktree: {reason}",
                            candidate.session
                        );
                    }
                    reported.remove(&candidate.session);
                    worktrees_changed |= !worktrees.is_empty();
                    deleted += 1;
                }
                Ok(_) => {}
                Err(error) => eprintln!("xd: automatic deletion skipped: {error}"),
            }
        }
        if deleted > 0 {
            self.events.publish(json!({"event": "tree"}));
        }
        if worktrees_changed {
            self.events.publish(json!({"event": "worktrees-changed"}));
        }
        deleted
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
        let owner_icons = self.owner_icons.clone();
        events.publish(json!({
            "event": "folder-clone",
            "folder": folder_id.clone(),
            "url": url.clone(),
            "state": "cloning",
        }));
        thread::spawn(move || {
            let result = clone_repository(&url, &destination)
                .and_then(|_| store.finish_folder_clone(&folder_id, &destination));
            // The workspace was an empty directory when the tree last looked;
            // read its remote again now that the repository is there.
            owner_icons.forget(&destination);
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
        self.send_message_inner(request, None)
    }

    /// Continues a turn that was cut short. Unlike a plain send it refuses once
    /// the chat has moved on, so two clients pressing resume cannot queue a
    /// second "continue" behind the first.
    fn resume_turn(&self, request: &Value) -> Value {
        let chat_id = match required_string(request, "chat", "resume-turn needs a chat id") {
            Ok(chat_id) => chat_id,
            Err(error) => return error_reply(error),
        };
        self.send_message_inner(
            &json!({"op": "send", "chat": chat_id, "text": storage::CONTINUE_PROMPT}),
            Some(Continuation::Resume),
        )
    }

    fn send_message_inner(&self, request: &Value, continues: Option<Continuation>) -> Value {
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
        if let Err(error) = store.await_chat_defaults(chat_id) {
            return error_reply(error);
        }
        // Asked before the send, which can make the worktree, from the
        // message's first line; the naming model renames it once the turn is
        // under way.
        let names_worktree = continues.is_none()
            && store.send_names_worktree(request).unwrap_or_else(|error| {
                eprintln!("xd: cannot prepare AI worktree naming: {error}");
                false
            });
        // Asked before the message is stored, since it depends on there being none.
        let chat_title = store.prepare_chat_title(request).unwrap_or_else(|error| {
            eprintln!("xd: cannot prepare a chat title: {error}");
            None
        });
        let _resources = match self.session_resources.lock() {
            Ok(resources) => resources,
            Err(_) => return error_reply("Session resources are unavailable."),
        };
        let was_paused = store.chat_is_paused(chat_id).unwrap_or(false);
        let resumes = store.chat_has_settlement_override(chat_id).unwrap_or(false) || was_paused;
        let prepared = match continues {
            Some(Continuation::AfterRestart(last_message)) => {
                store.prepare_continuation(request, last_message)
            }
            Some(Continuation::Resume) => store.prepare_resume(request),
            Some(Continuation::AfterLimit(last_message)) => {
                store.prepare_after_limit(request, last_message)
            }
            None => store.prepare_send(request),
        };
        if resumes && prepared.is_ok() {
            self.events.publish(json!({"event": "tree"}));
        }
        match prepared {
            Ok(SendDisposition::Queued { mut reply, event }) => {
                let newest = event["queue"].as_array().and_then(|queue| {
                    Some((
                        queue.len().checked_sub(1)?,
                        queue.last()?.as_str()?.to_owned(),
                    ))
                });
                self.events.publish(event);
                // A chat stopped at its quota keeps the message queued.
                let held = store.chat_quota(chat_id).is_ok_and(|quota| quota.reached);
                if request.get("steer").and_then(Value::as_bool) == Some(true)
                    && !held
                    && let Some((index, text)) = newest
                {
                    let steer = self.steer_queued(
                        &json!({"chat": chat_id, "index": index, "text": text}),
                        true,
                    );
                    if steer.get("ok").and_then(Value::as_bool) != Some(true) {
                        return steer;
                    }
                    reply["steered"] = steer["steered"].clone();
                }
                reply
            }
            Ok(SendDisposition::Start { reply, turn }) => match runtime.start(turn.clone()) {
                Ok(()) => {
                    if let Some(spec) = chat_title {
                        self.chat_titles.start(spec);
                    }
                    if names_worktree {
                        self.worktree_names
                            .start_after_send(chat_id.to_owned(), request.clone());
                    }
                    reply
                }
                Err(error) => {
                    let _ = store.abort_turn_start(&turn.chat_id, &error);
                    self.keep_paused_after_failed_start(chat_id, was_paused);
                    // An agent's card says so either way; the chat that
                    // started it is told too, unless it sent the message
                    // itself and has this answer already.
                    let quiet = request.get("from_parent").and_then(Value::as_bool) == Some(true);
                    runtime.report_start_failure(chat_id, &error, quiet);
                    error_reply(error)
                }
            },
            Err(error) => error_reply(error),
        }
    }

    /// A turn that would have picked a paused chat up never started, so the
    /// chat stays paused: its queue waits and resuming tries it again.
    fn keep_paused_after_failed_start(&self, chat_id: &str, was_paused: bool) {
        if was_paused
            && let Some(store) = self.store.as_ref()
            && store.repause_chat(chat_id).unwrap_or(false)
        {
            self.events.publish(json!({"event": "tree"}));
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

    /// Stops every chat at work, and every chat whose agent still runs
    /// background agents or shells between turns, so nothing goes on while the
    /// user is away. Each keeps its queue until [`Self::resume_all`].
    fn pause_all(&self, request: &Value) -> Value {
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return error_reply("Rust host state storage is not configured.");
        };
        let folder = match pause_scope(request) {
            Ok(folder) => folder,
            Err(error) => return error_reply(error),
        };
        // Every chat that may have work, a superset decided one by one below
        // under its locks. A send holds the chat's gate from before the chat
        // reads as working until its turn has started, so one in progress now
        // still holds it, or has started a turn by the time the agents are
        // listed. Any turn, background work, or report on it runs in an agent
        // process this runtime keeps, so a chat whose work moves between those
        // while this looks is listed either way. Working rows add turns that
        // died with a previous host. The second pass waits for the gates.
        let gates = self
            .chat_execution
            .lock()
            .map(|gates| {
                gates
                    .iter()
                    .map(|(chat_id, gate)| (chat_id.clone(), gate.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut chats = gates
            .into_iter()
            .filter(|(_, gate)| matches!(gate.try_lock(), Err(std::sync::TryLockError::WouldBlock)))
            .map(|(chat_id, _)| chat_id)
            .collect::<Vec<_>>();
        let mut add = |chat_id: String| {
            if !chats.contains(&chat_id) {
                chats.push(chat_id);
            }
        };
        runtime.chats_with_agents().into_iter().for_each(&mut add);
        match store.working_chats() {
            Ok(working) => working.into_iter().for_each(&mut add),
            Err(error) => return error_reply(error),
        }
        // Another project's chats are never waited on; the check is made again
        // under each gate below.
        chats.retain(|chat_id| in_scope(store, chat_id, folder));
        let mut paused = Vec::new();
        let mut pause = |chat_id: String, gate: std::sync::MutexGuard<'_, ()>| {
            // Asked under the gate too: a chat moved out of the folder stays.
            if !in_scope(store, &chat_id, folder) {
                return;
            }
            // The gate is held so no message can start a turn between marking
            // the chat and stopping it: the stop would take the new turn instead.
            let stopped = runtime.stop_chat_if(&chat_id, |background| {
                store
                    .pause_chat(&chat_id, background)
                    .unwrap_or_else(|error| {
                        eprintln!("xd-host: cannot pause chat {chat_id}: {error}");
                        false
                    })
            });
            match stopped {
                None => return,
                // As with stop: a turn that died with a previous host still ends.
                Some(false) => {
                    let _ = store.clear_interrupted_turn(&chat_id);
                }
                Some(true) => {}
            }
            drop(gate);
            self.events
                .publish(json!({"event": "changed", "chat": chat_id}));
            paused.push(chat_id);
        };
        // Chats a message is being sent to wait until it is: everything else
        // stops first, so one slow send does not hold up the rest.
        let mut busy = Vec::new();
        for chat_id in chats {
            let Ok(gate) = self.chat_execution_gate(&chat_id) else {
                continue;
            };
            match gate.try_lock() {
                Ok(execution) => pause(chat_id, execution),
                Err(std::sync::TryLockError::WouldBlock) => busy.push((chat_id, gate.clone())),
                Err(std::sync::TryLockError::Poisoned(_)) => {}
            }
        }
        for (chat_id, gate) in &busy {
            if let Ok(execution) = gate.lock() {
                pause(chat_id.clone(), execution);
            }
        }
        if !paused.is_empty() {
            self.events.publish(json!({"event": "tree"}));
        }
        json!({"ok": true, "paused": paused})
    }

    /// Picks every paused chat back up: continues the turn pausing cut short,
    /// after which its queue runs as usual.
    fn resume_all(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let folder = match pause_scope(request) {
            Ok(folder) => folder,
            Err(error) => return error_reply(error),
        };
        let chats = match store.paused_chats() {
            Ok(chats) => chats
                .into_iter()
                .filter(|chat_id| in_scope(store, chat_id, folder))
                .collect::<Vec<_>>(),
            Err(error) => return error_reply(error),
        };
        let mut resumed = Vec::new();
        for chat_id in chats {
            // Asked again as each chat's turn comes: one moved to another
            // project since the list was read stays paused. A move racing this
            // check is ordered either way, as if it came just after the resume.
            if !in_scope(store, &chat_id, folder) {
                continue;
            }
            if self.resume_paused_chat(&chat_id, &|chat_id| in_scope(store, chat_id, folder)) {
                resumed.push(chat_id);
            }
        }
        json!({"ok": true, "resumed": resumed})
    }

    /// Picks one paused chat back up: continues the turn pausing cut short,
    /// after which its queue runs as usual. `in_scope` is asked again under
    /// the chat's gate before the chat is let go without a turn. True when
    /// the chat went on.
    fn resume_paused_chat(&self, chat_id: &str, in_scope: &dyn Fn(&str) -> bool) -> bool {
        let Some(store) = self.store.as_ref() else {
            return false;
        };
        let chat_id = chat_id.to_owned();
        {
            let before = store.latest_user_message(&chat_id).ok();
            let reply = self.send_message_inner(
                &json!({"op": "send", "chat": chat_id, "text": storage::CONTINUE_PROMPT}),
                Some(Continuation::Resume),
            );
            if reply["ok"] != true {
                // A continuation that was stored got as far as starting its
                // turn, which failed and left the chat paused for another try.
                // Only one refused outright falls back to the queue.
                if store.latest_user_message(&chat_id).ok() != before
                    || !store.chat_is_paused(&chat_id).unwrap_or(false)
                {
                    return false;
                }
                // Nothing to continue: the chat moved on, or its turn is still
                // stopping. Let the queue go on, and leave the turn to resume
                // from the chat. Unpaused first, so a turn ending meanwhile
                // hands the queue on itself; an idle chat starts it here. All
                // under the gate, so a pause-all since is either seen here or
                // stops what this starts.
                let Ok(gate) = self.chat_execution_gate(&chat_id) else {
                    return false;
                };
                let Ok(_execution) = gate.lock() else {
                    return false;
                };
                if !in_scope(&chat_id) || !store.unpause_chat(&chat_id).unwrap_or(false) {
                    return false;
                }
                self.events.publish(json!({"event": "tree"}));
                let queued = store
                    .chat(&chat_id)
                    .ok()
                    .and_then(|chat| chat["queue"][0].as_str().map(str::to_owned));
                if let Some(text) = queued
                    && let Err(error) = self.start_queued_if_idle(&chat_id, &text, true, true)
                {
                    eprintln!("xd-host: cannot resume chat {chat_id}'s queue: {error}");
                    return false;
                }
            }
        }
        self.events
            .publish(json!({"event": "changed", "chat": chat_id}));
        true
    }

    /// Redirects the running turn to a queued message: into the turn itself
    /// when its agent can take it, otherwise by stopping the turn with the
    /// message first in line.
    /// `gated` when the caller already holds the chat's execution gate.
    fn steer_queued(&self, request: &Value, gated: bool) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        // The whole steer runs under the chat's gate, as a send does, so a
        // pause-all either comes first and is undone by this explicit steer, or
        // comes after and stops whatever it started.
        let gate = if gated {
            None
        } else {
            match self.chat_execution_gate(request["chat"].as_str().unwrap_or_default()) {
                Ok(gate) => Some(gate),
                Err(error) => return error_reply(error),
            }
        };
        let _execution = match &gate {
            Some(gate) => match gate.lock() {
                Ok(execution) => Some(execution),
                Err(_) => return error_reply("This session's agent state is unavailable."),
            },
            None => None,
        };
        // A chat stopped at its quota keeps its queue as it is until the
        // user gives it a new quota.
        if store
            .chat_quota(request["chat"].as_str().unwrap_or_default())
            .is_ok_and(|quota| quota.reached)
        {
            return error_reply(storage::QUOTA_REACHED_MESSAGE);
        }
        match store.take_queued(request) {
            Ok((mut reply, event)) => {
                self.events.publish(event);
                let chat_id = request["chat"].as_str().unwrap_or_default();
                let text = request["text"].as_str().unwrap_or_default();
                // Steering asks for this message now, so a paused chat goes on:
                // a turn still stopping for the pause then hands it over.
                let unpaused = store.unpause_chat(chat_id).unwrap_or(false);
                if unpaused {
                    self.events.publish(json!({"event": "tree"}));
                }
                let steered = self
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.steer(chat_id, text));
                if !steered {
                    match store.requeue_front(chat_id, text) {
                        Ok(event) => self.events.publish(event),
                        Err(error) => return error_reply(error),
                    }
                    if let Some(runtime) = &self.runtime {
                        runtime.cancel(chat_id);
                    }
                    if let Err(error) = self.start_queued_if_idle(chat_id, text, true, unpaused) {
                        return error_reply(error);
                    }
                }
                reply["steered"] = Value::Bool(steered);
                reply
            }
            Err(error) => error_reply(error),
        }
    }

    /// The turn a steer was meant for may have ended before it could take it,
    /// with nothing left to start the message when it does. A turn still ending
    /// picks the message up from the queue itself.
    ///
    /// Like any send it starts the turn under the chat's execution gate, taken
    /// here unless the caller (`gated`) already holds it, so `pause-all` sees
    /// either the idle chat or the started turn.
    ///
    /// `was_paused` when the caller has just unpaused the chat, which pauses
    /// again if the message fails to start.
    fn start_queued_if_idle(
        &self,
        chat_id: &str,
        text: &str,
        gated: bool,
        was_paused: bool,
    ) -> Result<(), String> {
        let (Some(store), Some(runtime)) = (self.store.as_ref(), self.runtime.as_ref()) else {
            return Ok(());
        };
        let gate = if gated {
            None
        } else {
            Some(self.chat_execution_gate(chat_id)?)
        };
        let _execution = match &gate {
            Some(gate) => Some(
                gate.lock()
                    .map_err(|_| "This session's agent state is unavailable.".to_owned())?,
            ),
            None => None,
        };
        let chat = store.chat(chat_id).map_err(|error| error.to_string())?;
        if chat["working"] == true || chat["queue"][0].as_str() != Some(text) {
            return Ok(());
        }
        let unpauses = store.chat_is_paused(chat_id).unwrap_or(false);
        let (_, event) = store
            .take_queued(&json!({"chat": chat_id, "index": 0, "text": text}))
            .map_err(|error| error.to_string())?;
        self.events.publish(event);
        match store
            .prepare_send(&json!({"chat": chat_id, "text": text}))
            .map_err(|error| error.to_string())?
        {
            SendDisposition::Queued { event, .. } => {
                self.events.publish(event);
                Ok(())
            }
            SendDisposition::Start { turn, .. } => {
                if unpauses {
                    self.events.publish(json!({"event": "tree"}));
                }
                if let Err(error) = runtime.start(turn.clone()) {
                    let _ = store.abort_turn_start(&turn.chat_id, &error);
                    self.keep_paused_after_failed_start(chat_id, unpauses || was_paused);
                    return Err(error);
                }
                Ok(())
            }
        }
    }

    fn remove_worktree(&self, request: &Value) -> Value {
        let Some(store) = self.store.as_ref() else {
            return error_reply("Rust host state storage is not configured.");
        };
        let _resources = self
            .session_resources
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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

    fn release_app_client(&self, id: u64) {
        if let Ok(mut clients) = self.app_clients.lock()
            && clients.open.remove(&id)
        {
            clients.last_seen = Instant::now();
        }
    }

    fn unsubscribe(&self, id: u64) {
        self.release_app_client(id);
        self.pairing.unregister(id);
        self.events.unsubscribe(id);
    }
}

impl EventBus {
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

    fn publish(&self, mut event: Value) {
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
/// Links pull requests the host found by itself, and tells clients when the
/// chat's list changed. A failure only costs the link, so it is logged.
pub(crate) fn link_pull_requests(
    store: &StateStore,
    events: &EventBus,
    chat_id: &str,
    urls: &[String],
    source: LinkSource,
) {
    match store.link_pull_requests(chat_id, urls, source) {
        Ok(outcome) if outcome.changed => events.publish(json!({"event": "tree"})),
        Ok(_) => {}
        Err(error) => eprintln!("xd: cannot link a pull request to {chat_id}: {error}"),
    }
}

/// Asks GitHub again about a chat's links. A refresh already running for the
/// chat takes over a request made meanwhile and runs once more for it, and
/// keeps going while links wait their turn. Links asked about within the last
/// minute are skipped, so repeated requests cannot pile up lookups.
fn refresh_pull_request_states(store: &StateStore, events: &EventBus, chat_id: &str) {
    /// Chats being refreshed, and whether another refresh was asked for
    /// since the current pass began.
    static REFRESHING: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());
    // A chat links at most 32, eight a pass, with room for requests made
    // meanwhile.
    const MAX_PASSES: usize = 8;
    {
        let Ok(mut refreshing) = REFRESHING.lock() else {
            return;
        };
        if let Some((_, again)) = refreshing.iter_mut().find(|(chat, _)| chat == chat_id) {
            *again = true;
            return;
        }
        refreshing.push((chat_id.to_owned(), false));
    }
    let mut changed = false;
    for _ in 0..MAX_PASSES {
        let more = match store.refresh_linked_pull_request_states(chat_id) {
            Ok((pass_changed, more)) => {
                changed |= pass_changed;
                more
            }
            Err(error) => {
                eprintln!("xd: cannot refresh the pull requests of {chat_id}: {error}");
                false
            }
        };
        let Ok(mut refreshing) = REFRESHING.lock() else {
            break;
        };
        let asked_again = refreshing
            .iter_mut()
            .find(|(chat, _)| chat == chat_id)
            .is_some_and(|(_, again)| std::mem::take(again));
        if !more && !asked_again {
            refreshing.retain(|(chat, _)| chat != chat_id);
            drop(refreshing);
            if changed {
                events.publish(json!({"event": "tree"}));
            }
            return;
        }
    }
    if let Ok(mut refreshing) = REFRESHING.lock() {
        refreshing.retain(|(chat, _)| chat != chat_id);
    }
    if changed {
        events.publish(json!({"event": "tree"}));
    }
}

/// Answers one connection's requests from `queued` in the order they came.
fn answer_in_order(
    engine: Arc<Engine>,
    subscriber: u64,
    queued: std::sync::mpsc::Receiver<Value>,
    replies: SyncSender<Value>,
    stream: UnixStream,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while let Ok(request) = queued.recv() {
            if !engine.events.subscribed(subscriber) {
                break;
            }
            let reply = engine.dispatch_for(subscriber, request);
            if replies.try_send(reply).is_err() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                break;
            }
        }
    })
}

/// Git requests slow enough to hold up the reads queued behind them.
fn answered_in_git_lane(request: &Value) -> bool {
    matches!(
        request["op"].as_str(),
        Some(
            "diff-read"
                | "git-status"
                | "git-pr-status"
                | "git-pr-create"
                | "git-commit"
                | "git-push"
        )
    )
}

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
    if let Ok(mut subscribers) = engine.events.subscribers.lock()
        && let Some(client) = subscribers.get_mut(&subscriber)
    {
        client.events_ready = Some(events_ready.clone());
    }
    let writer_stream = stream.try_clone()?;
    let writer = thread::spawn(move || write_frames(writer_stream, frames));
    let (commands, queued) = sync_channel::<Value>(128);
    let worker = answer_in_order(
        engine.clone(),
        subscriber,
        queued,
        outbound.clone(),
        stream.try_clone()?,
    );
    // Git requests that can take seconds (a push, a GitHub round trip) are
    // answered in order on a lane of their own, so the chat and transcript
    // reads queued behind them are not held up. Only requests carrying a
    // request id go there: their replies may then overtake one another.
    let mut git_lane: Option<(SyncSender<Value>, JoinHandle<()>)> = None;
    let new_chats_aside = Arc::new(NewChatsAside::default());
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
                            | "pause-all"
                            | "respond"
                            | "subagent"
                            | "agent-auth-cancel"
                            | "daemon-stop"
                            | "app-client"
                    )
                );
            if authenticated && request["op"] == "pause-all" {
                // Off the reader as well: pausing waits for sends in progress,
                // and other control requests must not wait with it.
                let engine = engine.clone();
                let outbound = outbound.clone();
                thread::spawn(move || {
                    let reply = engine.dispatch_for(subscriber, request);
                    let _ = outbound.try_send(reply);
                });
            } else if control {
                let authenticating = matches!(request["op"].as_str(), Some("pair" | "hello"));
                let reply = engine.dispatch_for(subscriber, request);
                let authenticated = authenticating && reply["ok"] == true;
                outbound.try_send(reply).map_err(std::io::Error::other)?;
                if authenticated {
                    events_ready.store(true, Ordering::Release);
                }
            } else if authenticated
                && request["op"] == "new-chat"
                // Without an id to match it by, a reply must come in order.
                && request.get(REQUEST_ID).is_some()
            {
                // Off the request worker: creating a worktree can take a
                // while, and the client's other requests need not wait for
                // it. Replies carry their request id, so their order does
                // not matter.
                if let Some(request) = new_chats_aside.take_or_queue(request)? {
                    // A slot taken is given back if its thread cannot start,
                    // or closing the connection would wait for it forever.
                    let stream = match stream.try_clone() {
                        Ok(stream) => stream,
                        Err(error) => {
                            let _ = new_chats_aside.next(false);
                            return Err(error);
                        }
                    };
                    let engine = engine.clone();
                    let outbound = outbound.clone();
                    let aside = new_chats_aside.clone();
                    let spawned =
                        thread::Builder::new()
                            .name("xd-new-chat".into())
                            .spawn(move || {
                                let mut request = Some(request);
                                while let Some(next) = request {
                                    // A panic counts as no answer, so its slot is
                                    // still given back and the connection can close.
                                    let answered = !engine.events.subscribed(subscriber)
                                        || std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                            || engine.dispatch_for(subscriber, next),
                                        ))
                                        .is_ok_and(|reply| outbound.try_send(reply).is_ok());
                                    if !answered {
                                        let _ = stream.shutdown(std::net::Shutdown::Both);
                                    }
                                    request = aside.next(answered);
                                }
                            });
                    if let Err(error) = spawned {
                        let _ = new_chats_aside.next(false);
                        return Err(error);
                    }
                }
            } else if authenticated
                && request.get(REQUEST_ID).is_some()
                && answered_in_git_lane(&request)
            {
                let lane = match &git_lane {
                    Some((lane, _)) => lane.clone(),
                    None => {
                        let (lane, requests) = sync_channel::<Value>(128);
                        let worker = answer_in_order(
                            engine.clone(),
                            subscriber,
                            requests,
                            outbound.clone(),
                            stream.try_clone()?,
                        );
                        git_lane = Some((lane.clone(), worker));
                        lane
                    }
                };
                lane.try_send(request)
                    .map_err(|_| std::io::Error::other("Client command queue overflowed"))?;
            } else {
                commands
                    .try_send(request)
                    .map_err(|_| std::io::Error::other("Client command queue overflowed"))?;
            }
        }
    })();
    drop(commands);
    let git_worker = git_lane.map(|(lane, worker)| {
        drop(lane);
        worker
    });
    // The app has gone even while requests it sent are still being answered.
    engine.release_app_client(subscriber);
    if result.is_err() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    // A half-close requests a final drain; a disconnected client cannot block
    // this forever because daemon sockets have bounded write deadlines.
    let _ = worker.join();
    if let Some(worker) = git_worker {
        let _ = worker.join();
    }
    new_chats_aside.wait_idle();
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
    // A full event queue usually means stdout itself is blocked. Waiting for
    // that writer here would keep a dead stdio transport alive forever;
    // returning lets the dedicated host process exit and the client resync.
    result?;
    writer
        .join()
        .map_err(|_| std::io::Error::other("host writer panicked"))?
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

/// The folder `pause-all` and `resume-all` keep to, if they were given one.
fn pause_scope(request: &Value) -> Result<Option<&str>, &'static str> {
    match request.get("folder") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(folder)) if !folder.is_empty() => Ok(Some(folder)),
        Some(_) => Err("folder must be a folder id"),
    }
}

/// Whether the chat is in `folder`, or in a folder or project inside it.
/// Without a folder, every chat is.
fn in_scope(store: &StateStore, chat_id: &str, folder: Option<&str>) -> bool {
    folder.is_none_or(|folder| {
        store
            .folder_lineage_for_chat(chat_id)
            .is_ok_and(|lineage| lineage.iter().any(|id| id == folder))
    })
}

/// Adds `background_tasks` to every chat and session in the tree: how many
/// background agents and shells its kept agent process is still running. A chat
/// with no running turn and some of these is waiting on them. Only live
/// processes have any, so after a daemon restart nothing is waiting.
fn overlay_background_tasks(tree: &mut Value, counts: &HashMap<String, usize>) {
    let mut by_session: HashMap<String, usize> = HashMap::new();
    for chat in tree
        .get_mut("chats")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let count = chat["id"]
            .as_str()
            .and_then(|id| counts.get(id))
            .copied()
            .unwrap_or(0);
        chat["background_tasks"] = count.into();
        if let Some(session) = chat["session"].as_str() {
            *by_session.entry(session.to_owned()).or_default() += count;
        }
    }
    for session in tree
        .get_mut("sessions")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let count = session["id"]
            .as_str()
            .and_then(|id| by_session.get(id))
            .copied()
            .unwrap_or(0);
        session["background_tasks"] = count.into();
    }
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
    #[cfg(unix)]
    fn linking_over_the_protocol_updates_every_client_only_when_the_list_changes() {
        let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!("xd-engine-links-{}-{id}", std::process::id()));
        let workspaces = root.join("Workspaces");
        std::fs::create_dir_all(&workspaces).unwrap();
        let store = StateStore::open(root.join("chats.db"), &workspaces).unwrap();
        let engine = Engine::with_store(store);
        let folder = engine.dispatch(json!({"op": "new-folder", "name": "Project"}))["id"].clone();
        let chat = engine.dispatch(json!({"op": "new-chat", "folder": folder}))["id"].clone();
        let (sender, events) = sync_channel::<Value>(64);
        let (connection, _peer) = UnixStream::pair().unwrap();
        let _subscriber = engine.events.subscribe(sender, connection).unwrap();
        let saw_tree = |events: &std::sync::mpsc::Receiver<Value>| {
            std::iter::from_fn(|| events.try_recv().ok()).any(|event| event["event"] == "tree")
        };
        let url = "https://github.com/o/r/pull/3";

        let linked = engine.dispatch(json!({"op": "link-pull-requests", "chat": chat,
            "urls": [format!("{url}/files"), "not a link"], "source": "agent"}));
        assert_eq!(linked["linked"], json!([url]), "{linked}");
        assert_eq!(linked["invalid"], json!(["not a link"]));
        assert!(saw_tree(&events));
        engine.dispatch(json!({"op": "link-pull-requests", "chat": chat, "urls": [url]}));
        assert!(!saw_tree(&events), "nothing changed");
        assert_eq!(
            engine.dispatch(json!({"op": "tree"}))["chats"][0]["pull_requests"],
            json!([{"url": url}])
        );
        for refused in [
            json!({"op": "link-pull-requests", "chat": chat, "urls": []}),
            json!({"op": "link-pull-requests", "chat": chat, "urls": [url], "source": "branch"}),
            json!({"op": "link-pull-requests", "chat": "missing", "urls": [url]}),
            json!({"op": "unlink-pull-request", "chat": chat, "url": "https://example.com"}),
        ] {
            assert_eq!(engine.dispatch(refused.clone())["ok"], false, "{refused}");
        }

        let unlinked =
            engine.dispatch(json!({"op": "unlink-pull-request", "chat": chat, "url": url}));
        assert_eq!(unlinked["changed"], true);
        assert!(saw_tree(&events));
        assert!(
            engine.dispatch(json!({"op": "tree"}))["chats"][0]
                .get("pull_requests")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn settling_over_the_protocol_updates_every_client_and_respects_a_pending_question() {
        let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!("xd-engine-settle-{}-{id}", std::process::id()));
        let workspaces = root.join("Workspaces");
        std::fs::create_dir_all(&workspaces).unwrap();
        let store = StateStore::open(root.join("chats.db"), &workspaces).unwrap();
        let engine = Engine::with_store(store);
        let folder = engine.dispatch(json!({"op": "new-folder", "name": "Project"}))["id"].clone();
        let chat = engine.dispatch(json!({"op": "new-chat", "folder": folder, "title": "Task"}));
        let session = chat["session"].clone();
        let (sender, events) = sync_channel::<Value>(64);
        let (connection, _peer) = UnixStream::pair().unwrap();
        let _subscriber = engine.events.subscribe(sender, connection).unwrap();
        let saw_tree = |events: &std::sync::mpsc::Receiver<Value>| {
            std::iter::from_fn(|| events.try_recv().ok()).any(|event| event["event"] == "tree")
        };

        engine
            .runtime
            .as_ref()
            .unwrap()
            .hold_interaction_for_test(chat["id"].as_str().unwrap());
        let held = engine.dispatch(json!({"op": "settle-session", "session": session}));
        assert_eq!(held["ok"], false);
        assert!(
            held["error"]
                .as_str()
                .unwrap()
                .contains("approval or question")
        );
        assert!(!saw_tree(&events));
        assert!(engine.dispatch(json!({"op": "tree"}))["chats"][0]["settled_at"].is_null());

        engine
            .runtime
            .as_ref()
            .unwrap()
            .discard_chat(chat["id"].as_str().unwrap());
        assert_eq!(
            engine.dispatch(json!({"op": "settle-session", "session": session}))["ok"],
            true
        );
        assert!(saw_tree(&events));
        assert!(engine.dispatch(json!({"op": "tree"}))["chats"][0]["settled_at"].is_i64());

        assert_eq!(
            engine.dispatch(json!({"op": "unsettle-session", "session": session}))["ok"],
            true
        );
        assert!(saw_tree(&events));
        assert!(engine.dispatch(json!({"op": "tree"}))["chats"][0]["settled_at"].is_null());

        assert_eq!(
            engine.dispatch(json!({"op": "settle-session", "session": "missing"}))["ok"],
            false
        );

        let rules = engine.dispatch(json!({"op": "auto-settle-settings"}));
        assert_eq!(rules["after_days"], 3);
        assert_eq!(rules["on_merge"], true);
        assert_eq!(rules["keep_pull_requests"], false);
        let changed =
            engine.dispatch(json!({"op": "set-auto-settle-settings", "after_days": null}));
        assert!(changed["after_days"].is_null());
        assert!(
            std::iter::from_fn(|| events.try_recv().ok())
                .any(|event| event["event"] == "auto-settle-settings"
                    && event["after_days"].is_null())
        );
        assert_eq!(
            engine.dispatch(json!({"op": "set-auto-settle-settings", "after_days": 91}))["ok"],
            false
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn auto_delete_is_off_until_chosen_and_waits_for_a_pending_question() {
        let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!("xd-engine-delete-{}-{id}", std::process::id()));
        let workspaces = root.join("Workspaces");
        std::fs::create_dir_all(&workspaces).unwrap();
        let store = StateStore::open(root.join("chats.db"), &workspaces).unwrap();
        let engine = Engine::with_store(store);
        let folder = engine.dispatch(json!({"op": "new-folder", "name": "Project"}))["id"].clone();
        let chat = engine.dispatch(json!({"op": "new-chat", "folder": folder, "title": "Task"}));
        let chat_id = chat["id"].as_str().unwrap();
        let (sender, events) = sync_channel::<Value>(64);
        let (connection, _peer) = UnixStream::pair().unwrap();
        let _subscriber = engine.events.subscribe(sender, connection).unwrap();
        let later = settlement::now_seconds() + 40 * settlement::DAY_SECONDS;
        let mut reported = HashSet::new();

        let rule = engine.dispatch(json!({"op": "auto-delete-settings"}));
        assert_eq!(rule["ok"], true);
        assert!(rule["after_days"].is_null());
        assert_eq!(engine.auto_delete_sweep(later, &mut reported), 0);

        for bad in [json!(0), json!(366), json!("30")] {
            assert_eq!(
                engine.dispatch(json!({"op": "set-auto-delete-settings", "after_days": bad}))["ok"],
                false
            );
        }
        let changed = engine.dispatch(json!({"op": "set-auto-delete-settings", "after_days": 30}));
        assert_eq!(changed["after_days"], 30);
        assert!(
            std::iter::from_fn(|| events.try_recv().ok())
                .any(|event| event["event"] == "auto-delete-settings" && event["after_days"] == 30)
        );

        let runtime = engine.runtime.as_ref().unwrap();
        runtime.hold_interaction_for_test(chat_id);
        assert_eq!(engine.auto_delete_sweep(later, &mut reported), 0);
        runtime.discard_chat(chat_id);
        assert_eq!(
            engine.auto_delete_sweep(settlement::now_seconds(), &mut reported),
            0,
            "not idle long enough yet"
        );

        assert_eq!(engine.auto_delete_sweep(later, &mut reported), 1);
        assert!(
            std::iter::from_fn(|| events.try_recv().ok()).any(|event| event["event"] == "tree")
        );
        let tree = engine.dispatch(json!({"op": "tree"}));
        assert!(tree["chats"].as_array().unwrap().is_empty());
        assert!(tree["sessions"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn using_a_terminal_keeps_an_idle_session_and_deleting_closes_it() {
        let root = test_directory();
        let store = StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap();
        let engine = Engine::with_store(store);
        let folder = engine.dispatch(json!({"op": "new-folder", "name": "Project"}))["id"].clone();
        let chat = engine.dispatch(json!({"op": "new-chat", "folder": folder}));
        let resource = chat["terminal_chat"].as_str().unwrap().to_owned();
        engine
            .terminals
            .open(&json!({"chat": resource}), &root.join("Workspaces/Project"))
            .unwrap();
        let old = settlement::now_seconds() - 40 * settlement::DAY_SECONDS;
        rusqlite::Connection::open(root.join("chats.db"))
            .unwrap()
            .execute_batch(&format!(
                "UPDATE chats SET created_at = {old}, last_user_message_at = {micros}; \
                 UPDATE workspace_sessions SET created_at = {old};",
                micros = old * 1_000_000
            ))
            .unwrap();
        engine.dispatch(json!({"op": "set-auto-delete-settings", "after_days": 30}));
        let mut reported = HashSet::new();

        assert_eq!(
            engine.auto_delete_sweep(settlement::now_seconds(), &mut reported),
            0,
            "the terminal was just used"
        );
        let owners = HashSet::from([resource.clone()]);
        assert!(!engine.terminals.close_unused(&owners, old));
        engine.terminals.kill_chat(&resource);
        assert!(
            !engine.terminals.close_unused(&owners, old),
            "a terminal closed since the sweep looked still counts"
        );
        assert_eq!(
            engine.auto_delete_sweep(settlement::now_seconds(), &mut reported),
            0,
            "a closed terminal's use is remembered"
        );
        let opened = engine
            .terminals
            .open(&json!({"chat": resource}), &root.join("Workspaces/Project"))
            .unwrap();

        let later = settlement::now_seconds() + 31 * settlement::DAY_SECONDS;
        assert_eq!(engine.auto_delete_sweep(later, &mut reported), 1);
        assert!(engine.terminals.last_used(&owners).is_none());
        assert!(
            engine
                .terminals
                .input(&json!({"terminal": opened["id"], "data": STANDARD.encode(b"ls\n")}))
                .is_err(),
            "the terminal is closed"
        );
        let _ = std::fs::remove_dir_all(root);
    }

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
    fn background_output_requires_a_real_chat_and_ignores_client_paths() {
        let root = env::temp_dir().join(format!("xd-background-protocol-{}", uuid::Uuid::new_v4()));
        let store = StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap();
        let engine = Engine::with_store(store);
        let folder = engine.dispatch(json!({"op":"new-folder", "name":"Demo"}));
        let chat =
            engine.dispatch(json!({"op":"new-chat", "folder":folder["id"], "backend":"claude"}));
        let reply = engine.dispatch(json!({"op":"background-task-output", "chat":chat["id"],
            "task":"/etc/passwd", "path":"/etc/passwd", "_xd_request":99}));
        assert_eq!(
            reply,
            json!({"ok":true,"chat":chat["id"],"tasks":[],"_xd_request":99})
        );
        assert_eq!(
            engine.dispatch(json!({"op":"background-task-output"}))["ok"],
            false
        );
        assert_eq!(
            engine.dispatch(json!({"op":"background-task-output","chat":"missing"}))["ok"],
            false
        );
        engine.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ping_echoes_the_private_request_id() {
        assert_eq!(
            dispatch(json!({"op": "ping", "_xd_request": 42})),
            json!({"ok": true, "_xd_request": 42})
        );
    }

    #[test]
    fn the_tree_says_how_many_background_tasks_each_chat_and_session_waits_on() {
        let mut tree = json!({
            "ok": true,
            "chats": [
                {"id": "a", "session": "s1", "working": false},
                {"id": "b", "session": "s1", "working": true},
                {"id": "c", "session": "s2", "working": false},
            ],
            "sessions": [{"id": "s1"}, {"id": "s2"}],
        });
        overlay_background_tasks(
            &mut tree,
            &HashMap::from([("a".to_owned(), 2), ("b".to_owned(), 1)]),
        );
        let counts = |key: &str| -> Vec<u64> {
            tree[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["background_tasks"].as_u64().unwrap())
                .collect()
        };
        assert_eq!(counts("chats"), [2, 1, 0]);
        assert_eq!(counts("sessions"), [3, 0]);
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

    /// An agent started through start_agent runs as a chat nested under the
    /// chat that started it, and its finished turn comes back to that chat.
    #[cfg(unix)]
    #[test]
    fn a_started_agent_runs_as_a_child_chat_and_reports_back() {
        use std::os::unix::fs::PermissionsExt;

        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("xd-child-agent-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let agent = root.join("claude");
        std::fs::write(
            &agent,
            r#"#!/bin/sh
case "$*" in *--strict-mcp-config*) exit 0;; esac
while read message; do
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}'
echo '{"type":"result","terminal_reason":"completed","result":"Done."}'
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::set_var("XD_CLAUDE_EXECUTABLE", &agent) };
        let (engine, parent) = steer_engine(&root);
        let engine = Arc::new(engine);
        engine.start_reporting_agents();
        let store = engine.store.as_ref().unwrap();
        let wait_for = |what: &str, ready: &dyn Fn() -> bool| {
            for _ in 0..400 {
                if ready() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            panic!("timed out waiting for {what}");
        };

        let started = engine.dispatch(json!({"op": "start-agent", "chat": parent,
            "prompt": "Fix the tests", "title": "Tests", "workspace": "this_worktree"}));
        assert_eq!(started["ok"], true, "{started}");
        assert_eq!(started["backend"], "claude");
        let child = started["agent"].as_str().unwrap().to_owned();
        let tree = engine.dispatch(json!({"op": "tree"}));
        let listed = tree["chats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|chat| chat["id"] == child)
            .unwrap();
        assert_eq!(listed["parent"], parent);
        assert_eq!(listed["title"], "Tests");
        assert_eq!(tree["child_chats"], true);

        let parent_reply = |text: &str| {
            store.messages(&json!({"chat": parent})).unwrap()["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| {
                    row["content"]
                        .as_str()
                        .is_some_and(|row| row.starts_with(text))
                })
        };
        wait_for("the report to reach the parent", &|| {
            parent_reply(&format!(
                "[xd] Agent \"Tests\" ({child}) is done with its turn."
            ))
        });
        wait_for("the parent to answer it", &|| {
            parent_reply("Done.") && store.chat(&parent).unwrap()["working"] == false
        });
        // One card stands for the worker in the parent, rewritten as it goes.
        let cards = store.messages(&json!({"chat": parent})).unwrap()["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["content"].as_str())
            .filter(|text| text.starts_with("subagent_run\n"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(cards.len(), 1, "{cards:?}");
        let card: Value = serde_json::from_str(&cards[0]["subagent_run\n".len()..]).unwrap();
        assert_eq!(card["chat"], child);
        assert_eq!(card["key"], format!("chat:{child}"));
        assert_eq!(card["status"], "completed");
        assert_eq!(card["summary"], "Done.");
        assert_eq!(card["prompt"], "Fix the tests");
        let agents = engine.dispatch(json!({"op": "list-agents", "chat": parent}));
        assert_eq!(agents["agents"][0]["id"], child, "{agents}");
        assert_eq!(agents["agents"][0]["status"], "finished");
        assert_eq!(agents["agents"][0]["reply"], "Done.");

        let nested = engine.dispatch(json!({"op": "start-agent", "chat": child, "prompt": "More"}));
        assert_eq!(nested["ok"], false);
        let stranger = engine
            .dispatch(json!({"op": "message-agent", "chat": child, "agent": parent, "text": "Hi"}));
        assert_eq!(stranger["ok"], false);
        let stopped = engine.dispatch(json!({"op": "stop-agent", "chat": parent, "agent": child}));
        assert_eq!(stopped, json!({"ok": true, "was_working": false}));

        // A message the user sends the worker that cannot start a turn is
        // reported too; the parent's own message_agent just gets the error.
        engine.runtime.as_ref().unwrap().discard_chat(&child);
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::set_var("XD_CLAUDE_EXECUTABLE", root.join("missing")) };
        let failed = engine.dispatch(json!({"op": "send", "chat": child, "text": "Again"}));
        assert_eq!(failed["ok"], false, "{failed}");
        wait_for("the failure to reach the parent", &|| {
            parent_reply(&format!(
                "[xd] Agent \"Tests\" ({child}) is stopped with an error."
            ))
        });
        let refused = engine.dispatch(
            json!({"op": "message-agent", "chat": parent, "agent": child, "text": "Again"}),
        );
        assert_eq!(refused["ok"], false, "{refused}");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let reports = store.messages(&json!({"chat": parent})).unwrap()["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| {
                row["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("is stopped with an error"))
            })
            .count();
        assert_eq!(reports, 1);
        let cards = |store: &StateStore| {
            store.messages(&json!({"chat": parent})).unwrap()["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|row| row["content"].as_str()?.strip_prefix("subagent_run\n"))
                .map(|text| serde_json::from_str::<Value>(text).unwrap())
                .collect::<Vec<_>>()
        };
        // The card says the latest attempt failed, though the parent's own
        // message_agent was told directly and got no report.
        let card = cards(store);
        assert_eq!(card.len(), 1);
        assert_eq!(card[0]["status"], "failed");
        // A worker that cannot start at all leaves no card behind.
        let failed_start = engine.dispatch(json!({"op": "start-agent", "chat": parent,
            "prompt": "More", "workspace": "this_worktree"}));
        assert_eq!(failed_start["ok"], false, "{failed_start}");
        assert_eq!(cards(store).len(), 1);

        for chat in [&parent, &child] {
            engine.runtime.as_ref().unwrap().discard_chat(chat);
        }
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::remove_var("XD_CLAUDE_EXECUTABLE") };
        let _ = std::fs::remove_dir_all(root);
    }

    fn steer_engine(root: &std::path::Path) -> (Engine, String) {
        std::fs::create_dir_all(root.join("Workspaces")).unwrap();
        let engine = Engine::with_store(
            StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap(),
        );
        let folder = engine.dispatch(json!({"op": "new-folder", "name": "Project"}))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let chat = engine
            .dispatch(json!({"op": "new-chat", "folder": folder, "backend": "claude"}))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        (engine, chat)
    }

    /// The turn is already over, so nothing would start the message later. It
    /// starts now, ahead of what was queued with it. A message that is no longer
    /// the one the client saw is refused.
    #[cfg(unix)]
    #[test]
    fn a_steer_with_no_turn_left_to_take_it_starts_the_message() {
        use std::os::unix::fs::PermissionsExt;

        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("xd-steer-idle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let agent = root.join("claude");
        std::fs::write(
            &agent,
            r#"#!/bin/sh
case "$*" in *--strict-mcp-config*) exit 0;; esac
read prompt
echo '{"type":"system","subtype":"init","session_id":"s1"}'
read never
"#,
        )
        .unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::set_var("XD_CLAUDE_EXECUTABLE", &agent) };
        let (engine, chat) = steer_engine(&root);
        let store = engine.store.as_ref().unwrap();
        store
            .requeue_front(&chat, "second")
            .and_then(|_| store.requeue_front(&chat, "first"))
            .unwrap();

        let refused = engine
            .dispatch(json!({"op": "steer-queue", "chat": chat, "index": 1, "text": "changed"}));
        assert_eq!(refused["ok"], false);
        assert_eq!(
            store.chat(&chat).unwrap()["queue"],
            json!(["first", "second"])
        );

        let reply = engine
            .dispatch(json!({"op": "steer-queue", "chat": chat, "index": 1, "text": "second"}));
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["steered"], false);
        let state = store.chat(&chat).unwrap();
        assert_eq!(state["queue"], json!(["first"]));
        assert_eq!(state["working"], true);
        let messages = store.messages(&json!({"chat": chat})).unwrap();
        assert!(
            messages["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["role"] == "user" && row["content"] == "second"),
            "{messages}"
        );

        engine.runtime.as_ref().unwrap().discard_chat(&chat);
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::remove_var("XD_CLAUDE_EXECUTABLE") };
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_send_marked_steer_goes_into_the_running_turn_and_not_the_queue() {
        use std::os::unix::fs::PermissionsExt;

        let _environment = crate::AGENT_ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("xd-steer-send-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let agent = root.join("claude");
        std::fs::write(
            &agent,
            r#"#!/bin/sh
case "$*" in *--strict-mcp-config*) exit 0;; esac
read first
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Begun."}}}'
read steer
echo '{"type":"result","is_error":true,"terminal_reason":"aborted_streaming"}'
echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Redirected."}]}}'
echo '{"type":"result","terminal_reason":"completed","result":"Redirected."}'
read next
"#,
        )
        .unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::set_var("XD_CLAUDE_EXECUTABLE", &agent) };

        let (engine, chat) = steer_engine(&root);
        let store = engine.store.as_ref().unwrap();
        let wait_for = |what: &str, ready: &dyn Fn() -> bool| {
            for _ in 0..400 {
                if ready() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            panic!("timed out waiting for {what}");
        };
        let first = engine.dispatch(json!({"op": "send", "chat": chat, "text": "first"}));
        assert_eq!(first["queued"], false, "{first}");
        // Until the run has begun the message would only return to the queue.
        wait_for("the agent to begin", &|| {
            engine
                .runtime
                .as_ref()
                .unwrap()
                .live_turn(&chat)
                .is_some_and(|turn| turn.segment == "Begun.")
        });

        let reply = engine.dispatch(
            json!({"op": "send", "chat": chat, "text": "Do this instead", "steer": true}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["steered"], true, "{reply}");
        wait_for("the turn to end", &|| {
            store.chat(&chat).unwrap()["working"] == false
        });
        let messages = store.messages(&json!({"chat": chat})).unwrap();
        let rows = messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["role"] != "duration")
            .map(|row| row["content"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows, ["first", "Begun.", "Do this instead", "Redirected."]);
        assert_eq!(store.chat(&chat).unwrap()["queue"], json!([]));

        engine.runtime.as_ref().unwrap().discard_chat(&chat);
        // SAFETY: agent executable overrides are serialized by AGENT_ENV_LOCK.
        unsafe { std::env::remove_var("XD_CLAUDE_EXECUTABLE") };
        let _ = std::fs::remove_dir_all(root);
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
    fn tree_names_the_github_owner_behind_a_workspace() {
        use std::process::Command;

        let root = test_directory();
        let workspaces = root.join("Workspaces");
        let store = StateStore::open(root.join("chats.db"), workspaces.clone()).unwrap();
        let engine = Engine::with_store(store);
        engine.dispatch(json!({"op": "new-folder", "name": "Plain"}));
        engine.dispatch(json!({"op": "new-folder", "name": "Project"}));
        let repository = workspaces.join("Project");
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
                .args([
                    "-C",
                    repository.to_str().unwrap(),
                    "remote",
                    "add",
                    "origin",
                    "git@github.com:RestartFU/xd.git",
                ])
                .status()
                .unwrap()
                .success()
        );

        let tree = engine.dispatch(json!({"op": "tree"}));
        let folders = tree["folders"].as_array().unwrap();
        let owned = folders
            .iter()
            .find(|folder| folder["name"] == "Project")
            .unwrap();
        let plain = folders
            .iter()
            .find(|folder| folder["name"] == "Plain")
            .unwrap();
        assert_eq!(owned["owner"], "RestartFU");
        assert_eq!(plain.get("owner"), None);

        drop(engine);
        fs::remove_dir_all(root).ok();
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
    fn usage_is_dispatched_and_validates_before_scanning() {
        let reply = dispatch(json!({"op": "usage-summary", "days": 3, "_xd_request": 10}));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["_xd_request"], 10);
        assert_eq!(reply["error"], "Usage windows are 1, 7, 30, or 90 days.");
        let reply = dispatch(json!({"op": "usage-consume-reset-credit", "provider": "claude"}));
        assert_eq!(reply["error"], "Only Codex has reset credits.");
        // Without a data directory there is nothing saved and nowhere to save.
        assert_eq!(dispatch(json!({"op": "usage-prices"}))["prices"], json!({}));
        assert_eq!(
            dispatch(json!({"op": "set-usage-prices", "prices": {}}))["ok"],
            false
        );
    }

    #[test]
    fn owner_icon_is_dispatched_and_validates_the_login_before_networking() {
        let reply = dispatch(json!({
            "op": "owner-icon",
            "owner": "not a login",
            "_xd_request": 9
        }));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["_xd_request"], 9);
        assert_eq!(reply["error"], "owner-icon needs a GitHub owner login.");
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
    fn slow_git_requests_do_not_hold_up_reads_sent_after_them() {
        use std::os::unix::fs::PermissionsExt;
        let root = test_directory();
        let store = StateStore::open(root.join("chats.db"), root.join("Workspaces")).unwrap();
        let folder = store.new_folder(&json!({"name": "Repo"})).unwrap()["id"].clone();
        let chat = store.new_chat(&json!({"folder": folder})).unwrap()["id"].clone();
        let repository = root.join("Workspaces/Repo");
        // git status asks this monitor first, which takes its time.
        let monitor = root.join("slow-monitor.sh");
        fs::write(&monitor, "#!/bin/sh\nsleep 2\nexit 1\n").unwrap();
        fs::set_permissions(&monitor, fs::Permissions::from_mode(0o755)).unwrap();
        for arguments in [
            vec!["init", "-q"],
            vec!["config", "core.fsmonitor", monitor.to_str().unwrap()],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(arguments)
                    .current_dir(&repository)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let engine = Arc::new(Engine::with_store(store));
        let (client, server) = UnixStream::pair().unwrap();
        let serving = {
            let engine = engine.clone();
            thread::spawn(move || serve_daemon_connection(server, engine, false))
        };
        let mut writer = client.try_clone().unwrap();
        writeln!(
            writer,
            "{}",
            json!({"op": "git-status", "chat": chat, "_xd_request": 1})
        )
        .unwrap();
        writeln!(writer, "{}", json!({"op": "tree", "_xd_request": 2})).unwrap();
        let started = Instant::now();
        let mut replies = BufReader::new(client.try_clone().unwrap()).lines();
        let mut order = Vec::new();
        let mut tree_after = None;
        while order.len() < 2 {
            let frame: Value = serde_json::from_str(&replies.next().unwrap().unwrap()).unwrap();
            if let Some(id) = frame.get("_xd_request").and_then(Value::as_u64) {
                if id == 2 {
                    tree_after = Some(started.elapsed());
                }
                order.push(id);
            }
        }
        assert_eq!(order, vec![2, 1], "the tree is answered first");
        let tree_after = tree_after.unwrap();
        assert!(
            tree_after < Duration::from_millis(1500),
            "the tree waited for git: {tree_after:?}"
        );
        client.shutdown(std::net::Shutdown::Both).unwrap();
        serving.join().unwrap().ok();
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
