use std::{
    collections::{HashMap, HashSet},
    fs,
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
const MAX_IMAGES: usize = 4;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

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
        Self::open_with_flags(
            database_path,
            workspace_root,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
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
        Ok(Self {
            database: Mutex::new(database),
            workspace_root: workspace_root.into(),
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
            .map(validate_attachments)
            .transpose()?;
        let attachments_json = attachments
            .as_ref()
            .map(serde_json::to_string)
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
        if name.starts_with('.') || name == ".." || name.contains('/') || name.contains('\\') {
            return Err(StorageError::InvalidRequest(
                "A folder name cannot be empty or hidden, or contain a path separator.".into(),
            ));
        }
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

fn validate_attachments(value: &Value) -> Result<Vec<Value>, StorageError> {
    let attachments = value.as_array().ok_or_else(|| {
        StorageError::InvalidRequest("Message attachments must be an array.".into())
    })?;
    if attachments.len() > MAX_IMAGES {
        return Err(StorageError::InvalidRequest(
            "A draft can contain at most 4 images.".into(),
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
            Ok(json!({"name": name, "mime": "image/png", "data": encoded}))
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
