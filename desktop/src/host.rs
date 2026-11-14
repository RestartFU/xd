use std::{
    collections::HashMap,
    env,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{
        Arc, LazyLock, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use async_channel::{Receiver, Sender};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::channel;
#[cfg(all(test, unix))]
use crate::local_socket::UnixStream;
use crate::model::{Attachment, AutoDeleteRules, AutoSettleRules, ChatQuotaSettings};
use crate::protocol::{AUTHENTICATED_FRAME_LIMIT, Frame, ProtocolCodec};

const MESSAGE_PAGE_SIZE: usize = 120;
/// What the host sends for `resume-turn`, shown until the transcript refetches.
pub const RESUME_TEXT: &str = "Continue where you left off.";
const PROCESS_STDERR_LIMIT: usize = 8 * 1024;
const COMMAND_QUEUE_LIMIT: usize = 256;
// Match mobile's ordinary call deadline. A live socket can still lose a
// snapshot reply, leaving both the transcript and composer loading forever.
const CHAT_LOAD_TIMEOUT: Duration = Duration::from_secs(30);
// Each timeout in a row doubles the next deadline for that snapshot, up to
// 2^4 times the first one: 30 s, 1, 2, 4, then 8 minutes.
const CHAT_LOAD_TIMEOUT_DOUBLINGS: u32 = 4;
// How long a request without a deadline of its own, such as a Git push, may
// hold up the snapshots queued behind it before the connection counts as
// stuck: the longest a snapshot's deadline grows.
const QUEUED_BEHIND_TIMEOUT: Duration =
    Duration::from_secs(CHAT_LOAD_TIMEOUT.as_secs() << CHAT_LOAD_TIMEOUT_DOUBLINGS);

struct PendingRequest {
    kind: RequestKind,
    started: Instant,
    /// How long a snapshot may go unanswered, fixed when it is sent so a
    /// later answer cannot shorten it; `None` for requests without one.
    deadline: Option<Duration>,
}

/// One snapshot that reloads a chat, which the deadline covers: the tree, a
/// chat's settings, or one page of its transcript.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Snapshot {
    Tree,
    Chat(String),
    Messages(String, MessageCursor),
}

impl Snapshot {
    fn of(kind: &RequestKind) -> Option<Self> {
        match kind {
            RequestKind::Tree => Some(Self::Tree),
            RequestKind::Chat { chat_id } => Some(Self::Chat(chat_id.clone())),
            RequestKind::Messages { chat_id, cursor } => {
                Some(Self::Messages(chat_id.clone(), *cursor))
            }
            _ => None,
        }
    }
}

/// How often each snapshot has timed out in a row on one host, shared by
/// every connection to it. A host that is slow rather than stuck, such as a
/// large transcript page over a slow link, would otherwise time out on every
/// reconnect; doubling the deadline lets it finish. An answer to that same
/// snapshot brings its deadline back down to the shortest that answer fit
/// in, so a page that keeps needing a minute keeps getting one.
#[derive(Default)]
struct SnapshotDeadlines {
    timeouts: Mutex<HashMap<Snapshot, u32>>,
}

impl SnapshotDeadlines {
    fn for_host(identity: &Path) -> Arc<Self> {
        static HOSTS: LazyLock<Mutex<HashMap<PathBuf, Arc<SnapshotDeadlines>>>> =
            LazyLock::new(Mutex::default);
        let mut hosts = HOSTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        hosts.entry(identity.to_owned()).or_default().clone()
    }

    fn deadline(&self, snapshot: &Snapshot, base: Duration) -> Duration {
        let timeouts = self
            .timeouts
            .lock()
            .map(|timeouts| timeouts.get(snapshot).copied().unwrap_or(0))
            .unwrap_or(0);
        base.saturating_mul(1 << timeouts.min(CHAT_LOAD_TIMEOUT_DOUBLINGS))
    }

    fn timed_out(&self, snapshot: Snapshot) {
        if let Ok(mut timeouts) = self.timeouts.lock() {
            let count = timeouts.entry(snapshot).or_default();
            *count = count.saturating_add(1);
        }
    }

    fn answered(&self, snapshot: &Snapshot, took: Duration, base: Duration) {
        let needed = (0..CHAT_LOAD_TIMEOUT_DOUBLINGS)
            .find(|&doublings| base.saturating_mul(1 << doublings) > took)
            .unwrap_or(CHAT_LOAD_TIMEOUT_DOUBLINGS);
        if let Ok(mut timeouts) = self.timeouts.lock() {
            if needed == 0 {
                timeouts.remove(snapshot);
            } else {
                timeouts.insert(snapshot.clone(), needed);
            }
        }
    }
}

/// The sidebar tree read on one connection. The host answers a connection's
/// requests in order, and a tree read can take a while on a busy host, so
/// every `tree` event sending its own read queued a chat's own snapshots
/// behind a row of them. One read is on its way at a time; asking meanwhile
/// reads the tree once more after it lands.
#[derive(Default)]
struct TreeRead {
    in_flight: bool,
    again: bool,
}

struct ConnectionRequests {
    pending: Mutex<HashMap<u64, PendingRequest>>,
    tree: Mutex<TreeRead>,
    /// When the oldest request then waiting was answered. The host answers a
    /// connection's requests one at a time, in order, so the next one only
    /// starts being worked on then.
    head_answered: Mutex<Option<Instant>>,
    closed: AtomicBool,
    /// Held to publish an update or to close, so nothing is published after
    /// the one Disconnected.
    publishing: Mutex<()>,
    deadlines: Arc<SnapshotDeadlines>,
    /// A snapshot's deadline before it has timed out on this host.
    timeout: Duration,
}

impl ConnectionRequests {
    fn new(deadlines: Arc<SnapshotDeadlines>, timeout: Duration) -> Self {
        Self {
            pending: Mutex::default(),
            tree: Mutex::default(),
            head_answered: Mutex::default(),
            closed: AtomicBool::new(false),
            publishing: Mutex::default(),
            deadlines,
            timeout,
        }
    }

    /// Held while a tree read is claimed and sent, so a caller asking for one
    /// meanwhile cannot be lost if the send fails.
    fn tree_read(&self) -> MutexGuard<'_, TreeRead> {
        self.tree
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A request about to be sent, with its snapshot deadline as of now.
    fn pending_request(&self, kind: RequestKind, started: Instant) -> PendingRequest {
        let deadline =
            Snapshot::of(&kind).map(|snapshot| self.deadlines.deadline(&snapshot, self.timeout));
        PendingRequest {
            kind,
            started,
            deadline,
        }
    }

    /// Takes the request a reply answers, noting when the oldest one was and
    /// how long a snapshot took to answer.
    fn take_reply(&self, request_id: u64, now: Instant) -> Option<RequestKind> {
        let mut pending = self.pending.lock().ok()?;
        let request = pending.remove(&request_id)?;
        let mut head_answered = self.head_answered.lock().ok()?;
        if let Some(snapshot) = Snapshot::of(&request.kind) {
            let since = head_answered.map_or(request.started, |at| at.max(request.started));
            self.deadlines.answered(
                &snapshot,
                now.saturating_duration_since(since),
                self.timeout,
            );
        }
        if pending.keys().all(|&other| other > request_id) {
            *head_answered = Some(now);
        }
        Some(request.kind)
    }

    fn publishing(&self) -> MutexGuard<'_, ()> {
        self.publishing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Publishes an update unless the connection has closed.
    fn publish(&self, updates: &Sender<HostUpdate>, update: HostUpdate) -> bool {
        let _publishing = self.publishing();
        !self.closed.load(Ordering::Acquire) && updates.send_blocking(update).is_ok()
    }

    /// Whether the connection has stopped answering while a snapshot waits.
    /// Only the oldest unanswered request is being worked on, so only its
    /// clock runs, from when it was sent or the request before it was
    /// answered: a snapshot queued behind a slow but healthy reply is not
    /// late. A later reply does not restart that clock, so a lost reply
    /// still expires.
    fn expire_chat_load(&self, now: Instant) -> bool {
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        if !pending.values().any(|request| request.deadline.is_some()) {
            return false;
        }
        let Some((_, head)) = pending.iter().min_by_key(|(id, _)| **id) else {
            return false;
        };
        let head_answered = self
            .head_answered
            .lock()
            .ok()
            .and_then(|head_answered| *head_answered);
        let since = head_answered.map_or(head.started, |answered| answered.max(head.started));
        let allowed = head.deadline.unwrap_or(QUEUED_BEHIND_TIMEOUT);
        if now.saturating_duration_since(since) < allowed
            || self.closed.swap(true, Ordering::AcqRel)
        {
            return false;
        }
        if let Some(snapshot) = Snapshot::of(&head.kind) {
            self.deadlines.timed_out(snapshot);
        }
        pending.clear();
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageCursor {
    Tail,
    Before(i64),
    After(i64),
}

/// A new chat's first message, which names the worktree made with the chat,
/// and the Git writer the user chose to name worktrees, if any.
#[derive(Clone, Copy, Debug)]
pub struct FirstMessage<'a> {
    pub text: &'a str,
    pub effort: Option<&'a str>,
    pub access: Option<&'a str>,
    pub plan: Option<bool>,
    /// The account the chat runs on, or none to let the host choose.
    pub account: Option<&'a str>,
    pub attachments: &'a [Attachment],
    pub worktree_backend: Option<&'a str>,
    pub worktree_model: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewSessionWorktree {
    New,
    Existing(String),
    Branch { reference: String, oid: String },
    PullRequest { number: u64, oid: String },
}

/// What to create with `new-folder`: a project, optionally backed by a
/// repository, or a folder of projects (`group`), which an organization's
/// `owner` names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NewFolder<'a> {
    pub name: &'a str,
    pub parent: Option<&'a str>,
    pub repo: Option<&'a str>,
    pub repo_url: Option<&'a str>,
    pub group: bool,
    pub owner: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestKind {
    Tree,
    BackgroundTaskOutput {
        chat_id: String,
        generation: u64,
    },
    AgentCatalog,
    AgentAuth,
    AgentAuthMutation,
    AgentClis,
    HostUpdate {
        action: String,
    },
    AgentCliUpdates,
    ResumeInterrupted,
    /// `pause-all`, or `resume-all` when `resume`.
    PauseAll {
        resume: bool,
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
    /// Offers this desktop's browser to the host's agents.
    BrowserProvider,
    /// What an agent's browser command did; see `browser-command`.
    BrowserResult,
    RenameDevice {
        device_id: String,
    },
    RevokeDevice {
        device_id: String,
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
    OwnerIcon {
        owner: String,
    },
    UsageSummary {
        generation: u64,
    },
    UsageLimits {
        generation: u64,
    },
    UsagePrices {
        dialog: u64,
    },
    SetUsagePrices {
        dialog: u64,
    },
    UsageResetCredit,
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
    LinkPullRequests {
        chat_id: String,
        generation: u64,
    },
    UnlinkPullRequest {
        chat_id: String,
        url: String,
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
    TerminalPasteImage {
        terminal_id: String,
    },
    TerminalResize {
        terminal_id: String,
    },
    /// A theme change's colours for a terminal; nothing waits on the reply.
    TerminalColors,
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
    NewChatSources {
        folder_id: String,
    },
    SetFolderSettings {
        folder_id: String,
    },
    NewFolder {
        name: String,
        repo: Option<String>,
        repo_url: Option<String>,
        parent: Option<String>,
        group: bool,
        owner: Option<String>,
    },
    NewChat {
        folder_id: String,
        title: String,
        workdir: Option<String>,
        /// Which new chat this is, for one started from the new-chat view;
        /// 0 otherwise.
        token: u64,
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
    RenameSession {
        session_id: String,
        title: String,
    },
    MoveSession {
        session_id: String,
        folder_id: Option<String>,
    },
    DeleteSession {
        session_id: String,
    },
    SettleSession {
        session_id: String,
    },
    UnsettleSession {
        session_id: String,
    },
    SetSessionAutoSettle {
        session_id: String,
    },
    AutoSettleSettings,
    SetAutoSettleSettings,
    AutoDeleteSettings,
    SetAutoDeleteSettings,
    /// `broadcasts` is how many `chat-quota-settings` broadcasts had arrived
    /// when the request was sent; a reply is older than any after it.
    ChatQuotaSettings {
        broadcasts: u64,
    },
    SetChatQuotaSettings {
        broadcasts: u64,
    },
    /// `set-chat-quota`: a new quota for one chat, which goes on if it had
    /// stopped at its last one.
    SetChatQuota {
        chat_id: String,
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
    /// An answer to an approval or a request for input.
    Respond {
        chat_id: String,
        request_id: String,
    },
    SetShortcuts {
        folder_id: Option<String>,
    },
    ChatTitleSettings,
    SetChatTitleSettings,
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
pub enum HostUpdate {
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
    #[cfg(all(test, unix))]
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
pub struct HostHandle {
    /// The writer stops once every handle is gone. The reader holds this
    /// weakly to send a tree read again, so it does not keep the writer up.
    commands: Arc<mpsc::SyncSender<Command>>,
    requests: Arc<ConnectionRequests>,
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

impl HostHandle {
    #[cfg(windows)]
    pub fn start_local() -> Result<(Self, Receiver<HostUpdate>, StartedHost), ConnectError> {
        let payload = crate::wsl::payload().map_err(ConnectError::Start)?;
        let data_name = channel::data_name().to_string_lossy().into_owned();
        let update_channel = if env::var("XD_UPDATE_CHANNEL").as_deref() == Ok("dev") {
            "dev"
        } else if channel::nightly() {
            "nightly"
        } else {
            "release"
        };
        let mut command = crate::wsl::command(
            crate::wsl::HOST_SCRIPT,
            &[
                crate::wsl::payload_argument(&payload),
                data_name.clone(),
                update_channel.into(),
            ],
        );
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Self::connect_command(command, PathBuf::from(format!("wsl:{data_name}"))).map_err(
            |error| {
                ConnectError::Start(format!(
                    "Cannot start WSL. Install a Linux distribution with wsl --install, finish its first-time setup, and install git and tmux inside it. {error}"
                ))
            },
        )
    }

    #[cfg(not(windows))]
    pub fn start_local() -> Result<(Self, Receiver<HostUpdate>, StartedHost), ConnectError> {
        let mut failures = Vec::new();
        let data = data_root();
        for launcher in launcher_candidates() {
            let mut command = ProcessCommand::new(&launcher);
            command
                .arg("stdio")
                .arg("--data")
                .arg(&data)
                // A host this starts stops once the app closes.
                .env("XD_APP_BRIDGE", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
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
    ) -> Result<(Self, Receiver<HostUpdate>, StartedHost), ConnectError> {
        crate::session_runtime::restore_browser_environment(&mut command);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
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
        let process_errors = child.stderr.take().map(capture_process_errors);
        let (handle, updates) =
            Self::connect_io_with_errors(reader, writer, identity, process_errors);
        Ok((handle, updates, StartedHost { child }))
    }

    pub fn connect_io(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        identity: PathBuf,
    ) -> (Self, Receiver<HostUpdate>) {
        Self::connect_io_with_errors(reader, writer, identity, None)
    }

    fn connect_io_with_errors(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        identity: PathBuf,
        process_errors: Option<Arc<Mutex<Vec<u8>>>>,
    ) -> (Self, Receiver<HostUpdate>) {
        Self::connect_io_with_timeout(reader, writer, identity, process_errors, CHAT_LOAD_TIMEOUT)
    }

    fn connect_io_with_timeout(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        identity: PathBuf,
        process_errors: Option<Arc<Mutex<Vec<u8>>>>,
        timeout: Duration,
    ) -> (Self, Receiver<HostUpdate>) {
        let (command_tx, command_rx) = mpsc::sync_channel(COMMAND_QUEUE_LIMIT);
        let (update_tx, update_rx) = async_channel::bounded(1024);
        let requests = Arc::new(ConnectionRequests::new(
            SnapshotDeadlines::for_host(&identity),
            timeout,
        ));

        let commands = Arc::new(command_tx);
        let _ = update_tx.try_send(HostUpdate::Connected { path: identity });
        spawn_writer(writer, command_rx, update_tx.clone(), requests.clone());
        spawn_reader(
            reader,
            update_tx.clone(),
            requests.clone(),
            Arc::downgrade(&commands),
            process_errors,
        );
        spawn_chat_load_watchdog(update_tx, requests.clone(), timeout);

        (Self { commands, requests }, update_rx)
    }

    #[cfg(all(test, unix))]
    pub fn connect(path: PathBuf) -> Result<(Self, Receiver<HostUpdate>), ConnectError> {
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

    /// Reads the sidebar tree, or once more after the read already on its
    /// way, which may predate whatever the caller wants to see.
    pub fn tree(&self) -> Result<(), String> {
        let mut tree = self.requests.tree_read();
        if self.requests.closed.load(Ordering::Acquire) {
            return Err(CONNECTION_CLOSED.to_owned());
        }
        if tree.in_flight {
            tree.again = true;
            return Ok(());
        }
        send_command(&self.commands, &self.requests, tree_command())?;
        tree.in_flight = true;
        Ok(())
    }

    pub fn agent_catalog(&self) -> Result<(), String> {
        self.send(RequestKind::AgentCatalog, json!({"op": "agent-catalog"}))
    }

    pub fn agent_auth(&self) -> Result<(), String> {
        self.send(RequestKind::AgentAuth, json!({"op": "agent-auth"}))
    }

    /// Lists the assistants' accounts without the host checking each again,
    /// to offer them for a chat.
    pub fn agent_accounts(&self) -> Result<(), String> {
        self.send(
            RequestKind::AgentAuth,
            json!({"op": "agent-auth", "check": false}),
        )
    }

    pub fn agent_clis(&self) -> Result<(), String> {
        self.send(RequestKind::AgentClis, json!({"op": "agent-clis"}))
    }

    pub fn host_update(&self, action: &str) -> Result<(), String> {
        self.send(
            RequestKind::HostUpdate {
                action: action.to_owned(),
            },
            json!({"op": "host-update", "action": action}),
        )
    }

    pub fn agent_cli_updates(&self, action: &str, provider: Option<&str>) -> Result<(), String> {
        let mut body = json!({"op": "agent-cli-updates", "action": action});
        if let Some(provider) = provider {
            body["provider"] = json!(provider);
        }
        self.send(RequestKind::AgentCliUpdates, body)
    }

    /// Tells the host whether to continue the turns its last restart cut off.
    pub fn resume_interrupted(&self, resume: bool) -> Result<(), String> {
        self.send(
            RequestKind::ResumeInterrupted,
            json!({"op": "resume-interrupted", "resume": resume}),
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

    /// An account change: `operation` with the provider and `extra` members,
    /// such as the account it is for.
    pub fn agent_account_action(
        &self,
        operation: &str,
        provider: &str,
        extra: Value,
    ) -> Result<(), String> {
        let mut body = json!({"op": operation, "provider": provider});
        if let (Some(body), Value::Object(extra)) = (body.as_object_mut(), extra) {
            body.extend(extra);
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

    /// Lets the host's agents use this desktop's integrated browser.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn offer_browser(&self) -> Result<(), String> {
        self.send(
            RequestKind::BrowserProvider,
            json!({"op": "browser-provider"}),
        )
    }

    /// Answers the host's `browser-command` numbered `request`.
    pub fn browser_result(
        &self,
        request: u64,
        outcome: Result<Value, String>,
    ) -> Result<(), String> {
        let mut body = json!({"op": "browser-result", "request": request});
        match outcome {
            Ok(result) => body["result"] = result,
            Err(error) => body["error"] = json!(error),
        }
        self.send(RequestKind::BrowserResult, body)
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

    /// Asks for token and cost totals. The answer arrives as a `usage-summary`
    /// event carrying `generation`, so a stale window can be told apart.
    pub fn usage_summary(
        &self,
        days: u32,
        time_zone: &str,
        now_ms: i64,
        refresh: bool,
        generation: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::UsageSummary { generation },
            json!({
                "op": "usage-summary",
                "days": days,
                "time_zone": time_zone,
                "now_ms": now_ms,
                "refresh": refresh,
                "generation": generation,
            }),
        )
    }

    /// Asks each installed assistant for its subscription limits. The answer
    /// arrives as a `usage-limits` event carrying `generation`.
    pub fn usage_limits(&self, time_zone: &str, generation: u64) -> Result<(), String> {
        self.send(
            RequestKind::UsageLimits { generation },
            json!({"op": "usage-limits", "time_zone": time_zone, "generation": generation}),
        )
    }

    pub fn usage_prices(&self, dialog: u64) -> Result<(), String> {
        self.send(
            RequestKind::UsagePrices { dialog },
            json!({"op": "usage-prices"}),
        )
    }

    pub fn set_usage_prices(
        &self,
        dialog: u64,
        prices: &Value,
        aliases: &Value,
    ) -> Result<(), String> {
        self.send(
            RequestKind::SetUsagePrices { dialog },
            json!({"op": "set-usage-prices", "prices": prices, "aliases": aliases}),
        )
    }

    /// Spends a banked Codex reset credit; `idempotency_key` names this attempt.
    /// The host refuses if `account_key` is no longer the signed-in account.
    pub fn usage_consume_reset_credit(
        &self,
        idempotency_key: &str,
        account_key: Option<&str>,
    ) -> Result<(), String> {
        self.send(
            RequestKind::UsageResetCredit,
            json!({
                "op": "usage-consume-reset-credit",
                "provider": "codex",
                "idempotency_key": idempotency_key,
                "account_key": account_key,
            }),
        )
    }

    /// Asks for the avatar of the account behind a workspace. The host answers
    /// from its cache, or reports the fetch as pending and sends an
    /// `owner-icon` event when it lands.
    pub fn owner_icon(&self, owner: &str) -> Result<(), String> {
        self.send(
            RequestKind::OwnerIcon {
                owner: owner.to_owned(),
            },
            json!({"op": "owner-icon", "owner": owner}),
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

    pub fn new_chat_sources(&self, folder_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::NewChatSources {
                folder_id: folder_id.to_owned(),
            },
            json!({"op": "new-chat-sources", "folder": folder_id}),
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

    pub fn chat_title_settings(&self) -> Result<(), String> {
        self.send(
            RequestKind::ChatTitleSettings,
            json!({"op": "chat-title-settings"}),
        )
    }

    pub fn set_chat_titles_enabled(&self, enabled: bool) -> Result<(), String> {
        self.send(
            RequestKind::SetChatTitleSettings,
            json!({"op": "set-chat-title-settings", "enabled": enabled}),
        )
    }

    pub fn set_chat_title_model(&self, backend: &str, model: &str) -> Result<(), String> {
        self.send(
            RequestKind::SetChatTitleSettings,
            json!({"op": "set-chat-title-settings", "backend": backend, "model": model}),
        )
    }

    pub fn new_folder(&self, folder: NewFolder<'_>) -> Result<(), String> {
        let mut body = json!({"op": "new-folder", "name": folder.name});
        if let Some(parent) = folder.parent {
            body["parent"] = Value::String(parent.to_owned());
        }
        if let Some(repo) = folder.repo {
            body["repo"] = Value::String(repo.to_owned());
        }
        if let Some(repo_url) = folder.repo_url {
            body["repo_url"] = Value::String(repo_url.to_owned());
        }
        if folder.group {
            body["group"] = Value::Bool(true);
        }
        if let Some(owner) = folder.owner {
            body["owner"] = Value::String(owner.to_owned());
        }
        self.send(
            RequestKind::NewFolder {
                name: folder.name.to_owned(),
                repo: folder.repo.map(str::to_owned),
                repo_url: folder.repo_url.map(str::to_owned),
                parent: folder.parent.map(str::to_owned),
                group: folder.group,
                owner: folder.owner.map(str::to_owned),
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
                NewSessionWorktree::Branch { reference, oid } => {
                    json!({"kind": "branch", "ref": reference, "oid": oid})
                }
                NewSessionWorktree::PullRequest { number, oid } => {
                    json!({"kind": "pull-request", "number": number, "oid": oid})
                }
            };
        }
        self.send(
            RequestKind::NewChat {
                folder_id: folder_id.to_owned(),
                title: title.to_owned(),
                workdir: workdir.map(str::to_owned),
                token: 0,
            },
            body,
        )
    }

    /// Materialize a composer draft only when its first message is submitted.
    #[allow(clippy::too_many_arguments)]
    pub fn create_draft_chat(
        &self,
        folder: &str,
        session: Option<&str>,
        title: &str,
        backend: &str,
        model: Option<&str>,
        worktree: Option<&NewSessionWorktree>,
        start_from_remote: bool,
        first_message: Option<FirstMessage<'_>>,
        token: u64,
    ) -> Result<(), String> {
        let mut body = json!({"op":"new-chat", "folder":folder, "title":title, "backend":backend});
        body["start_from_remote"] = json!(start_from_remote);
        if let Some(message) = first_message {
            body["draft"] = json!(message.text);
            body["draft_attachments"] = json!(
                message
                    .attachments
                    .iter()
                    .map(|a| json!({"name":a.name,"mime":a.mime,"data":a.data}))
                    .collect::<Vec<_>>()
            );
            if let Some(effort) = message.effort {
                body["effort"] = json!(effort);
            }
            if let Some(access) = message.access {
                body["access"] = json!(access);
            }
            if let Some(plan) = message.plan {
                body["plan"] = json!(plan);
            }
            if let Some(account) = message.account {
                body["account"] = json!(account);
            }
        }
        // A worktree made with the chat is named for what the chat is for:
        // from the message's first line at once, then by the naming model in
        // the background.
        if let Some(message) = first_message.filter(|message| !message.text.trim().is_empty()) {
            body["prompt"] = json!(message.text);
            body["generate_worktree_name"] = json!(true);
            if let Some(backend) = message.worktree_backend {
                body["worktree_backend"] = json!(backend);
            }
            if let Some(model) = message.worktree_model {
                body["worktree_model"] = json!(model);
            }
        }
        if let Some(session) = session {
            body["session"] = json!(session);
        }
        if let Some(model) = model {
            body["model"] = json!(model);
        }
        if session.is_none()
            && let Some(worktree) = worktree
        {
            body["worktree"] = match worktree {
                NewSessionWorktree::New => json!({"kind":"new"}),
                NewSessionWorktree::Existing(path) => json!({"kind":"existing", "path":path}),
                NewSessionWorktree::Branch { reference, oid } => {
                    json!({"kind":"branch", "ref":reference, "oid":oid})
                }
                NewSessionWorktree::PullRequest { number, oid } => {
                    json!({"kind":"pull-request", "number":number, "oid":oid})
                }
            };
        }
        self.send(
            RequestKind::NewChat {
                folder_id: folder.into(),
                title: title.into(),
                workdir: None,
                token,
            },
            body,
        )
    }

    pub fn new_chat_in_session(
        &self,
        folder_id: &str,
        session_id: &str,
        title: &str,
        backend: &str,
    ) -> Result<(), String> {
        self.send(
            RequestKind::NewChat {
                folder_id: folder_id.to_owned(),
                title: title.to_owned(),
                workdir: None,
                token: 0,
            },
            json!({"op": "new-chat", "folder": folder_id, "session": session_id,
                "title": title, "backend": backend}),
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
        self.place_folder_beside(folder_id, anchor_id, after, Ok(()))
    }

    /// Places a folder before or after `anchor_id`, moving it into the
    /// anchor's parent on disk first when they differ. The host refuses it if
    /// the anchor is no longer in `expect_parent` (`None`: the top level), so
    /// a stale tree cannot send the folder somewhere the user never saw.
    pub fn move_folder_beside(
        &self,
        folder_id: &str,
        anchor_id: &str,
        after: bool,
        expect_parent: Option<&str>,
    ) -> Result<(), String> {
        self.place_folder_beside(folder_id, anchor_id, after, Err(expect_parent))
    }

    /// Moves a folder into `parent_id` (`None`: the top level) and last among
    /// what is there, which the host decides when it handles the request.
    pub fn move_folder_to_end(
        &self,
        folder_id: &str,
        parent_id: Option<&str>,
    ) -> Result<(), String> {
        let mut body = json!({"op": "move-folder", "folder": folder_id, "end": true});
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

    /// `placement` is `Ok(())` to keep the parent, or the parent the anchor
    /// is expected in for a move.
    fn place_folder_beside(
        &self,
        folder_id: &str,
        anchor_id: &str,
        after: bool,
        placement: Result<(), Option<&str>>,
    ) -> Result<(), String> {
        let mut body = json!({"op": "move-folder", "folder": folder_id});
        match placement {
            Ok(()) => body["keep_parent"] = Value::Bool(true),
            Err(parent) => {
                body["expect_parent"] = parent.map_or(Value::Null, |parent| parent.into());
            }
        }
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

    pub fn rename_session(&self, session_id: &str, title: &str) -> Result<(), String> {
        self.send(
            RequestKind::RenameSession {
                session_id: session_id.to_owned(),
                title: title.to_owned(),
            },
            json!({"op": "rename-session", "session": session_id, "title": title}),
        )
    }

    pub fn move_session(&self, session_id: &str, folder_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::MoveSession {
                session_id: session_id.to_owned(),
                folder_id: Some(folder_id.to_owned()),
            },
            json!({"op": "move-session", "session": session_id, "folder": folder_id}),
        )
    }

    pub fn reorder_session(
        &self,
        session_id: &str,
        anchor_id: &str,
        after: bool,
    ) -> Result<(), String> {
        let mut body = json!({"op": "move-session", "session": session_id});
        body[if after { "after" } else { "before" }] = Value::String(anchor_id.to_owned());
        self.send(
            RequestKind::MoveSession {
                session_id: session_id.to_owned(),
                folder_id: None,
            },
            body,
        )
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::DeleteSession {
                session_id: session_id.to_owned(),
            },
            json!({"op": "delete-session", "session": session_id}),
        )
    }

    pub fn settle_session(&self, session_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::SettleSession {
                session_id: session_id.to_owned(),
            },
            json!({"op": "settle-session", "session": session_id}),
        )
    }

    pub fn unsettle_session(&self, session_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::UnsettleSession {
                session_id: session_id.to_owned(),
            },
            json!({"op": "unsettle-session", "session": session_id}),
        )
    }

    pub fn set_session_auto_settle(&self, session_id: &str, enabled: bool) -> Result<(), String> {
        self.send(
            RequestKind::SetSessionAutoSettle {
                session_id: session_id.to_owned(),
            },
            json!({"op": "set-session-auto-settle", "session": session_id, "enabled": enabled}),
        )
    }

    pub fn auto_settle_settings(&self) -> Result<(), String> {
        self.send(
            RequestKind::AutoSettleSettings,
            json!({"op": "auto-settle-settings"}),
        )
    }

    pub fn set_auto_settle_settings(&self, rules: AutoSettleRules) -> Result<(), String> {
        self.send(
            RequestKind::SetAutoSettleSettings,
            json!({
                "op": "set-auto-settle-settings",
                "after_days": rules.after_days,
                "on_merge": rules.on_merge,
                "keep_pull_requests": rules.keep_pull_requests,
            }),
        )
    }

    pub fn auto_delete_settings(&self) -> Result<(), String> {
        self.send(
            RequestKind::AutoDeleteSettings,
            json!({"op": "auto-delete-settings"}),
        )
    }

    pub fn set_auto_delete_settings(&self, rules: AutoDeleteRules) -> Result<(), String> {
        self.send(
            RequestKind::SetAutoDeleteSettings,
            json!({"op": "set-auto-delete-settings", "after_days": rules.after_days}),
        )
    }

    pub fn chat_quota_settings(&self, broadcasts: u64) -> Result<(), String> {
        self.send(
            RequestKind::ChatQuotaSettings { broadcasts },
            json!({"op": "chat-quota-settings"}),
        )
    }

    pub fn set_chat_quota_settings(
        &self,
        settings: ChatQuotaSettings,
        broadcasts: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::SetChatQuotaSettings { broadcasts },
            json!({"op": "set-chat-quota-settings", "percent": settings.percent}),
        )
    }

    /// Gives the chat its own quota, or with `None` no limit at all. The host
    /// counts its use again from zero, and continues it if it had stopped at
    /// its quota.
    pub fn set_chat_quota(&self, chat_id: &str, percent: Option<u8>) -> Result<(), String> {
        self.send(
            RequestKind::SetChatQuota {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "set-chat-quota", "chat": chat_id, "percent": percent}),
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

    pub fn background_task_output(&self, chat_id: &str, generation: u64) -> Result<(), String> {
        self.send(
            RequestKind::BackgroundTaskOutput {
                chat_id: chat_id.into(),
                generation,
            },
            json!({"op":"background-task-output", "chat":chat_id}),
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

    /// With `pull_request_lookup`, also asks about the branch's pull request;
    /// the event echoes that token so the answer can be matched to this request.
    pub fn git_state(
        &self,
        chat_id: &str,
        pull_request_lookup: Option<&str>,
    ) -> Result<(), String> {
        let mut body = json!({"op": "git-state", "chat": chat_id});
        if let Some(lookup) = pull_request_lookup {
            body["pull_request"] = Value::Bool(true);
            body["request"] = Value::String(lookup.to_owned());
        }
        self.send(
            RequestKind::GitState {
                chat_id: chat_id.to_owned(),
            },
            body,
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

    /// Links pull requests to a chat by their web addresses. The host answers
    /// the canonical addresses it kept in `linked` and the rest in `invalid`.
    pub fn link_pull_requests(
        &self,
        chat_id: &str,
        urls: &[String],
        generation: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::LinkPullRequests {
                chat_id: chat_id.to_owned(),
                generation,
            },
            json!({"op": "link-pull-requests", "chat": chat_id, "urls": urls}),
        )
    }

    pub fn unlink_pull_request(
        &self,
        chat_id: &str,
        url: &str,
        generation: u64,
    ) -> Result<(), String> {
        self.send(
            RequestKind::UnlinkPullRequest {
                chat_id: chat_id.to_owned(),
                url: url.to_owned(),
                generation,
            },
            json!({"op": "unlink-pull-request", "chat": chat_id, "url": url}),
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
            RequestKind::TerminalPasteImage {
                terminal_id: terminal_id.to_owned(),
            },
            json!({
                "op": "terminal-materialize-image",
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

    /// Replaces the colours `terminal_id` reports to programs that ask for
    /// them, which `terminal_open` set from the theme of the time.
    pub fn terminal_colors(
        &self,
        terminal_id: &str,
        foreground: u32,
        background: u32,
    ) -> Result<(), String> {
        self.send(
            RequestKind::TerminalColors,
            json!({
                "op": "terminal-colors",
                "terminal": terminal_id,
                "foreground": foreground,
                "background": background,
            }),
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
        steer: bool,
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
        // The host takes exactly these from the draft, so a message sent
        // without them, such as a shortcut, leaves the draft as it is.
        if !attachments.is_empty() {
            body["attachments"] = Value::Array(attachments);
        }
        if let Some(backend) = worktree_backend {
            body["worktree_backend"] = Value::String(backend.to_owned());
        }
        if let Some(model) = worktree_model {
            body["worktree_model"] = Value::String(model.to_owned());
        }
        if steer {
            body["steer"] = Value::Bool(true);
        }
        self.send(
            RequestKind::Send {
                chat_id: chat_id.to_owned(),
                text: text.to_owned(),
            },
            body,
        )
    }

    /// Continues the chat's cut-short turn. The host sends `RESUME_TEXT` for it,
    /// or refuses if the chat has moved on, and answers like any other send.
    pub fn resume_turn(&self, chat_id: &str) -> Result<(), String> {
        self.send(
            RequestKind::Send {
                chat_id: chat_id.to_owned(),
                text: RESUME_TEXT.to_owned(),
            },
            json!({"op": "resume-turn", "chat": chat_id}),
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
                // The host guards the edit against this key, hyphenated.
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

    /// Stops every chat at work in the folder, or with `resume` picks its
    /// paused ones up.
    pub fn pause_folder(&self, folder_id: &str, resume: bool) -> Result<(), String> {
        let op = if resume { "resume-all" } else { "pause-all" };
        self.send(
            RequestKind::PauseAll { resume },
            json!({"op": op, "folder": folder_id}),
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
        // `files`: the list sent replaces the files in the draft too.
        let mut body = json!({"op": "set-draft", "chat": chat_id, "text": text, "files": true});
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

    pub fn respond(&self, chat_id: &str, request_id: &str, response: Value) -> Result<(), String> {
        self.send(
            RequestKind::Respond {
                chat_id: chat_id.to_owned(),
                request_id: request_id.to_owned(),
            },
            json!({"op":"respond", "chat":chat_id,"request_id":request_id,"response":response}),
        )
    }

    /// Stops a running agent (`"stop"`) or sends one the turn waits on to the
    /// background (`"background"`).
    pub fn control_subagent(&self, chat_id: &str, key: &str, action: &str) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({"op": "subagent", "chat": chat_id, "key": key, "action": action}),
        )
    }

    pub fn set_model(&self, chat_id: &str, model: &str) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "model",
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

    /// Sets the account the chat runs on; an empty `account` lets the host
    /// choose again.
    pub fn set_account(&self, chat_id: &str, account: &str) -> Result<(), String> {
        self.send(
            RequestKind::SetOption {
                chat_id: chat_id.to_owned(),
            },
            json!({
                "op": "set-option",
                "chat": chat_id,
                "option": "account",
                "value": account,
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
        send_command(&self.commands, &self.requests, Command { kind, body })
    }
}

const CONNECTION_CLOSED: &str = "the xd state connection is closed";

fn tree_command() -> Command {
    Command {
        kind: RequestKind::Tree,
        body: json!({"op": "tree"}),
    }
}

fn send_command(
    commands: &mpsc::SyncSender<Command>,
    requests: &ConnectionRequests,
    command: Command,
) -> Result<(), String> {
    if requests.closed.load(Ordering::Acquire) {
        return Err(CONNECTION_CLOSED.to_owned());
    }
    commands.try_send(command).map_err(|error| match error {
        mpsc::TrySendError::Full(_) => "the xd state command queue is full".to_owned(),
        mpsc::TrySendError::Disconnected(_) => CONNECTION_CLOSED.to_owned(),
    })
}

fn spawn_writer(
    mut stream: impl Write + Send + 'static,
    commands: mpsc::Receiver<Command>,
    updates: Sender<HostUpdate>,
    requests: Arc<ConnectionRequests>,
) {
    thread::Builder::new()
        .name("xd-host-writer".into())
        .spawn(move || {
            let mut codec = ProtocolCodec::new();
            while let Ok(command) = commands.recv() {
                let (request_id, encoded) = match codec.encode_request(command.body) {
                    Ok(encoded) => encoded,
                    Err(error) => {
                        close_connection(&updates, &requests, error.to_string());
                        return;
                    }
                };
                if let Ok(mut pending) = requests.pending.lock() {
                    if requests.closed.load(Ordering::Acquire) {
                        return;
                    }
                    pending.insert(
                        request_id,
                        requests.pending_request(command.kind, Instant::now()),
                    );
                } else {
                    close_connection(
                        &updates,
                        &requests,
                        "host request state is unavailable".into(),
                    );
                    return;
                }
                if let Err(error) = stream.write_all(&encoded) {
                    if let Ok(mut pending) = requests.pending.lock() {
                        pending.remove(&request_id);
                    }
                    close_connection(
                        &updates,
                        &requests,
                        format!("could not write to xd host: {error}"),
                    );
                    return;
                }
            }
            // The last handle was dropped; the watchdog has no connection to
            // recover, even if its reader is still waiting for EOF.
            requests.closed.store(true, Ordering::Release);
        })
        .expect("spawn xd host writer");
}

fn spawn_reader(
    stream: impl Read + Send + 'static,
    updates: Sender<HostUpdate>,
    requests: Arc<ConnectionRequests>,
    commands: Weak<mpsc::SyncSender<Command>>,
    process_errors: Option<Arc<Mutex<Vec<u8>>>>,
) {
    thread::Builder::new()
        .name("xd-host-reader".into())
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            // Until the host has said something in its own protocol, what
            // arrives may be whatever started it explaining why it could not.
            let mut answered = false;
            loop {
                let mut line = Vec::new();
                let read = reader
                    .by_ref()
                    .take((AUTHENTICATED_FRAME_LIMIT + 1) as u64)
                    .read_until(b'\n', &mut line);
                match read {
                    Ok(0) => {
                        close_connection(
                            &updates,
                            &requests,
                            with_process_errors(
                                "xd host closed the connection",
                                process_errors.as_ref(),
                            ),
                        );
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        close_connection(
                            &updates,
                            &requests,
                            format!("could not read from xd host: {error}"),
                        );
                        return;
                    }
                }

                if line.len() <= AUTHENTICATED_FRAME_LIMIT && line.last() != Some(&b'\n') {
                    close_connection(
                        &updates,
                        &requests,
                        "xd host closed while sending a response".into(),
                    );
                    return;
                }

                let frame = match ProtocolCodec::decode_line(&line, AUTHENTICATED_FRAME_LIMIT) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => continue,
                    Err(error) => {
                        let message = (!answered)
                            .then(|| launcher_output(&line))
                            .flatten()
                            .map(|output| {
                                with_process_errors(
                                    &launcher_failure(&output),
                                    process_errors.as_ref(),
                                )
                            })
                            .unwrap_or_else(|| error.to_string());
                        close_connection(&updates, &requests, message);
                        return;
                    }
                };
                answered = true;
                if requests.closed.load(Ordering::Acquire) {
                    return;
                }
                let mut tree_answered = false;
                let update = match frame {
                    Frame::Event { name, mut body } => {
                        let attachments = take_draft_attachments(&mut body);
                        HostUpdate::Event {
                            name,
                            body,
                            attachments,
                        }
                    }
                    Frame::Reply {
                        request_id: Some(request_id),
                        mut body,
                    } => {
                        let Some(kind) = requests.take_reply(request_id, Instant::now()) else {
                            continue;
                        };
                        tree_answered = matches!(kind, RequestKind::Tree);
                        let attachments = take_draft_attachments(&mut body);
                        HostUpdate::Reply {
                            kind,
                            body,
                            attachments,
                        }
                    }
                    Frame::Reply {
                        request_id: None, ..
                    } => continue,
                };
                if !requests.publish(&updates, update) {
                    return;
                }
                // Asked for while this read was on its way, so it may predate
                // what that caller wants to see.
                if tree_answered {
                    let mut tree = requests.tree_read();
                    // Without a handle the app has let go of this connection.
                    let commands = commands.upgrade();
                    if !std::mem::take(&mut tree.again) || commands.is_none() {
                        tree.in_flight = false;
                        continue;
                    }
                    // The read stays claimed for the one sent now.
                    if let Some(commands) = commands
                        && let Err(error) = send_command(&commands, &requests, tree_command())
                    {
                        // A full queue means the host has stopped reading.
                        // Reconnecting reads the tree afresh; dropping the
                        // read here would leave it stale with nothing to
                        // notice.
                        tree.in_flight = false;
                        drop(tree);
                        close_connection(&updates, &requests, error);
                        return;
                    }
                }
            }
        })
        .expect("spawn xd host reader");
}

fn capture_process_errors(mut stream: impl Read + Send + 'static) -> Arc<Mutex<Vec<u8>>> {
    let errors = Arc::new(Mutex::new(Vec::new()));
    let destination = errors.clone();
    let _ = thread::Builder::new()
        .name("xd-host-errors".into())
        .spawn(move || {
            let mut buffer = [0_u8; 1024];
            loop {
                let count = match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                if let Ok(mut destination) = destination.lock() {
                    destination.extend_from_slice(&buffer[..count]);
                    if destination.len() > PROCESS_STDERR_LIMIT {
                        let excess = destination.len() - PROCESS_STDERR_LIMIT;
                        destination.drain(..excess);
                    }
                }
            }
        });
    errors
}

fn with_process_errors(message: &str, errors: Option<&Arc<Mutex<Vec<u8>>>>) -> String {
    let Some(error) = errors
        .and_then(|errors| errors.lock().ok())
        .filter(|errors| !errors.is_empty())
        .map(|errors| console_text(&errors))
        .filter(|error| !error.is_empty())
    else {
        return message.to_owned();
    };
    format!("{message}: {error}")
}

/// Text a program wrote for a person to read, in whichever encoding it used.
///
/// `wsl.exe` writes its own messages as UTF-16, where every other byte of
/// plain text is zero. Read as UTF-8 that is one unreadable character per
/// letter, which is how "no distribution is installed" used to be shown.
fn console_text(bytes: &[u8]) -> String {
    let wide = bytes.len() >= 2
        && bytes
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|byte| **byte == 0)
            .count()
            * 2
            >= bytes.len() / 2;
    let text = if wide {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.chars()
        .filter(|character| !matches!(character, '\0' | '\u{feff}'))
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The first thing a host process printed, when it is prose and not protocol:
/// the program asked to start the host saying why it did not.
fn launcher_output(line: &[u8]) -> Option<String> {
    let text = console_text(line);
    (!text.is_empty() && !text.starts_with('{')).then_some(text)
}

fn launcher_failure(output: &str) -> String {
    if cfg!(windows) {
        format!(
            "WSL could not start xd's Linux host: {output} Install a Linux distribution with \"wsl --install\", finish its first-time setup, and install git and tmux inside it."
        )
    } else {
        format!("The xd host could not start: {output}")
    }
}

fn take_draft_attachments(body: &mut Map<String, Value>) -> Option<Vec<Attachment>> {
    let mut taken = Map::new();
    for key in ["draft_attachments", "draft_files"] {
        if let Some(value) = body.remove(key) {
            taken.insert(key.to_owned(), value);
        }
    }
    crate::model::draft_attachments(&Value::Object(taken))
}

fn close_connection(updates: &Sender<HostUpdate>, requests: &ConnectionRequests, message: String) {
    let _publishing = requests.publishing();
    if requests.closed.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Ok(mut pending) = requests.pending.lock() {
        pending.clear();
    }
    let _ = updates.send_blocking(HostUpdate::Disconnected { message });
}

fn spawn_chat_load_watchdog(
    updates: Sender<HostUpdate>,
    requests: Arc<ConnectionRequests>,
    timeout: Duration,
) {
    thread::Builder::new()
        .name("xd-host-load-watchdog".into())
        .spawn(move || {
            let interval = timeout.min(Duration::from_millis(250));
            while !updates.is_closed() && !requests.closed.load(Ordering::Acquire) {
                thread::sleep(interval);
                let _publishing = requests.publishing();
                if requests.expire_chat_load(Instant::now()) {
                    let _ = updates.send_blocking(HostUpdate::Disconnected {
                        message: "xd host took too long to load the chat; reconnecting".into(),
                    });
                    return;
                }
            }
        })
        .expect("spawn xd host load watchdog");
}

#[cfg(not(windows))]
fn data_root() -> PathBuf {
    let data_home = env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(|home| PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from(".local/share"));

    data_home.join(channel::data_name())
}

#[cfg(not(windows))]
fn launcher_candidates() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("XD_HOST_EXECUTABLE").filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(path)];
    }
    let mut candidates = Vec::new();
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        let sibling = parent.join("xd-host");
        if sibling.is_file() {
            candidates.push(sibling);
        }
    }
    candidates.push(PathBuf::from("xd-host"));
    candidates
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn wsl_messages_are_read_in_the_encoding_wsl_wrote_them() {
        let message = "Windows Subsystem for Linux has no installed distributions.\r\n";
        let wide = message
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(
            super::console_text(&wide),
            "Windows Subsystem for Linux has no installed distributions."
        );
        // A line is cut at the newline's first byte, leaving half a character.
        assert_eq!(
            super::console_text(&wide[..wide.len() - 1]),
            "Windows Subsystem for Linux has no installed distributions."
        );
        assert_eq!(super::console_text(message.as_bytes()), message.trim());
        assert_eq!(
            super::console_text("xd: Install git ünd tmux\n".as_bytes()),
            "xd: Install git ünd tmux"
        );
        assert_eq!(super::console_text(b""), "");
    }

    #[test]
    fn prose_before_the_first_frame_is_the_launcher_explaining_itself() {
        assert_eq!(
            super::launcher_output(b"The system cannot find the file specified.\r\n").as_deref(),
            Some("The system cannot find the file specified.")
        );
        // A frame that fails to parse is a protocol error, not a message.
        assert_eq!(super::launcher_output(b"{\"ok\":tru\n"), None);
        assert_eq!(super::launcher_output(b"\r\n"), None);
        let failure = super::launcher_failure("No distribution.");
        assert!(failure.contains("No distribution."));
    }

    use super::*;
    use crate::local_socket::UnixListener;
    use std::{
        fs, io,
        sync::{Arc, Condvar, Mutex},
    };

    fn next_update(updates: &Receiver<HostUpdate>) -> HostUpdate {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match updates.try_recv() {
                Ok(update) => return update,
                Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                error => panic!("host update did not arrive: {error:?}"),
            }
        }
    }

    #[test]
    fn tree_reads_asked_for_while_one_is_on_its_way_become_one_more_read() {
        let (reader, peer) = crate::local_socket::UnixStream::pair().unwrap();
        let (host, updates) = HostHandle::connect_io(
            reader.try_clone().unwrap(),
            reader,
            PathBuf::from("busy-tree-host"),
        );
        assert!(matches!(
            next_update(&updates),
            HostUpdate::Connected { .. }
        ));
        peer.set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let mut sent = BufReader::new(peer.try_clone().unwrap());
        let mut peer = peer;
        let mut next_request = || {
            let mut line = String::new();
            match sent.read_line(&mut line) {
                Ok(0) => None,
                Ok(_) => Some(serde_json::from_str::<Value>(&line).unwrap()),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    None
                }
                Err(error) => panic!("{error}"),
            }
        };

        host.tree().unwrap();
        let first = next_request().expect("the first read is sent");
        assert_eq!(first["op"], "tree");
        // Every tree event while that read is out asks again.
        for _ in 0..5 {
            host.tree().unwrap();
        }
        host.chat("c1").unwrap();
        let chat = next_request().expect("other requests still go out");
        assert_eq!(chat["op"], "chat");
        assert!(next_request().is_none(), "no second tree read queued");

        let first_id = first["_xd_request"].as_u64().unwrap();
        writeln!(
            peer,
            "{{\"ok\":true,\"folders\":[],\"chats\":[],\"_xd_request\":{first_id}}}"
        )
        .unwrap();
        assert!(matches!(
            next_update(&updates),
            HostUpdate::Reply {
                kind: RequestKind::Tree,
                ..
            }
        ));
        let again = next_request().expect("one more read after the answer");
        assert_eq!(again["op"], "tree");
        assert!(next_request().is_none(), "only one more");

        let again_id = again["_xd_request"].as_u64().unwrap();
        writeln!(
            peer,
            "{{\"ok\":true,\"folders\":[],\"chats\":[],\"_xd_request\":{again_id}}}"
        )
        .unwrap();
        assert!(matches!(
            next_update(&updates),
            HostUpdate::Reply {
                kind: RequestKind::Tree,
                ..
            }
        ));
        assert!(next_request().is_none(), "nothing was asked meanwhile");
        host.tree().unwrap();
        assert_eq!(
            next_request().expect("a later read goes out at once")["op"],
            "tree"
        );

        // The reader's way to send a read again does not keep the connection
        // up once the app lets go of it.
        let requests = host.requests.clone();
        drop(host);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !requests.closed.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "the writer outlived its handles");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_missing_chat_reply_reconnects_even_when_events_keep_arriving() {
        let (reader, mut peer) = crate::local_socket::UnixStream::pair().unwrap();
        let (host, updates) = HostHandle::connect_io_with_timeout(
            reader.try_clone().unwrap(),
            reader,
            PathBuf::from("stalled-host"),
            None,
            Duration::from_secs(60),
        );
        assert!(matches!(
            next_update(&updates),
            HostUpdate::Connected { .. }
        ));
        host.chat("chat-1").unwrap();
        let mut line = String::new();
        BufReader::new(peer.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["op"], "chat");
        let sent = host
            .requests
            .pending
            .lock()
            .unwrap()
            .values()
            .map(|request| (request.started, request.deadline))
            .collect::<Vec<_>>();
        assert_eq!(sent.len(), 1);
        for _ in 0..3 {
            writeln!(peer, "{{\"event\":\"working\",\"chat\":\"other-chat\"}}").unwrap();
            assert!(matches!(next_update(&updates), HostUpdate::Event { .. }));
        }
        let after_events = host
            .requests
            .pending
            .lock()
            .unwrap()
            .values()
            .map(|request| (request.started, request.deadline))
            .collect::<Vec<_>>();
        assert_eq!(
            after_events, sent,
            "events leave the reply's deadline alone"
        );
        // Let the deadline pass without waiting a minute.
        for request in host.requests.pending.lock().unwrap().values_mut() {
            request.deadline = Some(Duration::ZERO);
        }
        writeln!(peer, "{{\"event\":\"working\",\"chat\":\"other-chat\"}}").unwrap();
        let mut disconnected = false;
        for _ in 0..2 {
            match next_update(&updates) {
                HostUpdate::Event { .. } => {}
                HostUpdate::Disconnected { .. } => {
                    disconnected = true;
                    break;
                }
                _ => panic!("unexpected host update"),
            }
        }
        assert!(disconnected, "events still arriving do not keep it alive");
        assert_eq!(
            host.tree().unwrap_err(),
            "the xd state connection is closed"
        );
        // Let every connection thread finish, then check what they left.
        drop(host);
        drop(peer);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !updates.is_closed() {
            assert!(Instant::now() < deadline, "connection threads kept running");
            thread::sleep(Duration::from_millis(5));
        }
        while let Ok(update) = updates.try_recv() {
            assert!(
                matches!(update, HostUpdate::Event { .. }),
                "a connection disconnects only once and drops late replies"
            );
        }
    }

    #[test]
    fn a_completed_snapshot_reply_cancels_its_deadline() {
        let (reader, mut peer) = crate::local_socket::UnixStream::pair().unwrap();
        let (host, updates) = HostHandle::connect_io_with_timeout(
            reader.try_clone().unwrap(),
            reader,
            PathBuf::from("healthy-host"),
            None,
            Duration::from_secs(5),
        );
        assert!(matches!(
            next_update(&updates),
            HostUpdate::Connected { .. }
        ));
        // An earlier connection to this host let the tree time out.
        host.requests.deadlines.timed_out(Snapshot::Tree);
        host.tree().unwrap();
        let mut line = String::new();
        BufReader::new(peer.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let id = serde_json::from_str::<Value>(&line).unwrap()["_xd_request"]
            .as_u64()
            .unwrap();
        assert_eq!(
            host.requests.pending.lock().unwrap()[&id].deadline,
            Some(Duration::from_secs(10)),
            "the tree was sent with the longer deadline"
        );
        writeln!(peer, "{{\"ok\":true,\"_xd_request\":{id}}}").unwrap();
        assert!(matches!(next_update(&updates), HostUpdate::Reply { .. }));
        assert!(
            !host
                .requests
                .expire_chat_load(Instant::now() + Duration::from_secs(60))
        );
        assert!(!host.requests.closed.load(Ordering::Acquire));
        assert_eq!(
            host.requests
                .deadlines
                .deadline(&Snapshot::Tree, Duration::from_secs(5)),
            Duration::from_secs(5),
            "the answer brings the deadline back down"
        );
    }

    #[test]
    fn a_slow_snapshot_gets_longer_deadlines_until_it_is_answered() {
        let host = SnapshotDeadlines::for_host(Path::new("slow-host"));
        assert!(Arc::ptr_eq(
            &host,
            &SnapshotDeadlines::for_host(Path::new("slow-host"))
        ));
        let page = |cursor| RequestKind::Messages {
            chat_id: "big".into(),
            cursor,
        };
        let older = || page(MessageCursor::Before(500));
        // One connection per attempt, as reconnecting makes, each asking for
        // the same older page, which takes longer than the first deadline.
        let attempt = |kind: RequestKind, waited: Duration| {
            let requests = ConnectionRequests::new(host.clone(), CHAT_LOAD_TIMEOUT);
            let started = Instant::now();
            let request = requests.pending_request(kind, started);
            requests.pending.lock().unwrap().insert(1, request);
            requests.expire_chat_load(started + waited)
        };
        assert!(attempt(older(), CHAT_LOAD_TIMEOUT));
        assert!(!attempt(older(), CHAT_LOAD_TIMEOUT));
        assert!(attempt(older(), CHAT_LOAD_TIMEOUT * 2));
        assert!(!attempt(older(), CHAT_LOAD_TIMEOUT * 3));
        assert!(attempt(older(), CHAT_LOAD_TIMEOUT * 4));
        // Quick answers to the tree and to the chat's latest page, as each
        // reconnect brings, leave the slow page's longer deadline alone.
        host.answered(&Snapshot::Tree, Duration::ZERO, CHAT_LOAD_TIMEOUT);
        host.answered(
            &Snapshot::Messages("big".into(), MessageCursor::Tail),
            Duration::ZERO,
            CHAT_LOAD_TIMEOUT,
        );
        let slow = Snapshot::Messages("big".into(), MessageCursor::Before(500));
        assert_eq!(
            host.deadline(&slow, CHAT_LOAD_TIMEOUT),
            CHAT_LOAD_TIMEOUT * 8
        );
        assert_eq!(
            host.deadline(
                &Snapshot::Messages("big".into(), MessageCursor::Tail),
                CHAT_LOAD_TIMEOUT
            ),
            CHAT_LOAD_TIMEOUT
        );
        // A request keeps the deadline it was sent with, even if an answer
        // to the same page arrives on another connection meanwhile.
        let requests = ConnectionRequests::new(host.clone(), CHAT_LOAD_TIMEOUT);
        let started = Instant::now();
        let request = requests.pending_request(older(), started);
        requests.pending.lock().unwrap().insert(1, request);
        host.answered(&slow, Duration::ZERO, CHAT_LOAD_TIMEOUT);
        assert!(!requests.expire_chat_load(started + CHAT_LOAD_TIMEOUT * 7));
        assert!(requests.expire_chat_load(started + CHAT_LOAD_TIMEOUT * 8));
        for _ in 0..10 {
            host.timed_out(slow.clone());
        }
        assert_eq!(
            host.deadline(&slow, CHAT_LOAD_TIMEOUT),
            Duration::from_secs(8 * 60),
            "the deadline stops growing at eight minutes"
        );
        host.answered(&slow, Duration::ZERO, CHAT_LOAD_TIMEOUT);
        assert!(attempt(older(), CHAT_LOAD_TIMEOUT));
        assert!(
            !Arc::ptr_eq(&host, &SnapshotDeadlines::for_host(Path::new("other-host"))),
            "each host keeps its own deadlines"
        );
    }

    #[test]
    fn a_snapshot_queued_behind_a_slow_reply_is_not_late() {
        let deadlines = Arc::new(SnapshotDeadlines::default());
        let page = Snapshot::Messages("big".into(), MessageCursor::Before(500));
        deadlines.timed_out(page.clone());
        let requests = ConnectionRequests::new(deadlines.clone(), CHAT_LOAD_TIMEOUT);
        let sent = Instant::now();
        let after = |seconds| sent + Duration::from_secs(seconds);
        let older_page = requests.pending_request(
            RequestKind::Messages {
                chat_id: "big".into(),
                cursor: MessageCursor::Before(500),
            },
            sent,
        );
        let tree = requests.pending_request(RequestKind::Tree, sent);
        requests
            .pending
            .lock()
            .unwrap()
            .extend([(1, older_page), (2, tree)]);
        // The tree's own 30 seconds pass while the page, with 60, is still
        // being sent ahead of it.
        assert!(!requests.expire_chat_load(after(40)));
        assert!(requests.take_reply(1, after(45)).is_some());
        // The tree's clock starts once the page is in.
        assert!(!requests.expire_chat_load(after(70)));
        assert!(requests.expire_chat_load(after(75)));
        assert_eq!(
            deadlines.deadline(&Snapshot::Tree, CHAT_LOAD_TIMEOUT),
            CHAT_LOAD_TIMEOUT * 2
        );
    }

    #[test]
    fn a_snapshot_that_keeps_needing_longer_keeps_its_longer_deadline() {
        let deadlines = Arc::new(SnapshotDeadlines::default());
        let page = || RequestKind::Messages {
            chat_id: "big".into(),
            cursor: MessageCursor::Before(500),
        };
        let snapshot = Snapshot::Messages("big".into(), MessageCursor::Before(500));
        // Each load of the page takes 45 seconds and is answered.
        let load = |id| {
            let requests = ConnectionRequests::new(deadlines.clone(), CHAT_LOAD_TIMEOUT);
            let sent = Instant::now();
            let request = requests.pending_request(page(), sent);
            let deadline = request.deadline;
            requests.pending.lock().unwrap().insert(id, request);
            let late = requests.expire_chat_load(sent + Duration::from_secs(45));
            if !late {
                requests
                    .take_reply(id, sent + Duration::from_secs(45))
                    .unwrap();
            }
            (deadline, late)
        };
        assert_eq!(load(1), (Some(CHAT_LOAD_TIMEOUT), true));
        assert_eq!(load(2), (Some(CHAT_LOAD_TIMEOUT * 2), false));
        assert_eq!(
            load(3),
            (Some(CHAT_LOAD_TIMEOUT * 2), false),
            "no new timeout"
        );
        // A slower answer after the deadline grew further lowers it only to
        // what that answer needed; a quick one returns it to 30 seconds.
        deadlines.timed_out(snapshot.clone());
        deadlines.timed_out(snapshot.clone());
        deadlines.answered(&snapshot, Duration::from_secs(70), CHAT_LOAD_TIMEOUT);
        assert_eq!(
            deadlines.deadline(&snapshot, CHAT_LOAD_TIMEOUT),
            CHAT_LOAD_TIMEOUT * 4
        );
        deadlines.answered(&snapshot, Duration::from_secs(10), CHAT_LOAD_TIMEOUT);
        assert_eq!(
            deadlines.deadline(&snapshot, CHAT_LOAD_TIMEOUT),
            CHAT_LOAD_TIMEOUT
        );
    }

    #[test]
    fn nothing_is_published_after_the_disconnect() {
        let (updates_tx, updates) = async_channel::bounded(8);
        let requests = ConnectionRequests::new(Arc::default(), CHAT_LOAD_TIMEOUT);
        let sent = Instant::now();
        let older = requests.pending_request(RequestKind::Tree, sent);
        let later = requests.pending_request(
            RequestKind::Chat {
                chat_id: "one".into(),
            },
            sent,
        );
        requests
            .pending
            .lock()
            .unwrap()
            .extend([(1, older), (2, later)]);
        // The reader takes the later reply, then the older request expires
        // before the reader publishes it.
        let kind = requests.take_reply(2, sent).unwrap();
        {
            let _publishing = requests.publishing();
            assert!(requests.expire_chat_load(sent + CHAT_LOAD_TIMEOUT));
            updates_tx
                .send_blocking(HostUpdate::Disconnected {
                    message: "late".into(),
                })
                .unwrap();
        }
        assert!(!requests.publish(
            &updates_tx,
            HostUpdate::Reply {
                kind,
                body: Map::new(),
                attachments: None,
            }
        ));
        close_connection(&updates_tx, &requests, "closed again".into());
        assert!(matches!(
            updates.try_recv(),
            Ok(HostUpdate::Disconnected { .. })
        ));
        assert!(updates.try_recv().is_err(), "the late reply was dropped");
    }

    #[test]
    fn a_lost_reply_expires_even_when_later_requests_are_answered() {
        let requests = ConnectionRequests::new(Arc::default(), CHAT_LOAD_TIMEOUT);
        let sent = Instant::now();
        let chat = requests.pending_request(
            RequestKind::Chat {
                chat_id: "lost".into(),
            },
            sent,
        );
        let tree = requests.pending_request(RequestKind::Tree, sent);
        requests
            .pending
            .lock()
            .unwrap()
            .extend([(1, chat), (2, tree)]);
        assert!(
            requests
                .take_reply(2, sent + Duration::from_secs(20))
                .is_some()
        );
        assert!(requests.expire_chat_load(sent + CHAT_LOAD_TIMEOUT));
    }

    #[test]
    fn a_request_without_a_deadline_holds_up_snapshots_only_so_long() {
        let requests = ConnectionRequests::new(Arc::default(), CHAT_LOAD_TIMEOUT);
        let sent = Instant::now();
        let push = requests.pending_request(
            RequestKind::GitPush {
                chat_id: "one".into(),
                generation: 0,
            },
            sent,
        );
        let tree = requests.pending_request(RequestKind::Tree, sent);
        requests
            .pending
            .lock()
            .unwrap()
            .extend([(1, push), (2, tree)]);
        assert!(!requests.expire_chat_load(sent + Duration::from_secs(5 * 60)));
        assert!(requests.expire_chat_load(sent + QUEUED_BEHIND_TIMEOUT));
        assert_eq!(QUEUED_BEHIND_TIMEOUT, Duration::from_secs(8 * 60));
    }

    #[test]
    fn only_snapshot_requests_have_a_deadline() {
        for kind in [
            RequestKind::Tree,
            RequestKind::Chat {
                chat_id: "one".into(),
            },
            RequestKind::Messages {
                chat_id: "one".into(),
                cursor: MessageCursor::Tail,
            },
        ] {
            let requests = ConnectionRequests::new(Arc::default(), CHAT_LOAD_TIMEOUT);
            let started = Instant::now();
            let request = requests.pending_request(kind, started);
            assert_eq!(request.deadline, Some(CHAT_LOAD_TIMEOUT));
            requests.pending.lock().unwrap().insert(1, request);
            assert!(requests.expire_chat_load(started + CHAT_LOAD_TIMEOUT));
            assert!(requests.pending.lock().unwrap().is_empty());
        }
        let requests = ConnectionRequests::new(Arc::default(), CHAT_LOAD_TIMEOUT);
        let started = Instant::now();
        let request = requests.pending_request(
            RequestKind::GitPush {
                chat_id: "one".into(),
                generation: 0,
            },
            started,
        );
        assert_eq!(request.deadline, None);
        requests.pending.lock().unwrap().insert(1, request);
        assert!(!requests.expire_chat_load(started + Duration::from_secs(600)));
        assert_eq!(requests.pending.lock().unwrap().len(), 1);
    }

    #[derive(Default)]
    struct WriteGate {
        entered: bool,
        released: bool,
    }

    struct BlockingWriter {
        gate: Arc<(Mutex<WriteGate>, Condvar)>,
    }

    impl Write for BlockingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let (state, wake) = &*self.gate;
            let mut state = state.lock().unwrap();
            state.entered = true;
            wake.notify_all();
            while !state.released {
                state = wake.wait(state).unwrap();
            }
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn black_holed_host_writer_eventually_rejects_more_commands() {
        let gate = Arc::new((Mutex::new(WriteGate::default()), Condvar::new()));
        let (reader, _peer) = crate::local_socket::UnixStream::pair().unwrap();
        let (host, _updates) = HostHandle::connect_io(
            reader,
            BlockingWriter { gate: gate.clone() },
            PathBuf::from("blocked-host"),
        );
        // Not tree reads: only one of those is ever out at a time.
        host.agent_catalog().unwrap();
        let (gate_state, wake) = &*gate;
        let mut state = gate_state.lock().unwrap();
        while !state.entered {
            state = wake.wait(state).unwrap();
        }
        drop(state);

        let failure = (0..2_048).find_map(|_| host.agent_catalog().err());
        let mut state = gate_state.lock().unwrap();
        state.released = true;
        wake.notify_all();
        drop(state);

        assert_eq!(
            failure.as_deref(),
            Some("the xd state command queue is full")
        );
    }

    #[test]
    fn shell_terminals_send_the_selected_palette_on_the_wire() {
        let directory =
            env::temp_dir().join(format!("xd-shell-terminal-palette-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.terminal_open("chat-1", 120, 32, false, 0xf1f1f1, 0x202020)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::TerminalOpen { chat_id, reuse, agent },
                ..
            } if chat_id == "chat-1" && !reuse && agent.is_none()
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn a_send_names_only_what_it_takes_from_the_draft() {
        let directory = env::temp_dir().join(format!("xd-send-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut seen = Vec::new();
            for _ in 0..3 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let request_id = request["_xd_request"].as_u64().unwrap();
                writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
                seen.push(request);
            }
            seen
        });

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        let report = Attachment::from_file("Report.pdf", b"%PDF".to_vec());
        host.send_message("chat-1", "read it", &[report], None, None, false)
            .unwrap();
        host.send_message("chat-1", "a shortcut", &[], None, None, false)
            .unwrap();
        // The clear after a send claims the whole list, files included.
        host.set_draft("chat-1", "", Some(&[]), Some(7)).unwrap();

        let seen = server.join().unwrap();
        assert_eq!(seen[0]["attachments"][0]["mime"], "application/pdf");
        assert!(seen[1].get("attachments").is_none());
        assert_eq!(seen[2]["op"], "set-draft");
        assert_eq!(seen[2]["files"], true);
        assert_eq!(seen[2]["attachments"], serde_json::json!([]));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn terminal_images_are_materialized_without_targeting_the_retired_host_terminal() {
        let directory = env::temp_dir().join(format!("xd-terminal-image-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["op"], "terminal-materialize-image");
            assert_eq!(request["terminal"], "terminal-1");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(
                stream,
                "{{\"ok\":true,\"path\":\"/remote/paste.png\",\"_xd_request\":{request_id}}}"
            )
            .unwrap();
        });

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.terminal_paste_image(
            "terminal-1",
            &Attachment::from_png("paste.png", b"\x89PNG\r\n\x1a\n".to_vec()).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::TerminalPasteImage { terminal_id },
                ..
            } if terminal_id == "terminal-1"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn correlates_replies_and_continues_delivering_events() {
        let directory = env::temp_dir().join(format!("xd-host-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.tree().unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::Tree,
                ..
            }
        ));
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Event { name, .. } if name == "tree"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn reports_a_connection_closed_mid_frame_without_a_json_parser_error() {
        let directory = env::temp_dir().join(format!("xd-partial-frame-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(br#"{"event":"text","text":"unfinished"#)
                .unwrap();
        });

        let (_host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Disconnected { message }
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
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.messages("chat-1", MessageCursor::Before(42)).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::Messages {
                    cursor: MessageCursor::Before(42),
                    ..
                },
                ..
            }
        ));
        host.messages("chat-1", MessageCursor::After(84)).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
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
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.pair_remote("ABCD2345", "Laptop").unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::PairRemote,
                ..
            }
        ));
        host.hello_remote("private").unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
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
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.edit_queue("chat-1", 2, "before", "after").unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
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
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.queue_message("chat-2", "shared context").unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::QueueMutation { chat_id },
                ..
            } if chat_id == "chat-2"
        ));
        host.reorder_queue("chat-2", 2, "third", 0, "first", false)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
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
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        for after in [false, true] {
            host.reorder_chat("chat-1", "chat-2", after).unwrap();
            assert!(matches!(
                updates.recv_blocking().unwrap(),
                HostUpdate::Reply {
                    kind: RequestKind::MoveChat { chat_id, .. },
                    ..
                } if chat_id == "chat-1"
            ));
        }

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn reorders_folders_before_and_after_an_anchor() {
        let directory = env::temp_dir().join(format!("xd-reorder-folder-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            for (field, after, keep_parent) in [
                ("before", false, true),
                ("after", true, true),
                ("after", true, false),
                ("end", true, false),
            ] {
                let mut request = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                let request: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(request["op"], "move-folder");
                assert_eq!(request["folder"], "folder-1");
                assert_eq!(request.get("parent"), None);
                if field == "end" {
                    // Last at the top level, with no anchor to go stale.
                    assert_eq!(request["end"], true);
                    assert_eq!(request.get("before"), None);
                    assert_eq!(request.get("after"), None);
                    assert_eq!(request.get("expect_parent"), None);
                    let request_id = request["_xd_request"].as_u64().unwrap();
                    writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
                    continue;
                }
                assert_eq!(request[field], "folder-2");
                assert_eq!(request.get(if after { "before" } else { "after" }), None);
                if keep_parent {
                    assert_eq!(request["keep_parent"], true);
                    assert_eq!(request.get("expect_parent"), None);
                } else {
                    assert_eq!(request.get("keep_parent"), None, "a move may change parent");
                    assert_eq!(request["expect_parent"], "folder-3");
                }
                let request_id = request["_xd_request"].as_u64().unwrap();
                writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
            }
        });

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        for after in [false, true] {
            host.reorder_folder("folder-1", "folder-2", after).unwrap();
            assert!(matches!(
                updates.recv_blocking().unwrap(),
                HostUpdate::Reply {
                    kind: RequestKind::MoveFolder { folder_id, parent_id: None },
                    ..
                } if folder_id == "folder-1"
            ));
        }
        host.move_folder_beside("folder-1", "folder-2", true, Some("folder-3"))
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::MoveFolder { folder_id, parent_id: None },
                ..
            } if folder_id == "folder-1"
        ));
        host.move_folder_to_end("folder-1", None).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::MoveFolder { folder_id, parent_id: None },
                ..
            } if folder_id == "folder-1"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn sends_the_configured_git_writer_for_worktree_naming() {
        let directory = env::temp_dir().join(format!("xd-worktree-name-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.send_message(
            "chat-1",
            "fix the queue",
            &[],
            Some("claude"),
            Some("claude-sonnet-5"),
            false,
        )
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::Send { chat_id, text },
                ..
            } if chat_id == "chat-1" && text == "fix the queue"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn a_new_chat_sends_its_first_message_to_name_its_worktree() {
        let directory = env::temp_dir().join(format!("xd-new-chat-name-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            for expect_prompt in [true, false] {
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let request: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(request["op"], "new-chat");
                assert_eq!(request["worktree"]["kind"], "new");
                if expect_prompt {
                    assert_eq!(request["prompt"], "fix the footer");
                    assert_eq!(request["generate_worktree_name"], true);
                    assert_eq!(request["worktree_backend"], "claude");
                    assert_eq!(request["worktree_model"], "claude-sonnet-5");
                } else {
                    // Only attachments: nothing to name it from.
                    assert!(request.get("prompt").is_none());
                    assert!(request.get("generate_worktree_name").is_none());
                }
                let request_id = request["_xd_request"].as_u64().unwrap();
                writeln!(
                    stream,
                    "{{\"ok\":true,\"id\":\"chat-1\",\"_xd_request\":{request_id}}}"
                )
                .unwrap();
            }
        });

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        for text in ["fix the footer", " "] {
            host.create_draft_chat(
                "folder-1",
                None,
                "New Chat",
                "codex",
                None,
                Some(&NewSessionWorktree::New),
                false,
                Some(FirstMessage {
                    attachments: &[],
                    effort: None,
                    access: None,
                    plan: None,
                    account: None,
                    text,
                    worktree_backend: Some("claude"),
                    worktree_model: Some("claude-sonnet-5"),
                }),
                0,
            )
            .unwrap();
            assert!(matches!(
                updates.recv_blocking().unwrap(),
                HostUpdate::Reply {
                    kind: RequestKind::NewChat { .. },
                    ..
                }
            ));
        }

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn resuming_a_turn_answers_like_a_send() {
        let directory = env::temp_dir().join(format!("xd-resume-turn-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["op"], "resume-turn");
            assert_eq!(request["chat"], "chat-1");
            let request_id = request["_xd_request"].as_u64().unwrap();
            writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
        });

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.resume_turn("chat-1").unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::Send { chat_id, text },
                ..
            } if chat_id == "chat-1" && text == RESUME_TEXT
        ));
        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn sends_conflict_guarded_workspace_file_writes() {
        let directory = env::temp_dir().join(format!("xd-file-write-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.file_browse_write("chat-1", "src/main.rs", "before\n", "after\n", 12)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
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
    fn requests_host_side_directories_for_remote_safe_browsing() {
        let directory = env::temp_dir().join(format!("xd-list-directory-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.list_directory(Some("/srv/workspaces"), 14).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
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
    fn session_tab_and_lifecycle_requests_keep_session_and_chat_ids_distinct() {
        let directory = env::temp_dir().join(format!("xd-session-contract-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            for expected in [
                json!({"op":"new-chat","folder":"project","session":"workspace-session","title":"Claude tab","backend":"claude"}),
                json!({"op":"rename-session","session":"workspace-session","title":"Shared task"}),
                json!({"op":"move-session","session":"workspace-session","folder":"other-project"}),
                json!({"op":"move-session","session":"workspace-session","before":"anchor-session"}),
                json!({"op":"move-session","session":"workspace-session","after":"anchor-session"}),
                json!({"op":"delete-session","session":"workspace-session"}),
                json!({"op":"settle-session","session":"workspace-session"}),
                json!({"op":"unsettle-session","session":"workspace-session"}),
                json!({"op":"set-session-auto-settle","session":"workspace-session","enabled":false}),
                json!({"op":"auto-settle-settings"}),
                json!({"op":"set-auto-settle-settings","after_days":null,"on_merge":false,"keep_pull_requests":true}),
                json!({"op":"auto-delete-settings"}),
                json!({"op":"set-auto-delete-settings","after_days":30}),
                json!({"op":"chat-quota-settings"}),
                json!({"op":"set-chat-quota-settings","percent":10}),
                json!({"op":"set-chat-quota-settings","percent":null}),
                json!({"op":"set-chat-quota","chat":"chat-1","percent":20}),
                json!({"op":"set-chat-quota","chat":"chat-1","percent":null}),
            ] {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut request: Value = serde_json::from_str(&line).unwrap();
                let request_id = request
                    .as_object_mut()
                    .unwrap()
                    .remove("_xd_request")
                    .unwrap();
                assert_eq!(request, expected);
                let reply = json!({"ok":true,"id":"new-chat-id","session":"workspace-session",
                    "terminal_chat":"session:workspace-session","_xd_request":request_id});
                writeln!(stream, "{reply}").unwrap();
            }
        });
        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.new_chat_in_session("project", "workspace-session", "Claude tab", "claude")
            .unwrap();
        assert!(
            matches!(updates.recv_blocking().unwrap(), HostUpdate::Reply {
            kind:RequestKind::NewChat { folder_id, title, workdir:None, token: 0 }, body, ..
        } if folder_id == "project" && title == "Claude tab" && body["id"] == "new-chat-id" && body["session"] == "workspace-session")
        );
        host.rename_session("workspace-session", "Shared task")
            .unwrap();
        assert!(
            matches!(updates.recv_blocking().unwrap(), HostUpdate::Reply {
            kind:RequestKind::RenameSession {session_id,title}, ..
        } if session_id == "workspace-session" && title == "Shared task")
        );
        host.move_session("workspace-session", "other-project")
            .unwrap();
        assert!(
            matches!(updates.recv_blocking().unwrap(), HostUpdate::Reply {
            kind:RequestKind::MoveSession {session_id,folder_id:Some(folder_id)}, ..
        } if session_id == "workspace-session" && folder_id == "other-project")
        );
        for after in [false, true] {
            host.reorder_session("workspace-session", "anchor-session", after)
                .unwrap();
            assert!(
                matches!(updates.recv_blocking().unwrap(), HostUpdate::Reply {
                kind:RequestKind::MoveSession {session_id,folder_id:None}, ..
            } if session_id == "workspace-session")
            );
        }
        host.delete_session("workspace-session").unwrap();
        assert!(
            matches!(updates.recv_blocking().unwrap(), HostUpdate::Reply {
            kind:RequestKind::DeleteSession {session_id}, ..
        } if session_id == "workspace-session")
        );
        host.settle_session("workspace-session").unwrap();
        assert!(
            matches!(updates.recv_blocking().unwrap(), HostUpdate::Reply {
            kind:RequestKind::SettleSession {session_id}, ..
        } if session_id == "workspace-session")
        );
        host.unsettle_session("workspace-session").unwrap();
        assert!(
            matches!(updates.recv_blocking().unwrap(), HostUpdate::Reply {
            kind:RequestKind::UnsettleSession {session_id}, ..
        } if session_id == "workspace-session")
        );
        host.set_session_auto_settle("workspace-session", false)
            .unwrap();
        assert!(
            matches!(updates.recv_blocking().unwrap(), HostUpdate::Reply {
            kind:RequestKind::SetSessionAutoSettle {session_id}, ..
        } if session_id == "workspace-session")
        );
        host.auto_settle_settings().unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::AutoSettleSettings,
                ..
            }
        ));
        host.set_auto_settle_settings(AutoSettleRules {
            after_days: None,
            on_merge: false,
            keep_pull_requests: true,
        })
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::SetAutoSettleSettings,
                ..
            }
        ));
        host.auto_delete_settings().unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::AutoDeleteSettings,
                ..
            }
        ));
        host.set_auto_delete_settings(AutoDeleteRules {
            after_days: Some(30),
        })
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::SetAutoDeleteSettings,
                ..
            }
        ));
        host.chat_quota_settings(0).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::ChatQuotaSettings { .. },
                ..
            }
        ));
        for percent in [Some(10), None] {
            host.set_chat_quota_settings(ChatQuotaSettings { percent }, 0)
                .unwrap();
            assert!(matches!(
                updates.recv_blocking().unwrap(),
                HostUpdate::Reply {
                    kind: RequestKind::SetChatQuotaSettings { .. },
                    ..
                }
            ));
        }
        for percent in [Some(20), None] {
            host.set_chat_quota("chat-1", percent).unwrap();
            assert!(
                matches!(updates.recv_blocking().unwrap(), HostUpdate::Reply {
                kind: RequestKind::SetChatQuota { chat_id }, ..
            } if chat_id == "chat-1")
            );
        }
        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn pull_request_links_name_the_chat_and_the_addresses() {
        let directory = env::temp_dir().join(format!("xd-pr-links-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            for (expected, reply) in [
                (
                    json!({"op":"link-pull-requests","chat":"chat-1",
                        "urls":["https://github.com/o/r/pull/7/files","nope"]}),
                    json!({"ok":true,"linked":["https://github.com/o/r/pull/7"],"invalid":["nope"]}),
                ),
                (
                    json!({"op":"unlink-pull-request","chat":"chat-1",
                        "url":"https://github.com/o/r/pull/7"}),
                    json!({"ok":true,"changed":true}),
                ),
            ] {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut request: Value = serde_json::from_str(&line).unwrap();
                let request_id = request
                    .as_object_mut()
                    .unwrap()
                    .remove("_xd_request")
                    .unwrap();
                assert_eq!(request, expected);
                let mut reply = reply;
                reply["_xd_request"] = request_id;
                writeln!(stream, "{reply}").unwrap();
            }
        });
        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.link_pull_requests(
            "chat-1",
            &["https://github.com/o/r/pull/7/files".into(), "nope".into()],
            3,
        )
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::LinkPullRequests { chat_id, generation: 3 },
                body,
                ..
            } if chat_id == "chat-1" && body["invalid"] == json!(["nope"])
        ));
        host.unlink_pull_request("chat-1", "https://github.com/o/r/pull/7", 4)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::UnlinkPullRequest { chat_id, url, generation: 4 },
                body,
                ..
            } if chat_id == "chat-1" && url == "https://github.com/o/r/pull/7" && body["changed"] == true
        ));
        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn new_chats_send_selected_workdirs_and_omit_workspace_defaults() {
        let directory = env::temp_dir().join(format!("xd-new-chat-workdir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
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
                (
                    "Branch worktree",
                    None,
                    Some("codex"),
                    Some(json!({
                        "kind": "branch",
                        "ref": "refs/remotes/origin/feature",
                        "oid": "0123456789abcdef0123456789abcdef01234567",
                    })),
                ),
                (
                    "Pull request worktree",
                    None,
                    Some("copilot"),
                    Some(json!({
                        "kind": "pull-request",
                        "number": 42,
                        "oid": "89abcdef0123456789abcdef0123456789abcdef",
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.new_chat(
            "folder-1",
            "Selected directory",
            Some("/srv/workspaces/project"),
        )
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: Some(workdir),
                    ..
                },
                ..
            } if folder_id == "folder-1"
                && title == "Selected directory"
                && workdir == "/srv/workspaces/project"
        ));
        host.new_chat("folder-1", "Workspace default", None)
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: None,
                    ..
                },
                ..
            } if folder_id == "folder-1" && title == "Workspace default"
        ));
        host.new_chat_with_backend("folder-1", "Direct Claude", None, "claude")
            .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: None,
                    ..
                },
                ..
            } if folder_id == "folder-1" && title == "Direct Claude"
        ));
        host.new_chat_with_backend_in_worktree(
            "folder-1",
            "Fresh worktree",
            "codex",
            &NewSessionWorktree::New,
        )
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: None,
                    ..
                },
                ..
            } if folder_id == "folder-1" && title == "Fresh worktree"
        ));
        host.new_chat_with_backend_in_worktree(
            "folder-1",
            "Existing worktree",
            "claude",
            &NewSessionWorktree::Existing("/srv/workspaces/feature".into()),
        )
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::NewChat {
                    folder_id,
                    title,
                    workdir: None,
                    ..
                },
                ..
            } if folder_id == "folder-1" && title == "Existing worktree"
        ));
        host.new_chat_with_backend_in_worktree(
            "folder-1",
            "Branch worktree",
            "codex",
            &NewSessionWorktree::Branch {
                reference: "refs/remotes/origin/feature".into(),
                oid: "0123456789abcdef0123456789abcdef01234567".into(),
            },
        )
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::NewChat { title, .. },
                ..
            } if title == "Branch worktree"
        ));
        host.new_chat_with_backend_in_worktree(
            "folder-1",
            "Pull request worktree",
            "copilot",
            &NewSessionWorktree::PullRequest {
                number: 42,
                oid: "89abcdef0123456789abcdef0123456789abcdef".into(),
            },
        )
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::NewChat { title, .. },
                ..
            } if title == "Pull request worktree"
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn sends_scoped_secret_updates_without_placeholder_values() {
        let directory = env::temp_dir().join(format!("xd-secrets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
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

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.set_agent_secrets(
            Some("folder-1"),
            &[
                ("EXISTING".into(), None),
                ("NEW_TOKEN".into(), Some("private".into())),
            ],
        )
        .unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Reply {
                kind: RequestKind::SetAgentSecrets { folder_id },
                ..
            } if folder_id.as_deref() == Some("folder-1")
        ));

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn asks_the_host_to_update_a_single_assistant() {
        let directory = env::temp_dir().join(format!("xd-cli-updates-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            for (action, provider) in [("status", Value::Null), ("install", json!("claude"))] {
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let request: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(request["op"], "agent-cli-updates");
                assert_eq!(request["action"], action);
                assert_eq!(request["provider"], provider);
                let request_id = request["_xd_request"].as_u64().unwrap();
                writeln!(stream, "{{\"ok\":true,\"_xd_request\":{request_id}}}").unwrap();
            }
        });

        let (host, updates) = HostHandle::connect(socket).unwrap();
        assert!(matches!(
            updates.recv_blocking().unwrap(),
            HostUpdate::Connected { .. }
        ));
        host.agent_cli_updates("status", None).unwrap();
        host.agent_cli_updates("install", Some("claude")).unwrap();
        for _ in 0..2 {
            assert!(matches!(
                updates.recv_blocking().unwrap(),
                HostUpdate::Reply {
                    kind: RequestKind::AgentCliUpdates,
                    ..
                }
            ));
        }

        server.join().unwrap();
        let _ = fs::remove_dir_all(directory);
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
        assert_eq!(
            attachments[0].preview.as_ref().unwrap().bytes,
            b"\x89PNG\r\n\x1a\n"
        );
        assert!(!body.contains_key("draft_attachments"));
    }
}
