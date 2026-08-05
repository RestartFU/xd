use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{Image, ImageFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub data: String,
    pub preview: Arc<Image>,
}

impl Attachment {
    pub fn from_png(name: impl Into<String>, bytes: Vec<u8>) -> Result<Self, String> {
        if !bytes.starts_with(PNG_SIGNATURE) {
            return Err("Only PNG images can be attached.".into());
        }
        Ok(Self {
            name: name.into(),
            mime: "image/png".into(),
            data: STANDARD.encode(&bytes),
            preview: Arc::new(Image::from_bytes(ImageFormat::Png, bytes)),
        })
    }

    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        let name = value.get("name").and_then(Value::as_str)?;
        let mime = value.get("mime").and_then(Value::as_str)?;
        let data = value.get("data").and_then(Value::as_str)?;
        if mime != "image/png" {
            return None;
        }
        let bytes = STANDARD.decode(data).ok()?;
        let mut attachment = Self::from_png(name, bytes).ok()?;
        attachment.data = data.to_owned();
        Some(attachment)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSummary {
    pub id: String,
    pub folder: String,
    #[serde(default)]
    pub title: Option<String>,
    pub backend: String,
    #[serde(default)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppModel {
    pub folders: Vec<Folder>,
    pub chats: Vec<ChatSummary>,
    pub selected_chat: Option<String>,
    pub messages: Vec<Message>,
    pub queue: Vec<String>,
    pub working: bool,
    pub connected: bool,
    pub connection_error: Option<String>,
    pub draft: String,
    pub draft_attachments: Vec<Attachment>,
    pub draft_revision: i64,
    pub live_text: String,
    pub live_activity: Vec<Message>,
}

#[derive(Debug, Deserialize)]
struct TreeSnapshot {
    folders: Vec<Folder>,
    chats: Vec<ChatSummary>,
}

#[derive(Debug, Deserialize)]
struct MessagesSnapshot {
    #[serde(default)]
    messages: Vec<Message>,
}

impl AppModel {
    pub fn apply_tree(&mut self, body: &Value) -> Result<(), serde_json::Error> {
        let snapshot: TreeSnapshot = serde_json::from_value(body.clone())?;
        self.folders = snapshot.folders;
        self.chats = snapshot.chats;
        self.connected = true;

        if self
            .selected_chat
            .as_ref()
            .is_some_and(|selected| !self.chats.iter().any(|chat| &chat.id == selected))
        {
            self.selected_chat = None;
            self.messages.clear();
            self.queue.clear();
            self.working = false;
            self.draft.clear();
            self.draft_attachments.clear();
            self.live_text.clear();
            self.live_activity.clear();
        }
        Ok(())
    }

    pub fn select_chat(&mut self, chat_id: impl Into<String>) {
        self.selected_chat = Some(chat_id.into());
        self.messages.clear();
        self.queue.clear();
        self.working = false;
        self.draft.clear();
        self.draft_attachments.clear();
        self.draft_revision = -1;
        self.live_text.clear();
        self.live_activity.clear();
    }

    pub fn apply_chat(&mut self, body: &Value) {
        self.queue = body
            .get("queue")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect();
        self.working = body
            .get("working")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.apply_draft(body);
    }

    pub fn apply_messages(&mut self, body: &Value) -> Result<(), serde_json::Error> {
        let snapshot: MessagesSnapshot = serde_json::from_value(body.clone())?;
        self.messages = snapshot.messages;
        Ok(())
    }

    pub fn apply_event(&mut self, name: &str, body: &Value) {
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
            "turn-started" if self.event_is_active(body) => {
                self.working = true;
                self.live_text.clear();
                self.live_activity.clear();
            }
            "text" if self.event_is_active(body) => {
                if let Some(text) = body.get("text").and_then(Value::as_str) {
                    self.live_text.push_str(text);
                }
            }
            "tool" if self.event_is_active(body) => {
                self.live_activity.push(Message {
                    id: None,
                    role: "tool".into(),
                    content: body
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("Used a tool")
                        .to_owned(),
                    label: None,
                });
            }
            "turn-finished" if self.event_is_active(body) => self.working = false,
            _ => {}
        }
    }

    pub fn selected_summary(&self) -> Option<&ChatSummary> {
        let selected = self.selected_chat.as_deref()?;
        self.chats.iter().find(|chat| chat.id == selected)
    }

    pub fn display_messages(&self) -> Vec<Message> {
        let mut messages = self.messages.clone();
        if !self.live_text.is_empty() {
            messages.push(Message {
                id: None,
                role: "assistant".into(),
                content: self.live_text.clone(),
                label: self.selected_summary().map(|chat| chat.backend.clone()),
            });
        }
        messages.extend(self.live_activity.iter().cloned());
        messages
    }

    pub fn display_message_count(&self) -> usize {
        self.messages.len() + usize::from(!self.live_text.is_empty()) + self.live_activity.len()
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
        if let Some(attachments) = body.get("draft_attachments").and_then(Value::as_array) {
            self.draft_attachments = attachments
                .iter()
                .filter_map(Attachment::from_value)
                .collect();
        }
    }

    fn event_is_active(&self, body: &Value) -> bool {
        body.get("chat").and_then(Value::as_str) == self.selected_chat.as_deref()
    }

    pub fn demo() -> Self {
        Self {
            folders: vec![Folder {
                id: "workspace-xd".into(),
                name: "xd".into(),
                parent: None,
            }],
            chats: vec![
                ChatSummary {
                    id: "chat-gpui".into(),
                    folder: "workspace-xd".into(),
                    title: Some("Rewrite desktop with GPUI".into()),
                    backend: "codex".into(),
                    working: true,
                },
                ChatSummary {
                    id: "chat-scroll".into(),
                    folder: "workspace-xd".into(),
                    title: Some("Smooth transcript scrolling".into()),
                    backend: "codex".into(),
                    working: false,
                },
            ],
            selected_chat: Some("chat-gpui".into()),
            messages: vec![
                Message {
                    id: Some(1),
                    role: "user".into(),
                    content: "Rewrite xd's desktop UI using GPUI, but keep the daemon.".into(),
                    label: None,
                },
                Message {
                    id: Some(2),
                    role: "assistant".into(),
                    content: "The new frontend is connected through the existing JSON Lines protocol. Transcript rows are virtualized from the first milestone.".into(),
                    label: Some("Codex".into()),
                },
                Message {
                    id: Some(3),
                    role: "tool".into(),
                    content: "Building the Rust desktop shell".into(),
                    label: None,
                },
            ],
            queue: vec!["Port workspace and chat shortcuts".into()],
            working: true,
            connected: true,
            connection_error: None,
            draft: String::new(),
            draft_attachments: Vec::new(),
            draft_revision: 0,
            live_text: String::new(),
            live_activity: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn applies_complete_tree_snapshots_and_retires_deleted_selection() {
        let mut model = AppModel {
            selected_chat: Some("deleted".into()),
            messages: vec![Message {
                id: Some(1),
                role: "user".into(),
                content: "stale".into(),
                label: None,
            }],
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
    fn streaming_text_and_tools_are_live_rows_until_history_is_reloaded() {
        let mut model = AppModel {
            selected_chat: Some("chat-1".into()),
            ..Default::default()
        };
        model.apply_event("turn-started", &json!({"chat":"chat-1"}));
        model.apply_event("text", &json!({"chat":"chat-1", "text":"hello"}));
        model.apply_event("text", &json!({"chat":"chat-1", "text":" world"}));
        model.apply_event("tool", &json!({"chat":"chat-1", "text":"Read file"}));

        assert_eq!(model.live_text, "hello world");
        assert_eq!(model.display_message_count(), 2);
        assert_eq!(model.display_messages()[1].content, "Read file");
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
        assert_eq!(model.draft_attachments[0].preview.bytes, PNG_SIGNATURE);

        model.apply_draft_snapshot(&json!({"draft":"typing", "draft_revision":2}));
        assert_eq!(model.draft_attachments.len(), 1);

        model.apply_draft_snapshot(&json!({
            "draft": "",
            "draft_revision": 3,
            "draft_attachments": []
        }));
        assert!(model.draft_attachments.is_empty());
    }
}
