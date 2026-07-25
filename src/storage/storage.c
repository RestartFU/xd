#include "storage.h"

#include <errno.h>
#include <sqlite3.h>

#define HY_STORAGE_SCHEMA_VERSION 2

struct _HyStorage
{
  GObject parent_instance;

  sqlite3 *db;
};

G_DEFINE_FINAL_TYPE (HyStorage, hy_storage, G_TYPE_OBJECT)

void
hy_chat_free (HyChat *self)
{
  if (self == NULL)
    return;

  g_free (self->id);
  g_free (self->folder_id);
  g_free (self->title);
  g_free (self->backend);
  g_free (self->session_id);
  g_free (self->workdir);
  g_free (self);
}

void
hy_message_free (HyMessage *self)
{
  if (self == NULL)
    return;

  g_free (self->chat_id);
  g_free (self->role);
  g_free (self->content);
  g_free (self->raw_json);
  g_free (self);
}

/* --- sqlite plumbing ------------------------------------------------------ */

static void
set_sqlite_error (GError    **error,
                  sqlite3    *db,
                  const char *what)
{
  g_set_error (error, G_IO_ERROR, G_IO_ERROR_FAILED,
               "%s: %s", what, sqlite3_errmsg (db));
}

static gboolean
exec_sql (HyStorage   *self,
          const char  *sql,
          GError     **error)
{
  char *message = NULL;

  if (sqlite3_exec (self->db, sql, NULL, NULL, &message) != SQLITE_OK)
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_FAILED, "%s", message);
      sqlite3_free (message);
      return FALSE;
    }

  return TRUE;
}

/* Binds NULL for a NULL string, which SQLite and our schema both want. */
static void
bind_text (sqlite3_stmt *stmt,
           int           index,
           const char   *value)
{
  if (value != NULL)
    sqlite3_bind_text (stmt, index, value, -1, SQLITE_TRANSIENT);
  else
    sqlite3_bind_null (stmt, index);
}

static char *
column_text (sqlite3_stmt *stmt,
             int           index)
{
  const unsigned char *text = sqlite3_column_text (stmt, index);

  return text != NULL ? g_strdup ((const char *) text) : NULL;
}

/* --- schema --------------------------------------------------------------- */

static const char *SCHEMA_SQL =
  "CREATE TABLE IF NOT EXISTS meta ("
  "  key   TEXT PRIMARY KEY,"
  "  value TEXT NOT NULL"
  ");"

  "CREATE TABLE IF NOT EXISTS chats ("
  "  id         TEXT PRIMARY KEY,"
  "  folder_id  TEXT NOT NULL,"
  "  title      TEXT,"
  "  backend    TEXT NOT NULL,"
  "  session_id TEXT,"
  "  created_at INTEGER NOT NULL,"
  "  updated_at INTEGER NOT NULL"
  ");"
  "CREATE INDEX IF NOT EXISTS chats_folder ON chats (folder_id, updated_at DESC);"

  "CREATE TABLE IF NOT EXISTS messages ("
  "  id         INTEGER PRIMARY KEY AUTOINCREMENT,"
  "  chat_id    TEXT NOT NULL REFERENCES chats (id) ON DELETE CASCADE,"
  "  role       TEXT NOT NULL,"
  "  content    TEXT NOT NULL,"
  "  raw_json   TEXT,"
  "  created_at INTEGER NOT NULL"
  ");"
  "CREATE INDEX IF NOT EXISTS messages_chat ON messages (chat_id, id);"

  /* External-content FTS: the index mirrors messages.content and is kept in
   * step by triggers, so there is only ever one copy of the text. */
  "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5 ("
  "  content,"
  "  content='messages',"
  "  content_rowid='id'"
  ");"

  "CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN"
  "  INSERT INTO messages_fts (rowid, content) VALUES (new.id, new.content);"
  "END;"
  "CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN"
  "  INSERT INTO messages_fts (messages_fts, rowid, content)"
  "    VALUES ('delete', old.id, old.content);"
  "END;"
  "CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE ON messages BEGIN"
  "  INSERT INTO messages_fts (messages_fts, rowid, content)"
  "    VALUES ('delete', old.id, old.content);"
  "  INSERT INTO messages_fts (rowid, content) VALUES (new.id, new.content);"
  "END;";

static int
read_schema_version (HyStorage *self)
{
  sqlite3_stmt *stmt = NULL;
  int version = 0;

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT value FROM meta WHERE key = 'schema_version';",
                          -1, &stmt, NULL) != SQLITE_OK)
    return 0;

  if (sqlite3_step (stmt) == SQLITE_ROW)
    version = sqlite3_column_int (stmt, 0);

  sqlite3_finalize (stmt);

  return version;
}

static gboolean
migrate (HyStorage  *self,
         GError    **error)
{
  g_autofree char *sql = NULL;
  int version;

  if (!exec_sql (self, SCHEMA_SQL, error))
    return FALSE;

  /* A fresh database reports version 0, so it walks the same upgrade path as
   * an old one; there is only ever one way a column gets added. */
  version = read_schema_version (self);

  if (version < 2 &&
      !exec_sql (self, "ALTER TABLE chats ADD COLUMN workdir TEXT;", error))
    return FALSE;

  sql = g_strdup_printf ("INSERT INTO meta (key, value) VALUES ('schema_version', '%d')"
                         "  ON CONFLICT (key) DO UPDATE SET value = excluded.value;",
                         HY_STORAGE_SCHEMA_VERSION);

  return exec_sql (self, sql, error);
}

HyStorage *
hy_storage_new (const char  *db_path,
                GError     **error)
{
  g_autoptr (HyStorage) self = NULL;
  g_autofree char *dir = NULL;

  g_return_val_if_fail (db_path != NULL, NULL);

  dir = g_path_get_dirname (db_path);
  if (g_mkdir_with_parents (dir, 0700) != 0)
    {
      g_set_error (error, G_IO_ERROR, g_io_error_from_errno (errno),
                   "Cannot create %s", dir);
      return NULL;
    }

  self = g_object_new (HY_TYPE_STORAGE, NULL);

  if (sqlite3_open (db_path, &self->db) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot open the chat database");
      return NULL;
    }

  /* WAL plus NORMAL keeps the per-turn writes off the critical path while
   * still surviving a crash of the application. */
  if (!exec_sql (self, "PRAGMA journal_mode = WAL;"
                       "PRAGMA synchronous = NORMAL;"
                       "PRAGMA foreign_keys = ON;", error))
    return NULL;

  if (!migrate (self, error))
    return NULL;

  return g_steal_pointer (&self);
}

/* --- chats ---------------------------------------------------------------- */

char *
hy_storage_create_chat (HyStorage   *self,
                        const char  *folder_id,
                        const char  *title,
                        const char  *backend,
                        const char  *workdir,
                        GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  g_autofree char *id = NULL;
  gint64 now;

  g_return_val_if_fail (HY_IS_STORAGE (self), NULL);
  g_return_val_if_fail (folder_id != NULL, NULL);
  g_return_val_if_fail (backend != NULL, NULL);

  id = g_uuid_string_random ();
  now = g_get_real_time () / G_USEC_PER_SEC;

  if (sqlite3_prepare_v2 (self->db,
                          "INSERT INTO chats (id, folder_id, title, backend,"
                          "                   session_id, workdir, created_at, updated_at)"
                          " VALUES (?, ?, ?, ?, NULL, ?, ?, ?);",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot create the chat");
      return NULL;
    }

  bind_text (stmt, 1, id);
  bind_text (stmt, 2, folder_id);
  bind_text (stmt, 3, title);
  bind_text (stmt, 4, backend);
  bind_text (stmt, 5, workdir);
  sqlite3_bind_int64 (stmt, 6, now);
  sqlite3_bind_int64 (stmt, 7, now);

  if (sqlite3_step (stmt) != SQLITE_DONE)
    {
      set_sqlite_error (error, self->db, "Cannot create the chat");
      sqlite3_finalize (stmt);
      return NULL;
    }

  sqlite3_finalize (stmt);

  return g_steal_pointer (&id);
}

static HyChat *
chat_from_row (sqlite3_stmt *stmt)
{
  HyChat *chat = g_new0 (HyChat, 1);

  chat->id         = column_text (stmt, 0);
  chat->folder_id  = column_text (stmt, 1);
  chat->title      = column_text (stmt, 2);
  chat->backend    = column_text (stmt, 3);
  chat->session_id = column_text (stmt, 4);
  chat->workdir    = column_text (stmt, 5);
  chat->created_at = sqlite3_column_int64 (stmt, 6);
  chat->updated_at = sqlite3_column_int64 (stmt, 7);

  return chat;
}

#define CHAT_COLUMNS \
  "id, folder_id, title, backend, session_id, workdir, created_at, updated_at"

HyChat *
hy_storage_get_chat (HyStorage   *self,
                     const char  *chat_id,
                     GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  HyChat *chat = NULL;

  g_return_val_if_fail (HY_IS_STORAGE (self), NULL);

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT " CHAT_COLUMNS " FROM chats WHERE id = ?;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot read the chat");
      return NULL;
    }

  bind_text (stmt, 1, chat_id);

  if (sqlite3_step (stmt) == SQLITE_ROW)
    chat = chat_from_row (stmt);
  else
    g_set_error (error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "No chat %s", chat_id);

  sqlite3_finalize (stmt);

  return chat;
}

GPtrArray *
hy_storage_list_chats (HyStorage   *self,
                       const char  *folder_id,
                       GError     **error)
{
  GPtrArray *chats;
  sqlite3_stmt *stmt = NULL;

  g_return_val_if_fail (HY_IS_STORAGE (self), NULL);

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT " CHAT_COLUMNS " FROM chats"
                          " WHERE folder_id = ? ORDER BY updated_at DESC;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot list chats");
      return NULL;
    }

  bind_text (stmt, 1, folder_id);

  chats = g_ptr_array_new_with_free_func ((GDestroyNotify) hy_chat_free);
  while (sqlite3_step (stmt) == SQLITE_ROW)
    g_ptr_array_add (chats, chat_from_row (stmt));

  sqlite3_finalize (stmt);

  return chats;
}

/* Every chat mutation also bumps updated_at, which is what orders the list. */
static gboolean
update_chat_column (HyStorage   *self,
                    const char  *sql,
                    const char  *value,
                    const char  *chat_id,
                    const char  *what,
                    GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  if (sqlite3_prepare_v2 (self->db, sql, -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, what);
      return FALSE;
    }

  bind_text (stmt, 1, value);
  sqlite3_bind_int64 (stmt, 2, g_get_real_time () / G_USEC_PER_SEC);
  bind_text (stmt, 3, chat_id);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, what);

  sqlite3_finalize (stmt);

  return ok;
}

gboolean
hy_storage_set_chat_title (HyStorage   *self,
                           const char  *chat_id,
                           const char  *title,
                           GError     **error)
{
  g_return_val_if_fail (HY_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET title = ?, updated_at = ? WHERE id = ?;",
                             title, chat_id, "Cannot rename the chat", error);
}

gboolean
hy_storage_set_session_id (HyStorage   *self,
                           const char  *chat_id,
                           const char  *session_id,
                           GError     **error)
{
  g_return_val_if_fail (HY_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET session_id = ?, updated_at = ? WHERE id = ?;",
                             session_id, chat_id, "Cannot store the session id", error);
}

gboolean
hy_storage_set_backend (HyStorage   *self,
                        const char  *chat_id,
                        const char  *backend,
                        GError     **error)
{
  g_return_val_if_fail (HY_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET backend = ?, updated_at = ? WHERE id = ?;",
                             backend, chat_id, "Cannot change the backend", error);
}

gboolean
hy_storage_set_workdir (HyStorage   *self,
                        const char  *chat_id,
                        const char  *workdir,
                        GError     **error)
{
  g_return_val_if_fail (HY_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET workdir = ?, updated_at = ? WHERE id = ?;",
                             workdir, chat_id, "Cannot change the working directory",
                             error);
}

gboolean
hy_storage_delete_chat (HyStorage   *self,
                        const char  *chat_id,
                        GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (HY_IS_STORAGE (self), FALSE);

  /* Messages go with it: the foreign key cascades. */
  if (sqlite3_prepare_v2 (self->db, "DELETE FROM chats WHERE id = ?;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot delete the chat");
      return FALSE;
    }

  bind_text (stmt, 1, chat_id);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, "Cannot delete the chat");

  sqlite3_finalize (stmt);

  return ok;
}

/* --- messages ------------------------------------------------------------- */

static void
touch_chat (HyStorage  *self,
            const char *chat_id)
{
  sqlite3_stmt *stmt = NULL;

  if (sqlite3_prepare_v2 (self->db, "UPDATE chats SET updated_at = ? WHERE id = ?;",
                          -1, &stmt, NULL) != SQLITE_OK)
    return;

  sqlite3_bind_int64 (stmt, 1, g_get_real_time () / G_USEC_PER_SEC);
  bind_text (stmt, 2, chat_id);
  sqlite3_step (stmt);
  sqlite3_finalize (stmt);
}

gboolean
hy_storage_append_message (HyStorage   *self,
                           const char  *chat_id,
                           const char  *role,
                           const char  *content,
                           const char  *raw_json,
                           GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (HY_IS_STORAGE (self), FALSE);
  g_return_val_if_fail (chat_id != NULL, FALSE);
  g_return_val_if_fail (role != NULL, FALSE);

  if (sqlite3_prepare_v2 (self->db,
                          "INSERT INTO messages (chat_id, role, content, raw_json, created_at)"
                          " VALUES (?, ?, ?, ?, ?);",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot store the message");
      return FALSE;
    }

  bind_text (stmt, 1, chat_id);
  bind_text (stmt, 2, role);
  bind_text (stmt, 3, content != NULL ? content : "");
  bind_text (stmt, 4, raw_json);
  sqlite3_bind_int64 (stmt, 5, g_get_real_time () / G_USEC_PER_SEC);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, "Cannot store the message");

  sqlite3_finalize (stmt);

  /* A new message makes the chat the most recent one in its folder. */
  if (ok)
    touch_chat (self, chat_id);

  return ok;
}

static HyMessage *
message_from_row (sqlite3_stmt *stmt)
{
  HyMessage *message = g_new0 (HyMessage, 1);

  message->id         = sqlite3_column_int64 (stmt, 0);
  message->chat_id    = column_text (stmt, 1);
  message->role       = column_text (stmt, 2);
  message->content    = column_text (stmt, 3);
  message->raw_json   = column_text (stmt, 4);
  message->created_at = sqlite3_column_int64 (stmt, 5);

  return message;
}

#define MESSAGE_COLUMNS "id, chat_id, role, content, raw_json, created_at"

GPtrArray *
hy_storage_list_messages (HyStorage   *self,
                          const char  *chat_id,
                          GError     **error)
{
  GPtrArray *messages;
  sqlite3_stmt *stmt = NULL;

  g_return_val_if_fail (HY_IS_STORAGE (self), NULL);

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT " MESSAGE_COLUMNS " FROM messages"
                          " WHERE chat_id = ? ORDER BY id;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot read the conversation");
      return NULL;
    }

  bind_text (stmt, 1, chat_id);

  messages = g_ptr_array_new_with_free_func ((GDestroyNotify) hy_message_free);
  while (sqlite3_step (stmt) == SQLITE_ROW)
    g_ptr_array_add (messages, message_from_row (stmt));

  sqlite3_finalize (stmt);

  return messages;
}

GPtrArray *
hy_storage_search (HyStorage   *self,
                   const char  *query,
                   guint        limit,
                   GError     **error)
{
  GPtrArray *messages;
  sqlite3_stmt *stmt = NULL;

  g_return_val_if_fail (HY_IS_STORAGE (self), NULL);
  g_return_val_if_fail (query != NULL, NULL);

  /* Every column needs qualifying: messages and messages_fts both have a
   * "content" column, and an unqualified reference is ambiguous. */
  if (sqlite3_prepare_v2 (self->db,
                          "SELECT m.id, m.chat_id, m.role, m.content, m.raw_json,"
                          "       m.created_at"
                          " FROM messages_fts f"
                          " JOIN messages m ON m.id = f.rowid"
                          " WHERE f.messages_fts MATCH ?"
                          " ORDER BY f.rank LIMIT ?;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot search");
      return NULL;
    }

  bind_text (stmt, 1, query);
  sqlite3_bind_int (stmt, 2, (int) limit);

  messages = g_ptr_array_new_with_free_func ((GDestroyNotify) hy_message_free);
  while (sqlite3_step (stmt) == SQLITE_ROW)
    g_ptr_array_add (messages, message_from_row (stmt));

  sqlite3_finalize (stmt);

  return messages;
}

GPtrArray *
hy_storage_list_folder_ids (HyStorage  *self,
                            GError    **error)
{
  GPtrArray *ids;
  sqlite3_stmt *stmt = NULL;

  g_return_val_if_fail (HY_IS_STORAGE (self), NULL);

  if (sqlite3_prepare_v2 (self->db, "SELECT DISTINCT folder_id FROM chats;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot list folders");
      return NULL;
    }

  ids = g_ptr_array_new_with_free_func (g_free);
  while (sqlite3_step (stmt) == SQLITE_ROW)
    g_ptr_array_add (ids, column_text (stmt, 0));

  sqlite3_finalize (stmt);

  return ids;
}

/* --- GObject -------------------------------------------------------------- */

static void
hy_storage_finalize (GObject *object)
{
  HyStorage *self = HY_STORAGE (object);

  if (self->db != NULL)
    {
      sqlite3_close (self->db);
      self->db = NULL;
    }

  G_OBJECT_CLASS (hy_storage_parent_class)->finalize (object);
}

static void
hy_storage_class_init (HyStorageClass *klass)
{
  G_OBJECT_CLASS (klass)->finalize = hy_storage_finalize;
}

static void
hy_storage_init (HyStorage *self)
{
}
