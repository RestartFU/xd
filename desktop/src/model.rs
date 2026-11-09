use std::{
    collections::{HashMap, HashSet},
    io::Cursor,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{Image, ImageFormat, RenderImage};
use image::Frame;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    activity::{ActivityCard, SubagentView},
    markdown::{self, Document},
};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// An owner avatar the host fetched, ready to draw. The formats are the two
/// GitHub serves; anything else keeps the folder icon.
pub fn owner_icon(mime: &str, data: &str) -> Option<Arc<Image>> {
    let format = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        _ => return None,
    };
    let bytes = STANDARD.decode(data).ok()?;
    (!bytes.is_empty()).then(|| Arc::new(Image::from_bytes(format, bytes)))
}

/// The most pixels a chat will decode for one image. A small file can still
/// describe an enormous picture, and decoding it would allocate all of it.
const MAX_MESSAGE_IMAGE_PIXELS: u64 = 64_000_000;
/// The largest pixels worth keeping: twice the widest and tallest a transcript
/// draws an image, for dense displays. Layout goes by the original's size.
const MAX_KEPT_WIDTH: u32 = 1560;
const MAX_KEPT_HEIGHT: u32 = 960;
/// What one decoded image can hold in memory at most.
pub const MAX_MESSAGE_IMAGE_BYTES: usize = MAX_KEPT_WIDTH as usize * MAX_KEPT_HEIGHT as usize * 4;

fn worth_decoding(width: u32, height: u32) -> bool {
    let pixels = u64::from(width) * u64::from(height);
    pixels > 0 && pixels <= MAX_MESSAGE_IMAGE_PIXELS
}

/// An image a message shows, ready to draw. The size is the original's, which is
/// what it is laid out by.
#[derive(Clone)]
pub struct MessageImage {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
}

impl MessageImage {
    /// Decodes an `image-read` reply. This is the slow part of showing an image,
    /// so it does not belong on the UI thread.
    pub fn decode(mime: &str, data: &str) -> Option<Self> {
        if !matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            return None;
        }
        let bytes = STANDARD.decode(data).ok()?;
        let reader = || {
            image::ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .ok()
        };
        let (width, height) = reader()?.into_dimensions().ok()?;
        if !worth_decoding(width, height) {
            return None;
        }
        let mut decoded = reader()?.decode().ok()?;
        if width > MAX_KEPT_WIDTH || height > MAX_KEPT_HEIGHT {
            decoded = decoded.thumbnail(MAX_KEPT_WIDTH, MAX_KEPT_HEIGHT);
        }
        let mut frame = decoded.into_rgba8();
        // The renderer takes BGRA.
        for pixel in frame.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Some(Self {
            image: Arc::new(RenderImage::new(vec![Frame::new(frame)])),
            width,
            height,
        })
    }

    /// The memory its pixels hold.
    pub fn bytes(&self) -> usize {
        self.image.as_bytes(0).map_or(0, <[u8]>::len)
    }

    /// The size to draw at: shrunk to fit the bounds, and never enlarged.
    pub fn fitted(&self, max_width: f32, max_height: f32) -> (f32, f32) {
        let (width, height) = (self.width as f32, self.height as f32);
        let scale = (max_width.max(1.0) / width)
            .min(max_height.max(1.0) / height)
            .min(1.0);
        ((width * scale).max(1.0), (height * scale).max(1.0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub data: String,
    /// The picture a PNG shows. Any other file is sent as a file, and has none.
    pub preview: Option<Arc<Image>>,
    /// Pixel size from the PNG's header, when it has a readable one and the
    /// picture is small enough to be worth decoding.
    pub size: Option<(u32, u32)>,
    /// The decoded size, which is what the limits count.
    pub bytes: usize,
    /// The same for the same content, so a row drawn earlier finds it again.
    pub id: u64,
}

impl Attachment {
    pub fn from_png(name: impl Into<String>, bytes: Vec<u8>) -> Result<Self, String> {
        if !bytes.starts_with(PNG_SIGNATURE) {
            return Err("Only PNG images can be attached.".into());
        }
        let size = image::ImageReader::with_format(Cursor::new(&bytes), image::ImageFormat::Png)
            .into_dimensions()
            .ok()
            .filter(|&(width, height)| worth_decoding(width, height));
        let length = bytes.len();
        let preview = Arc::new(Image::from_bytes(ImageFormat::Png, bytes));
        let name = name.into();
        // The name is part of it, so the same picture attached twice under
        // two names is two attachments.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(&name, preview.id()), &mut hasher);
        Ok(Self {
            name,
            mime: "image/png".into(),
            data: STANDARD.encode(&preview.bytes),
            id: std::hash::Hasher::finish(&hasher),
            preview: Some(preview),
            size,
            bytes: length,
        })
    }

    /// Any file from disk: a PNG becomes an image the agent sees, anything
    /// else a file it reads from the host.
    pub fn from_file(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        let name = name.into();
        if bytes.starts_with(PNG_SIGNATURE)
            && let Ok(image) = Self::from_png(name.clone(), bytes.clone())
        {
            return image;
        }
        let mime = file_mime(&name).to_owned();
        Self::file(name, mime, STANDARD.encode(&bytes), bytes.len())
    }

    /// The same attachment as the host counts it: name, type, and content.
    pub fn same_as(&self, other: &Self) -> bool {
        self.name == other.name && self.mime == other.mime && self.data == other.data
    }

    fn file(name: String, mime: String, data: String, bytes: usize) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(&name, &data), &mut hasher);
        Self {
            name,
            mime,
            data,
            preview: None,
            size: None,
            bytes,
            id: std::hash::Hasher::finish(&hasher),
        }
    }

    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        let name = value.get("name").and_then(Value::as_str)?;
        let mime = value.get("mime").and_then(Value::as_str)?;
        let data = value.get("data").and_then(Value::as_str)?;
        let bytes = STANDARD.decode(data).ok()?;
        if mime != "image/png" {
            return Some(Self::file(
                name.to_owned(),
                mime.to_owned(),
                data.to_owned(),
                bytes.len(),
            ));
        }
        let mut attachment = Self::from_png(name, bytes).ok()?;
        attachment.data = data.to_owned();
        Some(attachment)
    }
}

/// A synchronized draft's images, then its files, when it carries them. The
/// host sends the two apart so clients that know only images never see files.
pub fn draft_attachments(body: &Value) -> Option<Vec<Attachment>> {
    let images = body.get("draft_attachments")?.as_array()?;
    let files = body.get("draft_files").and_then(Value::as_array);
    Some(
        images
            .iter()
            .chain(files.into_iter().flatten())
            .filter_map(Attachment::from_value)
            .collect(),
    )
}

/// A media type for a file the user attached, from its extension. Only
/// whether it is a PNG changes how the host passes it on; the rest is a label.
fn file_mime(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "json" => "application/json",
        "zip" => "application/zip",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "md" | "markdown" => "text/markdown",
        "txt" | "log" | "rs" | "py" | "js" | "ts" | "tsx" | "go" | "c" | "h" | "cpp" | "java"
        | "kt" | "toml" | "yaml" | "yml" | "xml" | "sh" | "sql" => "text/plain",
        _ => "application/octet-stream",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
    /// The GitHub organization or user behind the folder's `origin` remote,
    /// whose avatar stands in for the folder icon. Absent when the host finds
    /// no GitHub remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// A folder the user made to hold projects, such as one per organization,
    /// rather than a project that sessions run in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub group: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSummary {
    pub id: String,
    pub folder: String,
    #[serde(default)]
    pub title: Option<String>,
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub working: bool,
    #[serde(default)]
    pub terminal_working: bool,
    /// Background agents and shells its agent process is still running. With
    /// no turn running, the chat is waiting on them; see [`Self::waiting_on`].
    #[serde(default)]
    pub background_tasks: u32,
    /// When this chat's session settled; absent while it is in the active list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<i64>,
    /// The session is kept out of automatic settling.
    #[serde(default)]
    pub auto_settle_disabled: bool,
}

impl ChatSummary {
    /// How many background tasks the chat is waiting on: none while a turn
    /// runs, since then it is working.
    pub fn waiting_on(&self) -> u32 {
        if self.working {
            0
        } else {
            self.background_tasks
        }
    }
}

/// How the connected host settles sessions by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoSettleRules {
    pub after_days: Option<i64>,
    pub on_merge: bool,
    pub keep_pull_requests: bool,
}

impl AutoSettleRules {
    pub const DEFAULT_AFTER_DAYS: i64 = 3;
    pub const MIN_AFTER_DAYS: i64 = 1;
    pub const MAX_AFTER_DAYS: i64 = 90;

    pub fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            after_days: match value.get("after_days")? {
                Value::Null => None,
                days => Some(days.as_i64()?),
            },
            on_merge: value.get("on_merge")?.as_bool()?,
            keep_pull_requests: value.get("keep_pull_requests")?.as_bool()?,
        })
    }
}

/// How the connected host deletes idle sessions by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoDeleteRules {
    pub after_days: Option<i64>,
}

impl AutoDeleteRules {
    pub const DEFAULT_AFTER_DAYS: i64 = 30;
    /// The choices the stepper moves through. The host takes any whole number
    /// of days from 1 to 365, so a value set elsewhere steps to its neighbours.
    pub const STEPS: [i64; 11] = [1, 2, 3, 5, 7, 14, 30, 60, 90, 180, 365];

    pub fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            after_days: match value.get("after_days")? {
                Value::Null => None,
                days => Some(days.as_i64()?),
            },
        })
    }

    /// The next choice after `days`, longer or shorter, if there is one.
    pub fn step(days: i64, longer: bool) -> Option<i64> {
        if longer {
            Self::STEPS.into_iter().find(|step| *step > days)
        } else {
            Self::STEPS.into_iter().rev().find(|step| *step < days)
        }
    }
}

/// A persistent workspace shared by independent agent conversations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub folder: String,
    pub title: String,
    #[serde(default)]
    pub chats: Vec<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub working: bool,
    #[serde(default)]
    pub terminal_working: bool,
    /// The sum of its chats' `background_tasks`.
    #[serde(default)]
    pub background_tasks: u32,
    #[serde(default)]
    pub terminal_chat: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalActivityStamp {
    pub revision: u64,
    pub working: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub id: Option<i64>,
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(skip)]
    markdown: Option<Arc<Document>>,
    #[serde(skip)]
    image_paths: Vec<String>,
    /// Files the user attached, by their path on the host.
    #[serde(skip)]
    file_paths: Vec<String>,
    /// A tool row's card, parsed once when the row arrives rather than on
    /// every frame it is drawn.
    #[serde(skip)]
    tool: Option<Arc<ToolView>>,
    /// For a message sent from here and not stored yet, where it was sent
    /// from. Without it, the message is dropped by the next newest page, as
    /// a message sent from an older window of the transcript is.
    #[serde(skip)]
    pub sent_from: Option<SentFrom>,
}

/// Where in a chat a message was sent from: after the newest stored message
/// shown, while the newest page was shown. Its stored copy comes after that.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SentFrom {
    pub newest: Option<i64>,
}

/// What a tool row draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolView {
    Subagent(SubagentView),
    Activity(ActivityCard),
}

impl ToolView {
    fn parse(content: &str) -> Self {
        SubagentView::parse(content)
            .map(Self::Subagent)
            .unwrap_or_else(|| Self::Activity(ActivityCard::parse(content)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl Message {
    pub fn new(
        id: Option<i64>,
        role: impl Into<String>,
        content: impl Into<String>,
        label: Option<String>,
    ) -> Self {
        let content = content.into();
        let role = role.into();
        let (shown, mut image_paths, file_paths) = message_content(&role, &content);
        // A tool row draws its card, not its text as markdown; a patch can be
        // large, so its document is only built if something asks for it.
        let tool = (role == "tool").then(|| Arc::new(ToolView::parse(&content)));
        let markdown = tool.is_none().then(|| {
            Arc::new(if role == "assistant" {
                let document = markdown::parse_reply(&shown);
                image_paths = document.image_paths();
                document
            } else {
                markdown::parse(&shown)
            })
        });
        Self {
            id,
            role,
            content,
            label,
            markdown,
            image_paths,
            file_paths,
            tool,
            sent_from: None,
        }
    }

    /// A streaming reply, formatted while it streams.
    pub fn new_streaming(
        id: Option<i64>,
        role: impl Into<String>,
        content: impl Into<String>,
        label: Option<String>,
    ) -> Self {
        let content = content.into();
        let role = role.into();
        let document = markdown::parse_streaming(&content);
        let image_paths = document.image_paths();
        let markdown = Some(Arc::new(document));
        Self {
            id,
            role,
            content,
            label,
            markdown,
            image_paths,
            file_paths: Vec::new(),
            tool: None,
            sent_from: None,
        }
    }

    pub fn new_plain(
        id: Option<i64>,
        role: impl Into<String>,
        content: impl Into<String>,
        label: Option<String>,
    ) -> Self {
        let content = content.into();
        let role = role.into();
        let (shown, image_paths, file_paths) = message_content(&role, &content);
        let markdown = Some(Arc::new(markdown::plain_document(&shown)));
        let tool = (role == "tool").then(|| Arc::new(ToolView::parse(&content)));
        Self {
            id,
            role,
            content,
            label,
            markdown,
            image_paths,
            file_paths,
            tool,
            sent_from: None,
        }
    }

    pub fn markdown(&self) -> Arc<Document> {
        self.markdown
            .clone()
            .unwrap_or_else(|| Arc::new(markdown::parse(&markdown::display_text(&self.content))))
    }

    pub fn image_paths(&self) -> &[String] {
        &self.image_paths
    }

    pub fn file_paths(&self) -> &[String] {
        &self.file_paths
    }

    /// The card a tool row draws; `None` for every other role.
    pub fn tool_view(&self) -> Option<&ToolView> {
        self.tool.as_deref()
    }

    fn cache_markdown(&mut self) {
        let (shown, mut image_paths, file_paths) = message_content(&self.role, &self.content);
        if self.role == "tool" {
            self.tool = Some(Arc::new(ToolView::parse(&self.content)));
            self.markdown = None;
        } else {
            self.markdown = Some(Arc::new(if self.role == "assistant" {
                let document = markdown::parse_reply(&shown);
                image_paths = document.image_paths();
                document
            } else {
                markdown::parse(&shown)
            }));
        }
        self.image_paths = image_paths;
        self.file_paths = file_paths;
    }
}

/// What a message shows as text, and the images and files sent with it, each
/// named on a line of its own. An assistant's images sit among its blocks.
fn message_content(role: &str, content: &str) -> (String, Vec<String>, Vec<String>) {
    if role == "assistant" {
        return (content.to_owned(), Vec::new(), Vec::new());
    }
    let mut shown = Vec::new();
    let mut images = Vec::new();
    let mut files = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if let Some(path) = attached_path(line, "[image: ") {
            images.push(path.to_owned());
        } else if let Some(path) = attached_path(line, "[file: ") {
            files.push(path.to_owned());
        } else {
            shown.push(line);
        }
    }
    if images.is_empty() && files.is_empty() {
        (content.to_owned(), images, files)
    } else {
        (shown.join("\n").trim().to_owned(), images, files)
    }
}

fn attached_path<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.strip_prefix(marker)
        .and_then(|line| line.strip_suffix(']'))
        .filter(|path| !path.is_empty())
}

/// The name a host path ends in, for a file sent from either kind of system.
pub fn attached_file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
    pub detached: bool,
    pub main: bool,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModel {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBackend {
    pub id: String,
    pub name: String,
    pub default_model: String,
    pub models: Vec<AgentModel>,
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppModel {
    pub folders: Vec<Folder>,
    pub chats: Vec<ChatSummary>,
    pub sessions: Vec<SessionSummary>,
    /// The connected host can settle sessions.
    pub settlement_supported: bool,
    /// The connected host refuses a folder reorder that would move it on disk.
    pub folder_reorder_supported: bool,
    /// The connected host keeps folders of projects. An older one ignores the
    /// request and makes a project instead.
    pub folder_groups_supported: bool,
    pub terminal_activity_epoch: Option<String>,
    pub terminal_activity_by_chat: HashMap<String, TerminalActivityStamp>,
    pub selected_chat: Option<String>,
    /// Counts chat selections, so work begun for one visit to a chat can tell
    /// it from a later visit to the same chat.
    pub chat_visits: u64,
    pub unread_chats: HashSet<String>,
    /// Pending approval or question request ids per chat, for every chat the
    /// daemon reports on, so the sidebar can flag sessions waiting on the user.
    pub waiting_chats: HashMap<String, HashSet<String>>,
    pub messages: Vec<Message>,
    pub queue: Vec<String>,
    /// The chat's last turn was cut short and can be continued.
    pub resumable: bool,
    pub working: bool,
    pub working_started_at: Option<Instant>,
    /// The running turn's label ("Sonnet · High"), which its stored reply
    /// will carry, so the live reply does not change name when it lands.
    pub turn_label: Option<String>,
    pub new_worktree: bool,
    pub has_messages: bool,
    pub worktrees: Vec<Worktree>,
    pub agent_backends: Vec<AgentBackend>,
    pub backend: String,
    pub model: Option<String>,
    pub effort: String,
    pub access: String,
    pub plan: bool,
    pub fast: bool,
    pub context_used: u64,
    pub context_window: u64,
    pub auth_state: String,
    pub workdir: Option<String>,
    pub commands: Vec<String>,
    pub global_shortcuts: Vec<String>,
    pub workspace_shortcuts: Vec<String>,
    pub shortcuts: Vec<String>,
    pub connected: bool,
    pub connection_error: Option<String>,
    pub draft: String,
    pub draft_attachments: Vec<Attachment>,
    pub draft_revision: i64,
    pub live_text: String,
    pub live_items: Vec<Message>,
    pub todos: Vec<TodoItem>,
    pub interactions: Vec<Value>,
    pub interaction_revision: u64,
    pub interaction_epoch: String,
    pub turn_epoch: String,
    pub turn_watermark_id: u64,
    pub turn_watermark_sequence: u64,
}

#[derive(Debug, Deserialize)]
struct TreeSnapshot {
    folders: Vec<Folder>,
    chats: Vec<ChatSummary>,
    #[serde(default)]
    sessions: Vec<SessionSummary>,
    /// The host can settle sessions; older hosts leave this out.
    #[serde(default)]
    settlement: bool,
    /// The host refuses a folder reorder that would move it on disk; older
    /// hosts leave this out.
    #[serde(default)]
    folder_reorder: bool,
    /// The host keeps folders of projects; older hosts leave this out.
    #[serde(default)]
    folder_groups: bool,
    #[serde(default)]
    terminal_activity_epoch: Option<String>,
    #[serde(default)]
    terminal_activity_revision: Option<u64>,
}

fn normalize_sessions(snapshot: &mut TreeSnapshot, body: &Value) {
    let known_chats = snapshot
        .chats
        .iter()
        .map(|chat| chat.id.as_str())
        .collect::<HashSet<_>>();
    for session in &mut snapshot.sessions {
        session.chats.retain(|id| known_chats.contains(id.as_str()));
        if session.terminal_chat.is_empty() {
            session.terminal_chat = format!("session:{}", session.id);
        }
    }
    for chat in &snapshot.chats {
        if snapshot
            .sessions
            .iter()
            .any(|session| session.chats.contains(&chat.id))
        {
            continue;
        }
        let stored = body
            .get("chats")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|value| value.get("id").and_then(Value::as_str) == Some(chat.id.as_str()));
        let session_id = stored
            .and_then(|value| value.get("session"))
            .and_then(Value::as_str);
        if let Some(session) = snapshot
            .sessions
            .iter_mut()
            .find(|session| Some(session.id.as_str()) == session_id)
        {
            session.chats.push(chat.id.clone());
            continue;
        }
        snapshot.sessions.push(SessionSummary {
            id: session_id.unwrap_or(&chat.id).to_owned(),
            folder: chat.folder.clone(),
            title: stored
                .and_then(|value| value.get("session_title"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| chat.title.clone())
                .unwrap_or_else(|| "New Session".into()),
            chats: vec![chat.id.clone()],
            branch: chat.branch.clone(),
            working: chat.working,
            terminal_working: chat.terminal_working,
            background_tasks: chat.background_tasks,
            terminal_chat: session_id
                .map(|id| format!("session:{id}"))
                .unwrap_or_else(|| chat.id.clone()),
        });
    }
    snapshot
        .sessions
        .retain(|session| !session.chats.is_empty());
}

#[derive(Debug, Deserialize)]
struct MessagesSnapshot {
    #[serde(default)]
    messages: Vec<Message>,
}

pub const MAX_RETAINED_MESSAGES: usize = 480;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePageDirection {
    Tail,
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePageChange {
    pub inserted_at_start: usize,
    pub inserted_at_end: usize,
    pub removed_from_start: usize,
    pub removed_from_end: usize,
    pub has_older: bool,
    pub has_newer: bool,
    /// Rows changed other than at the ends, so the list starts over.
    pub reset: bool,
}

impl AppModel {
    pub fn session_for_chat(&self, chat_id: &str) -> Option<&SessionSummary> {
        self.sessions
            .iter()
            .find(|session| session.chats.iter().any(|id| id == chat_id))
    }

    pub fn session_key_for_chat(&self, chat_id: &str) -> String {
        self.session_for_chat(chat_id)
            .map(|session| session.id.clone())
            .unwrap_or_else(|| chat_id.to_owned())
    }

    pub fn terminal_key_for_chat(&self, chat_id: &str) -> String {
        self.session_for_chat(chat_id)
            .map(|session| {
                if session.terminal_chat.is_empty() {
                    format!("session:{}", session.id)
                } else {
                    session.terminal_chat.clone()
                }
            })
            .unwrap_or_else(|| chat_id.to_owned())
    }

    pub fn canonical_terminal_key(&self, owner: &str) -> String {
        if owner.starts_with("global:") {
            return owner.to_owned();
        }
        self.sessions
            .iter()
            .find(|session| {
                session.id == owner
                    || session.terminal_chat == owner
                    || session.chats.iter().any(|chat| chat == owner)
            })
            .map(|session| {
                if session.terminal_chat.is_empty() {
                    format!("session:{}", session.id)
                } else {
                    session.terminal_chat.clone()
                }
            })
            .unwrap_or_else(|| owner.to_owned())
    }

    fn refresh_session_activity(&mut self) {
        for session in &mut self.sessions {
            session.working = self
                .chats
                .iter()
                .any(|chat| session.chats.contains(&chat.id) && chat.working);
            session.terminal_working = self
                .chats
                .iter()
                .any(|chat| session.chats.contains(&chat.id) && chat.terminal_working);
            session.background_tasks = self
                .chats
                .iter()
                .filter(|chat| session.chats.contains(&chat.id))
                .map(|chat| chat.background_tasks)
                .sum();
        }
    }

    pub fn apply_desktop_terminal_activity(&mut self, chat_id: &str, working: bool) {
        let chat_id = self.canonical_terminal_key(chat_id);
        let current = self
            .sessions
            .iter()
            .find(|session| session.terminal_chat == chat_id)
            .map(|session| session.terminal_working)
            .or_else(|| {
                self.chats
                    .iter()
                    .find(|chat| chat.id == chat_id)
                    .map(|chat| chat.terminal_working)
            });
        if current == Some(working) {
            return;
        }
        let revision = self
            .terminal_activity_by_chat
            .values()
            .map(|activity| activity.revision)
            .max()
            .unwrap_or_default()
            .saturating_add(1);
        let mut body = serde_json::json!({
            "chat": chat_id,
            "terminal_working": working,
        });
        if let Some(epoch) = &self.terminal_activity_epoch {
            body["terminal_activity_epoch"] = Value::String(epoch.clone());
            body["terminal_activity_revision"] = Value::from(revision);
        }
        self.apply_event("terminal-activity", &body);
    }

    pub fn apply_tree(&mut self, body: &Value) -> Result<(), serde_json::Error> {
        let mut snapshot: TreeSnapshot = serde_json::from_value(body.clone())?;
        normalize_sessions(&mut snapshot, body);
        let terminal_activity_revision = self.prepare_terminal_activity_version(
            snapshot.terminal_activity_epoch.as_deref(),
            snapshot.terminal_activity_revision,
        );
        if let Some(revision) = terminal_activity_revision {
            for chat in &mut snapshot.chats {
                let resource = snapshot
                    .sessions
                    .iter()
                    .find(|session| session.chats.contains(&chat.id))
                    .map(|session| session.terminal_chat.as_str());
                if let Some(activity) = [Some(chat.id.as_str()), resource]
                    .into_iter()
                    .flatten()
                    .filter_map(|key| self.terminal_activity_by_chat.get(key))
                    .filter(|activity| activity.revision > revision)
                    .max_by_key(|activity| activity.revision)
                    .copied()
                {
                    chat.terminal_working = activity.working;
                } else {
                    self.terminal_activity_by_chat.insert(
                        chat.id.clone(),
                        TerminalActivityStamp {
                            revision,
                            working: chat.terminal_working,
                        },
                    );
                }
            }
            for session in &snapshot.sessions {
                if self
                    .terminal_activity_by_chat
                    .get(&session.terminal_chat)
                    .is_none_or(|activity| activity.revision <= revision)
                {
                    let working = snapshot
                        .chats
                        .iter()
                        .any(|chat| session.chats.contains(&chat.id) && chat.terminal_working);
                    self.terminal_activity_by_chat.insert(
                        session.terminal_chat.clone(),
                        TerminalActivityStamp { revision, working },
                    );
                }
            }
            let snapshot_chats = snapshot
                .chats
                .iter()
                .map(|chat| chat.id.as_str())
                .chain(
                    snapshot
                        .sessions
                        .iter()
                        .map(|session| session.terminal_chat.as_str()),
                )
                .collect::<HashSet<_>>();
            self.terminal_activity_by_chat.retain(|chat_id, activity| {
                snapshot_chats.contains(chat_id.as_str()) || activity.revision > revision
            });
        }
        self.folders = snapshot.folders;
        self.chats = snapshot.chats;
        self.sessions = snapshot.sessions;
        self.settlement_supported = snapshot.settlement;
        self.folder_reorder_supported = snapshot.folder_reorder;
        self.folder_groups_supported = snapshot.folder_groups;
        self.refresh_session_activity();
        self.connected = true;
        self.unread_chats
            .retain(|chat_id| self.chats.iter().any(|chat| &chat.id == chat_id));

        if self
            .selected_chat
            .as_ref()
            .is_some_and(|selected| !self.chats.iter().any(|chat| &chat.id == selected))
        {
            self.selected_chat = None;
            self.messages.clear();
            self.queue.clear();
            self.resumable = false;
            self.working = false;
            self.working_started_at = None;
            self.new_worktree = false;
            self.has_messages = false;
            self.worktrees.clear();
            self.backend.clear();
            self.model = None;
            self.effort.clear();
            self.access.clear();
            self.plan = false;
            self.fast = false;
            self.context_used = 0;
            self.context_window = 0;
            self.auth_state.clear();
            self.workdir = None;
            self.commands.clear();
            self.global_shortcuts.clear();
            self.workspace_shortcuts.clear();
            self.shortcuts.clear();
            self.draft.clear();
            self.draft_attachments.clear();
            self.live_text.clear();
            self.live_items.clear();
            self.todos.clear();
        }
        Ok(())
    }

    pub fn select_chat(&mut self, chat_id: impl Into<String>) {
        self.interactions.clear();
        self.interaction_revision = 0;
        self.interaction_epoch.clear();
        self.turn_epoch.clear();
        self.turn_watermark_id = 0;
        self.turn_watermark_sequence = 0;
        let chat_id = chat_id.into();
        self.unread_chats.remove(&chat_id);
        self.selected_chat = Some(chat_id);
        self.chat_visits = self.chat_visits.wrapping_add(1);
        self.messages.clear();
        self.queue.clear();
        self.resumable = false;
        self.working = false;
        self.working_started_at = None;
        self.turn_label = None;
        self.new_worktree = false;
        self.has_messages = false;
        self.worktrees.clear();
        self.backend.clear();
        self.model = None;
        self.effort.clear();
        self.access.clear();
        self.plan = false;
        self.fast = false;
        self.context_used = 0;
        self.context_window = 0;
        self.auth_state.clear();
        self.workdir = None;
        self.commands.clear();
        self.global_shortcuts.clear();
        self.workspace_shortcuts.clear();
        self.shortcuts.clear();
        self.draft.clear();
        self.draft_attachments.clear();
        self.draft_revision = -1;
        self.live_text.clear();
        self.live_items.clear();
        self.todos.clear();
    }

    /// Whether a chat has an approval or question waiting on the user.
    pub fn is_waiting(&self, chat_id: &str) -> bool {
        self.waiting_chats
            .get(chat_id)
            .is_some_and(|pending| !pending.is_empty())
    }

    fn track_waiting(&mut self, name: &str, chat: &str, body: &Value) {
        if name == "interactions-cleared" {
            self.waiting_chats.remove(chat);
            return;
        }
        let Some(id) = body.get("request_id").and_then(Value::as_str) else {
            return;
        };
        let pending = self.waiting_chats.entry(chat.to_owned()).or_default();
        if name == "interaction" {
            pending.insert(id.to_owned());
        } else {
            pending.remove(id);
        }
        if pending.is_empty() {
            self.waiting_chats.remove(chat);
        }
    }

    pub fn apply_chat(&mut self, body: &Value) {
        let epoch = body
            .get("interaction_epoch")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if epoch != self.interaction_epoch {
            self.interaction_epoch = epoch.to_owned();
            self.interaction_revision = 0;
            self.interactions.clear();
        }
        let revision = body
            .get("interaction_revision")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if revision >= self.interaction_revision {
            self.interaction_revision = revision;
            self.interactions = body
                .get("interactions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(chat) = self.selected_chat.clone() {
                let pending = self
                    .interactions
                    .iter()
                    .filter_map(|pending| pending.get("request_id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect::<HashSet<_>>();
                if pending.is_empty() {
                    self.waiting_chats.remove(&chat);
                } else {
                    self.waiting_chats.insert(chat, pending);
                }
            }
        }
        if let Some(backend) = body.get("backend").and_then(Value::as_str) {
            self.backend = backend.to_owned();
            if let Some(selected) = self.selected_chat.as_deref()
                && let Some(summary) = self.chats.iter_mut().find(|chat| chat.id == selected)
            {
                summary.backend = backend.to_owned();
            }
        }
        self.model = body
            .get("model")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.agent_backends
                    .iter()
                    .find(|backend| backend.id == self.backend)
                    .map(|backend| backend.default_model.clone())
            });
        self.effort = body
            .get("effort")
            .and_then(Value::as_str)
            .unwrap_or("high")
            .to_owned();
        self.access = body
            .get("access")
            .and_then(Value::as_str)
            .unwrap_or("read-only")
            .to_owned();
        self.plan = body.get("plan").and_then(Value::as_bool).unwrap_or(false);
        self.fast =
            self.backend == "codex" && body.get("fast").and_then(Value::as_bool).unwrap_or(false);
        self.context_used = body
            .get("context_used")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.context_window = body
            .get("context_window")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.auth_state = body
            .get("auth_state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        self.workdir = body
            .get("workdir")
            .and_then(Value::as_str)
            .filter(|workdir| !workdir.is_empty())
            .map(str::to_owned);
        if let Some(commands) = body.get("commands").and_then(Value::as_array) {
            self.commands = string_array(commands);
        }
        if let Some(shortcuts) = body.get("shortcuts").and_then(Value::as_array) {
            self.shortcuts = string_array(shortcuts);
        }
        self.queue = body
            .get("queue")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect();
        if let Some(label) = body.get("label").and_then(Value::as_str) {
            self.turn_label = Some(label.to_owned());
        }
        let snapshot_working = body
            .get("working")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let (Some(id), Some(sequence)) = (
            body.get("turn_id").and_then(Value::as_u64),
            body.get("turn_sequence").and_then(Value::as_u64),
        ) {
            let epoch = body
                .get("turn_epoch")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let fresh = epoch != self.turn_epoch
                || id > self.turn_watermark_id
                || (id == self.turn_watermark_id && sequence >= self.turn_watermark_sequence);
            if fresh {
                self.turn_epoch = epoch.to_owned();
                self.turn_watermark_id = id;
                self.turn_watermark_sequence = sequence;
                self.working = snapshot_working;
                self.live_text = body
                    .get("segment")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.live_items = body
                    .get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|item| {
                        let text = item.get("text")?.as_str()?;
                        if let Some(todos) = todo_snapshot(text) {
                            self.todos = todos;
                            return None;
                        }
                        if let Some(sent) = steered_text(text) {
                            return Some(Message::new(None, "user", sent, None));
                        }
                        Some(Message::new(
                            item.get("message_id").and_then(Value::as_i64),
                            if item.get("tool").and_then(Value::as_bool) == Some(true) {
                                "tool"
                            } else {
                                "assistant"
                            },
                            text,
                            Some(self.backend.clone()),
                        ))
                    })
                    .collect();
            }
        } else {
            self.working = snapshot_working;
            if self.working
                && self.live_text.is_empty()
                && self.live_items.is_empty()
                && let Some(segment) = body.get("segment").and_then(Value::as_str)
            {
                self.live_text = segment.to_owned();
            }
            if !self.working {
                self.live_text.clear();
                self.live_items.clear();
            }
        }
        if let Some(selected_chat) = self.selected_chat.as_deref()
            && let Some(summary) = self
                .chats
                .iter_mut()
                .find(|summary| summary.id == selected_chat)
        {
            summary.working = self.working;
            if let Some(count) = body.get("background_tasks").and_then(Value::as_u64) {
                summary.background_tasks = u32::try_from(count).unwrap_or(u32::MAX);
            }
        }
        self.refresh_session_activity();
        let now = Instant::now();
        self.working_started_at = if self.working {
            body.get("working_for")
                .and_then(Value::as_u64)
                .and_then(|elapsed| now.checked_sub(Duration::from_secs(elapsed)))
                .or(self.working_started_at)
                .or(Some(now))
        } else {
            None
        };
        self.resumable = !self.working
            && body
                .get("resumable")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        self.new_worktree = body
            .get("new_worktree")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.has_messages = body
            .get("has_messages")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.worktrees = body
            .get("worktrees")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        self.apply_draft(body);
        self.refresh_session_activity();
    }

    pub fn apply_agent_catalog(&mut self, body: &Value) -> Result<(), serde_json::Error> {
        self.agent_backends = serde_json::from_value(
            body.get("backends")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        )?;
        if self.model.is_none() {
            self.model = self
                .agent_backends
                .iter()
                .find(|backend| backend.id == self.backend)
                .map(|backend| backend.default_model.clone());
        }
        Ok(())
    }

    pub fn selected_model_name(&self) -> Option<&str> {
        let model = self.model.as_deref()?;
        self.agent_backends
            .iter()
            .find(|backend| backend.id == self.backend)
            .into_iter()
            .flat_map(|backend| &backend.models)
            .find(|candidate| candidate.id == model)
            .map(|candidate| candidate.name.as_str())
            .or(Some(model))
    }

    pub fn selected_model_label(&self) -> String {
        crate::presentation::model_name(
            self.model.as_deref().unwrap_or_default(),
            self.selected_model_name(),
        )
    }

    pub fn apply_shortcuts(&mut self, body: &Value) {
        if let Some(shortcuts) = body.get("global").and_then(Value::as_array) {
            self.global_shortcuts = string_array(shortcuts);
        }
        if let Some(shortcuts) = body.get("workspace").and_then(Value::as_array) {
            self.workspace_shortcuts = string_array(shortcuts);
        }
        if let Some(shortcuts) = body.get("effective").and_then(Value::as_array) {
            self.shortcuts = string_array(shortcuts);
        }
    }

    pub fn apply_message_page(
        &mut self,
        body: &Value,
        direction: MessagePageDirection,
    ) -> Result<MessagePageChange, serde_json::Error> {
        let mut snapshot: MessagesSnapshot = serde_json::from_value(body.clone())?;
        snapshot
            .messages
            .iter_mut()
            .for_each(Message::cache_markdown);
        if direction != MessagePageDirection::Before
            && let Some(todos) = snapshot
                .messages
                .iter()
                .rev()
                .find_map(|message| todo_snapshot(&message.content))
        {
            self.todos = todos;
        }
        snapshot
            .messages
            .retain(|message| todo_snapshot(&message.content).is_none());
        let has_older = body
            .get("has_older")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_newer = body
            .get("has_newer")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut change = MessagePageChange {
            inserted_at_start: 0,
            inserted_at_end: 0,
            removed_from_start: 0,
            removed_from_end: 0,
            has_older,
            has_newer,
            reset: false,
        };
        // A stored copy of a message sent from here is newer than every
        // stored message shown when it was sent; an older one with the same
        // words was said before.
        // The host stores attachments as lines after the text, so the words
        // are compared as they are shown, on both sides.
        let stores = |stored: &Message, sent: &Message| {
            let Some(from) = sent.sent_from else {
                return false;
            };
            stored.role == "user"
                && message_content(&stored.role, &stored.content).0.trim()
                    == message_content(&sent.role, &sent.content).0.trim()
                && stored
                    .id
                    .is_some_and(|id| from.newest.is_none_or(|newest| id > newest))
        };
        // Whether a newest page goes back far enough to hold that copy. One
        // that starts after the point the message was sent from shows a chat
        // that has moved on since, so the message was stored long ago.
        let oldest = snapshot
            .messages
            .iter()
            .filter_map(|message| message.id)
            .min();
        let reaches = |sent: &Message| match sent.sent_from {
            None => false,
            Some(from) => {
                !has_older
                    || matches!((from.newest, oldest), (Some(newest), Some(oldest)) if oldest <= newest)
            }
        };
        if direction == MessagePageDirection::Tail {
            // A message sent from here but not stored yet stays at the end
            // until a page has it. A chat's first page is often asked for
            // before its first message is stored, and would otherwise wipe
            // that message off the screen until the next page.
            let unsent = self
                .messages
                .iter()
                .filter(|message| message.id.is_none() && message.role == "user")
                .filter(|message| {
                    reaches(message)
                        && !snapshot
                            .messages
                            .iter()
                            .any(|stored| stores(stored, message))
                })
                .cloned()
                .collect::<Vec<_>>();
            self.messages = snapshot.messages;
            self.messages.extend(unsent);
            if self.messages.len() > MAX_RETAINED_MESSAGES {
                change.removed_from_start = self.messages.len() - MAX_RETAINED_MESSAGES;
                self.messages.drain(0..change.removed_from_start);
            }
            return Ok(change);
        }

        snapshot.messages.retain(|candidate| {
            candidate.id.is_none()
                || !self
                    .messages
                    .iter()
                    .any(|existing| existing.id == candidate.id)
        });
        match direction {
            MessagePageDirection::Before => {
                change.inserted_at_start = snapshot.messages.len();
                self.messages.splice(0..0, snapshot.messages);
                change.removed_from_end = self.messages.len().saturating_sub(MAX_RETAINED_MESSAGES);
                self.messages.truncate(MAX_RETAINED_MESSAGES);
            }
            MessagePageDirection::After => {
                // The stored copy of a message sent from here replaces it.
                let before = self.messages.len();
                let snapshot_messages = &snapshot.messages;
                self.messages.retain(|message| {
                    message.id.is_some()
                        || message.role != "user"
                        || !snapshot_messages
                            .iter()
                            .any(|stored| stores(stored, message))
                });
                change.reset = self.messages.len() != before;
                change.inserted_at_end = snapshot.messages.len();
                self.messages.extend(snapshot.messages);
                change.removed_from_start =
                    self.messages.len().saturating_sub(MAX_RETAINED_MESSAGES);
                if change.removed_from_start > 0 {
                    self.messages.drain(0..change.removed_from_start);
                }
            }
            MessagePageDirection::Tail => unreachable!(),
        }
        Ok(change)
    }

    pub fn apply_event(&mut self, name: &str, body: &Value) {
        if matches!(name, "turn-started" | "text" | "tool" | "turn-finished")
            && self.event_is_active(body)
            && let (Some(id), Some(sequence)) = (
                body.get("turn_id").and_then(Value::as_u64),
                body.get("turn_sequence").and_then(Value::as_u64),
            )
        {
            let epoch = body
                .get("turn_epoch")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if epoch != self.turn_epoch {
                self.turn_epoch = epoch.to_owned();
                self.turn_watermark_id = 0;
                self.turn_watermark_sequence = 0;
                self.live_items.clear();
                self.live_text.clear();
            }
            if id < self.turn_watermark_id
                || (id == self.turn_watermark_id && sequence <= self.turn_watermark_sequence)
            {
                return;
            }
            self.turn_watermark_id = id;
            self.turn_watermark_sequence = sequence;
        }

        if matches!(
            name,
            "interaction" | "interaction-resolved" | "interactions-cleared"
        ) && let Some(chat) = body.get("chat").and_then(Value::as_str)
        {
            self.track_waiting(name, chat, body);
        }
        if matches!(
            name,
            "interaction" | "interaction-resolved" | "interactions-cleared"
        ) && body.get("chat").and_then(Value::as_str) == self.selected_chat.as_deref()
        {
            let epoch = body
                .get("interaction_epoch")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if epoch != self.interaction_epoch {
                self.interaction_epoch = epoch.to_owned();
                self.interaction_revision = 0;
                self.interactions.clear();
            }
            let revision = body
                .get("interaction_revision")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if revision > self.interaction_revision {
                self.interaction_revision = revision;
                if name == "interactions-cleared" {
                    self.interactions.clear();
                } else if let Some(id) = body.get("request_id").and_then(Value::as_str) {
                    self.interactions.retain(|pending| {
                        pending.get("request_id").and_then(Value::as_str) != Some(id)
                    });
                    if name == "interaction" {
                        self.interactions.push(body.clone());
                    }
                }
            }
            return;
        }
        if name == "terminal-activity"
            && let Some(chat_id) = body.get("chat").and_then(Value::as_str)
            && let Some(working) = body.get("terminal_working").and_then(Value::as_bool)
        {
            let owner = self.canonical_terminal_key(chat_id);
            let chat_id = owner.as_str();
            let revision = self.prepare_terminal_activity_version(
                body.get("terminal_activity_epoch").and_then(Value::as_str),
                body.get("terminal_activity_revision")
                    .and_then(Value::as_u64),
            );
            let accept = revision.is_none_or(|revision| {
                if self
                    .terminal_activity_by_chat
                    .get(chat_id)
                    .is_some_and(|activity| revision < activity.revision)
                {
                    return false;
                }
                self.terminal_activity_by_chat.insert(
                    chat_id.to_owned(),
                    TerminalActivityStamp { revision, working },
                );
                true
            });
            if accept {
                let shared = self
                    .sessions
                    .iter()
                    .find(|session| session.terminal_chat == chat_id);
                let member_ids = shared
                    .map(|session| session.chats.clone())
                    .unwrap_or_else(|| vec![chat_id.to_owned()]);
                let shared_is_selected = shared.is_some_and(|session| {
                    self.selected_chat
                        .as_ref()
                        .is_some_and(|selected| session.chats.contains(selected))
                });
                for chat in self
                    .chats
                    .iter_mut()
                    .filter(|chat| member_ids.contains(&chat.id))
                {
                    let completed = chat.terminal_working && !working;
                    chat.terminal_working = working;
                    if completed
                        && !shared_is_selected
                        && self.selected_chat.as_deref() != Some(chat.id.as_str())
                    {
                        self.unread_chats.insert(chat.id.clone());
                    }
                }
            }
        }
        if name == "background-tasks"
            && let Some(chat_id) = body.get("chat").and_then(Value::as_str)
            && let Some(count) = body.get("background_tasks").and_then(Value::as_u64)
            && let Some(chat) = self.chats.iter_mut().find(|chat| chat.id == chat_id)
        {
            chat.background_tasks = u32::try_from(count).unwrap_or(u32::MAX);
        }
        let chat_working = match name {
            "turn-started" => Some(true),
            "turn-finished" => Some(false),
            _ => None,
        };
        if let Some(working) = chat_working
            && let Some(chat_id) = body.get("chat").and_then(Value::as_str)
            && let Some(chat) = self.chats.iter_mut().find(|chat| chat.id == chat_id)
        {
            chat.working = working;
        }
        self.refresh_session_activity();
        if name == "turn-finished"
            && let Some(chat_id) = body.get("chat").and_then(Value::as_str)
            && self.selected_chat.as_deref() != Some(chat_id)
        {
            self.unread_chats.insert(chat_id.to_owned());
            return;
        }
        match name {
            "queued" if self.event_is_active(body) => {
                if let Some(queue) = body.get("queue").and_then(Value::as_array) {
                    self.queue = queue
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect();
                }
            }
            "draft" if self.event_is_active(body) => self.apply_draft(body),
            "commands" if self.event_is_active(body) => {
                if let Some(commands) = body.get("commands").and_then(Value::as_array) {
                    self.commands = string_array(commands);
                }
            }
            "turn-started" if self.event_is_active(body) => {
                if let Some(label) = body.get("label").and_then(Value::as_str) {
                    self.turn_label = Some(label.to_owned());
                }
                self.start_working();
                self.live_text.clear();
                self.live_items.clear();
            }
            "text" if self.event_is_active(body) => {
                if let Some(text) = body.get("text").and_then(Value::as_str) {
                    self.live_text.push_str(text);
                }
            }
            "tool" if self.event_is_active(body) => {
                if let Some(todos) = body
                    .get("text")
                    .and_then(Value::as_str)
                    .and_then(todo_snapshot)
                {
                    self.todos = todos;
                    return;
                }
                if !self.live_text.is_empty() {
                    let text = std::mem::take(&mut self.live_text);
                    let label = self.live_label();
                    self.live_items
                        .push(Message::new(None, "assistant", text, label));
                }
                if let Some(sent) = body
                    .get("text")
                    .and_then(Value::as_str)
                    .and_then(steered_text)
                {
                    self.live_items.push(Message::new(None, "user", sent, None));
                    return;
                }
                self.live_items.push(Message::new(
                    body.get("message_id").and_then(Value::as_i64),
                    "tool",
                    body.get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("Used a tool")
                        .to_owned(),
                    None,
                ));
            }
            "turn-finished" if self.event_is_active(body) => self.stop_working(),
            _ => {}
        }
    }

    /// Replaces a tool record -- a subagent's card, a turn's diff -- with its
    /// newest snapshot. Returns the transcript row that changed.
    pub fn apply_record(&mut self, body: &Value) -> Option<usize> {
        if !self.event_is_active(body) {
            return None;
        }
        let id = body.get("message_id").and_then(Value::as_i64)?;
        let text = body.get("text").and_then(Value::as_str)?;
        // Records published during a turn are sequenced with it, so a snapshot
        // that already includes this update is not rolled back by a late event.
        if let (Some(turn), Some(sequence)) = (
            body.get("turn_id").and_then(Value::as_u64),
            body.get("turn_sequence").and_then(Value::as_u64),
        ) && body.get("turn_epoch").and_then(Value::as_str) == Some(self.turn_epoch.as_str())
        {
            if turn < self.turn_watermark_id
                || (turn == self.turn_watermark_id && sequence <= self.turn_watermark_sequence)
            {
                return None;
            }
            self.turn_watermark_id = turn;
            self.turn_watermark_sequence = sequence;
        }
        if let Some(index) = self
            .messages
            .iter()
            .position(|message| message.id == Some(id))
        {
            self.messages[index] = Message::new(Some(id), "tool", text, None);
            return Some(index);
        }
        let index = self
            .live_items
            .iter()
            .position(|message| message.id == Some(id))?;
        self.live_items[index] = Message::new(Some(id), "tool", text, None);
        Some(self.messages.len() + index)
    }

    fn prepare_terminal_activity_version(
        &mut self,
        epoch: Option<&str>,
        revision: Option<u64>,
    ) -> Option<u64> {
        let (Some(epoch), Some(revision)) = (epoch, revision) else {
            if self.terminal_activity_epoch.take().is_some() {
                for chat in &mut self.chats {
                    chat.terminal_working = false;
                }
            }
            self.terminal_activity_by_chat.clear();
            return None;
        };
        if self.terminal_activity_epoch.as_deref() != Some(epoch) {
            self.terminal_activity_epoch = Some(epoch.to_owned());
            self.terminal_activity_by_chat.clear();
            for chat in &mut self.chats {
                chat.terminal_working = false;
            }
        }
        Some(revision)
    }

    pub fn selected_summary(&self) -> Option<&ChatSummary> {
        let selected = self.selected_chat.as_deref()?;
        self.chats.iter().find(|chat| chat.id == selected)
    }

    pub fn start_working(&mut self) {
        self.working = true;
        self.resumable = false;
        self.working_started_at.get_or_insert_with(Instant::now);
    }

    pub fn stop_working(&mut self) {
        self.working = false;
        // Only the host knows how the turn ended; the chat refetch that follows says.
        self.resumable = false;
        self.working_started_at = None;
    }

    /// Moves a queued message to the front, leaving the others in order.
    pub fn promote_queued(&mut self, index: usize) {
        if index < self.queue.len() {
            self.queue[..=index].rotate_right(1);
        }
    }

    pub fn working_for(&self) -> Option<u64> {
        self.working.then(|| {
            self.working_started_at
                .map_or(0, |started_at| started_at.elapsed().as_secs())
        })
    }

    pub fn display_messages(&self) -> Vec<Message> {
        let mut messages = self.messages.clone();
        messages.extend(self.live_items.iter().cloned());
        if !self.live_text.is_empty() {
            messages.push(Message::new(
                None,
                "assistant",
                self.live_text.clone(),
                self.selected_summary().map(|chat| chat.backend.clone()),
            ));
        }
        messages
    }

    /// What a live reply is labelled: the turn's label, or its assistant.
    pub fn live_label(&self) -> Option<String> {
        self.turn_label
            .clone()
            .or_else(|| self.selected_summary().map(|chat| chat.backend.clone()))
    }

    pub fn display_message_count(&self) -> usize {
        self.messages.len() + self.live_items.len() + usize::from(!self.live_text.is_empty())
    }

    pub fn apply_draft_snapshot(&mut self, body: &Value) {
        self.apply_draft(body);
    }

    fn apply_draft(&mut self, body: &Value) {
        let Some(revision) = body.get("draft_revision").and_then(Value::as_i64) else {
            return;
        };
        if revision <= self.draft_revision {
            return;
        }
        let Some(text) = body.get("draft").and_then(Value::as_str) else {
            return;
        };
        self.draft_revision = revision;
        self.draft = text.to_owned();
        if let Some(attachments) = draft_attachments(body) {
            self.draft_attachments = attachments;
        }
    }

    fn event_is_active(&self, body: &Value) -> bool {
        body.get("chat").and_then(Value::as_str) == self.selected_chat.as_deref()
    }

    pub fn demo() -> Self {
        let mut model = Self {
            settlement_supported: true,
            folder_reorder_supported: true,
            folder_groups_supported: true,
            folders: vec![Folder {
                id: "workspace-xd".into(),
                name: "xd".into(),
                parent: None,
                owner: None,
                group: false,
            }],
            chats: vec![
                ChatSummary {
                    id: "chat-gpui".into(),
                    folder: "workspace-xd".into(),
                    title: Some("Rewrite desktop with GPUI".into()),
                    backend: "codex".into(),
                    branch: Some("main".into()),
                    working: true,
                    terminal_working: false,
                    background_tasks: 0,
                    settled_at: None,
                    auto_settle_disabled: false,
                },
                ChatSummary {
                    id: "chat-scroll".into(),
                    folder: "workspace-xd".into(),
                    title: Some("Smooth transcript scrolling".into()),
                    backend: "codex".into(),
                    branch: Some("xd/smooth-transcript-scrolling".into()),
                    working: false,
                    terminal_working: false,
                    background_tasks: 0,
                    settled_at: None,
                    auto_settle_disabled: false,
                },
            ],
            sessions: Vec::new(),
            terminal_activity_epoch: None,
            terminal_activity_by_chat: HashMap::new(),
            selected_chat: Some("chat-gpui".into()),
            chat_visits: 0,
            unread_chats: HashSet::new(),
            waiting_chats: HashMap::new(),
            messages: vec![
                Message::new(
                    Some(1),
                    "user",
                    "Rewrite xd's desktop UI using GPUI, but keep the host.",
                    None,
                ),
                Message::new(
                    Some(2),
                    "assistant",
                    "The new frontend is connected through the existing JSON Lines protocol. Transcript rows are virtualized from the first milestone.",
                    Some("Codex".into()),
                ),
                Message::new(Some(3), "tool", "Building the Rust desktop shell", None),
            ],
            queue: vec!["Port workspace and chat shortcuts".into()],
            resumable: false,
            working: true,
            working_started_at: Some(Instant::now()),
            turn_label: None,
            new_worktree: false,
            has_messages: true,
            worktrees: Vec::new(),
            agent_backends: Vec::new(),
            backend: "codex".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: "high".into(),
            access: "edit".into(),
            plan: false,
            fast: false,
            context_used: 16_948,
            context_window: 272_000,
            auth_state: "signed-in".into(),
            workdir: None,
            commands: vec!["review".into(), "compact".into()],
            global_shortcuts: Vec::new(),
            workspace_shortcuts: Vec::new(),
            shortcuts: vec!["Review the current diff".into()],
            connected: true,
            connection_error: None,
            draft: String::new(),
            draft_attachments: Vec::new(),
            draft_revision: 0,
            live_text: String::new(),
            live_items: Vec::new(),
            todos: Vec::new(),
            interactions: Vec::new(),
            interaction_revision: 0,
            interaction_epoch: String::new(),
            turn_epoch: String::new(),
            turn_watermark_id: 0,
            turn_watermark_sequence: 0,
        };
        model.sessions = model
            .chats
            .iter()
            .map(|chat| SessionSummary {
                id: chat.id.clone(),
                folder: chat.folder.clone(),
                title: chat.title.clone().unwrap_or_else(|| "New Session".into()),
                chats: vec![chat.id.clone()],
                branch: chat.branch.clone(),
                working: chat.working,
                terminal_working: chat.terminal_working,
                background_tasks: chat.background_tasks,
                terminal_chat: chat.id.clone(),
            })
            .collect();
        model
    }
}

fn todo_snapshot(content: &str) -> Option<Vec<TodoItem>> {
    serde_json::from_str(content.strip_prefix("todo_list\n")?).ok()
}

fn string_array(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

/// The text of a live row that is a message the user sent into the running
/// turn. The host stores it as an ordinary user message, so this only matters
/// until the transcript is next loaded.
fn steered_text(text: &str) -> Option<&str> {
    text.strip_prefix("steer\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_tree_says_which_sessions_settled_and_whether_the_host_can_settle() {
        let mut model = AppModel::default();
        model.apply_tree(&grouped_tree()).unwrap();
        assert!(!model.settlement_supported, "an older host leaves it out");
        assert_eq!(model.chats[0].settled_at, None);
        assert!(!model.chats[0].auto_settle_disabled);

        let mut tree = grouped_tree();
        tree["settlement"] = json!(true);
        tree["chats"][0]["settled_at"] = json!(1234);
        tree["chats"][0]["auto_settle_disabled"] = json!(true);
        model.apply_tree(&tree).unwrap();
        assert!(model.settlement_supported);
        assert_eq!(model.chats[0].settled_at, Some(1234));
        assert!(model.chats[0].auto_settle_disabled);
        assert_eq!(model.chats[1].settled_at, None);
    }

    #[test]
    fn the_tree_says_whether_the_host_can_reorder_folders_safely() {
        let mut model = AppModel::default();
        model.apply_tree(&grouped_tree()).unwrap();
        assert!(
            !model.folder_reorder_supported,
            "an older host leaves it out"
        );

        let mut tree = grouped_tree();
        tree["folder_reorder"] = json!(true);
        model.apply_tree(&tree).unwrap();
        assert!(model.folder_reorder_supported);
    }

    #[test]
    fn the_tree_says_whether_the_host_keeps_folders_of_projects() {
        let mut model = AppModel::default();
        model.apply_tree(&grouped_tree()).unwrap();
        assert!(
            !model.folder_groups_supported,
            "an older host leaves it out and would make a project instead"
        );

        let mut tree = grouped_tree();
        tree["folder_groups"] = json!(true);
        model.apply_tree(&tree).unwrap();
        assert!(model.folder_groups_supported);
    }

    #[test]
    fn auto_delete_rules_read_the_host_and_step_through_the_choices() {
        assert_eq!(
            AutoDeleteRules::from_value(&json!({"ok":true,"after_days":null})),
            Some(AutoDeleteRules { after_days: None })
        );
        assert_eq!(
            AutoDeleteRules::from_value(&json!({"after_days":45})),
            Some(AutoDeleteRules {
                after_days: Some(45)
            })
        );
        assert_eq!(AutoDeleteRules::from_value(&json!({"ok":false})), None);
        assert_eq!(AutoDeleteRules::step(30, true), Some(60));
        assert_eq!(AutoDeleteRules::step(30, false), Some(14));
        assert_eq!(AutoDeleteRules::step(45, true), Some(60));
        assert_eq!(AutoDeleteRules::step(45, false), Some(30));
        assert_eq!(AutoDeleteRules::step(365, true), None);
        assert_eq!(AutoDeleteRules::step(1, false), None);
    }

    #[test]
    fn auto_settle_rules_need_every_member() {
        assert_eq!(
            AutoSettleRules::from_value(
                &json!({"ok":true,"after_days":null,"on_merge":false,"keep_pull_requests":true})
            ),
            Some(AutoSettleRules {
                after_days: None,
                on_merge: false,
                keep_pull_requests: true,
            })
        );
        assert_eq!(
            AutoSettleRules::from_value(
                &json!({"after_days":7,"on_merge":true,"keep_pull_requests":false})
            )
            .and_then(|rules| rules.after_days),
            Some(7)
        );
        assert_eq!(AutoSettleRules::from_value(&json!({"ok":false})), None);
        assert_eq!(
            AutoSettleRules::from_value(&json!({"on_merge":true,"keep_pull_requests":false})),
            None
        );
    }

    fn grouped_tree() -> Value {
        json!({
            "folders":[{"id":"project","name":"Project"}],
            "chats":[
                {"id":"codex-chat","folder":"project","title":"Codex tab","backend":"codex","session":"shared"},
                {"id":"claude-chat","folder":"project","title":"Claude tab","backend":"claude","session":"shared"}
            ],
            "sessions":[{"id":"shared","folder":"project","title":"Workspace task","chats":["codex-chat","claude-chat"],"terminal_chat":"session:shared"}],
            "terminal_activity_epoch":"daemon","terminal_activity_revision":0
        })
    }

    #[test]
    fn shared_workspace_membership_preserves_independent_chat_selection_and_approvals() {
        let mut model = AppModel::default();
        model.apply_tree(&grouped_tree()).unwrap();
        assert_eq!(model.session_key_for_chat("codex-chat"), "shared");
        assert_eq!(model.session_key_for_chat("claude-chat"), "shared");
        assert_eq!(model.terminal_key_for_chat("claude-chat"), "session:shared");
        assert_eq!(model.sessions[0].title, "Workspace task");
        model.select_chat("codex-chat");
        model.apply_event("interaction", &json!({"chat":"claude-chat","request_id":"approval","interaction_epoch":"epoch","interaction_revision":1}));
        assert!(model.is_waiting("claude-chat"));
        assert!(!model.is_waiting("codex-chat"));
        assert_eq!(model.selected_chat.as_deref(), Some("codex-chat"));
        let mut updated = grouped_tree();
        updated["chats"].as_array_mut().unwrap().remove(0);
        updated["sessions"][0]["chats"] = json!(["claude-chat"]);
        model.apply_tree(&updated).unwrap();
        assert_eq!(model.session_key_for_chat("claude-chat"), "shared");
        assert_eq!(model.sessions[0].title, "Workspace task");
        assert_eq!(model.terminal_key_for_chat("claude-chat"), "session:shared");
        model.apply_event(
            "terminal-activity",
            &json!({"chat":"shared","terminal_working":true,
            "terminal_activity_epoch":"daemon","terminal_activity_revision":1}),
        );
        assert!(model.chats[0].terminal_working);
        assert!(model.sessions[0].terminal_working);
        assert_eq!(model.canonical_terminal_key("global:tools"), "global:tools");
    }

    #[test]
    fn legacy_trees_normalize_to_single_chat_sessions_with_legacy_terminal_owners() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({"folders":[],"chats":[
                {"id":"legacy","folder":"project","title":"Old task","backend":"codex"},
                {"id":"untitled","folder":"project","title":null,"backend":"claude"}
            ]}))
            .unwrap();
        assert_eq!(model.sessions.len(), 2);
        assert_eq!(model.sessions[0].title, "Old task");
        assert_eq!(model.sessions[1].title, "New Session");
        assert_eq!(model.session_key_for_chat("legacy"), "legacy");
        assert_eq!(model.terminal_key_for_chat("legacy"), "legacy");
        assert_eq!(model.session_key_for_chat("unlisted"), "unlisted");
        assert_eq!(model.terminal_key_for_chat("unlisted"), "unlisted");
    }

    #[test]
    fn a_chat_is_resumable_only_while_the_host_says_so_and_nothing_runs() {
        let mut model = AppModel::default();
        model.select_chat("chat-1");
        model.apply_chat(&json!({"working": false, "resumable": true}));
        assert!(model.resumable);

        // An older host does not report it.
        model.apply_chat(&json!({"working": false}));
        assert!(!model.resumable);

        model.apply_chat(&json!({"working": true, "resumable": true}));
        assert!(!model.resumable);

        model.apply_chat(&json!({"working": false, "resumable": true}));
        model.start_working();
        assert!(!model.resumable);

        model.apply_chat(&json!({"working": false, "resumable": true}));
        model.stop_working();
        assert!(!model.resumable);

        model.apply_chat(&json!({"working": false, "resumable": true}));
        model.select_chat("chat-2");
        assert!(!model.resumable);
    }

    #[test]
    fn session_activity_aggregates_agents_and_orders_shared_terminal_events_against_tree_snapshots()
    {
        let mut model = AppModel::default();
        let tree = grouped_tree();
        model.apply_tree(&tree).unwrap();
        model.select_chat("codex-chat");
        model.apply_chat(&json!({"working":true}));
        assert!(model.sessions[0].working);
        model.apply_chat(&json!({"working":false}));
        assert!(!model.sessions[0].working);
        model.apply_event("turn-started", &json!({"chat":"claude-chat"}));
        assert!(model.sessions[0].working);
        assert!(!model.chats[0].working);
        model.apply_event("turn-finished", &json!({"chat":"claude-chat"}));
        assert!(!model.sessions[0].working);
        model.apply_event(
            "terminal-activity",
            &json!({"chat":"session:shared","terminal_working":true,
            "terminal_activity_epoch":"daemon","terminal_activity_revision":2}),
        );
        assert!(model.chats.iter().all(|chat| chat.terminal_working));
        assert!(model.sessions[0].terminal_working);
        model.apply_tree(&tree).unwrap();
        assert!(model.chats.iter().all(|chat| chat.terminal_working));
        assert!(model.sessions[0].terminal_working);
        model.unread_chats.clear();
        model.apply_event(
            "terminal-activity",
            &json!({"chat":"session:shared","terminal_working":false,
            "terminal_activity_epoch":"daemon","terminal_activity_revision":3}),
        );
        assert!(model.chats.iter().all(|chat| !chat.terminal_working));
        assert!(!model.sessions[0].terminal_working);
        assert!(model.unread_chats.is_empty());
        model.apply_event(
            "terminal-activity",
            &json!({"chat":"session:shared","terminal_working":true,
            "terminal_activity_epoch":"daemon","terminal_activity_revision":2}),
        );
        assert!(!model.sessions[0].terminal_working);
        let mut newer_tree = tree.clone();
        newer_tree["terminal_activity_revision"] = json!(4);
        model.apply_tree(&newer_tree).unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({"chat":"session:shared","terminal_working":true,
            "terminal_activity_epoch":"daemon","terminal_activity_revision":3}),
        );
        assert!(!model.sessions[0].terminal_working);
    }

    #[test]
    fn repeated_local_terminal_output_preserves_the_next_daemon_activity_revision() {
        let mut model = AppModel::default();
        model.apply_tree(&grouped_tree()).unwrap();
        model.apply_desktop_terminal_activity("codex-chat", true);
        for owner in ["session:shared", "shared", "claude-chat"] {
            model.apply_desktop_terminal_activity(owner, true);
        }
        assert_eq!(
            model.terminal_activity_by_chat["session:shared"].revision,
            1
        );
        model.apply_event(
            "terminal-activity",
            &json!({"chat":"session:shared","terminal_working":false,
            "terminal_activity_epoch":"daemon","terminal_activity_revision":2}),
        );
        assert!(model.chats.iter().all(|chat| !chat.terminal_working));
        assert!(!model.sessions[0].terminal_working);
    }

    #[test]
    fn chat_snapshots_restore_the_live_turn_age() {
        let mut model = AppModel::default();
        model.apply_chat(&json!({"working": true, "working_for": 41}));

        assert!(matches!(model.working_for(), Some(41..=42)));

        model.selected_chat = Some("chat-1".into());
        model.apply_event("turn-finished", &json!({"chat":"chat-1"}));
        assert_eq!(model.working_for(), None);
    }

    #[test]
    fn live_snapshots_deduplicate_events_and_restart_the_watermark_with_the_daemon() {
        let mut model = AppModel::default();
        model.select_chat("chat");
        model.apply_chat(&json!({"working":true,"turn_epoch":"daemon-a","turn_id":4,"turn_sequence":2,"segment":"after","items":[{"tool":false,"text":"before"},{"tool":true,"text":"command"}]}));
        model.apply_event("text",&json!({"chat":"chat","turn_epoch":"daemon-a","turn_id":4,"turn_sequence":2,"text":"duplicate"}));
        assert_eq!(model.live_text, "after");
        assert_eq!(model.live_items.len(), 2);
        model.apply_event("text",&json!({"chat":"chat","turn_epoch":"daemon-a","turn_id":4,"turn_sequence":3,"text":" newer"}));
        model.apply_chat(&json!({"working":true,"turn_epoch":"daemon-a","turn_id":4,"turn_sequence":2,"segment":"stale","items":[]}));
        assert_eq!(model.live_text, "after newer");
        model.apply_event(
            "turn-started",
            &json!({"chat":"chat","turn_epoch":"daemon-b","turn_id":1,"turn_sequence":0}),
        );
        assert!(model.live_items.is_empty());
        assert!(model.live_text.is_empty());
        assert_eq!(model.turn_watermark_id, 1);
    }

    #[test]
    fn records_replace_their_row_in_place_whether_live_or_stored() {
        let mut model = AppModel::default();
        model.select_chat("chat");
        model.apply_chat(&json!({"working":true,"turn_epoch":"d","turn_id":2,"turn_sequence":1,"segment":"","items":[{"tool":true,"text":"subagent_run\n{\"status\":\"running\"}","record":"toolu_1","message_id":7}]}));
        let row = model.apply_record(&json!({"chat":"chat","turn_epoch":"d","turn_id":2,"turn_sequence":2,"message_id":7,"text":"subagent_run\n{\"status\":\"completed\"}"}));
        assert_eq!(row, Some(model.messages.len()));
        assert!(model.live_items[0].content.contains("completed"));
        // A late copy of an older update cannot roll the card back.
        assert_eq!(
            model.apply_record(&json!({"chat":"chat","turn_epoch":"d","turn_id":2,"turn_sequence":1,"message_id":7,"text":"old"})),
            None
        );
        assert!(model.live_items[0].content.contains("completed"));

        model.messages = vec![Message::new(Some(3), "tool", "subagent_run\n{}", None)];
        assert_eq!(
            model.apply_record(
                &json!({"chat":"chat","message_id":3,"text":"subagent_run\n{\"summary\":\"done\"}"})
            ),
            Some(0)
        );
        assert!(model.messages[0].content.contains("done"));
        assert_eq!(
            model.apply_record(&json!({"chat":"other","message_id":3,"text":"x"})),
            None
        );
    }

    #[test]
    fn interactions_flag_their_chat_as_waiting_until_resolved() {
        let mut model = AppModel::default();
        model.apply_event(
            "interaction",
            &json!({"chat":"background","request_id":"a","interaction_epoch":"e","interaction_revision":1}),
        );
        assert!(model.is_waiting("background"));
        assert!(!model.is_waiting("other"));
        model.apply_event(
            "interaction-resolved",
            &json!({"chat":"background","request_id":"a","interaction_epoch":"e","interaction_revision":2}),
        );
        assert!(!model.is_waiting("background"));
        model.apply_event(
            "interaction",
            &json!({"chat":"background","request_id":"b","interaction_epoch":"e","interaction_revision":3}),
        );
        model.apply_event(
            "interactions-cleared",
            &json!({"chat":"background","interaction_epoch":"e","interaction_revision":4}),
        );
        assert!(!model.is_waiting("background"));
    }

    #[test]
    fn resolved_interactions_are_not_resurrected_by_an_older_snapshot() {
        let mut model = AppModel::default();
        model.select_chat("chat");
        model.apply_event("interaction",&json!({"chat":"chat","interaction_epoch":"daemon-a","interaction_revision":2,"request_id":"request","method":"approval","params":{}}));
        model.apply_event("interaction-resolved",&json!({"chat":"chat","interaction_epoch":"daemon-a","interaction_revision":3,"request_id":"request"}));
        model.apply_chat(&json!({"working":false,"interaction_epoch":"daemon-a","interaction_revision":2,"interactions":[{"request_id":"request"}]}));
        assert!(model.interactions.is_empty());
        model.apply_chat(&json!({"working":false,"interaction_epoch":"daemon-b","interaction_revision":0,"interactions":[]}));
        assert_eq!(model.interaction_revision, 0);
    }

    #[test]
    fn chat_snapshots_restore_in_progress_text_without_replacing_newer_events() {
        let mut model = AppModel::default();
        model.apply_chat(&json!({
            "working": true,
            "segment": "Claude already streamed this"
        }));
        assert_eq!(model.live_text, "Claude already streamed this");

        model.selected_chat = Some("chat-1".into());
        model.apply_event(
            "text",
            &json!({"chat":"chat-1", "text":" and this is newer"}),
        );
        model.apply_chat(&json!({
            "working": true,
            "segment": "stale snapshot"
        }));
        assert_eq!(
            model.live_text,
            "Claude already streamed this and this is newer"
        );
    }

    #[test]
    fn tree_folders_carry_the_account_behind_them() {
        let mut model = AppModel::default();

        model
            .apply_tree(&json!({
                "folders": [
                    {"id": "owned", "name": "xd", "owner": "RestartFU"},
                    {"id": "plain", "name": "Notes"},
                    {"id": "org", "name": "acme", "owner": "acme", "group": true}
                ],
                "chats": []
            }))
            .unwrap();

        assert_eq!(model.folders[0].owner.as_deref(), Some("RestartFU"));
        assert_eq!(model.folders[1].owner, None);
        assert!(!model.folders[0].group && !model.folders[1].group);
        assert!(model.folders[2].group);
    }

    #[test]
    fn owner_avatars_decode_only_the_formats_github_serves() {
        // A one-pixel PNG: enough to decode, nothing to look at.
        const AVATAR: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==";

        assert!(owner_icon("image/png", AVATAR).is_some());
        assert!(owner_icon("image/jpeg", AVATAR).is_some());
        assert!(owner_icon("image/svg+xml", AVATAR).is_none());
        assert!(owner_icon("image/png", "not base64 ~~").is_none());
        assert!(owner_icon("image/png", "").is_none());
    }

    #[test]
    fn chat_snapshots_keep_the_selected_sidebar_summary_in_sync() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id": "folder-1", "name": "xd"}],
                "chats": [{
                    "id": "chat-1", "folder": "folder-1", "title": "GPUI",
                    "backend": "codex", "working": true
                }]
            }))
            .unwrap();
        model.select_chat("chat-1");

        model.apply_chat(&json!({"working": false}));

        assert!(!model.working);
        assert!(!model.selected_summary().unwrap().working);
    }

    #[test]
    fn applies_complete_tree_snapshots_and_retires_deleted_selection() {
        let mut model = AppModel {
            selected_chat: Some("deleted".into()),
            messages: vec![Message::new(Some(1), "user", "stale", None)],
            ..Default::default()
        };

        model
            .apply_tree(&json!({
                "folders": [{"id":"folder-1", "name":"xd"}],
                "chats": [{
                    "id":"chat-1", "folder":"folder-1", "title":"GPUI",
                    "backend":"codex", "working":false
                }]
            }))
            .unwrap();

        assert!(model.connected);
        assert_eq!(model.selected_chat, None);
        assert!(model.messages.is_empty());
    }

    #[test]
    fn tree_snapshots_preserve_each_chats_branch() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id":"folder-1", "name":"xd"}],
                "chats": [{
                    "id":"chat-1", "folder":"folder-1", "title":"GPUI",
                    "backend":"codex", "working":false,
                    "branch":"session/scheming-hawk-jhgk"
                }]
            }))
            .unwrap();

        let serialized = serde_json::to_value(&model.chats[0]).unwrap();
        assert_eq!(serialized["branch"], "session/scheming-hawk-jhgk");
    }

    #[test]
    fn queued_events_update_only_the_active_chat_queue() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            queue: vec!["old".into()],
            ..Default::default()
        };

        model.apply_event("queued", &json!({"chat":"chat-2", "queue":["wrong chat"]}));
        assert_eq!(model.queue, ["old"]);

        model.apply_event(
            "queued",
            &json!({"chat":"chat-1", "queue":["first", "second"]}),
        );
        assert_eq!(model.queue, ["first", "second"]);
    }

    #[test]
    fn promoting_a_queued_message_keeps_the_others_in_order() {
        let mut model = AppModel {
            queue: vec!["first".into(), "second".into(), "third".into()],
            ..Default::default()
        };

        model.promote_queued(2);
        assert_eq!(model.queue, ["third", "first", "second"]);

        model.promote_queued(0);
        assert_eq!(model.queue, ["third", "first", "second"]);

        model.promote_queued(3);
        assert_eq!(model.queue, ["third", "first", "second"]);
    }

    #[test]
    fn command_snapshots_and_events_stay_chat_scoped() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_chat(&json!({"commands":["review", "compact"]}));
        assert_eq!(model.commands, ["review", "compact"]);
        model.apply_event("commands", &json!({"chat":"chat-2", "commands":["wrong"]}));
        assert_eq!(model.commands, ["review", "compact"]);
        model.apply_event("commands", &json!({"chat":"chat-1", "commands":["rename"]}));
        assert_eq!(model.commands, ["rename"]);
    }

    #[test]
    fn turn_lifecycle_is_reduced_without_refetching_transcript_state() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_event("turn-started", &json!({"chat":"chat-1"}));
        assert!(model.working);
        model.apply_event("turn-finished", &json!({"chat":"chat-1"}));
        assert!(!model.working);
    }

    #[test]
    fn turn_lifecycle_updates_the_background_chat_summary() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {
                        "id": "selected",
                        "folder": "folder",
                        "title": "Selected",
                        "backend": "codex"
                    },
                    {
                        "id": "background",
                        "folder": "folder",
                        "title": "Background",
                        "backend": "codex"
                    }
                ]
            }))
            .unwrap();
        model.select_chat("selected");

        model.apply_event("turn-started", &json!({"chat": "background"}));

        assert!(model.chats[1].working);
        assert!(!model.working);

        model.apply_event("turn-finished", &json!({"chat": "background"}));

        assert!(!model.chats[1].working);
        assert!(model.unread_chats.contains("background"));
    }

    #[test]
    fn a_chat_waits_on_its_background_tasks_only_between_turns() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {"id": "a", "folder": "folder", "backend": "claude", "session": "s",
                     "background_tasks": 2},
                    {"id": "b", "folder": "folder", "backend": "claude", "session": "s"}
                ],
                "sessions": [{"id": "s", "folder": "folder", "title": "Work",
                              "chats": ["a", "b"], "background_tasks": 2}]
            }))
            .unwrap();
        let waiting = |model: &AppModel| -> Vec<u32> {
            model.chats.iter().map(ChatSummary::waiting_on).collect()
        };
        assert_eq!(waiting(&model), [2, 0]);
        assert_eq!(model.sessions[0].background_tasks, 2);

        model.apply_event("turn-started", &json!({"chat": "a"}));
        assert_eq!(waiting(&model), [0, 0], "a running turn is work");
        model.apply_event(
            "background-tasks",
            &json!({"chat": "b", "background_tasks": 1}),
        );
        assert_eq!(model.sessions[0].background_tasks, 3);
        model.apply_event("turn-finished", &json!({"chat": "a"}));
        assert_eq!(waiting(&model), [2, 1]);

        for chat in ["a", "b"] {
            model.apply_event(
                "background-tasks",
                &json!({"chat": chat, "background_tasks": 0}),
            );
        }
        assert_eq!(waiting(&model), [0, 0]);
        assert_eq!(model.sessions[0].background_tasks, 0);
    }

    #[test]
    fn terminal_activity_updates_only_the_matching_chat_summary() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {
                        "id": "selected",
                        "folder": "folder",
                        "title": "Selected",
                        "backend": "codex",
                        "working": true
                    },
                    {
                        "id": "background",
                        "folder": "folder",
                        "title": "Background",
                        "backend": "claude"
                    }
                ]
            }))
            .unwrap();
        model.select_chat("selected");
        model.working = true;

        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "background",
                "working": false,
                "terminal_working": true
            }),
        );

        assert!(model.chats[1].terminal_working);
        assert!(!model.chats[1].working);
        assert!(!model.chats[0].terminal_working);
        assert!(model.chats[0].working);
        assert!(model.working);

        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "background",
                "working": true,
                "terminal_working": false
            }),
        );

        assert!(!model.chats[1].terminal_working);
        assert!(model.working);
    }

    #[test]
    fn a_message_sent_into_a_running_turn_reads_as_the_users_own() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_event("text", &json!({"chat":"chat-1", "text":"Working"}));
        model.apply_event(
            "tool",
            &json!({"chat":"chat-1", "text":"steer\nDo this instead"}),
        );
        let rows = model
            .live_items
            .iter()
            .map(|item| (item.role.as_str(), item.content.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            [("assistant", "Working"), ("user", "Do this instead")]
        );

        // And the same for a client that opens the chat mid-turn.
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_chat(
            &json!({"working":true, "turn_epoch":"d", "turn_id":1, "turn_sequence":1,
            "items":[{"tool":true, "text":"steer\nDo this instead"}]}),
        );
        assert_eq!(model.live_items[0].role, "user");
        assert_eq!(model.live_items[0].content, "Do this instead");
    }

    #[test]
    fn background_terminal_completion_stays_unread_until_selected() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {"id": "selected", "folder": "folder", "backend": "codex"},
                    {"id": "background", "folder": "folder", "backend": "claude"}
                ]
            }))
            .unwrap();
        model.select_chat("selected");

        model.apply_event(
            "terminal-activity",
            &json!({"chat": "background", "terminal_working": true}),
        );
        assert!(!model.unread_chats.contains("background"));

        model.apply_event(
            "terminal-activity",
            &json!({"chat": "background", "terminal_working": false}),
        );
        assert!(model.unread_chats.contains("background"));

        model.select_chat("background");
        assert!(!model.unread_chats.contains("background"));
    }

    #[test]
    fn stale_tree_preserves_newer_terminal_activity_in_the_same_epoch() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 1,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": false
                }]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "working": true,
                "terminal_working": true,
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 2
            }),
        );

        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 1,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": false
                }]
            }))
            .unwrap();

        assert!(model.chats[0].terminal_working);

        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 2,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": false
                }]
            }))
            .unwrap();

        assert!(!model.chats[0].terminal_working);
    }

    #[test]
    fn desktop_runtime_activity_survives_an_unchanged_host_tree() {
        let mut model = AppModel::default();
        model
            .apply_tree(&serde_json::json!({
                "folders": [{"id": "folder-1", "name": "Workspace"}],
                "chats": [{
                    "id": "chat-1",
                    "folder": "folder-1",
                    "title": "Chat",
                    "backend": "jcode",
                    "terminal_working": false
                }],
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 4
            }))
            .unwrap();

        model.apply_desktop_terminal_activity("chat-1", true);
        model
            .apply_tree(&serde_json::json!({
                "folders": [{"id": "folder-1", "name": "Workspace"}],
                "chats": [{
                    "id": "chat-1",
                    "folder": "folder-1",
                    "title": "Chat",
                    "backend": "jcode",
                    "terminal_working": false
                }],
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 4
            }))
            .unwrap();

        assert!(model.chats[0].terminal_working);
    }

    #[test]
    fn older_terminal_activity_events_are_ignored_in_the_same_epoch() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 0,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{"id": "chat", "folder": "folder", "backend": "codex"}]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": true,
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 2
            }),
        );
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": false,
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 1
            }),
        );

        assert!(model.chats[0].terminal_working);

        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": false,
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 2
            }),
        );
        assert!(!model.chats[0].terminal_working);
    }

    #[test]
    fn terminal_activity_event_revisions_are_scoped_per_chat() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 0,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {"id": "chat-a", "folder": "folder", "backend": "codex"},
                    {"id": "chat-b", "folder": "folder", "backend": "claude"}
                ]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat-a",
                "terminal_working": true,
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 2
            }),
        );
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat-b",
                "terminal_working": true,
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 1
            }),
        );

        assert!(model.chats[0].terminal_working);
        assert!(model.chats[1].terminal_working);
    }

    #[test]
    fn stale_tree_preserves_only_chats_with_newer_activity_events() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 0,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {"id": "chat-a", "folder": "folder", "backend": "codex"},
                    {"id": "chat-b", "folder": "folder", "backend": "claude"}
                ]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat-a",
                "terminal_working": true,
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 2
            }),
        );

        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 1,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {
                        "id": "chat-a",
                        "folder": "folder",
                        "backend": "codex",
                        "terminal_working": false
                    },
                    {
                        "id": "chat-b",
                        "folder": "folder",
                        "backend": "claude",
                        "terminal_working": true
                    }
                ]
            }))
            .unwrap();

        assert!(model.chats[0].terminal_working);
        assert!(model.chats[1].terminal_working);
    }

    #[test]
    fn terminal_activity_received_before_its_chat_row_survives_a_stale_tree() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 0,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": []
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": true,
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 2
            }),
        );

        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 1,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": false
                }]
            }))
            .unwrap();

        assert!(model.chats[0].terminal_working);
    }

    #[test]
    fn a_new_terminal_activity_epoch_accepts_a_lower_revision() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 10,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": true
                }]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": false,
                "terminal_activity_epoch": "host-b",
                "terminal_activity_revision": 1
            }),
        );
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": true,
                "terminal_activity_epoch": "host-b",
                "terminal_activity_revision": 0
            }),
        );

        assert!(!model.chats[0].terminal_working);
    }

    #[test]
    fn a_new_terminal_activity_epoch_clears_other_chats_before_its_tree() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 10,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [
                    {
                        "id": "chat-a",
                        "folder": "folder",
                        "backend": "codex",
                        "terminal_working": false
                    },
                    {
                        "id": "chat-b",
                        "folder": "folder",
                        "backend": "claude",
                        "terminal_working": true
                    }
                ]
            }))
            .unwrap();

        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat-a",
                "terminal_working": true,
                "terminal_activity_epoch": "host-b",
                "terminal_activity_revision": 1
            }),
        );

        assert!(model.chats[0].terminal_working);
        assert!(!model.chats[1].terminal_working);
    }

    #[test]
    fn unversioned_terminal_activity_resets_to_legacy_acceptance() {
        let mut model = AppModel::default();
        model
            .apply_tree(&json!({
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 10,
                "folders": [{"id": "folder", "name": "Workspace"}],
                "chats": [{
                    "id": "chat",
                    "folder": "folder",
                    "backend": "codex",
                    "terminal_working": true
                }]
            }))
            .unwrap();
        model.apply_event(
            "terminal-activity",
            &json!({"chat": "chat", "terminal_working": false}),
        );
        model.apply_event(
            "terminal-activity",
            &json!({
                "chat": "chat",
                "terminal_working": true,
                "terminal_activity_epoch": "host-a",
                "terminal_activity_revision": 1
            }),
        );

        assert!(model.chats[0].terminal_working);
    }

    #[test]
    fn background_turns_stay_unread_until_selected() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_event("turn-finished", &json!({"chat":"chat-2"}));
        assert!(model.unread_chats.contains("chat-2"));
        assert_eq!(model.selected_chat.as_deref(), Some("chat-1"));

        model.select_chat("chat-2");
        assert!(!model.unread_chats.contains("chat-2"));
    }

    #[test]
    fn chat_snapshots_control_first_message_worktree_selection() {
        let mut model = AppModel::default();
        model.apply_chat(&json!({
            "queue": [], "working": false, "new_worktree": true, "has_messages": false,
            "worktrees": [{
                "path": "/repo", "branch": "main", "detached": false,
                "main": true, "current": true
            }]
        }));
        assert!(model.new_worktree);
        assert!(!model.has_messages);
        assert_eq!(model.worktrees[0].branch.as_deref(), Some("main"));

        model.apply_chat(&json!({
            "queue": [], "working": false, "new_worktree": false, "has_messages": true
        }));
        assert!(!model.new_worktree);
        assert!(model.has_messages);
    }

    #[test]
    fn catalog_names_are_scoped_to_the_current_assistant() {
        let mut model = AppModel {
            backend: "codex".into(),
            model: Some("shared-id".into()),
            ..Default::default()
        };
        model.apply_agent_catalog(&json!({"backends":[
            {"id":"claude","name":"Claude Code","default_model":"shared-id","models":[{"id":"shared-id","name":"Other assistant model"}],"efforts":[]},
            {"id":"codex","name":"Codex","default_model":"shared-id","models":[{"id":"shared-id","name":"My team model"}],"efforts":["high"]}
        ]})).unwrap();
        assert_eq!(model.selected_model_label(), "My team model");
        model.model = Some("gpt-5.3-codex".into());
        assert_eq!(model.selected_model_label(), "GPT-5.3 Codex");
        assert_eq!(model.model.as_deref(), Some("gpt-5.3-codex"));
    }

    #[test]
    fn catalog_and_chat_snapshots_resolve_assistant_labels() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            chats: vec![ChatSummary {
                id: "chat-1".into(),
                folder: "folder-1".into(),
                title: None,
                backend: "codex".into(),
                branch: None,
                working: false,
                terminal_working: false,
                background_tasks: 0,
                settled_at: None,
                auto_settle_disabled: false,
            }],
            ..Default::default()
        };
        model
            .apply_agent_catalog(&json!({
                "backends": [{
                    "id": "claude", "name": "Claude Code",
                    "default_model": "claude-opus-5",
                    "models": [{"id": "claude-opus-5", "name": "Claude Opus 5"}],
                    "efforts": ["low", "high", "ultracode"]
                }]
            }))
            .unwrap();
        model.apply_chat(&json!({
            "backend": "claude", "model": "claude-opus-5", "effort": "high",
            "access": "edit", "plan": true, "fast": true,
            "queue": [], "working": false,
            "context_used": 21_335, "context_window": 1_000_000
        }));

        assert_eq!(model.backend, "claude");
        assert_eq!(model.selected_summary().unwrap().backend, "claude");
        assert_eq!(model.selected_model_name(), Some("Claude Opus 5"));
        assert_eq!(model.effort, "high");
        assert_eq!(model.access, "edit");
        assert_eq!(model.context_used, 21_335);
        assert_eq!(model.context_window, 1_000_000);
        assert!(model.plan);
        assert!(!model.fast);

        model.apply_chat(&json!({
            "backend": "codex", "model": "gpt-5.6-sol", "effort": "max",
            "access": "edit", "plan": false, "fast": true,
            "queue": [], "working": false
        }));
        assert!(model.fast);
        assert_eq!(model.context_used, 0);
        assert_eq!(model.context_window, 0);
    }

    #[test]
    fn shortcut_snapshots_keep_ownership_and_effective_order() {
        let mut model = AppModel::default();
        model.apply_shortcuts(&json!({
            "global": ["Review"],
            "workspace": ["Test"],
            "effective": ["Review", "Test"]
        }));
        assert_eq!(model.global_shortcuts, ["Review"]);
        assert_eq!(model.workspace_shortcuts, ["Test"]);
        assert_eq!(model.shortcuts, ["Review", "Test"]);

        model.apply_chat(&json!({
            "queue": [], "working": true, "shortcuts": ["Review", "Test", "Deploy"]
        }));
        assert_eq!(model.shortcuts, ["Review", "Test", "Deploy"]);
        assert_eq!(model.global_shortcuts, ["Review"]);
    }

    #[test]
    fn streaming_text_and_tools_preserve_wire_order_like_mobile() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_event("turn-started", &json!({"chat":"chat-1"}));
        model.apply_event("text", &json!({"chat":"chat-1", "text":"hello"}));
        model.apply_event("text", &json!({"chat":"chat-1", "text":" world"}));
        model.apply_event("tool", &json!({"chat":"chat-1", "text":"Read file"}));
        model.apply_event("text", &json!({"chat":"chat-1", "text":"done"}));

        let displayed = model.display_messages();
        assert_eq!(displayed.len(), 3);
        assert_eq!(
            displayed
                .iter()
                .map(|message| (message.role.as_str(), message.content.as_str()))
                .collect::<Vec<_>>(),
            [
                ("assistant", "hello world"),
                ("tool", "Read file"),
                ("assistant", "done"),
            ],
        );
    }

    #[test]
    fn draft_replies_do_not_replace_queue_or_turn_state() {
        let mut model = AppModel {
            queue: vec!["next".into()],
            working: true,
            draft_revision: 2,
            ..Default::default()
        };

        model.apply_draft_snapshot(&json!({"draft":"shared", "draft_revision":3}));

        assert_eq!(model.draft, "shared");
        assert_eq!(model.queue, ["next"]);
        assert!(model.working);
    }

    /// A PNG's signature and headers, describing a picture its bytes don't hold.
    const PNG_OF_8000_BY_8000: &str = "89504e470d0a1a0a0000000d4948445200001f4000001f40080600000006f1adf4000000004944415435af061e";
    const PNG_OF_8001_BY_8000: &str = "89504e470d0a1a0a0000000d4948445200001f4100001f400806000000e933c6ca000000004944415435af061e";

    fn hex_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn an_attachment_too_large_to_decode_has_no_size() {
        let largest = Attachment::from_png("a.png", hex_bytes(PNG_OF_8000_BY_8000)).unwrap();
        assert_eq!(largest.size, Some((8000, 8000)));

        let oversized = Attachment::from_png("b.png", hex_bytes(PNG_OF_8001_BY_8000)).unwrap();
        assert_eq!(oversized.size, None);
    }

    #[test]
    fn an_attachment_reads_its_size_from_the_png_header() {
        let encoded = encoded_png(7, 3, [10, 20, 30, 255]);
        let attachment = Attachment::from_value(&serde_json::json!({
            "name": "screen.png", "mime": "image/png", "data": encoded
        }))
        .unwrap();
        assert_eq!(attachment.size, Some((7, 3)));

        // Still attachable, as it always was, with nothing to size it by.
        let truncated = Attachment::from_png("broken.png", PNG_SIGNATURE.to_vec()).unwrap();
        assert_eq!(truncated.size, None);
    }

    #[test]
    fn synchronized_attachment_previews_replace_only_when_present() {
        let mut model = AppModel::default();
        model.apply_draft_snapshot(&json!({
            "draft": "look",
            "draft_revision": 1,
            "draft_attachments": [{
                "name": "screen.png",
                "mime": "image/png",
                "data": "iVBORw0KGgo="
            }]
        }));
        assert_eq!(model.draft_attachments.len(), 1);
        assert_eq!(model.draft_attachments[0].name, "screen.png");
        assert_eq!(
            model.draft_attachments[0].preview.as_ref().unwrap().bytes,
            PNG_SIGNATURE
        );

        model.apply_draft_snapshot(&json!({"draft":"typing", "draft_revision":2}));
        assert_eq!(model.draft_attachments.len(), 1);

        model.apply_draft_snapshot(&json!({
            "draft": "",
            "draft_revision": 3,
            "draft_attachments": []
        }));
        assert!(model.draft_attachments.is_empty());
    }

    #[test]
    fn persisted_image_references_are_hidden_from_user_text() {
        let (shown, paths, files) = message_content(
            "user",
            "before\n[image: /private/paste-one.png]\nafter\n[image: /private/paste-two.png]",
        );
        assert_eq!(paths, ["/private/paste-one.png", "/private/paste-two.png"]);
        assert!(files.is_empty());
        assert_eq!(shown, "before\nafter");

        let (shown, paths, files) =
            message_content("assistant", "Keep [image: /not/a/reference.png] literal");
        assert!(paths.is_empty() && files.is_empty());
        assert_eq!(shown, "Keep [image: /not/a/reference.png] literal");
    }

    #[test]
    fn attached_files_are_listed_apart_from_user_text() {
        let message = Message::new(
            Some(1),
            "user",
            "read these\r\n[file: /host/files/1/Quarterly Report.pdf]\n[file: ]\nsee [file: /x] here",
            None,
        );
        assert_eq!(message.file_paths(), ["/host/files/1/Quarterly Report.pdf"]);
        assert_eq!(
            attached_file_name(&message.file_paths()[0]),
            "Quarterly Report.pdf"
        );
        assert_eq!(attached_file_name("C:\\files\\a.txt"), "a.txt");
        let shown = message.markdown();
        assert_eq!(shown.blocks.len(), 1);
    }

    #[test]
    fn a_file_from_disk_is_an_image_only_when_it_is_a_png() {
        let png = Attachment::from_file("shot.png", PNG_SIGNATURE.to_vec());
        assert_eq!(png.mime, "image/png");
        assert!(png.preview.is_some());

        let renamed = Attachment::from_file("shot.png", b"%PDF-1.7".to_vec());
        assert_eq!(renamed.mime, "application/octet-stream");
        assert!(renamed.preview.is_none());

        let report = Attachment::from_file("Report.PDF", b"%PDF-1.7".to_vec());
        assert_eq!(report.mime, "application/pdf");
        assert_eq!(report.bytes, 8);
        assert_eq!(report.data, STANDARD.encode(b"%PDF-1.7"));
        assert_eq!(
            report.id,
            Attachment::from_file("Report.PDF", b"%PDF-1.7".to_vec()).id
        );

        let body = serde_json::json!({
            "draft_attachments": [{"name": "shot.png", "mime": "image/png", "data": png.data}],
            "draft_files": [{"name": "Report.PDF", "mime": "application/pdf", "data": report.data}],
        });
        let names = draft_attachments(&body)
            .unwrap()
            .into_iter()
            .map(|attachment| attachment.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["shot.png", "Report.PDF"]);
        assert!(draft_attachments(&serde_json::json!({"draft_files": []})).is_none());

        let synced = Attachment::from_value(&serde_json::json!({
            "name": "Report.PDF",
            "mime": "application/pdf",
            "data": report.data,
        }))
        .unwrap();
        assert_eq!(synced, report);
    }

    #[test]
    fn a_reply_shows_its_images_where_it_names_them() {
        let message = Message::new(
            Some(1),
            "assistant",
            "Before\n[image: /kept/a.png]\nAfter\n\n```\n[image: /in/a/fence.png]\n```",
            None,
        );
        assert_eq!(message.image_paths(), ["/kept/a.png"]);
        let document = message.markdown();
        assert!(matches!(document.blocks[0], markdown::Block::Paragraph(_)));
        assert_eq!(
            document.blocks[1],
            markdown::Block::Image("/kept/a.png".into())
        );
        assert!(matches!(document.blocks[2], markdown::Block::Paragraph(_)));
        assert!(matches!(document.blocks[3], markdown::Block::Code(_)));

        let streaming =
            Message::new_streaming(None, "assistant", "Look\n[image: /kept/b.png]", None);
        assert_eq!(streaming.image_paths(), ["/kept/b.png"]);
    }

    #[test]
    fn a_user_message_keeps_its_images_apart_from_its_text() {
        let message = Message::new(Some(1), "user", "```\nunclosed\n[image: /kept/a.png]", None);
        assert_eq!(message.image_paths(), ["/kept/a.png"]);
        assert!(
            message
                .markdown()
                .blocks
                .iter()
                .all(|block| !matches!(block, markdown::Block::Image(_)))
        );
    }

    /// A PNG of one color, as base64.
    fn encoded_png(width: u32, height: u32, color: [u8; 4]) -> String {
        use image::ImageEncoder;
        let pixels = color.repeat(width as usize * height as usize);
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        STANDARD.encode(png)
    }

    #[test]
    fn a_message_image_keeps_its_shape_and_is_never_enlarged() {
        let image = |width, height| MessageImage {
            image: Arc::new(RenderImage::new(vec![Frame::new(image::RgbaImage::new(
                1, 1,
            ))])),
            width,
            height,
        };
        assert_eq!(image(1920, 1080).fitted(960.0, 480.0), (853.3333, 480.0));
        assert_eq!(image(2000, 500).fitted(500.0, 480.0), (500.0, 125.0));
        assert_eq!(image(64, 32).fitted(780.0, 480.0), (64.0, 32.0));
        // Before the column has a width there is still something to measure.
        assert_eq!(image(64, 64).fitted(0.0, 0.0), (1.0, 1.0));
    }

    #[test]
    fn a_message_image_is_decoded_ready_for_the_renderer() {
        let shown =
            MessageImage::decode("image/png", &encoded_png(2, 1, [255, 10, 20, 255])).unwrap();
        assert_eq!((shown.width, shown.height), (2, 1));
        assert_eq!(shown.bytes(), 8);
        // The renderer takes BGRA.
        assert_eq!(&shown.image.as_bytes(0).unwrap()[..4], &[20, 10, 255, 255]);
    }

    #[test]
    fn a_large_image_keeps_its_size_for_layout_but_not_its_pixels() {
        let shown =
            MessageImage::decode("image/png", &encoded_png(3120, 960, [9, 9, 9, 255])).unwrap();
        assert_eq!((shown.width, shown.height), (3120, 960));
        assert_eq!(shown.bytes(), 1560 * 480 * 4);

        // A square one is limited by the height it can be drawn at.
        let shown =
            MessageImage::decode("image/png", &encoded_png(1920, 1920, [9, 9, 9, 255])).unwrap();
        assert_eq!(shown.bytes(), 960 * 960 * 4);
        assert!(shown.bytes() <= MAX_MESSAGE_IMAGE_BYTES);

        // One that already fits is left alone.
        let shown =
            MessageImage::decode("image/png", &encoded_png(1560, 960, [9, 9, 9, 255])).unwrap();
        assert_eq!(shown.bytes(), MAX_MESSAGE_IMAGE_BYTES);
    }

    #[test]
    fn only_the_first_frame_of_a_gif_is_decoded() {
        // Every frame held in memory would make one small file very expensive.
        let frame = |shade| {
            image::Frame::new(image::RgbaImage::from_pixel(
                4,
                4,
                image::Rgba([shade, shade, shade, 255]),
            ))
        };
        let mut gif = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut gif)
            .encode_frames([frame(10), frame(200)])
            .unwrap();
        let shown = MessageImage::decode("image/gif", &STANDARD.encode(gif)).unwrap();
        assert_eq!(shown.image.frame_count(), 1);
        assert_eq!(shown.bytes(), 4 * 4 * 4);
    }

    #[test]
    fn a_message_image_refuses_what_it_should_not_decode() {
        assert!(worth_decoding(8000, 8000));
        assert!(!worth_decoding(8001, 8000));
        assert!(!worth_decoding(0, 8000));
        // Multiplying these as 32 bits would wrap around to something small.
        assert!(!worth_decoding(u32::MAX, u32::MAX));
        assert!(MessageImage::decode("image/png", &STANDARD.encode(b"junk")).is_none());
        assert!(MessageImage::decode("image/png", "not base64 ~~").is_none());
        let png = encoded_png(1, 1, [0, 0, 0, 255]);
        assert!(MessageImage::decode("image/svg+xml", &png).is_none());
    }

    #[test]
    fn cursor_pages_keep_a_bounded_bidirectional_message_window() {
        let mut model = AppModel {
            messages: (121..=600)
                .map(|id| Message::new(Some(id), "assistant", format!("message {id}"), None))
                .collect(),
            ..Default::default()
        };
        let older = (1..=120)
            .map(|id| json!({"id": id, "role": "assistant", "content": format!("message {id}")}))
            .collect::<Vec<_>>();
        let change = model
            .apply_message_page(
                &json!({"messages": older, "has_older": false, "has_newer": true}),
                MessagePageDirection::Before,
            )
            .unwrap();
        assert_eq!(change.inserted_at_start, 120);
        assert_eq!(change.removed_from_end, 120);
        assert_eq!(model.messages.len(), MAX_RETAINED_MESSAGES);
        assert_eq!(
            model.messages.first().and_then(|message| message.id),
            Some(1)
        );
        assert_eq!(
            model.messages.last().and_then(|message| message.id),
            Some(480)
        );

        let newer = (481..=600)
            .map(|id| json!({"id": id, "role": "assistant", "content": format!("message {id}")}))
            .collect::<Vec<_>>();
        let change = model
            .apply_message_page(
                &json!({"messages": newer, "has_older": true, "has_newer": false}),
                MessagePageDirection::After,
            )
            .unwrap();
        assert_eq!(change.inserted_at_end, 120);
        assert_eq!(change.removed_from_start, 120);
        assert_eq!(
            model.messages.first().and_then(|message| message.id),
            Some(121)
        );
        assert_eq!(
            model.messages.last().and_then(|message| message.id),
            Some(600)
        );
    }

    #[test]
    fn a_sent_message_stays_until_a_page_has_it() {
        let mut model = AppModel::default();
        model.select_chat("chat-1");
        let mut sent: Message =
            serde_json::from_value(json!({"role": "user", "content": "Fix the footer"})).unwrap();
        sent.sent_from = Some(SentFrom::default());
        model.messages.push(sent);
        // Asked for before the message was stored.
        model
            .apply_message_page(&json!({"messages": []}), MessagePageDirection::Tail)
            .unwrap();
        assert_eq!(model.messages.len(), 1);
        assert_eq!(model.messages[0].content, "Fix the footer");
        assert!(model.messages[0].id.is_none());

        model
            .apply_message_page(
                &json!({"messages": [
                    {"id": 1, "role": "user", "content": "Fix the footer"},
                    {"id": 2, "role": "assistant", "content": "On it"}
                ]}),
                MessagePageDirection::Tail,
            )
            .unwrap();
        let ids = model
            .messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, [Some(1), Some(2)]);

        // Or a later page, reached going forward.
        let mut sent: Message =
            serde_json::from_value(json!({"role": "user", "content": "And the header"})).unwrap();
        sent.sent_from = Some(SentFrom::default());
        model.messages.push(sent);
        let change = model
            .apply_message_page(
                &json!({"messages": [
                    {"id": 3, "role": "user", "content": "And the header"}
                ]}),
                MessagePageDirection::After,
            )
            .unwrap();
        assert!(change.reset);

        // Words said before are not the copy of the same words sent again.
        let mut again: Message =
            serde_json::from_value(json!({"role": "user", "content": "Fix the footer"})).unwrap();
        again.sent_from = Some(SentFrom { newest: Some(3) });
        model.messages.push(again);
        model
            .apply_message_page(
                &json!({"messages": [
                    {"id": 1, "role": "user", "content": "Fix the footer"},
                    {"id": 2, "role": "assistant", "content": "On it"},
                    {"id": 3, "role": "user", "content": "And the header"}
                ]}),
                MessagePageDirection::Tail,
            )
            .unwrap();
        assert_eq!(model.messages.len(), 4);
        assert!(model.messages[3].id.is_none());

        // A newest page that starts after it was sent: the chat has moved on,
        // so it was stored, just not on this page.
        model
            .apply_message_page(
                &json!({
                    "messages": [
                        {"id": 9, "role": "tool", "content": "ran the tests"},
                        {"id": 10, "role": "assistant", "content": "Done"}
                    ],
                    "has_older": true
                }),
                MessagePageDirection::Tail,
            )
            .unwrap();
        assert!(model.messages.iter().all(|message| message.id.is_some()));
        model
            .apply_message_page(
                &json!({"messages": [
                    {"id": 1, "role": "user", "content": "Fix the footer"},
                    {"id": 2, "role": "assistant", "content": "On it"},
                    {"id": 3, "role": "user", "content": "And the header"}
                ]}),
                MessagePageDirection::Tail,
            )
            .unwrap();

        // Its stored copy carries its attachments as lines after its words.
        let mut with_image: Message =
            serde_json::from_value(json!({"role": "user", "content": "Look at this"})).unwrap();
        with_image.sent_from = Some(SentFrom { newest: Some(3) });
        model.messages.push(with_image);
        model
            .apply_message_page(
                &json!({"messages": [
                    {"id": 1, "role": "user", "content": "Fix the footer"},
                    {"id": 2, "role": "assistant", "content": "On it"},
                    {"id": 3, "role": "user", "content": "And the header"},
                    {"id": 4, "role": "user", "content": "Look at this\n[image: /tmp/shot.png]"}
                ]}),
                MessagePageDirection::Tail,
            )
            .unwrap();
        assert_eq!(model.messages.len(), 4);
        assert!(model.messages.iter().all(|message| message.id.is_some()));
        // Or words that look like an attachment line themselves.
        let mut marked: Message = serde_json::from_value(
            json!({"role": "user", "content": "Explain this:\n[file: /tmp/a.txt]"}),
        )
        .unwrap();
        marked.sent_from = Some(SentFrom { newest: Some(4) });
        model.messages.push(marked);
        model
            .apply_message_page(
                &json!({"messages": [
                    {"id": 1, "role": "user", "content": "Fix the footer"},
                    {"id": 2, "role": "assistant", "content": "On it"},
                    {"id": 3, "role": "user", "content": "And the header"},
                    {"id": 4, "role": "user", "content": "Look at this\n[image: /tmp/shot.png]"},
                    {"id": 5, "role": "user", "content": "Explain this:\n[file: /tmp/a.txt]"}
                ]}),
                MessagePageDirection::Tail,
            )
            .unwrap();
        assert_eq!(model.messages.len(), 5);
        assert!(model.messages.iter().all(|message| message.id.is_some()));

        // One sent from an older window of the transcript is not kept.
        let older: Message =
            serde_json::from_value(json!({"role": "user", "content": "Try again"})).unwrap();
        model.messages.push(older);
        model
            .apply_message_page(&json!({"messages": []}), MessagePageDirection::Tail)
            .unwrap();
        assert!(model.messages.is_empty());
        model
            .apply_message_page(
                &json!({"messages": [
                    {"id": 1, "role": "user", "content": "Fix the footer"},
                    {"id": 2, "role": "assistant", "content": "On it"},
                    {"id": 3, "role": "user", "content": "And the header"}
                ]}),
                MessagePageDirection::Tail,
            )
            .unwrap();
        let ids = model
            .messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, [Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn todo_snapshots_feed_the_pane_without_adding_transcript_rows() {
        let marker = "todo_list\n[{\"id\":\"1\",\"text\":\"Build pane\",\"status\":\"in_progress\"},{\"id\":\"2\",\"text\":\"Verify pane\",\"status\":\"pending\"}]";
        let mut model = AppModel::default();
        model.select_chat("chat-1");
        model
            .apply_message_page(
                &json!({"messages": [
                    {"id": 1, "role": "assistant", "content": "Starting"},
                    {"id": 2, "role": "tool", "content": marker}
                ]}),
                MessagePageDirection::Tail,
            )
            .unwrap();

        assert_eq!(model.messages.len(), 1);
        assert_eq!(model.todos.len(), 2);
        assert_eq!(model.todos[0].status, TodoStatus::InProgress);

        model.apply_event(
            "tool",
            &json!({
                "chat": "chat-1",
                "text": "todo_list\n[{\"id\":\"1\",\"text\":\"Build pane\",\"status\":\"completed\"}]"
            }),
        );
        assert!(model.live_items.is_empty());
        assert_eq!(model.todos[0].status, TodoStatus::Completed);
    }
}

/// Latest read-only output of a task owned by the selected chat's agent.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct BackgroundTaskOutput {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub started_at: u64,
    #[serde(default)]
    pub lines: Vec<BackgroundOutputLine>,
    #[serde(default)]
    pub output_error: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct BackgroundOutputLine {
    pub text: String,
    #[serde(default)]
    pub error: bool,
}

#[cfg(test)]
mod background_output_tests {
    use super::*;
    #[test]
    fn task_output_keeps_error_markers_and_unavailable_files() {
        let task: BackgroundTaskOutput = serde_json::from_value(serde_json::json!({
            "id":"watch", "kind":"Monitor", "description":"Watch CI", "started_at":123,
            "output_file":"/tmp/watch", "lines":[{"text":"Checking"},{"text":"jq: command not found", "error":true}]
        })).unwrap();
        assert!(!task.lines[0].error);
        assert!(task.lines[1].error);
        let missing: BackgroundTaskOutput = serde_json::from_value(serde_json::json!({
            "id":"gone", "kind":"Shell", "description":"Run tests", "started_at":123,
            "output_error":"Output file unavailable."
        }))
        .unwrap();
        assert!(missing.lines.is_empty());
        assert_eq!(
            missing.output_error.as_deref(),
            Some("Output file unavailable.")
        );
    }
}
