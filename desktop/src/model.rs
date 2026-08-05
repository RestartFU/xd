use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    pub title: String,
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
        }
        Ok(())
    }

    pub fn select_chat(&mut self, chat_id: impl Into<String>) {
        self.selected_chat = Some(chat_id.into());
        self.messages.clear();
        self.queue.clear();
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
            "turn-started" if self.event_is_active(body) => self.working = true,
            "turn-finished" if self.event_is_active(body) => self.working = false,
            _ => {}
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
                    title: "Rewrite desktop with GPUI".into(),
                    backend: "codex".into(),
                    working: true,
                },
                ChatSummary {
                    id: "chat-scroll".into(),
                    folder: "workspace-xd".into(),
                    title: "Smooth transcript scrolling".into(),
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
}
