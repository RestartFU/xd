use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

const MAX_MESSAGE_PAGE: i64 = 1_600;
const MAX_DRAFT_BYTES: usize = 1024 * 1024;
const MAX_SHORTCUTS: usize = 24;
const MAX_SHORTCUT_BYTES: usize = 4_096;
const MAX_IMAGES: usize = 4;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const CODEX_MODELS: &[(&str, &str, i64)] = &[
    ("gpt-5.6-sol", "GPT-5.6 Sol", 272_000),
    ("gpt-5.6-luna", "GPT-5.6 Luna", 272_000),
    ("gpt-5.6-terra", "GPT-5.6 Terra", 272_000),
    ("gpt-5.5", "GPT-5.5", 272_000),
    ("gpt-5.3-codex-spark", "GPT-5.3 Codex Spark", 128_000),
];
const CLAUDE_MODELS: &[(&str, &str, i64)] = &[
    ("claude-opus-5", "Claude Opus 5", 0),
    ("claude-fable-5", "Claude Fable 5", 0),
    ("claude-sonnet-5", "Claude Sonnet 5", 0),
    ("claude-haiku-4-5", "Claude Haiku 4.5", 0),
    ("claude-opus-4-8", "Claude Opus 4.8", 0),
];
const BASE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("cannot open xd database {path}: {source}")]
    Open {
        path: PathBuf,
        source: rusqlite::Error,
    },
    #[error("xd database is unavailable")]
    Poisoned,
    #[error("cannot read xd state: {0}")]
    Query(#[from] rusqlite::Error),
    #[error("No chat {0}")]
    NoChat(String),
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{context}: {source}")]
    Filesystem {
        context: String,
        source: std::io::Error,
    },
}

pub struct StateStore {
    database: Mutex<Connection>,
    workspace_root: PathBuf,
    paste_root: PathBuf,
}

struct ValidatedAttachment {
    name: String,
    encoded: String,
    data: Vec<u8>,
}

struct MaterializedMessage {
    prompt: String,
    paths: Vec<PathBuf>,
    keep: bool,
}

impl MaterializedMessage {
    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for MaterializedMessage {
    fn drop(&mut self) {
        if !self.keep {
            for path in &self.paths {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TurnSpec {
    pub chat_id: String,
    pub prompt: String,
    pub workdir: String,
    pub model: String,
    pub effort: String,
    pub access: String,
    pub session_id: Option<String>,
    pub label: String,
}

pub enum SendDisposition {
    Queued { reply: Value, event: Value },
    Start { reply: Value, turn: TurnSpec },
}

pub struct TurnFinish {
    pub last_message_id: i64,
    pub next: Option<TurnSpec>,
    pub queue_event: Option<Value>,
}

#[derive(Clone)]
struct WorkspaceRow {
    id: String,
    relative_path: String,
}

impl StateStore {
    pub fn open(
        database_path: impl AsRef<Path>,
        workspace_root: impl Into<PathBuf>,
    ) -> Result<Self, StorageError> {
        let database_path = database_path.as_ref();
        if let Some(parent) = database_path.parent() {
            fs::create_dir_all(parent).map_err(|source| StorageError::Filesystem {
                context: "Cannot create the xd state directory".into(),
                source,
            })?;
        }
        let workspace_root = workspace_root.into();
        fs::create_dir_all(&workspace_root).map_err(|source| StorageError::Filesystem {
            context: "Cannot create the workspace directory".into(),
            source,
        })?;
        Self::open_with_flags(
            database_path,
            workspace_root,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
    }

    pub fn open_read_only(
        database_path: impl AsRef<Path>,
        workspace_root: impl Into<PathBuf>,
    ) -> Result<Self, StorageError> {
        Self::open_with_flags(
            database_path.as_ref(),
            workspace_root,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
    }

    fn open_with_flags(
        database_path: impl AsRef<Path>,
        workspace_root: impl Into<PathBuf>,
        flags: OpenFlags,
    ) -> Result<Self, StorageError> {
        let database_path = database_path.as_ref();
        let database = Connection::open_with_flags(database_path, flags).map_err(|source| {
            StorageError::Open {
                path: database_path.to_owned(),
                source,
            }
        })?;
        database.pragma_update(None, "foreign_keys", true)?;
        if !flags.contains(OpenFlags::SQLITE_OPEN_READ_ONLY) {
            initialize_schema(&database)?;
        }
        Ok(Self {
            database: Mutex::new(database),
            workspace_root: workspace_root.into(),
            paste_root: database_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("remote-pasted"),
        })
    }

    pub fn tree(&self) -> Result<Value, StorageError> {
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let root = self.workspace_root.to_string_lossy();
        let mut statement = database.prepare(
            "SELECT id, relative_path FROM workspace_folders \
             WHERE root_path = ? ORDER BY LENGTH(relative_path), relative_path",
        )?;
        let stored = statement
            .query_map([root.as_ref()], |row| {
                Ok(WorkspaceRow {
                    id: row.get(0)?,
                    relative_path: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let by_path = stored
            .iter()
            .map(|row| (row.relative_path.clone(), row.clone()))
            .collect::<HashMap<_, _>>();
        let mut visible = Vec::new();
        for row in stored {
            if row.relative_path.is_empty() {
                continue;
            }
            let path = self.workspace_root.join(&row.relative_path);
            if !path.is_dir() || hidden_component(&row.relative_path) {
                continue;
            }
            if ancestors(&row.relative_path).any(|ancestor| {
                by_path.contains_key(ancestor)
                    && self.workspace_root.join(ancestor).join(".git").exists()
            }) {
                continue;
            }
            visible.push(row);
        }

        let visible_ids = visible
            .iter()
            .map(|row| row.id.as_str())
            .collect::<HashSet<_>>();
        let folders = visible
            .iter()
            .map(|row| {
                let mut folder = json!({
                    "id": row.id,
                    "name": Path::new(&row.relative_path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(&row.relative_path),
                });
                if let Some(parent) = parent_workspace(&row.relative_path, &by_path)
                    && visible_ids.contains(parent.id.as_str())
                {
                    folder["parent"] = Value::String(parent.id.clone());
                }
                folder
            })
            .collect::<Vec<_>>();

        let mut statement = database.prepare(
            "SELECT id, folder_id, title, backend, daemon_working FROM chats \
             ORDER BY last_user_message_at DESC, created_at DESC",
        )?;
        let chats = statement
            .query_map([], |row| {
                let folder: String = row.get(1)?;
                Ok((
                    folder.clone(),
                    json!({
                        "id": row.get::<_, String>(0)?,
                        "folder": folder,
                        "title": row.get::<_, Option<String>>(2)?,
                        "backend": row.get::<_, String>(3)?,
                        "working": row.get::<_, bool>(4)?,
                    }),
                ))
            })?
            .filter_map(|result| match result {
                Ok((folder, chat)) if visible_ids.contains(folder.as_str()) => Some(Ok(chat)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, rusqlite::Error>>()?;
        Ok(json!({"ok": true, "folders": folders, "chats": chats}))
    }

    pub fn chat(&self, chat_id: &str) -> Result<Value, StorageError> {
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let row = database
            .query_row(
                "SELECT folder_id, title, backend, workdir, model, effort, access, plan, fast, \
                 claude_mode, queued, new_worktree, daemon_working, draft, draft_attachments, \
                 draft_revision FROM chats WHERE id = ?",
                [chat_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, bool>(7)?,
                        row.get::<_, bool>(8)?,
                        row.get::<_, bool>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, bool>(11)?,
                        row.get::<_, bool>(12)?,
                        row.get::<_, String>(13)?,
                        row.get::<_, String>(14)?,
                        row.get::<_, i64>(15)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NoChat(chat_id.to_owned()))?;
        let queue = queue_from_column(row.10.as_deref());
        let attachments = serde_json::from_str::<Value>(&row.14).unwrap_or_else(|_| json!([]));
        let has_messages: bool = database.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE chat_id = ?)",
            [chat_id],
            |row| row.get(0),
        )?;
        let shortcuts = effective_shortcuts(&database, &self.workspace_root, &row.0)?;
        let mut response = json!({
            "ok": true,
            "title": row.1,
            "backend": row.2,
            "auth_state": "unknown",
            "commands": [],
            "plan": row.7,
            "fast": row.2 == "codex" && row.8,
            "claude_mode": row.2 == "codex" && row.9,
            "queue": queue,
            "draft": row.13,
            "draft_attachments": attachments,
            "draft_revision": row.15,
            "shortcuts": shortcuts,
            "working": row.12,
            "effort": row.5.unwrap_or_else(|| "high".into()),
            "new_worktree": row.11,
            "has_messages": has_messages,
        });
        if let Some(first) = response["queue"].as_array().and_then(|queue| queue.first()) {
            response["queued"] = first.clone();
        }
        if let Some(workdir) = row.3 {
            response["workdir"] = Value::String(workdir);
        }
        if let Some(model) = row.4 {
            response["model"] = Value::String(model);
        }
        if let Some(access) = row.6 {
            response["access"] = Value::String(access);
        }
        Ok(response)
    }

    pub fn shortcuts(&self, request: &Value) -> Result<Value, StorageError> {
        let folder_id = if request.get("folder").is_some() {
            Some(required_string(
                request,
                "folder",
                "folder must be a workspace id.",
            )?)
        } else {
            None
        };
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        shortcut_fields(&database, &self.workspace_root, folder_id)
    }

    pub fn agent_catalog(&self) -> Result<Value, StorageError> {
        let backends = [
            catalog_backend(
                "claude",
                "Claude Code",
                "claude-opus-5",
                CLAUDE_MODELS,
                "ultracode",
            ),
            catalog_backend("codex", "Codex", "gpt-5.6-sol", CODEX_MODELS, "ultra"),
        ];
        Ok(json!({"ok": true, "backends": backends}))
    }

    pub fn set_option(&self, request: &Value) -> Result<(Value, Value), StorageError> {
        let message = "set-option needs a chat and an option.";
        let chat_id = required_string(request, "chat", message)?;
        let option = required_string(request, "option", message)?;
        let value = optional_string(request, "value")?;
        let mut database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = database.transaction()?;
        let previous = transaction
            .query_row(
                "SELECT backend, model, effort, fast, claude_mode FROM chats WHERE id = ?",
                [chat_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, bool>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NoChat(chat_id.into()))?;
        let now = now_seconds();
        let changed = match option {
            "model" if request.get("backend").is_some() => {
                let backend = required_string(request, "backend", "A backend value is required.")?;
                let model = value.filter(|model| !model.is_empty()).ok_or_else(|| {
                    StorageError::InvalidRequest("A model value is required.".into())
                })?;
                validate_model(backend, model)?;
                let effort = previous
                    .2
                    .as_deref()
                    .filter(|effort| effort_supported(backend, effort));
                let changed = transaction.execute(
                    "UPDATE chats SET backend = ?, model = ?, effort = ?, \
                     fast = CASE WHEN ? = 'codex' THEN fast ELSE 0 END, \
                     claude_mode = CASE WHEN ? = 'codex' THEN claude_mode ELSE 0 END, \
                     updated_at = ? WHERE id = ?",
                    params![backend, model, effort, backend, backend, now, chat_id],
                )?;
                if previous.0 != backend || previous.1.as_deref() != Some(model) {
                    transaction.execute(
                        "INSERT INTO messages (chat_id, role, content, created_at) \
                         VALUES (?, 'event', ?, ?)",
                        params![
                            chat_id,
                            format!("Switched to {}", model_label(backend, model)),
                            now
                        ],
                    )?;
                }
                changed
            }
            "model" => transaction.execute(
                "UPDATE chats SET model = ?, updated_at = ? WHERE id = ?",
                params![value, now, chat_id],
            )?,
            "effort" => {
                if let Some(effort) = value {
                    if !effort_supported(&previous.0, effort) || (previous.4 && effort == "ultra") {
                        return Err(StorageError::InvalidRequest(
                            "That reasoning effort is not available for this assistant.".into(),
                        ));
                    }
                }
                transaction.execute(
                    "UPDATE chats SET effort = ?, updated_at = ? WHERE id = ?",
                    params![value, now, chat_id],
                )?
            }
            "access" => transaction.execute(
                "UPDATE chats SET access = ?, updated_at = ? WHERE id = ?",
                params![value, now, chat_id],
            )?,
            "plan" => {
                update_boolean_option(&transaction, chat_id, "plan", value == Some("true"), now)?
            }
            "fast" => {
                let enabled = value == Some("true");
                if enabled && previous.0 != "codex" {
                    return Err(StorageError::InvalidRequest(
                        "Fast mode is only available for Codex.".into(),
                    ));
                }
                update_boolean_option(&transaction, chat_id, "fast", enabled, now)?
            }
            "claude-mode" => {
                let enabled = value == Some("true");
                if enabled && previous.0 != "codex" {
                    return Err(StorageError::InvalidRequest(
                        "Claude mode is only available for Codex.".into(),
                    ));
                }
                let effort = if enabled && previous.2.as_deref() == Some("ultra") {
                    Some("max")
                } else {
                    previous.2.as_deref()
                };
                transaction.execute(
                    "UPDATE chats SET claude_mode = ?, effort = ?, updated_at = ? WHERE id = ?",
                    params![enabled, effort, now, chat_id],
                )?
            }
            "backend" => {
                let backend = value.filter(|backend| !backend.is_empty()).ok_or_else(|| {
                    StorageError::InvalidRequest("A backend value is required.".into())
                })?;
                validate_backend(backend)?;
                transaction.execute(
                    "UPDATE chats SET backend = ?, \
                     fast = CASE WHEN ? = 'codex' THEN fast ELSE 0 END, \
                     claude_mode = CASE WHEN ? = 'codex' THEN claude_mode ELSE 0 END, \
                     updated_at = ? WHERE id = ?",
                    params![backend, backend, backend, now, chat_id],
                )?
            }
            "new-worktree" => transaction.execute(
                "UPDATE chats SET new_worktree = ?, updated_at = ? WHERE id = ? \
                 AND NOT EXISTS (SELECT 1 FROM messages WHERE chat_id = ?)",
                params![value == Some("true"), now, chat_id, chat_id],
            )?,
            "workspace" => {
                return Err(StorageError::InvalidRequest(
                    "Workspace selection is not implemented by the Rust daemon yet.".into(),
                ));
            }
            _ => return Err(StorageError::InvalidRequest("No such option.".into())),
        };
        if changed != 1 {
            if option == "new-worktree" {
                return Err(StorageError::InvalidRequest(
                    "The workspace can only be changed before the first message.".into(),
                ));
            }
            return Err(StorageError::NoChat(chat_id.into()));
        }
        transaction.commit()?;
        Ok((
            json!({"ok": true}),
            json!({"event": "changed", "chat": chat_id}),
        ))
    }

    pub fn prepare_send(&self, request: &Value) -> Result<SendDisposition, StorageError> {
        let chat_id = required_string(
            request,
            "chat",
            "A message needs a chat and something to say.",
        )?;
        let text = optional_string(request, "text")?.unwrap_or("");
        if text.is_empty() && request.get("attachments").is_none() {
            return Err(StorageError::InvalidRequest(
                "A message needs a chat and something to say.".into(),
            ));
        }
        let mut message = self.materialize_message(request, text)?;
        let mut database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = database.transaction()?;
        let working = transaction
            .query_row(
                "SELECT daemon_working FROM chats WHERE id = ?",
                [chat_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::NoChat(chat_id.into()))?;
        if working {
            let stored = transaction.query_row(
                "SELECT COALESCE(queued, '') FROM chats WHERE id = ?",
                [chat_id],
                |row| row.get::<_, String>(0),
            )?;
            let mut queue = queue_from_column(Some(&stored));
            queue.push(message.prompt.clone());
            transaction.execute(
                "UPDATE chats SET queued = ?, updated_at = ? WHERE id = ?",
                params![
                    serde_json::to_string(&queue).unwrap(),
                    now_seconds(),
                    chat_id
                ],
            )?;
            transaction.commit()?;
            message.keep();
            let event = queued_event(chat_id, &queue);
            return Ok(SendDisposition::Queued {
                reply: json!({"ok": true, "queued": true}),
                event,
            });
        }
        let turn = prepare_turn(&transaction, &self.workspace_root, chat_id, &message.prompt)?;
        transaction.execute(
            "UPDATE chats SET daemon_working = 1, draft = '', draft_attachments = '[]', \
             draft_revision = draft_revision + 1, updated_at = ? WHERE id = ?",
            params![now_seconds(), chat_id],
        )?;
        transaction.commit()?;
        message.keep();
        Ok(SendDisposition::Start {
            reply: json!({"ok": true, "queued": false}),
            turn,
        })
    }

    fn materialize_message(
        &self,
        request: &Value,
        text: &str,
    ) -> Result<MaterializedMessage, StorageError> {
        let Some(value) = request.get("attachments") else {
            return Ok(MaterializedMessage {
                prompt: text.to_owned(),
                paths: Vec::new(),
                keep: true,
            });
        };
        let attachments = validate_attachments(value, false)?;
        fs::create_dir_all(&self.paste_root).map_err(|source| StorageError::Filesystem {
            context: "Cannot create the remote image cache".into(),
            source,
        })?;
        fs::set_permissions(&self.paste_root, fs::Permissions::from_mode(0o700)).map_err(
            |source| StorageError::Filesystem {
                context: "Cannot secure the remote image cache".into(),
                source,
            },
        )?;

        let mut materialized = MaterializedMessage {
            prompt: text.to_owned(),
            paths: Vec::with_capacity(attachments.len()),
            keep: false,
        };
        for attachment in attachments {
            let path = self
                .paste_root
                .join(format!("paste-{}.png", Uuid::new_v4()));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|source| StorageError::Filesystem {
                    context: "Cannot create a remote image".into(),
                    source,
                })?;
            materialized.paths.push(path.clone());
            file.write_all(&attachment.data)
                .map_err(|source| StorageError::Filesystem {
                    context: "Cannot write a remote image".into(),
                    source,
                })?;
            if !materialized.prompt.is_empty() {
                materialized.prompt.push('\n');
            }
            materialized
                .prompt
                .push_str(&format!("[image: {}]", path.display()));
        }
        Ok(materialized)
    }

    pub fn append_turn_message(
        &self,
        chat_id: &str,
        role: &str,
        content: &str,
        label: Option<&str>,
    ) -> Result<i64, StorageError> {
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        database.execute(
            "INSERT INTO messages (chat_id, role, content, created_at, label) VALUES (?, ?, ?, ?, ?)",
            params![chat_id, role, content, now_seconds(), label],
        )?;
        Ok(database.last_insert_rowid())
    }

    pub fn set_session(&self, chat_id: &str, session_id: &str) -> Result<(), StorageError> {
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        database.execute(
            "INSERT INTO chat_sessions (chat_id, backend, session_id) VALUES (?, 'codex', ?) \
             ON CONFLICT (chat_id, backend) DO UPDATE SET session_id = excluded.session_id",
            params![chat_id, session_id],
        )?;
        Ok(())
    }

    pub fn finish_turn(
        &self,
        chat_id: &str,
        success: bool,
        error: Option<&str>,
        duration: u64,
        silent: bool,
    ) -> Result<TurnFinish, StorageError> {
        let mut database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = database.transaction()?;
        if success && silent {
            transaction.execute(
                "INSERT INTO messages (chat_id, role, content, created_at) \
                 VALUES (?, 'assistant', '(no reply)', ?)",
                params![chat_id, now_seconds()],
            )?;
        }
        transaction.execute(
            "INSERT INTO messages (chat_id, role, content, created_at) VALUES (?, 'duration', ?, ?)",
            params![chat_id, duration.to_string(), now_seconds()],
        )?;
        if let Some(error) = error.filter(|error| !error.is_empty()) {
            transaction.execute(
                "INSERT INTO messages (chat_id, role, content, created_at) VALUES (?, 'error', ?, ?)",
                params![chat_id, error, now_seconds()],
            )?;
        }
        let stored = transaction
            .query_row(
                "SELECT COALESCE(queued, '') FROM chats WHERE id = ?",
                [chat_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::NoChat(chat_id.into()))?;
        let mut queue = queue_from_column(Some(&stored));
        let next_text = if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        };
        let encoded = if queue.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&queue).unwrap())
        };
        transaction.execute(
            "UPDATE chats SET queued = ?, daemon_working = ?, updated_at = ? WHERE id = ?",
            params![encoded, next_text.is_some(), now_seconds(), chat_id],
        )?;
        let last_message_id = transaction.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM messages WHERE chat_id = ?",
            [chat_id],
            |row| row.get(0),
        )?;
        let next = next_text
            .as_deref()
            .map(|text| prepare_turn(&transaction, &self.workspace_root, chat_id, text))
            .transpose()?;
        transaction.commit()?;
        Ok(TurnFinish {
            last_message_id,
            next,
            queue_event: next_text.map(|_| queued_event(chat_id, &queue)),
        })
    }

    pub fn abort_turn_start(&self, chat_id: &str, error: &str) -> Result<(), StorageError> {
        let mut database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = database.transaction()?;
        transaction.execute(
            "INSERT INTO messages (chat_id, role, content, created_at) VALUES (?, 'error', ?, ?)",
            params![chat_id, error, now_seconds()],
        )?;
        transaction.execute(
            "UPDATE chats SET daemon_working = 0, updated_at = ? WHERE id = ?",
            params![now_seconds(), chat_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn set_shortcuts(&self, request: &Value) -> Result<(Value, Value), StorageError> {
        let prompts = clean_shortcuts(request.get("shortcuts").ok_or_else(|| {
            StorageError::InvalidRequest("set-shortcuts needs a shortcuts array.".into())
        })?)?;
        let folder_id = if request.get("folder").is_some() {
            Some(required_string(request, "folder", "folder must be a workspace id.")?.to_owned())
        } else {
            None
        };
        let encoded = serde_json::to_string(&prompts)
            .map_err(|error| StorageError::InvalidRequest(error.to_string()))?;
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        match folder_id.as_deref() {
            Some(folder_id) => {
                let changed = database.execute(
                    "UPDATE workspace_folders SET shortcuts = ?, updated_at = ? \
                     WHERE root_path = ? AND id = ?",
                    params![
                        encoded,
                        now_seconds(),
                        self.workspace_root.to_string_lossy(),
                        folder_id
                    ],
                )?;
                if changed != 1 {
                    return Err(StorageError::InvalidRequest(
                        "No such folder on the daemon.".into(),
                    ));
                }
            }
            None => {
                database.execute(
                    "INSERT INTO meta (key, value) VALUES ('global_shortcuts', ?) \
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    [encoded],
                )?;
            }
        }
        let reply = shortcut_fields(&database, &self.workspace_root, folder_id.as_deref())?;
        let mut event = json!({"event": "shortcuts-changed"});
        if let Some(folder_id) = folder_id {
            event["folder"] = Value::String(folder_id);
        }
        Ok((reply, event))
    }

    pub fn messages(&self, request: &Value) -> Result<Value, StorageError> {
        let chat_id = required_string(request, "chat", "messages needs a chat id")?;
        let requested_limit = optional_integer(request, "limit")?.unwrap_or(0);
        let requested_offset = optional_integer(request, "offset")?;
        if requested_limit < 0 {
            return Err(StorageError::InvalidRequest(
                "limit must not be negative".into(),
            ));
        }
        if requested_offset.is_some_and(|offset| offset < 0) {
            return Err(StorageError::InvalidRequest(
                "offset must not be negative".into(),
            ));
        }
        if requested_offset.is_some() && requested_limit <= 0 {
            return Err(StorageError::InvalidRequest(
                "offset requests need a positive limit".into(),
            ));
        }

        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let exists: bool = database.query_row(
            "SELECT EXISTS(SELECT 1 FROM chats WHERE id = ?)",
            [chat_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StorageError::NoChat(chat_id.into()));
        }
        let total: i64 = database.query_row(
            "SELECT COUNT(*) FROM messages WHERE chat_id = ?",
            [chat_id],
            |row| row.get(0),
        )?;
        let last_message_id: i64 = database.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM messages WHERE chat_id = ?",
            [chat_id],
            |row| row.get(0),
        )?;
        let (offset, limit) = if let Some(offset) = requested_offset {
            (offset.min(total), requested_limit.min(MAX_MESSAGE_PAGE))
        } else if requested_limit > 0 {
            let limit = requested_limit.min(MAX_MESSAGE_PAGE);
            ((total - limit).max(0), limit)
        } else {
            (0, total)
        };
        let mut statement = database.prepare(
            "SELECT role, content, created_at, label FROM messages \
             WHERE chat_id = ? ORDER BY id LIMIT ? OFFSET ?",
        )?;
        let messages = statement
            .query_map(params![chat_id, limit, offset], |row| {
                let mut message = json!({
                    "role": row.get::<_, String>(0)?,
                    "content": row.get::<_, String>(1)?,
                    "at": row.get::<_, i64>(2)?,
                });
                if let Some(label) = row.get::<_, Option<String>>(3)? {
                    message["label"] = Value::String(label);
                }
                Ok(message)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({
            "ok": true,
            "total_messages": total,
            "last_message_id": last_message_id,
            "offset": offset,
            "messages": messages,
            "turn_start": null,
        }))
    }

    pub fn set_draft(&self, request: &Value) -> Result<(Value, Value), StorageError> {
        let chat_id = required_string(request, "chat", "set-draft needs a chat and text.")?;
        let text =
            required_string_allow_empty(request, "text", "set-draft needs a chat and text.")?;
        if text.len() > MAX_DRAFT_BYTES {
            return Err(StorageError::InvalidRequest(
                "A message draft is too large.".into(),
            ));
        }
        let attachments = request
            .get("attachments")
            .map(|value| validate_attachments(value, true))
            .transpose()?;
        let attachments_json = attachments
            .as_ref()
            .map(|attachments| {
                let normalized = attachments
                    .iter()
                    .map(|attachment| {
                        json!({
                            "name": attachment.name,
                            "mime": "image/png",
                            "data": attachment.encoded,
                        })
                    })
                    .collect::<Vec<_>>();
                serde_json::to_string(&normalized)
            })
            .transpose()
            .map_err(|error| StorageError::InvalidRequest(error.to_string()))?;
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let state = database
            .query_row(
                "UPDATE chats SET draft = ?, draft_attachments = COALESCE(?, draft_attachments), \
                 draft_revision = draft_revision + 1 WHERE id = ? \
                 RETURNING draft, draft_attachments, draft_revision",
                params![text, attachments_json, chat_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NoChat(chat_id.into()))?;
        let mut reply = json!({
            "ok": true,
            "draft": state.0,
            "draft_revision": state.2,
        });
        let mut event = json!({
            "event": "draft",
            "chat": chat_id,
            "draft": state.0,
            "draft_revision": state.2,
        });
        if attachments.is_some() {
            let attachments = serde_json::from_str::<Value>(&state.1).unwrap_or_else(|_| json!([]));
            reply["draft_attachments"] = attachments.clone();
            event["draft_attachments"] = attachments;
        }
        Ok((reply, event))
    }

    pub fn new_folder(&self, request: &Value) -> Result<Value, StorageError> {
        let name = required_string(
            request,
            "name",
            "A folder name cannot be empty or hidden, or contain a path separator.",
        )?;
        validate_workspace_name(name)?;
        let parent_id = optional_string(request, "parent")?;
        let repo = optional_string(request, "repo")?;
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let root = self.workspace_root.to_string_lossy();
        let parent_relative = match parent_id {
            Some(parent_id) => database
                .query_row(
                    "SELECT relative_path FROM workspace_folders WHERE root_path = ? AND id = ?",
                    params![root.as_ref(), parent_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or_else(|| {
                    StorageError::InvalidRequest("No such folder on the daemon.".into())
                })?,
            None => String::new(),
        };
        let relative = if parent_relative.is_empty() {
            name.to_owned()
        } else {
            format!("{parent_relative}/{name}")
        };
        if let Some(id) = database
            .query_row(
                "SELECT id FROM workspace_folders WHERE root_path = ? AND relative_path = ?",
                params![root.as_ref(), &relative],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(json!({"ok": true, "id": id}));
        }
        let path = self.workspace_root.join(&relative);
        let created = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => false,
            Ok(_) => {
                return Err(StorageError::InvalidRequest(
                    "There is already something of that name there.".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&path).map_err(|source| StorageError::Filesystem {
                    context: "Cannot create folder".into(),
                    source,
                })?;
                true
            }
            Err(source) => {
                return Err(StorageError::Filesystem {
                    context: "Cannot inspect folder".into(),
                    source,
                });
            }
        };
        let id = Uuid::new_v4().to_string();
        let now = now_seconds();
        let inserted = database.execute(
            "INSERT INTO workspace_folders \
             (id, root_path, relative_path, backend, model, workdir, repo, instructions, \
              shortcuts, created_at, updated_at) VALUES (?, ?, ?, NULL, NULL, NULL, ?, NULL, '[]', ?, ?)",
            params![id, root.as_ref(), relative, repo, now, now],
        );
        if let Err(error) = inserted {
            if created {
                let _ = fs::remove_dir(&path);
            }
            return Err(StorageError::Query(error));
        }
        Ok(json!({"ok": true, "id": id}))
    }

    pub fn rename_folder(&self, request: &Value) -> Result<Value, StorageError> {
        let folder_id = required_string(request, "folder", "That request needs a folder.")?;
        let name = required_string(
            request,
            "name",
            "A folder name cannot be empty or hidden, or contain a path separator.",
        )?;
        validate_workspace_name(name)?;
        let mut database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let root = self.workspace_root.to_string_lossy();
        let relative = workspace_relative(&database, root.as_ref(), folder_id)?;
        let source = self.workspace_root.join(&relative);
        let destination = source.parent().unwrap_or(&self.workspace_root).join(name);
        if destination == source {
            return Ok(json!({"ok": true}));
        }
        if destination.exists() {
            return Err(StorageError::InvalidRequest(
                "There is already a folder of that name there.".into(),
            ));
        }
        fs::rename(&source, &destination).map_err(|source| StorageError::Filesystem {
            context: "Cannot rename folder".into(),
            source,
        })?;
        let destination_relative = relative_from_root(&self.workspace_root, &destination)?;
        if let Err(error) = relocate_workspace_subtree(
            &mut database,
            root.as_ref(),
            &relative,
            &destination_relative,
        ) {
            let _ = fs::rename(&destination, &source);
            return Err(error);
        }
        Ok(json!({"ok": true}))
    }

    pub fn move_folder(&self, request: &Value) -> Result<Value, StorageError> {
        let folder_id = required_string(request, "folder", "That request needs a folder.")?;
        let parent_id = optional_string(request, "parent")?;
        let mut database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let root = self.workspace_root.to_string_lossy();
        let relative = workspace_relative(&database, root.as_ref(), folder_id)?;
        let parent_relative = match parent_id {
            Some(parent_id) => workspace_relative(&database, root.as_ref(), parent_id)?,
            None => String::new(),
        };
        if parent_relative == relative || parent_relative.starts_with(&format!("{relative}/")) {
            return Err(StorageError::InvalidRequest(
                "A folder cannot be moved inside itself.".into(),
            ));
        }
        let source = self.workspace_root.join(&relative);
        let parent = self.workspace_root.join(&parent_relative);
        if source.parent() == Some(parent.as_path()) {
            return Ok(json!({"ok": true}));
        }
        if parent.join(".git").exists() {
            return Err(StorageError::InvalidRequest(
                "A folder cannot be moved inside a repository.".into(),
            ));
        }
        let name = source
            .file_name()
            .ok_or_else(|| StorageError::InvalidRequest("No such folder on the daemon.".into()))?;
        let destination = parent.join(name);
        if destination.exists() {
            return Err(StorageError::InvalidRequest(
                "There is already a folder of that name there.".into(),
            ));
        }
        fs::rename(&source, &destination).map_err(|source| StorageError::Filesystem {
            context: "Cannot move folder".into(),
            source,
        })?;
        let destination_relative = relative_from_root(&self.workspace_root, &destination)?;
        if let Err(error) = relocate_workspace_subtree(
            &mut database,
            root.as_ref(),
            &relative,
            &destination_relative,
        ) {
            let _ = fs::rename(&destination, &source);
            return Err(error);
        }
        Ok(json!({"ok": true}))
    }

    pub fn trash_folder(&self, request: &Value) -> Result<Value, StorageError> {
        let folder_id = required_string(request, "folder", "That request needs a folder.")?;
        let mut database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let root = self.workspace_root.to_string_lossy();
        let relative = workspace_relative(&database, root.as_ref(), folder_id)?;
        let source = self.workspace_root.join(&relative);
        let trash = self.workspace_root.join(".Trash");
        fs::create_dir_all(&trash).map_err(|source| StorageError::Filesystem {
            context: "Cannot prepare workspace trash".into(),
            source,
        })?;
        fs::set_permissions(&trash, fs::Permissions::from_mode(0o700)).map_err(|source| {
            StorageError::Filesystem {
                context: "Cannot secure workspace trash".into(),
                source,
            }
        })?;
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace");
        let destination = trash.join(format!("{}-{}-{name}", now_seconds(), Uuid::new_v4()));
        fs::rename(&source, &destination).map_err(|source| StorageError::Filesystem {
            context: "Cannot trash folder".into(),
            source,
        })?;
        let destination_relative = relative_from_root(&self.workspace_root, &destination)?;
        if let Err(error) = relocate_workspace_subtree(
            &mut database,
            root.as_ref(),
            &relative,
            &destination_relative,
        ) {
            let _ = fs::rename(&destination, &source);
            return Err(error);
        }
        Ok(json!({"ok": true}))
    }

    pub fn new_chat(&self, request: &Value) -> Result<Value, StorageError> {
        let folder_id = required_string(request, "folder", "That request needs a folder.")?;
        let title = optional_string(request, "title")?.unwrap_or("New Chat");
        let workdir = optional_string(request, "workdir")?;
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let root = self.workspace_root.to_string_lossy();
        let folder_exists: bool = database.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_folders WHERE root_path = ? AND id = ?)",
            params![root.as_ref(), folder_id],
            |row| row.get(0),
        )?;
        if !folder_exists {
            return Err(StorageError::InvalidRequest(
                "No such folder on the daemon.".into(),
            ));
        }
        let defaults = database
            .query_row(
                "SELECT backend, model, effort, access, plan, fast, claude_mode \
                 FROM agent_defaults WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, bool>(4)?,
                        row.get::<_, bool>(5)?,
                        row.get::<_, bool>(6)?,
                    ))
                },
            )
            .optional()?
            .unwrap_or_else(|| ("claude".into(), None, None, None, false, false, false));
        let id = Uuid::new_v4().to_string();
        let now = now_seconds();
        database.execute(
            "INSERT INTO chats \
             (id, folder_id, title, backend, model, effort, access, plan, fast, claude_mode, \
              workdir, created_at, updated_at, last_user_message_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                folder_id,
                title,
                defaults.0,
                defaults.1,
                defaults.2,
                defaults.3,
                defaults.4,
                defaults.5,
                defaults.6,
                workdir,
                now,
                now,
                now * 1_000_000,
            ],
        )?;
        Ok(json!({"ok": true, "id": id}))
    }

    pub fn rename_chat(&self, request: &Value) -> Result<Value, StorageError> {
        let chat_id = required_string(request, "chat", "A chat needs an id and a title.")?;
        let title = required_string(request, "title", "A chat needs an id and a title.")?;
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let changed = database.execute(
            "UPDATE chats SET title = ?, updated_at = ? WHERE id = ?",
            params![title, now_seconds(), chat_id],
        )?;
        if changed != 1 {
            return Err(StorageError::NoChat(chat_id.into()));
        }
        Ok(json!({"ok": true}))
    }

    pub fn move_chat(&self, request: &Value) -> Result<Value, StorageError> {
        let chat_id = required_string(request, "chat", "move-chat needs a chat id")?;
        let folder_id = required_string(request, "folder", "move-chat needs a folder")?;
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let root = self.workspace_root.to_string_lossy();
        let folder_exists: bool = database.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_folders WHERE root_path = ? AND id = ?)",
            params![root.as_ref(), folder_id],
            |row| row.get(0),
        )?;
        if !folder_exists {
            return Err(StorageError::InvalidRequest(
                "No such folder on the daemon.".into(),
            ));
        }
        let changed = database.execute(
            "UPDATE chats SET folder_id = ?, updated_at = ? WHERE id = ?",
            params![folder_id, now_seconds(), chat_id],
        )?;
        if changed != 1 {
            return Err(StorageError::NoChat(chat_id.into()));
        }
        Ok(json!({"ok": true}))
    }

    pub fn delete_chat(&self, request: &Value) -> Result<Value, StorageError> {
        let chat_id = required_string(request, "chat", "delete-chat needs a chat id")?;
        let database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let changed = database.execute("DELETE FROM chats WHERE id = ?", [chat_id])?;
        if changed != 1 {
            return Err(StorageError::NoChat(chat_id.into()));
        }
        Ok(json!({"ok": true}))
    }

    pub fn queue(&self, request: &Value) -> Result<(Value, Value), StorageError> {
        let message = "A queued message needs a chat and text.";
        let chat_id = required_string(request, "chat", message)?;
        let text = required_string(request, "text", message)?.to_owned();
        self.mutate_queue(chat_id, move |queue| {
            queue.push(text);
            Ok(true)
        })
    }

    pub fn drop_queue(&self, request: &Value) -> Result<(Value, Value), StorageError> {
        let chat_id = required_string(request, "chat", "drop-queue needs a chat id")?;
        let index = if request.get("index").is_some() {
            Some(optional_integer(request, "index")?.unwrap_or(0))
        } else {
            None
        };
        self.mutate_queue(chat_id, move |queue| match index {
            Some(index) if index >= 0 && (index as usize) < queue.len() => {
                queue.remove(index as usize);
                Ok(true)
            }
            Some(_) => Ok(false),
            None => {
                let changed = !queue.is_empty();
                queue.clear();
                Ok(changed)
            }
        })
    }

    pub fn edit_queue(&self, request: &Value) -> Result<(Value, Value), StorageError> {
        let message = "edit-queue needs a chat id, queue index, and text.";
        let chat_id = required_string(request, "chat", message)?;
        let old_text = required_string_allow_empty(request, "old-text", message)?.to_owned();
        let text = required_string(request, "text", message)?.to_owned();
        let index = optional_integer(request, "index")?
            .filter(|index| *index >= 0)
            .ok_or_else(|| StorageError::InvalidRequest(message.into()))?
            as usize;
        self.mutate_queue(chat_id, move |queue| {
            if queue.get(index) != Some(&old_text) {
                return Err(StorageError::InvalidRequest(
                    "That queued message changed; try again.".into(),
                ));
            }
            queue[index] = text;
            Ok(true)
        })
    }

    pub fn steer_queue(&self, request: &Value) -> Result<(Value, Value), StorageError> {
        let message = "steer-queue needs a chat id, queue index, and text.";
        let chat_id = required_string(request, "chat", message)?;
        let text = required_string_allow_empty(request, "text", message)?.to_owned();
        let index = optional_integer(request, "index")?
            .filter(|index| *index >= 0)
            .ok_or_else(|| StorageError::InvalidRequest(message.into()))?
            as usize;
        self.mutate_queue(chat_id, move |queue| {
            if queue.get(index) != Some(&text) {
                return Err(StorageError::InvalidRequest(
                    "That queued message changed; try again.".into(),
                ));
            }
            if index == 0 {
                return Ok(false);
            }
            let selected = queue.remove(index);
            queue.insert(0, selected);
            Ok(true)
        })
    }

    fn mutate_queue(
        &self,
        chat_id: &str,
        mutate: impl FnOnce(&mut Vec<String>) -> Result<bool, StorageError>,
    ) -> Result<(Value, Value), StorageError> {
        let mut database = self.database.lock().map_err(|_| StorageError::Poisoned)?;
        let transaction = database.transaction()?;
        let stored = transaction
            .query_row(
                "SELECT COALESCE(queued, '') FROM chats WHERE id = ?",
                [chat_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::NoChat(chat_id.into()))?;
        let mut queue = queue_from_column(Some(&stored));
        if mutate(&mut queue)? {
            let encoded = if queue.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&queue).map_err(|error| {
                    StorageError::InvalidRequest(format!("Cannot encode the queue: {error}"))
                })?)
            };
            let changed = transaction.execute(
                "UPDATE chats SET queued = ?, updated_at = ? WHERE id = ?",
                params![encoded, now_seconds(), chat_id],
            )?;
            if changed != 1 {
                return Err(StorageError::NoChat(chat_id.into()));
            }
        }
        transaction.commit()?;

        let mut event = json!({
            "event": "queued",
            "chat": chat_id,
            "queue": queue,
        });
        if let Some(first) = event["queue"].as_array().and_then(|queue| queue.first()) {
            event["text"] = first.clone();
        }
        Ok((json!({"ok": true}), event))
    }
}

fn initialize_schema(database: &Connection) -> Result<(), StorageError> {
    let has_meta: bool = database.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |row| row.get(0),
    )?;
    if has_meta {
        let version = database
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|version| version.parse::<u32>().ok())
            .unwrap_or(0);
        if version > 24 {
            return Err(StorageError::InvalidRequest(format!(
                "The chat database requires a newer xd version (schema {version})."
            )));
        }
        if version > 0 {
            return Ok(());
        }
    }

    let transaction = database.unchecked_transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); \
         CREATE TABLE IF NOT EXISTS workspace_folders (id TEXT PRIMARY KEY, root_path TEXT NOT NULL, \
           relative_path TEXT NOT NULL, backend TEXT, model TEXT, workdir TEXT, repo TEXT, \
           instructions TEXT, shortcuts TEXT NOT NULL DEFAULT '[]', created_at INTEGER NOT NULL, \
           updated_at INTEGER NOT NULL, UNIQUE(root_path, relative_path)); \
         CREATE INDEX IF NOT EXISTS workspace_folders_root \
           ON workspace_folders (root_path, relative_path); \
         CREATE TABLE IF NOT EXISTS chats (id TEXT PRIMARY KEY, folder_id TEXT NOT NULL, title TEXT, \
           backend TEXT NOT NULL, session_id TEXT, workdir TEXT, model TEXT, effort TEXT, access TEXT, \
           plan INTEGER NOT NULL DEFAULT 0, fast INTEGER NOT NULL DEFAULT 0, \
           claude_mode INTEGER NOT NULL DEFAULT 0, queued TEXT, new_worktree INTEGER NOT NULL DEFAULT 0, \
           terminal_open INTEGER NOT NULL DEFAULT 0, diff_open INTEGER NOT NULL DEFAULT 0, \
           resume_after_restart INTEGER NOT NULL DEFAULT 0, original_workdir TEXT, \
           daemon_working INTEGER NOT NULL DEFAULT 0, draft TEXT NOT NULL DEFAULT '', \
           draft_attachments TEXT NOT NULL DEFAULT '[]', draft_revision INTEGER NOT NULL DEFAULT 0, \
           last_user_message_at INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, \
           updated_at INTEGER NOT NULL, FOREIGN KEY(folder_id) REFERENCES workspace_folders(id)); \
         CREATE INDEX IF NOT EXISTS chats_folder ON chats (folder_id, updated_at DESC); \
         CREATE INDEX IF NOT EXISTS chats_folder_user_message \
           ON chats (folder_id, last_user_message_at DESC); \
         CREATE TABLE IF NOT EXISTS chat_sessions (chat_id TEXT NOT NULL, backend TEXT NOT NULL, \
           session_id TEXT, last_message_id INTEGER NOT NULL DEFAULT 0, \
           context_used INTEGER NOT NULL DEFAULT 0, context_window INTEGER NOT NULL DEFAULT 0, \
           context_model TEXT, PRIMARY KEY(chat_id, backend), \
           FOREIGN KEY(chat_id) REFERENCES chats(id) ON DELETE CASCADE); \
         CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY AUTOINCREMENT, \
           chat_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL, raw_json TEXT, \
           created_at INTEGER NOT NULL, label TEXT, \
           FOREIGN KEY(chat_id) REFERENCES chats(id) ON DELETE CASCADE); \
         CREATE INDEX IF NOT EXISTS messages_chat ON messages (chat_id, id); \
         CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5 \
           (content, content='messages', content_rowid='id'); \
         CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN \
           INSERT INTO messages_fts (rowid, content) VALUES (new.id, new.content); END; \
         CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN \
           INSERT INTO messages_fts (messages_fts, rowid, content) \
           VALUES ('delete', old.id, old.content); END; \
         CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE ON messages BEGIN \
           INSERT INTO messages_fts (messages_fts, rowid, content) \
           VALUES ('delete', old.id, old.content); \
           INSERT INTO messages_fts (rowid, content) VALUES (new.id, new.content); END; \
         CREATE TABLE IF NOT EXISTS agent_defaults (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), \
           backend TEXT NOT NULL, model TEXT, effort TEXT, access TEXT, plan INTEGER NOT NULL, \
           fast INTEGER NOT NULL DEFAULT 0, claude_mode INTEGER NOT NULL DEFAULT 0); \
         CREATE TABLE IF NOT EXISTS worktree_containers (path TEXT PRIMARY KEY, \
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL); \
         CREATE TABLE IF NOT EXISTS devices (token_hash TEXT PRIMARY KEY, name TEXT NOT NULL, \
           created_at INTEGER NOT NULL, last_seen INTEGER NOT NULL); \
         CREATE TRIGGER IF NOT EXISTS remember_agent_defaults \
           AFTER UPDATE OF backend, model, effort, access, plan, fast, claude_mode ON chats \
           WHEN OLD.backend IS NOT NEW.backend OR OLD.model IS NOT NEW.model \
             OR OLD.effort IS NOT NEW.effort OR OLD.access IS NOT NEW.access \
             OR OLD.plan IS NOT NEW.plan OR OLD.fast IS NOT NEW.fast \
             OR OLD.claude_mode IS NOT NEW.claude_mode BEGIN \
           INSERT OR REPLACE INTO agent_defaults \
             (singleton, backend, model, effort, access, plan, fast, claude_mode) \
           VALUES (1, NEW.backend, NEW.model, NEW.effort, NEW.access, NEW.plan, \
             NEW.fast, NEW.claude_mode); END;",
    )?;
    transaction.execute(
        "INSERT OR IGNORE INTO agent_defaults \
         (singleton, backend, model, effort, access, plan, fast, claude_mode) \
         VALUES (1, 'codex', 'gpt-5.6-sol', 'high', 'edit', 0, 0, 0)",
        [],
    )?;
    transaction.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', '24') \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

fn hidden_component(relative_path: &str) -> bool {
    Path::new(relative_path).components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
    })
}

fn ancestors(relative_path: &str) -> impl Iterator<Item = &str> {
    let mut end = relative_path.len();
    std::iter::from_fn(move || {
        let slash = relative_path[..end].rfind('/')?;
        end = slash;
        Some(&relative_path[..slash])
    })
}

fn parent_workspace<'a>(
    relative_path: &str,
    rows: &'a HashMap<String, WorkspaceRow>,
) -> Option<&'a WorkspaceRow> {
    ancestors(relative_path).find_map(|ancestor| rows.get(ancestor))
}

fn validate_workspace_name(name: &str) -> Result<(), StorageError> {
    if name.starts_with('.') || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(StorageError::InvalidRequest(
            "A folder name cannot be empty or hidden, or contain a path separator.".into(),
        ));
    }
    Ok(())
}

fn workspace_relative(
    database: &Connection,
    root: &str,
    folder_id: &str,
) -> Result<String, StorageError> {
    database
        .query_row(
            "SELECT relative_path FROM workspace_folders WHERE root_path = ? AND id = ?",
            params![root, folder_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::InvalidRequest("No such folder on the daemon.".into()))
}

fn relative_from_root(root: &Path, path: &Path) -> Result<String, StorageError> {
    path.strip_prefix(root)
        .ok()
        .and_then(|relative| relative.to_str())
        .map(str::to_owned)
        .ok_or_else(|| StorageError::InvalidRequest("No such folder on the daemon.".into()))
}

fn relocate_workspace_subtree(
    database: &mut Connection,
    root: &str,
    old_relative: &str,
    new_relative: &str,
) -> Result<(), StorageError> {
    if old_relative == new_relative {
        return Ok(());
    }
    let transaction = database.transaction()?;
    let rows = {
        let mut statement = transaction
            .prepare("SELECT id, relative_path FROM workspace_folders WHERE root_path = ?")?;
        statement
            .query_map([root], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|result| match result {
                Ok((id, relative))
                    if relative == old_relative
                        || relative.starts_with(&format!("{old_relative}/")) =>
                {
                    Some(Ok((id, relative)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, rusqlite::Error>>()?
    };
    let now = now_seconds();
    for (id, relative) in rows {
        let suffix = relative.strip_prefix(old_relative).unwrap_or_default();
        transaction.execute(
            "UPDATE workspace_folders SET relative_path = ?, updated_at = ? \
             WHERE id = ? AND root_path = ?",
            params![format!("{new_relative}{suffix}"), now, id, root],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn queue_from_column(stored: Option<&str>) -> Vec<String> {
    let Some(stored) = stored.filter(|stored| !stored.is_empty()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(stored)
        .map(|queue| queue.into_iter().filter(|item| !item.is_empty()).collect())
        .unwrap_or_else(|_| vec![stored.to_owned()])
}

fn effective_shortcuts(
    database: &Connection,
    root: &Path,
    folder_id: &str,
) -> Result<Vec<String>, rusqlite::Error> {
    let global = database
        .query_row(
            "SELECT value FROM meta WHERE key = 'global_shortcuts'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| "[]".into());
    let relative = database
        .query_row(
            "SELECT relative_path FROM workspace_folders WHERE root_path = ? AND id = ?",
            params![root.to_string_lossy(), folder_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let mut values = serde_json::from_str::<Vec<String>>(&global).unwrap_or_default();
    if let Some(relative) = relative {
        let mut paths = ancestors(&relative).map(str::to_owned).collect::<Vec<_>>();
        paths.reverse();
        paths.push(relative);
        for path in paths {
            if let Some(shortcuts) = database
                .query_row(
                    "SELECT shortcuts FROM workspace_folders WHERE root_path = ? AND relative_path = ?",
                    params![root.to_string_lossy(), path],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            {
                values.extend(serde_json::from_str::<Vec<String>>(&shortcuts).unwrap_or_default());
            }
        }
    }
    let mut seen = HashSet::new();
    values.retain(|value| !value.is_empty() && seen.insert(value.clone()));
    Ok(values)
}

fn shortcut_fields(
    database: &Connection,
    root: &Path,
    folder_id: Option<&str>,
) -> Result<Value, StorageError> {
    let global = database
        .query_row(
            "SELECT value FROM meta WHERE key = 'global_shortcuts'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|stored| serde_json::from_str::<Vec<String>>(&stored).ok())
        .unwrap_or_default();
    let workspace = match folder_id {
        Some(folder_id) => database
            .query_row(
                "SELECT shortcuts FROM workspace_folders WHERE root_path = ? AND id = ?",
                params![root.to_string_lossy(), folder_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::InvalidRequest("No such folder on the daemon.".into()))
            .map(|stored| serde_json::from_str::<Vec<String>>(&stored).unwrap_or_default())?,
        None => Vec::new(),
    };
    let effective = match folder_id {
        Some(folder_id) => effective_shortcuts(database, root, folder_id)?,
        None => global.clone(),
    };
    Ok(json!({
        "ok": true,
        "global": global,
        "workspace": workspace,
        "effective": effective,
    }))
}

fn clean_shortcuts(value: &Value) -> Result<Vec<String>, StorageError> {
    let prompts = value.as_array().ok_or_else(|| {
        StorageError::InvalidRequest("set-shortcuts needs a shortcuts array.".into())
    })?;
    if prompts.len() > MAX_SHORTCUTS {
        return Err(StorageError::InvalidRequest(format!(
            "A shortcut list can contain at most {MAX_SHORTCUTS} prompts."
        )));
    }
    let mut cleaned = Vec::new();
    let mut seen = HashSet::new();
    for prompt in prompts {
        let prompt = prompt.as_str().ok_or_else(|| {
            StorageError::InvalidRequest("Every shortcut must be a text prompt.".into())
        })?;
        let prompt = prompt.trim();
        if prompt.is_empty() || !seen.insert(prompt.to_owned()) {
            continue;
        }
        if prompt.len() > MAX_SHORTCUT_BYTES {
            return Err(StorageError::InvalidRequest(format!(
                "A shortcut prompt can contain at most {MAX_SHORTCUT_BYTES} bytes."
            )));
        }
        cleaned.push(prompt.to_owned());
    }
    Ok(cleaned)
}

fn catalog_backend(
    id: &str,
    name: &str,
    default_model: &str,
    models: &[(&str, &str, i64)],
    extra_effort: &str,
) -> Value {
    let models = models
        .iter()
        .map(|(id, name, context_window)| {
            json!({"id": id, "name": name, "context_window": context_window})
        })
        .collect::<Vec<_>>();
    let mut efforts = BASE_EFFORTS.to_vec();
    efforts.push(extra_effort);
    json!({
        "id": id,
        "name": name,
        "default_model": default_model,
        "models": models,
        "efforts": efforts,
    })
}

fn validate_backend(backend: &str) -> Result<(), StorageError> {
    if matches!(backend, "codex" | "claude") {
        Ok(())
    } else {
        Err(StorageError::InvalidRequest("No such assistant.".into()))
    }
}

fn validate_model(backend: &str, model: &str) -> Result<(), StorageError> {
    validate_backend(backend)?;
    if backend_models(backend).iter().any(|known| known.0 == model) {
        Ok(())
    } else {
        Err(StorageError::InvalidRequest("No such model.".into()))
    }
}

fn backend_models(backend: &str) -> &'static [(&'static str, &'static str, i64)] {
    match backend {
        "codex" => CODEX_MODELS,
        "claude" => CLAUDE_MODELS,
        _ => &[],
    }
}

fn model_label<'a>(backend: &str, model: &'a str) -> &'a str {
    backend_models(backend)
        .iter()
        .find(|known| known.0 == model)
        .map(|known| known.1)
        .unwrap_or(model)
}

fn effort_supported(backend: &str, effort: &str) -> bool {
    BASE_EFFORTS.contains(&effort)
        || (backend == "codex" && effort == "ultra")
        || (backend == "claude" && effort == "ultracode")
}

fn update_boolean_option(
    transaction: &rusqlite::Transaction<'_>,
    chat_id: &str,
    column: &str,
    enabled: bool,
    now: i64,
) -> Result<usize, StorageError> {
    // The column is chosen only by the closed match in set_option.
    Ok(transaction.execute(
        &format!("UPDATE chats SET {column} = ?, updated_at = ? WHERE id = ?"),
        params![enabled, now, chat_id],
    )?)
}

fn prepare_turn(
    transaction: &rusqlite::Transaction<'_>,
    workspace_root: &Path,
    chat_id: &str,
    text: &str,
) -> Result<TurnSpec, StorageError> {
    let chat = transaction
        .query_row(
            "SELECT folder_id, backend, workdir, model, effort, access, plan FROM chats WHERE id = ?",
            [chat_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StorageError::NoChat(chat_id.into()))?;
    if chat.1 != "codex" {
        return Err(StorageError::InvalidRequest(
            "The Rust daemon currently runs Codex chats only.".into(),
        ));
    }
    let relative = transaction
        .query_row(
            "SELECT relative_path FROM workspace_folders WHERE root_path = ? AND id = ?",
            params![workspace_root.to_string_lossy(), chat.0],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::InvalidRequest("No such folder on the daemon.".into()))?;
    let mut inherited = None;
    let mut current = Some(relative.as_str());
    while inherited.is_none() {
        let Some(path) = current else { break };
        inherited = transaction
            .query_row(
                "SELECT COALESCE(workdir, repo) FROM workspace_folders \
                 WHERE root_path = ? AND relative_path = ?",
                params![workspace_root.to_string_lossy(), path],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        current = path.rsplit_once('/').map(|(parent, _)| parent);
    }
    let workdir = chat.2.unwrap_or_else(|| {
        inherited.unwrap_or_else(|| workspace_root.join(relative).to_string_lossy().into_owned())
    });
    if !Path::new(&workdir).is_dir() {
        return Err(StorageError::InvalidRequest(format!(
            "The chat working directory does not exist: {workdir}"
        )));
    }
    let model = chat.3.unwrap_or_else(|| "gpt-5.6-sol".into());
    let effort = chat.4.unwrap_or_else(|| "high".into());
    let access = if chat.6 {
        "read-only".into()
    } else {
        chat.5.unwrap_or_else(|| "read-only".into())
    };
    let session_id = transaction
        .query_row(
            "SELECT session_id FROM chat_sessions WHERE chat_id = ? AND backend = 'codex'",
            [chat_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let now = now_seconds();
    transaction.execute(
        "INSERT INTO messages (chat_id, role, content, created_at) VALUES (?, 'user', ?, ?)",
        params![chat_id, text, now],
    )?;
    transaction.execute(
        "UPDATE chats SET updated_at = ?, last_user_message_at = ? WHERE id = ?",
        params![now, now_microseconds(), chat_id],
    )?;
    Ok(TurnSpec {
        chat_id: chat_id.into(),
        prompt: text.into(),
        workdir,
        model: model.clone(),
        effort: effort.clone(),
        access,
        session_id,
        label: format!(
            "{} · {}",
            model_label("codex", &model),
            effort_label(&effort)
        ),
    })
}

fn effort_label(effort: &str) -> &str {
    match effort {
        "low" => "Low",
        "medium" => "Medium",
        "xhigh" => "Extra high",
        "max" => "Max",
        "ultra" => "Ultra",
        _ => "High",
    }
}

fn queued_event(chat_id: &str, queue: &[String]) -> Value {
    let mut event = json!({"event": "queued", "chat": chat_id, "queue": queue});
    if let Some(first) = queue.first() {
        event["text"] = Value::String(first.clone());
    }
    event
}

fn required_string<'a>(
    request: &'a Value,
    key: &str,
    message: &str,
) -> Result<&'a str, StorageError> {
    request
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| StorageError::InvalidRequest(message.into()))
}

fn required_string_allow_empty<'a>(
    request: &'a Value,
    key: &str,
    message: &str,
) -> Result<&'a str, StorageError> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| StorageError::InvalidRequest(message.into()))
}

fn optional_integer(request: &Value, key: &str) -> Result<Option<i64>, StorageError> {
    match request.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| StorageError::InvalidRequest(format!("{key} must be an integer"))),
    }
}

fn optional_string<'a>(request: &'a Value, key: &str) -> Result<Option<&'a str>, StorageError> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| StorageError::InvalidRequest(format!("{key} must be text"))),
    }
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn now_microseconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(i64::MAX as u128) as i64
}

fn validate_attachments(
    value: &Value,
    allow_empty: bool,
) -> Result<Vec<ValidatedAttachment>, StorageError> {
    let attachments = value.as_array().ok_or_else(|| {
        StorageError::InvalidRequest("Message attachments must be an array.".into())
    })?;
    if attachments.len() > MAX_IMAGES || (!allow_empty && attachments.is_empty()) {
        return Err(StorageError::InvalidRequest(
            if allow_empty {
                "A draft can contain at most 4 images."
            } else {
                "A message can contain between 1 and 4 images."
            }
            .into(),
        ));
    }
    let mut total = 0_usize;
    attachments
        .iter()
        .map(|attachment| {
            let object = attachment.as_object().ok_or_else(|| {
                StorageError::InvalidRequest("Only PNG images up to 10 MiB can be sent.".into())
            })?;
            let mime = object.get("mime").and_then(Value::as_str);
            let encoded = object.get("data").and_then(Value::as_str).ok_or_else(|| {
                StorageError::InvalidRequest("Only PNG images up to 10 MiB can be sent.".into())
            })?;
            let encoded_limit = MAX_IMAGE_BYTES.div_ceil(3) * 4;
            if mime != Some("image/png") || encoded.len() > encoded_limit {
                return Err(StorageError::InvalidRequest(
                    "Only PNG images up to 10 MiB can be sent.".into(),
                ));
            }
            let data = STANDARD.decode(encoded).map_err(|_| {
                StorageError::InvalidRequest("The attached images are invalid or too large.".into())
            })?;
            if data.len() > MAX_IMAGE_BYTES
                || !data.starts_with(PNG_SIGNATURE)
                || total > MAX_TOTAL_IMAGE_BYTES.saturating_sub(data.len())
            {
                return Err(StorageError::InvalidRequest(
                    "The attached images are invalid or too large.".into(),
                ));
            }
            total += data.len();
            let supplied = object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("image.png");
            let mut name = Path::new(supplied)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image.png");
            if name.is_empty() || name.len() > 255 {
                name = "image.png";
            }
            Ok(ValidatedAttachment {
                name: name.to_owned(),
                encoded: encoded.to_owned(),
                data,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn initializes_a_complete_database_on_first_run() {
        let fixture = Fixture::new();
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();
        let folder = store.new_folder(&json!({"name": "First"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let chat = store.new_chat(&json!({"folder": folder})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(store.chat(&chat).unwrap()["backend"], "codex");
        let database = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            database
                .query_row(
                    "SELECT value FROM meta WHERE key = 'schema_version'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "24"
        );
        let fts_exists: bool = database
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'messages_fts')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(fts_exists);
    }

    #[test]
    fn reads_visible_workspace_and_chat_snapshots() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.workspaces.join("Repo/.git")).unwrap();
        fs::create_dir_all(fixture.workspaces.join("Repo/HiddenByRepo")).unwrap();
        fs::create_dir_all(fixture.workspaces.join("Group/Child")).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        for (id, relative) in [
            ("repo", "Repo"),
            ("repo-child", "Repo/HiddenByRepo"),
            ("group", "Group"),
            ("child", "Group/Child"),
        ] {
            database
                .execute(
                    "INSERT INTO workspace_folders \
                     (id, root_path, relative_path, shortcuts) VALUES (?, ?, ?, '[]')",
                    params![id, fixture.workspaces.to_string_lossy(), relative],
                )
                .unwrap();
        }
        fixture.insert_chat(&database, "chat-1", "child");
        drop(database);

        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();
        let tree = store.tree().unwrap();
        assert_eq!(tree["folders"].as_array().unwrap().len(), 3);
        assert!(
            tree["folders"]
                .as_array()
                .unwrap()
                .iter()
                .all(|folder| folder["id"] != "repo-child")
        );
        let child = tree["folders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|folder| folder["id"] == "child")
            .unwrap();
        assert_eq!(child["parent"], "group");
        assert_eq!(tree["chats"][0]["id"], "chat-1");
        assert_eq!(tree["chats"][0]["working"], false);
    }

    #[test]
    fn reads_chat_draft_queue_and_effective_shortcuts() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.workspaces.join("Group/Child")).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        database
            .execute(
                "INSERT INTO meta (key, value) VALUES ('global_shortcuts', '[\"Global\"]')",
                [],
            )
            .unwrap();
        for (id, relative, shortcuts) in [
            ("group", "Group", "[\"Parent\"]"),
            ("child", "Group/Child", "[\"Child\",\"Global\"]"),
        ] {
            database
                .execute(
                    "INSERT INTO workspace_folders \
                     (id, root_path, relative_path, shortcuts) VALUES (?, ?, ?, ?)",
                    params![
                        id,
                        fixture.workspaces.to_string_lossy(),
                        relative,
                        shortcuts
                    ],
                )
                .unwrap();
        }
        fixture.insert_chat(&database, "chat-1", "child");
        database
            .execute(
                "UPDATE chats SET queued = '[\"First\",\"Second\"]', draft = 'unfinished', \
                 draft_revision = 4, model = 'gpt-5', effort = 'medium' WHERE id = 'chat-1'",
                [],
            )
            .unwrap();
        drop(database);

        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();
        let chat = store.chat("chat-1").unwrap();
        assert_eq!(chat["queue"], json!(["First", "Second"]));
        assert_eq!(chat["queued"], "First");
        assert_eq!(chat["draft"], "unfinished");
        assert_eq!(chat["draft_revision"], 4);
        assert_eq!(chat["model"], "gpt-5");
        assert_eq!(chat["shortcuts"], json!(["Global", "Parent", "Child"]));
    }

    #[test]
    fn pages_recent_messages_oldest_to_newest() {
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.workspaces).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        for (role, content, at, label) in [
            ("user", "one", 1, None),
            ("assistant", "two", 2, Some("Codex")),
            ("tool", "three", 3, None),
        ] {
            database
                .execute(
                    "INSERT INTO messages (chat_id, role, content, created_at, label) \
                     VALUES ('chat-1', ?, ?, ?, ?)",
                    params![role, content, at, label],
                )
                .unwrap();
        }
        drop(database);

        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();
        let messages = store
            .messages(&json!({"chat": "chat-1", "limit": 2}))
            .unwrap();
        assert_eq!(messages["total_messages"], 3);
        assert_eq!(messages["offset"], 1);
        assert_eq!(messages["messages"][0]["content"], "two");
        assert_eq!(messages["messages"][0]["label"], "Codex");
        assert_eq!(messages["messages"][1]["content"], "three");
    }

    #[test]
    fn writes_drafts_and_normalizes_synced_attachment_previews() {
        let fixture = Fixture::new();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        drop(database);

        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();
        let (reply, event) = store
            .set_draft(&json!({
                "chat": "chat-1",
                "text": "shared input",
                "attachments": [{
                    "name": "../preview.png",
                    "mime": "image/png",
                    "data": "iVBORw0KGgo="
                }]
            }))
            .unwrap();
        assert_eq!(reply["draft"], "shared input");
        assert_eq!(reply["draft_revision"], 1);
        assert_eq!(reply["draft_attachments"][0]["name"], "preview.png");
        assert_eq!(event["event"], "draft");
        assert_eq!(event["chat"], "chat-1");
        assert_eq!(event["draft_attachments"], reply["draft_attachments"]);

        let snapshot = store.chat("chat-1").unwrap();
        assert_eq!(snapshot["draft"], "shared input");
        assert_eq!(snapshot["draft_revision"], 1);
        assert_eq!(snapshot["draft_attachments"], reply["draft_attachments"]);
    }

    #[test]
    fn rejects_non_png_draft_attachments_without_mutating_the_draft() {
        let fixture = Fixture::new();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        drop(database);

        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();
        let error = store
            .set_draft(&json!({
                "chat": "chat-1",
                "text": "must not persist",
                "attachments": [{
                    "name": "bad.png",
                    "mime": "image/png",
                    "data": "bm90IGEgcG5n"
                }]
            }))
            .unwrap_err();
        assert!(error.to_string().contains("invalid or too large"));
        assert_eq!(store.chat("chat-1").unwrap()["draft"], "");
    }

    #[test]
    fn adopts_existing_directories_and_creates_nested_workspaces() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.workspaces.join("Existing")).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        let existing = store.new_folder(&json!({"name": "Existing"})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let child = store
            .new_folder(&json!({"name": "Child", "parent": existing}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(fixture.workspaces.join("Existing/Child").is_dir());
        let tree = store.tree().unwrap();
        assert_eq!(tree["folders"].as_array().unwrap().len(), 2);
        assert_eq!(
            tree["folders"]
                .as_array()
                .unwrap()
                .iter()
                .find(|folder| folder["id"] == child)
                .unwrap()["parent"],
            existing
        );
    }

    #[test]
    fn renames_moves_and_recovers_workspace_subtrees_through_trash() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.workspaces.join("Source/Child/Grandchild")).unwrap();
        fs::create_dir_all(fixture.workspaces.join("Target")).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        for (id, relative) in [
            ("source", "Source"),
            ("child", "Source/Child"),
            ("grandchild", "Source/Child/Grandchild"),
            ("target", "Target"),
        ] {
            database
                .execute(
                    "INSERT INTO workspace_folders \
                     (id, root_path, relative_path, shortcuts, created_at, updated_at) \
                     VALUES (?, ?, ?, '[]', 0, 0)",
                    params![id, fixture.workspaces.to_string_lossy(), relative],
                )
                .unwrap();
        }
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        store
            .rename_folder(&json!({"folder": "source", "name": "Renamed"}))
            .unwrap();
        assert!(fixture.workspaces.join("Renamed/Child/Grandchild").is_dir());
        store
            .move_folder(&json!({"folder": "child", "parent": "target"}))
            .unwrap();
        assert!(fixture.workspaces.join("Target/Child/Grandchild").is_dir());
        let tree = store.tree().unwrap();
        assert_eq!(
            tree["folders"]
                .as_array()
                .unwrap()
                .iter()
                .find(|folder| folder["id"] == "child")
                .unwrap()["parent"],
            "target"
        );
        assert!(
            store
                .move_folder(&json!({"folder": "target", "parent": "grandchild"}))
                .unwrap_err()
                .to_string()
                .contains("inside itself")
        );

        store.trash_folder(&json!({"folder": "target"})).unwrap();
        assert_eq!(
            store.tree().unwrap()["folders"].as_array().unwrap().len(),
            1
        );
        let database = Connection::open(&fixture.database).unwrap();
        let child_relative: String = database
            .query_row(
                "SELECT relative_path FROM workspace_folders WHERE id = 'child'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(child_relative.starts_with(".Trash/"));
        assert!(child_relative.ends_with("/Child"));
    }

    #[test]
    fn workspace_moves_reject_collisions_and_repository_parents() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.workspaces.join("One/Same")).unwrap();
        fs::create_dir_all(fixture.workspaces.join("Two/Same")).unwrap();
        fs::create_dir_all(fixture.workspaces.join("Repo/.git")).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        for (id, relative) in [
            ("one", "One"),
            ("same-one", "One/Same"),
            ("two", "Two"),
            ("same-two", "Two/Same"),
            ("repo", "Repo"),
        ] {
            database
                .execute(
                    "INSERT INTO workspace_folders \
                     (id, root_path, relative_path, shortcuts, created_at, updated_at) \
                     VALUES (?, ?, ?, '[]', 0, 0)",
                    params![id, fixture.workspaces.to_string_lossy(), relative],
                )
                .unwrap();
        }
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        assert!(
            store
                .move_folder(&json!({"folder": "same-one", "parent": "two"}))
                .unwrap_err()
                .to_string()
                .contains("already a folder")
        );
        assert!(
            store
                .move_folder(&json!({"folder": "same-one", "parent": "repo"}))
                .unwrap_err()
                .to_string()
                .contains("repository")
        );
        assert!(fixture.workspaces.join("One/Same").is_dir());
    }

    #[test]
    fn creates_renames_moves_and_deletes_chats() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.workspaces.join("One")).unwrap();
        fs::create_dir_all(fixture.workspaces.join("Two")).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        for (id, relative) in [("one", "One"), ("two", "Two")] {
            database
                .execute(
                    "INSERT INTO workspace_folders \
                     (id, root_path, relative_path, shortcuts, created_at, updated_at) \
                     VALUES (?, ?, ?, '[]', 0, 0)",
                    params![id, fixture.workspaces.to_string_lossy(), relative],
                )
                .unwrap();
        }
        database
            .execute(
                "INSERT INTO agent_defaults \
                 (singleton, backend, model, effort, access, plan, fast, claude_mode) \
                 VALUES (1, 'codex', 'gpt-5', 'high', 'workspace-write', 0, 1, 0)",
                [],
            )
            .unwrap();
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        let chat_id = store
            .new_chat(&json!({"folder": "one", "title": "Original"}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(store.chat(&chat_id).unwrap()["backend"], "codex");
        assert_eq!(store.chat(&chat_id).unwrap()["model"], "gpt-5");
        store
            .rename_chat(&json!({"chat": chat_id, "title": "Renamed"}))
            .unwrap();
        store
            .move_chat(&json!({"chat": chat_id, "folder": "two"}))
            .unwrap();
        let tree = store.tree().unwrap();
        let chat = tree["chats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|chat| chat["id"] == chat_id)
            .unwrap();
        assert_eq!(chat["title"], "Renamed");
        assert_eq!(chat["folder"], "two");
        store.delete_chat(&json!({"chat": chat_id})).unwrap();
        assert!(matches!(store.chat(&chat_id), Err(StorageError::NoChat(_))));
    }

    #[test]
    fn mutates_queues_transactionally_and_emits_complete_state() {
        let fixture = Fixture::new();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        let (_, event) = store
            .queue(&json!({"chat": "chat-1", "text": "first"}))
            .unwrap();
        assert_eq!(
            event,
            json!({
                "event": "queued", "chat": "chat-1", "queue": ["first"], "text": "first"
            })
        );
        store
            .queue(&json!({"chat": "chat-1", "text": "second"}))
            .unwrap();

        let (_, event) = store
            .edit_queue(&json!({
                "chat": "chat-1", "index": 1, "old-text": "second", "text": "edited"
            }))
            .unwrap();
        assert_eq!(event["queue"], json!(["first", "edited"]));
        let conflict = store
            .edit_queue(&json!({
                "chat": "chat-1", "index": 1, "old-text": "second", "text": "lost update"
            }))
            .unwrap_err();
        assert!(conflict.to_string().contains("changed; try again"));

        let (_, event) = store
            .steer_queue(&json!({"chat": "chat-1", "index": 1, "text": "edited"}))
            .unwrap();
        assert_eq!(event["queue"], json!(["edited", "first"]));
        let (_, event) = store
            .drop_queue(&json!({"chat": "chat-1", "index": 0}))
            .unwrap();
        assert_eq!(event["queue"], json!(["first"]));
        let (_, event) = store.drop_queue(&json!({"chat": "chat-1"})).unwrap();
        assert_eq!(event["queue"], json!([]));
        assert!(event.get("text").is_none());
        assert_eq!(store.chat("chat-1").unwrap()["queue"], json!([]));
    }

    #[test]
    fn queue_mutations_upgrade_legacy_single_message_storage() {
        let fixture = Fixture::new();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        database
            .execute("UPDATE chats SET queued = 'legacy' WHERE id = 'chat-1'", [])
            .unwrap();
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        let (_, event) = store
            .queue(&json!({"chat": "chat-1", "text": "new"}))
            .unwrap();
        assert_eq!(event["queue"], json!(["legacy", "new"]));
    }

    #[test]
    fn persists_clean_global_and_inherited_workspace_shortcuts() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.workspaces.join("Group/Child")).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        for (id, relative) in [("group", "Group"), ("child", "Group/Child")] {
            database
                .execute(
                    "INSERT INTO workspace_folders \
                     (id, root_path, relative_path, shortcuts) VALUES (?, ?, ?, '[]')",
                    params![id, fixture.workspaces.to_string_lossy(), relative],
                )
                .unwrap();
        }
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        let (global, global_event) = store
            .set_shortcuts(&json!({
                "shortcuts": ["  Review diff  ", "", "Review diff", "Run tests"]
            }))
            .unwrap();
        assert_eq!(global["global"], json!(["Review diff", "Run tests"]));
        assert_eq!(global["effective"], global["global"]);
        assert!(global_event.get("folder").is_none());
        store
            .set_shortcuts(&json!({"folder": "group", "shortcuts": ["Parent"]}))
            .unwrap();
        let (child, child_event) = store
            .set_shortcuts(&json!({
                "folder": "child", "shortcuts": ["Child", "Review diff"]
            }))
            .unwrap();
        assert_eq!(child["workspace"], json!(["Child", "Review diff"]));
        assert_eq!(
            child["effective"],
            json!(["Review diff", "Run tests", "Parent", "Child"])
        );
        assert_eq!(child_event["folder"], "child");
        assert_eq!(store.shortcuts(&json!({"folder": "child"})).unwrap(), child);
    }

    #[test]
    fn rejects_invalid_shortcuts_without_replacing_existing_values() {
        let fixture = Fixture::new();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();
        store
            .set_shortcuts(&json!({"shortcuts": ["Keep"]}))
            .unwrap();

        let error = store
            .set_shortcuts(&json!({"shortcuts": ["valid", 42]}))
            .unwrap_err();
        assert!(error.to_string().contains("text prompt"));
        assert_eq!(
            store.shortcuts(&json!({})).unwrap()["global"],
            json!(["Keep"])
        );
    }

    #[test]
    fn publishes_the_same_models_that_atomic_selection_accepts() {
        let fixture = Fixture::new();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        for backend in store.agent_catalog().unwrap()["backends"]
            .as_array()
            .unwrap()
        {
            for model in backend["models"].as_array().unwrap() {
                store
                    .set_option(&json!({
                        "chat": "chat-1",
                        "option": "model",
                        "backend": backend["id"],
                        "value": model["id"],
                    }))
                    .unwrap();
            }
        }
    }

    #[test]
    fn validates_and_applies_chat_options_atomically() {
        let fixture = Fixture::new();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        store
            .set_option(&json!({
                "chat": "chat-1", "option": "model", "backend": "codex", "value": "gpt-5.6-sol"
            }))
            .unwrap();
        store
            .set_option(&json!({"chat": "chat-1", "option": "effort", "value": "ultra"}))
            .unwrap();
        store
            .set_option(&json!({"chat": "chat-1", "option": "claude-mode", "value": "true"}))
            .unwrap();
        let chat = store.chat("chat-1").unwrap();
        assert_eq!(chat["backend"], "codex");
        assert_eq!(chat["model"], "gpt-5.6-sol");
        assert_eq!(chat["effort"], "max");
        store
            .set_option(&json!({"chat": "chat-1", "option": "backend", "value": "claude"}))
            .unwrap();
        let chat = store.chat("chat-1").unwrap();
        assert_eq!(chat["backend"], "claude");
        assert_eq!(chat["fast"], false);
        assert_eq!(chat["claude_mode"], false);
        let events = store.messages(&json!({"chat": "chat-1"})).unwrap()["messages"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(events[0]["role"], "event");
        assert_eq!(events[0]["content"], "Switched to GPT-5.6 Sol");
    }

    #[test]
    fn rejects_incompatible_effort_without_partial_option_changes() {
        let fixture = Fixture::new();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();
        store
            .set_option(&json!({
                "chat": "chat-1", "option": "backend", "value": "claude"
            }))
            .unwrap();

        let error = store
            .set_option(&json!({
                "chat": "chat-1", "option": "effort", "value": "ultra"
            }))
            .unwrap_err();
        assert!(error.to_string().contains("not available"));
        assert_eq!(store.chat("chat-1").unwrap()["effort"], "high");
    }

    #[test]
    fn send_starts_once_then_hands_queued_text_to_the_next_turn() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.workspaces.join("folder")).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();

        let first = store
            .prepare_send(&json!({"chat": "chat-1", "text": "first"}))
            .unwrap();
        let SendDisposition::Start { reply, turn } = first else {
            panic!("idle send should start");
        };
        assert_eq!(reply["queued"], false);
        assert_eq!(turn.prompt, "first");
        assert_eq!(
            turn.workdir,
            fixture.workspaces.join("folder").to_string_lossy()
        );

        let second = store
            .prepare_send(&json!({"chat": "chat-1", "text": "second"}))
            .unwrap();
        let SendDisposition::Queued { reply, event } = second else {
            panic!("working send should queue");
        };
        assert_eq!(reply["queued"], true);
        assert_eq!(event["queue"], json!(["second"]));
        assert_eq!(
            store.messages(&json!({"chat": "chat-1"})).unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let finish = store.finish_turn("chat-1", true, None, 2, false).unwrap();
        let next = finish.next.expect("queued turn");
        assert_eq!(next.prompt, "second");
        assert_eq!(finish.queue_event.unwrap()["queue"], json!([]));
        assert_eq!(store.chat("chat-1").unwrap()["working"], true);
        store.finish_turn("chat-1", true, None, 1, false).unwrap();
        assert_eq!(store.chat("chat-1").unwrap()["working"], false);
    }

    #[test]
    fn materializes_sent_images_for_immediate_and_queued_turns() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.workspaces.join("folder")).unwrap();
        let database = Connection::open(&fixture.database).unwrap();
        fixture.schema(&database);
        fixture.insert_chat(&database, "chat-1", "folder");
        drop(database);
        let store = StateStore::open(&fixture.database, &fixture.workspaces).unwrap();
        let encoded = STANDARD.encode(PNG_SIGNATURE);

        let first = store
            .prepare_send(&json!({
                "chat": "chat-1",
                "text": "inspect this",
                "attachments": [{
                    "name": "../screen.png",
                    "mime": "image/png",
                    "data": encoded.clone(),
                }],
            }))
            .unwrap();
        let SendDisposition::Start { turn, .. } = first else {
            panic!("idle image send should start");
        };
        let first_path = turn
            .prompt
            .lines()
            .nth(1)
            .and_then(|line| line.strip_prefix("[image: "))
            .and_then(|line| line.strip_suffix(']'))
            .map(PathBuf::from)
            .expect("materialized image reference");
        assert!(first_path.starts_with(fixture.root.join("remote-pasted")));
        assert_eq!(fs::read(&first_path).unwrap(), PNG_SIGNATURE);
        assert_eq!(
            fs::metadata(&first_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let second = store
            .prepare_send(&json!({
                "chat": "chat-1",
                "attachments": [{"mime": "image/png", "data": encoded}],
            }))
            .unwrap();
        let SendDisposition::Queued { event, .. } = second else {
            panic!("working image send should queue");
        };
        assert!(event["queue"][0].as_str().unwrap().starts_with("[image: "));
        let next = store
            .finish_turn("chat-1", true, None, 1, false)
            .unwrap()
            .next
            .expect("queued image turn");
        assert_eq!(next.prompt, event["queue"][0]);
    }

    struct Fixture {
        root: PathBuf,
        database: PathBuf,
        workspaces: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = env::temp_dir().join(format!(
                "xd-rust-storage-test-{}-{}",
                std::process::id(),
                NEXT_TEST.fetch_add(1, Ordering::Relaxed)
            ));
            let database = root.join("chats.db");
            let workspaces = root.join("Workspaces");
            fs::create_dir_all(&workspaces).unwrap();
            Self {
                root,
                database,
                workspaces,
            }
        }

        fn schema(&self, database: &Connection) {
            database
                .execute_batch(
                    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); \
                     CREATE TABLE workspace_folders (id TEXT PRIMARY KEY, root_path TEXT NOT NULL, \
                       relative_path TEXT NOT NULL, backend TEXT, model TEXT, workdir TEXT, repo TEXT, \
                       instructions TEXT, shortcuts TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT 0, \
                       updated_at INTEGER NOT NULL DEFAULT 0, UNIQUE(root_path, relative_path)); \
                     CREATE TABLE chats (id TEXT PRIMARY KEY, folder_id TEXT NOT NULL, title TEXT, \
                       backend TEXT NOT NULL, workdir TEXT, model TEXT, effort TEXT, access TEXT, \
                       plan INTEGER NOT NULL DEFAULT 0, fast INTEGER NOT NULL DEFAULT 0, \
                       claude_mode INTEGER NOT NULL DEFAULT 0, queued TEXT, \
                       new_worktree INTEGER NOT NULL DEFAULT 0, daemon_working INTEGER NOT NULL DEFAULT 0, \
                       draft TEXT NOT NULL DEFAULT '', draft_attachments TEXT NOT NULL DEFAULT '[]', \
                       draft_revision INTEGER NOT NULL DEFAULT 0, last_user_message_at INTEGER NOT NULL DEFAULT 0, \
                       created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0, \
                       FOREIGN KEY(folder_id) REFERENCES workspace_folders(id)); \
                     CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, chat_id TEXT NOT NULL, \
                       role TEXT NOT NULL, content TEXT NOT NULL, created_at INTEGER NOT NULL, label TEXT, \
                       FOREIGN KEY(chat_id) REFERENCES chats(id) ON DELETE CASCADE); \
                     CREATE TABLE chat_sessions (chat_id TEXT NOT NULL, backend TEXT NOT NULL, \
                       session_id TEXT, last_message_id INTEGER NOT NULL DEFAULT 0, \
                       PRIMARY KEY(chat_id, backend), \
                       FOREIGN KEY(chat_id) REFERENCES chats(id) ON DELETE CASCADE); \
                     CREATE TABLE agent_defaults (singleton INTEGER PRIMARY KEY, backend TEXT NOT NULL, \
                       model TEXT, effort TEXT, access TEXT, plan INTEGER NOT NULL, fast INTEGER NOT NULL, \
                       claude_mode INTEGER NOT NULL);",
                )
                .unwrap();
        }

        fn insert_chat(&self, database: &Connection, id: &str, folder: &str) {
            database
                .execute(
                    "INSERT OR IGNORE INTO workspace_folders \
                     (id, root_path, relative_path, shortcuts) VALUES (?, ?, ?, '[]')",
                    params![folder, self.workspaces.to_string_lossy(), folder],
                )
                .unwrap();
            database
                .execute(
                    "INSERT INTO chats (id, folder_id, title, backend) VALUES (?, ?, 'A chat', 'codex')",
                    params![id, folder],
                )
                .unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
