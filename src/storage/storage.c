#include "storage.h"

#include <errno.h>
#include <sqlite3.h>

#define XD_STORAGE_SCHEMA_VERSION 10

struct _XdStorage
{
  GObject parent_instance;

  sqlite3 *db;
};

G_DEFINE_FINAL_TYPE (XdStorage, xd_storage, G_TYPE_OBJECT)

void
xd_chat_free (XdChat *self)
{
  if (self == NULL)
    return;

  g_free (self->id);
  g_free (self->folder_id);
  g_free (self->title);
  g_free (self->backend);
  g_free (self->workdir);
  g_free (self->model);
  g_free (self->effort);
  g_free (self->access);
  g_free (self);
}

void
xd_message_free (XdMessage *self)
{
  if (self == NULL)
    return;

  g_free (self->chat_id);
  g_free (self->role);
  g_free (self->content);
  g_free (self->raw_json);
  g_free (self->label);
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
exec_sql (XdStorage   *self,
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

  /* One row per backend a chat has used: the resumable session id, which is
   * not interchangeable between CLIs, and how far through the conversation
   * that backend has been brought. */
  "CREATE TABLE IF NOT EXISTS chat_sessions ("
  "  chat_id         TEXT NOT NULL REFERENCES chats (id) ON DELETE CASCADE,"
  "  backend         TEXT NOT NULL,"
  "  session_id      TEXT,"
  "  last_message_id INTEGER NOT NULL DEFAULT 0,"
  "  PRIMARY KEY (chat_id, backend)"
  ");"

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
read_schema_version (XdStorage *self)
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
migrate (XdStorage  *self,
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

  if (version < 3 &&
      !exec_sql (self, "ALTER TABLE chats ADD COLUMN model TEXT;", error))
    return FALSE;

  /* Sessions used to live on the chat row, which meant a chat could only ever
   * remember one backend's. Move them across rather than dropping them. */
  if (version < 4 &&
      !exec_sql (self,
                 "INSERT OR IGNORE INTO chat_sessions (chat_id, backend, session_id)"
                 "  SELECT id, backend, session_id FROM chats"
                 "  WHERE session_id IS NOT NULL;",
                 error))
    return FALSE;

  /* v4's chat_sessions had no last_message_id and required session_id;
   * rebuilding is simpler than trying to relax a column in place. */
  if (version == 4 &&
      !exec_sql (self,
                 "CREATE TABLE chat_sessions_new ("
                 "  chat_id         TEXT NOT NULL REFERENCES chats (id) ON DELETE CASCADE,"
                 "  backend         TEXT NOT NULL,"
                 "  session_id      TEXT,"
                 "  last_message_id INTEGER NOT NULL DEFAULT 0,"
                 "  PRIMARY KEY (chat_id, backend)"
                 ");"
                 "INSERT OR IGNORE INTO chat_sessions_new (chat_id, backend, session_id)"
                 "  SELECT chat_id, backend, session_id FROM chat_sessions;"
                 "DROP TABLE chat_sessions;"
                 "ALTER TABLE chat_sessions_new RENAME TO chat_sessions;",
                 error))
    return FALSE;

  if (version < 7 &&
      !exec_sql (self,
                 "ALTER TABLE chats ADD COLUMN plan INTEGER NOT NULL DEFAULT 0;",
                 error))
    return FALSE;

  /* Paired devices for the remote daemon. The token itself is never stored,
   * only its hash: the database is not where a bearer credential belongs. */
  if (version < 10 &&
      !exec_sql (self,
                 "CREATE TABLE IF NOT EXISTS devices ("
                 "  token_hash TEXT PRIMARY KEY,"
                 "  name       TEXT NOT NULL,"
                 "  created_at INTEGER NOT NULL,"
                 "  last_seen  INTEGER NOT NULL"
                 ");",
                 error))
    return FALSE;

  if (version < 9 &&
      (!exec_sql (self, "ALTER TABLE chats ADD COLUMN terminal_open INTEGER NOT NULL DEFAULT 0;", error) ||
       !exec_sql (self, "ALTER TABLE chats ADD COLUMN diff_open INTEGER NOT NULL DEFAULT 0;", error)))
    return FALSE;

  /* Replies used to be labelled from the chat's current model, so changing
   * model relabelled everything already said. What produced a message is a
   * property of the message. */
  if (version < 8 &&
      !exec_sql (self, "ALTER TABLE messages ADD COLUMN label TEXT;", error))
    return FALSE;

  if (version < 6 &&
      (!exec_sql (self, "ALTER TABLE chats ADD COLUMN effort TEXT;", error) ||
       !exec_sql (self, "ALTER TABLE chats ADD COLUMN access TEXT;", error)))
    return FALSE;

  sql = g_strdup_printf ("INSERT INTO meta (key, value) VALUES ('schema_version', '%d')"
                         "  ON CONFLICT (key) DO UPDATE SET value = excluded.value;",
                         XD_STORAGE_SCHEMA_VERSION);

  return exec_sql (self, sql, error);
}

XdStorage *
xd_storage_new (const char  *db_path,
                GError     **error)
{
  g_autoptr (XdStorage) self = NULL;
  g_autofree char *dir = NULL;

  g_return_val_if_fail (db_path != NULL, NULL);

  dir = g_path_get_dirname (db_path);
  if (g_mkdir_with_parents (dir, 0700) != 0)
    {
      g_set_error (error, G_IO_ERROR, g_io_error_from_errno (errno),
                   "Cannot create %s", dir);
      return NULL;
    }

  self = g_object_new (XD_TYPE_STORAGE, NULL);

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
xd_storage_create_chat (XdStorage   *self,
                        const char  *folder_id,
                        const char  *title,
                        const char  *backend,
                        const char  *model,
                        const char  *effort,
                        const char  *workdir,
                        GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  g_autofree char *id = NULL;
  gint64 now;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);
  g_return_val_if_fail (folder_id != NULL, NULL);
  g_return_val_if_fail (backend != NULL, NULL);

  id = g_uuid_string_random ();
  now = g_get_real_time () / G_USEC_PER_SEC;

  if (sqlite3_prepare_v2 (self->db,
                          "INSERT INTO chats (id, folder_id, title, backend,"
                          "                   model, effort, workdir,"
                          "                   created_at, updated_at)"
                          " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot create the chat");
      return NULL;
    }

  bind_text (stmt, 1, id);
  bind_text (stmt, 2, folder_id);
  bind_text (stmt, 3, title);
  bind_text (stmt, 4, backend);
  bind_text (stmt, 5, model);
  bind_text (stmt, 6, effort);
  bind_text (stmt, 7, workdir);
  sqlite3_bind_int64 (stmt, 8, now);
  sqlite3_bind_int64 (stmt, 9, now);

  if (sqlite3_step (stmt) != SQLITE_DONE)
    {
      set_sqlite_error (error, self->db, "Cannot create the chat");
      sqlite3_finalize (stmt);
      return NULL;
    }

  sqlite3_finalize (stmt);

  return g_steal_pointer (&id);
}

static XdChat *
chat_from_row (sqlite3_stmt *stmt)
{
  XdChat *chat = g_new0 (XdChat, 1);

  chat->id         = column_text (stmt, 0);
  chat->folder_id  = column_text (stmt, 1);
  chat->title      = column_text (stmt, 2);
  chat->backend    = column_text (stmt, 3);
  chat->workdir    = column_text (stmt, 4);
  chat->model      = column_text (stmt, 5);
  chat->effort     = column_text (stmt, 6);
  chat->access     = column_text (stmt, 7);
  chat->plan       = sqlite3_column_int (stmt, 8) != 0;
  chat->created_at = sqlite3_column_int64 (stmt, 9);
  chat->updated_at = sqlite3_column_int64 (stmt, 10);
  chat->terminal_open = sqlite3_column_int (stmt, 11) != 0;
  chat->diff_open     = sqlite3_column_int (stmt, 12) != 0;

  return chat;
}

#define CHAT_COLUMNS \
  "id, folder_id, title, backend, workdir, model, effort, access, plan,"\
  " created_at, updated_at, terminal_open, diff_open"

XdChat *
xd_storage_get_chat (XdStorage   *self,
                     const char  *chat_id,
                     GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  XdChat *chat = NULL;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);

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
xd_storage_list_chats (XdStorage   *self,
                       const char  *folder_id,
                       GError     **error)
{
  GPtrArray *chats;
  sqlite3_stmt *stmt = NULL;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT " CHAT_COLUMNS " FROM chats"
                          " WHERE folder_id = ? ORDER BY updated_at DESC;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot list chats");
      return NULL;
    }

  bind_text (stmt, 1, folder_id);

  chats = g_ptr_array_new_with_free_func ((GDestroyNotify) xd_chat_free);
  while (sqlite3_step (stmt) == SQLITE_ROW)
    g_ptr_array_add (chats, chat_from_row (stmt));

  sqlite3_finalize (stmt);

  return chats;
}

/* Every chat mutation also bumps updated_at, which is what orders the list. */
static gboolean
update_chat_column (XdStorage   *self,
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
xd_storage_set_chat_title (XdStorage   *self,
                           const char  *chat_id,
                           const char  *title,
                           GError     **error)
{
  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET title = ?, updated_at = ? WHERE id = ?;",
                             title, chat_id, "Cannot rename the chat", error);
}

char *
xd_storage_get_session_id (XdStorage   *self,
                           const char  *chat_id,
                           const char  *backend,
                           GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  char *session_id = NULL;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT session_id FROM chat_sessions"
                          " WHERE chat_id = ? AND backend = ?;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot read the session id");
      return NULL;
    }

  bind_text (stmt, 1, chat_id);
  bind_text (stmt, 2, backend);

  if (sqlite3_step (stmt) == SQLITE_ROW)
    session_id = column_text (stmt, 0);

  sqlite3_finalize (stmt);

  return session_id;
}

gboolean
xd_storage_set_session_id (XdStorage   *self,
                           const char  *chat_id,
                           const char  *backend,
                           const char  *session_id,
                           GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);
  g_return_val_if_fail (backend != NULL, FALSE);

  /* A session that cannot be resumed remembers nothing, so forgetting it also
   * resets how much of the conversation that backend has been told. */
  if (session_id == NULL)
    {
      if (sqlite3_prepare_v2 (self->db,
                              "UPDATE chat_sessions"
                              "   SET session_id = NULL, last_message_id = 0"
                              " WHERE chat_id = ? AND backend = ?;",
                              -1, &stmt, NULL) != SQLITE_OK)
        {
          set_sqlite_error (error, self->db, "Cannot forget the session id");
          return FALSE;
        }
    }
  else if (sqlite3_prepare_v2 (self->db,
                               "INSERT INTO chat_sessions (chat_id, backend, session_id)"
                               " VALUES (?, ?, ?)"
                               " ON CONFLICT (chat_id, backend)"
                               "   DO UPDATE SET session_id = excluded.session_id;",
                               -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot store the session id");
      return FALSE;
    }

  bind_text (stmt, 1, chat_id);
  bind_text (stmt, 2, backend);
  if (session_id != NULL)
    bind_text (stmt, 3, session_id);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, "Cannot store the session id");

  sqlite3_finalize (stmt);

  return ok;
}

gboolean
xd_storage_set_backend (XdStorage   *self,
                        const char  *chat_id,
                        const char  *backend,
                        GError     **error)
{
  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET backend = ?, updated_at = ? WHERE id = ?;",
                             backend, chat_id, "Cannot change the backend", error);
}

gboolean
xd_storage_set_workdir (XdStorage   *self,
                        const char  *chat_id,
                        const char  *workdir,
                        GError     **error)
{
  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET workdir = ?, updated_at = ? WHERE id = ?;",
                             workdir, chat_id, "Cannot change the working directory",
                             error);
}

gint64
xd_storage_get_last_seen (XdStorage  *self,
                          const char *chat_id,
                          const char *backend)
{
  sqlite3_stmt *stmt = NULL;
  gint64 last_seen = 0;

  g_return_val_if_fail (XD_IS_STORAGE (self), 0);

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT last_message_id FROM chat_sessions"
                          " WHERE chat_id = ? AND backend = ?;",
                          -1, &stmt, NULL) != SQLITE_OK)
    return 0;

  bind_text (stmt, 1, chat_id);
  bind_text (stmt, 2, backend);

  if (sqlite3_step (stmt) == SQLITE_ROW)
    last_seen = sqlite3_column_int64 (stmt, 0);

  sqlite3_finalize (stmt);

  return last_seen;
}

gboolean
xd_storage_set_last_seen (XdStorage   *self,
                          const char  *chat_id,
                          const char  *backend,
                          gint64       message_id,
                          GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  /* The row may not exist yet: a backend can be brought up to date before it
   * has ever reported a session id. */
  if (sqlite3_prepare_v2 (self->db,
                          "INSERT INTO chat_sessions (chat_id, backend, last_message_id)"
                          " VALUES (?, ?, ?)"
                          " ON CONFLICT (chat_id, backend)"
                          "   DO UPDATE SET last_message_id = excluded.last_message_id;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot record what the assistant has seen");
      return FALSE;
    }

  bind_text (stmt, 1, chat_id);
  bind_text (stmt, 2, backend);
  sqlite3_bind_int64 (stmt, 3, message_id);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, "Cannot record what the assistant has seen");

  sqlite3_finalize (stmt);

  return ok;
}

gint64
xd_storage_last_message_id (XdStorage  *self,
                            const char *chat_id)
{
  sqlite3_stmt *stmt = NULL;
  gint64 id = 0;

  g_return_val_if_fail (XD_IS_STORAGE (self), 0);

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT COALESCE(MAX(id), 0) FROM messages WHERE chat_id = ?;",
                          -1, &stmt, NULL) != SQLITE_OK)
    return 0;

  bind_text (stmt, 1, chat_id);

  if (sqlite3_step (stmt) == SQLITE_ROW)
    id = sqlite3_column_int64 (stmt, 0);

  sqlite3_finalize (stmt);

  return id;
}

gboolean
xd_storage_set_model (XdStorage   *self,
                      const char  *chat_id,
                      const char  *model,
                      GError     **error)
{
  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET model = ?, updated_at = ? WHERE id = ?;",
                             model, chat_id, "Cannot change the model", error);
}

gboolean
xd_storage_set_effort (XdStorage   *self,
                       const char  *chat_id,
                       const char  *effort,
                       GError     **error)
{
  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET effort = ?, updated_at = ? WHERE id = ?;",
                             effort, chat_id, "Cannot change the effort", error);
}

gboolean
xd_storage_set_access (XdStorage   *self,
                       const char  *chat_id,
                       const char  *access,
                       GError     **error)
{
  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET access = ?, updated_at = ? WHERE id = ?;",
                             access, chat_id, "Cannot change the access level", error);
}

gboolean
xd_storage_set_panes (XdStorage   *self,
                      const char  *chat_id,
                      gboolean     terminal_open,
                      gboolean     diff_open,
                      GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  /* updated_at is left alone: opening a pane is not work on the chat, and
   * bumping it would reorder the sidebar for looking at something. */
  if (sqlite3_prepare_v2 (self->db,
                          "UPDATE chats SET terminal_open = ?, diff_open = ?"
                          " WHERE id = ?;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot remember the open panes");
      return FALSE;
    }

  sqlite3_bind_int (stmt, 1, terminal_open ? 1 : 0);
  sqlite3_bind_int (stmt, 2, diff_open ? 1 : 0);
  bind_text (stmt, 3, chat_id);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, "Cannot remember the open panes");

  sqlite3_finalize (stmt);

  return ok;
}

gboolean
xd_storage_set_plan (XdStorage   *self,
                     const char  *chat_id,
                     gboolean     plan,
                     GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  if (sqlite3_prepare_v2 (self->db,
                          "UPDATE chats SET plan = ?, updated_at = ? WHERE id = ?;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot change plan mode");
      return FALSE;
    }

  sqlite3_bind_int (stmt, 1, plan ? 1 : 0);
  sqlite3_bind_int64 (stmt, 2, g_get_real_time () / G_USEC_PER_SEC);
  bind_text (stmt, 3, chat_id);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, "Cannot change plan mode");

  sqlite3_finalize (stmt);

  return ok;
}

gboolean
xd_storage_delete_chat (XdStorage   *self,
                        const char  *chat_id,
                        GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

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
touch_chat (XdStorage  *self,
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
xd_storage_append_message (XdStorage   *self,
                           const char  *chat_id,
                           const char  *role,
                           const char  *content,
                           const char  *raw_json,
                           const char  *label,
                           GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);
  g_return_val_if_fail (chat_id != NULL, FALSE);
  g_return_val_if_fail (role != NULL, FALSE);

  if (sqlite3_prepare_v2 (self->db,
                          "INSERT INTO messages (chat_id, role, content, raw_json, created_at, label)"
                          " VALUES (?, ?, ?, ?, ?, ?);",
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
  bind_text (stmt, 6, label);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, "Cannot store the message");

  sqlite3_finalize (stmt);

  /* A new message makes the chat the most recent one in its folder. */
  if (ok)
    touch_chat (self, chat_id);

  return ok;
}

static XdMessage *
message_from_row (sqlite3_stmt *stmt)
{
  XdMessage *message = g_new0 (XdMessage, 1);

  message->id         = sqlite3_column_int64 (stmt, 0);
  message->chat_id    = column_text (stmt, 1);
  message->role       = column_text (stmt, 2);
  message->content    = column_text (stmt, 3);
  message->raw_json   = column_text (stmt, 4);
  message->created_at = sqlite3_column_int64 (stmt, 5);
  message->label      = column_text (stmt, 6);

  return message;
}

#define MESSAGE_COLUMNS "id, chat_id, role, content, raw_json, created_at, label"

GPtrArray *
xd_storage_list_messages (XdStorage   *self,
                          const char  *chat_id,
                          GError     **error)
{
  GPtrArray *messages;
  sqlite3_stmt *stmt = NULL;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT " MESSAGE_COLUMNS " FROM messages"
                          " WHERE chat_id = ? ORDER BY id;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot read the conversation");
      return NULL;
    }

  bind_text (stmt, 1, chat_id);

  messages = g_ptr_array_new_with_free_func ((GDestroyNotify) xd_message_free);
  while (sqlite3_step (stmt) == SQLITE_ROW)
    g_ptr_array_add (messages, message_from_row (stmt));

  sqlite3_finalize (stmt);

  return messages;
}

GPtrArray *
xd_storage_list_messages_since (XdStorage   *self,
                                const char  *chat_id,
                                gint64       after_id,
                                GError     **error)
{
  GPtrArray *messages;
  sqlite3_stmt *stmt = NULL;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT " MESSAGE_COLUMNS " FROM messages"
                          " WHERE chat_id = ? AND id > ? ORDER BY id;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot read the conversation");
      return NULL;
    }

  bind_text (stmt, 1, chat_id);
  sqlite3_bind_int64 (stmt, 2, after_id);

  messages = g_ptr_array_new_with_free_func ((GDestroyNotify) xd_message_free);
  while (sqlite3_step (stmt) == SQLITE_ROW)
    g_ptr_array_add (messages, message_from_row (stmt));

  sqlite3_finalize (stmt);

  return messages;
}

GPtrArray *
xd_storage_search (XdStorage   *self,
                   const char  *query,
                   guint        limit,
                   GError     **error)
{
  GPtrArray *messages;
  sqlite3_stmt *stmt = NULL;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);
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

  messages = g_ptr_array_new_with_free_func ((GDestroyNotify) xd_message_free);
  while (sqlite3_step (stmt) == SQLITE_ROW)
    g_ptr_array_add (messages, message_from_row (stmt));

  sqlite3_finalize (stmt);

  return messages;
}

GPtrArray *
xd_storage_list_folder_ids (XdStorage  *self,
                            GError    **error)
{
  GPtrArray *ids;
  sqlite3_stmt *stmt = NULL;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);

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
xd_storage_finalize (GObject *object)
{
  XdStorage *self = XD_STORAGE (object);

  if (self->db != NULL)
    {
      sqlite3_close (self->db);
      self->db = NULL;
    }

  G_OBJECT_CLASS (xd_storage_parent_class)->finalize (object);
}

static void
xd_storage_class_init (XdStorageClass *klass)
{
  G_OBJECT_CLASS (klass)->finalize = xd_storage_finalize;
}

static void
xd_storage_init (XdStorage *self)
{
}

gboolean
xd_storage_add_device (XdStorage   *self,
                       const char  *token_hash,
                       const char  *name,
                       GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;
  gint64 now = g_get_real_time () / G_USEC_PER_SEC;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);
  g_return_val_if_fail (token_hash != NULL, FALSE);

  if (sqlite3_prepare_v2 (self->db,
                          "INSERT INTO devices (token_hash, name, created_at, last_seen)"
                          " VALUES (?, ?, ?, ?);",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot pair the device");
      return FALSE;
    }

  bind_text (stmt, 1, token_hash);
  bind_text (stmt, 2, name != NULL ? name : "device");
  sqlite3_bind_int64 (stmt, 3, now);
  sqlite3_bind_int64 (stmt, 4, now);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, "Cannot pair the device");

  sqlite3_finalize (stmt);

  return ok;
}

/* The device's name when the hash is known, NULL otherwise; a successful
 * lookup also counts as having seen the device. */
char *
xd_storage_device_name (XdStorage  *self,
                        const char *token_hash)
{
  sqlite3_stmt *stmt = NULL;
  char *name = NULL;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);

  if (sqlite3_prepare_v2 (self->db,
                          "UPDATE devices SET last_seen = ? WHERE token_hash = ?"
                          " RETURNING name;",
                          -1, &stmt, NULL) != SQLITE_OK)
    return NULL;

  sqlite3_bind_int64 (stmt, 1, g_get_real_time () / G_USEC_PER_SEC);
  bind_text (stmt, 2, token_hash);

  if (sqlite3_step (stmt) == SQLITE_ROW)
    name = column_text (stmt, 0);

  sqlite3_finalize (stmt);

  return name;
}
