#include "storage.h"

#include <errno.h>
#include <sqlite3.h>

#define XD_STORAGE_SCHEMA_VERSION 14

struct _XdStorage
{
  GObject parent_instance;

  sqlite3 *db;
  char *path;

  /* Writes by anything on this machine, coalesced: SQLite touches its files
   * several times per statement. */
  GFileMonitor *watch;
  guint settled_id;
};

enum
{
  SIGNAL_CHANGED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

/* SQLite writes several times per statement; one event out the far side is
 * enough, and a moment late costs nothing. */
#define SETTLE_MS 400

G_DEFINE_FINAL_TYPE (XdStorage, xd_storage, G_TYPE_OBJECT)

static gboolean
on_settled (gpointer user_data)
{
  XdStorage *self = user_data;

  self->settled_id = 0;

  g_signal_emit (self, signals[SIGNAL_CHANGED], 0);

  return G_SOURCE_REMOVE;
}

static void
on_file_changed (GFileMonitor      *monitor,
                 GFile             *file,
                 GFile             *other_file,
                 GFileMonitorEvent  event,
                 gpointer           user_data)
{
  XdStorage *self = user_data;

  g_clear_handle_id (&self->settled_id, g_source_remove);
  self->settled_id = g_timeout_add (SETTLE_MS, on_settled, self);
}

void
xd_storage_watch (XdStorage *self)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *dir = NULL;
  g_autoptr (GFile) file = NULL;

  g_return_if_fail (XD_IS_STORAGE (self));

  if (self->watch != NULL || self->path == NULL)
    return;

  /* The directory rather than the file: in WAL mode the writes land beside it,
   * in a journal the database itself never mentions. */
  dir = g_path_get_dirname (self->path);
  file = g_file_new_for_path (dir);

  self->watch = g_file_monitor_directory (file, G_FILE_MONITOR_NONE, NULL, &error);
  if (self->watch == NULL)
    {
      /* Not fatal: what is lost is hearing about another process's writes. */
      g_warning ("cannot watch %s: %s", dir, error->message);
      return;
    }

  g_signal_connect (self->watch, "changed", G_CALLBACK (on_file_changed), self);
}

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
  g_free (self->queued);
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

  /* A message typed during a turn belongs to the chat, not the window that
   * happened to be showing it. */
  if (version < 11 &&
      !exec_sql (self, "ALTER TABLE chats ADD COLUMN queued TEXT;", error))
    return FALSE;

  if (version < 12 &&
      !exec_sql (self,
                 "ALTER TABLE chats ADD COLUMN new_worktree INTEGER NOT NULL DEFAULT 0;",
                 error))
    return FALSE;

  if (version < 13 &&
      (!exec_sql (self,
                  "ALTER TABLE chat_sessions"
                  " ADD COLUMN context_used INTEGER NOT NULL DEFAULT 0;",
                  error) ||
       !exec_sql (self,
                  "ALTER TABLE chat_sessions"
                  " ADD COLUMN context_window INTEGER NOT NULL DEFAULT 0;",
                  error) ||
       !exec_sql (self,
                  "ALTER TABLE chat_sessions ADD COLUMN context_model TEXT;",
                  error)))
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

  /*
   * A new chat follows the last agent configuration the user changed.
   *
   * This cannot be derived from chats.updated_at: sending a message, renaming
   * a chat, or choosing a workspace also changes that timestamp, and none of
   * those actions should silently replace the defaults. The trigger snapshots
   * all agent options atomically whenever one of them actually changes.
   */
  if (version < 14 &&
      !exec_sql (self,
                 "CREATE TABLE agent_defaults ("
                 "  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),"
                 "  backend   TEXT NOT NULL,"
                 "  model     TEXT,"
                 "  effort    TEXT,"
                 "  access    TEXT,"
                 "  plan      INTEGER NOT NULL"
                 ");"
                 "CREATE TRIGGER remember_agent_defaults"
                 " AFTER UPDATE OF backend, model, effort, access, plan ON chats"
                 " WHEN OLD.backend IS NOT NEW.backend"
                 "   OR OLD.model IS NOT NEW.model"
                 "   OR OLD.effort IS NOT NEW.effort"
                 "   OR OLD.access IS NOT NEW.access"
                 "   OR OLD.plan IS NOT NEW.plan"
                 " BEGIN"
                 "   INSERT OR REPLACE INTO agent_defaults"
                 "     (singleton, backend, model, effort, access, plan)"
                 "   VALUES (1, NEW.backend, NEW.model, NEW.effort,"
                 "           NEW.access, NEW.plan);"
                 " END;",
                 error))
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

  self->path = g_strdup (db_path);

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
  g_autofree char *saved_backend = NULL;
  g_autofree char *saved_model = NULL;
  g_autofree char *saved_effort = NULL;
  g_autofree char *saved_access = NULL;
  const char *actual_backend = backend;
  const char *actual_model = model;
  const char *actual_effort = effort;
  const char *actual_access = NULL;
  gboolean actual_plan = FALSE;
  int result;
  gint64 now;

  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);
  g_return_val_if_fail (folder_id != NULL, NULL);
  g_return_val_if_fail (backend != NULL, NULL);

  id = g_uuid_string_random ();
  now = g_get_real_time () / G_USEC_PER_SEC;

  if (sqlite3_prepare_v2 (self->db,
                          "SELECT backend, model, effort, access, plan"
                          " FROM agent_defaults WHERE singleton = 1;",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot read the agent defaults");
      return NULL;
    }

  result = sqlite3_step (stmt);
  if (result == SQLITE_ROW)
    {
      saved_backend = column_text (stmt, 0);
      saved_model = column_text (stmt, 1);
      saved_effort = column_text (stmt, 2);
      saved_access = column_text (stmt, 3);
      actual_backend = saved_backend;
      actual_model = saved_model;
      actual_effort = saved_effort;
      actual_access = saved_access;
      actual_plan = sqlite3_column_int (stmt, 4) != 0;
    }
  else if (result != SQLITE_DONE)
    {
      set_sqlite_error (error, self->db, "Cannot read the agent defaults");
      sqlite3_finalize (stmt);
      return NULL;
    }

  sqlite3_finalize (stmt);
  stmt = NULL;

  if (sqlite3_prepare_v2 (self->db,
                          "INSERT INTO chats (id, folder_id, title, backend,"
                          "                   model, effort, access, plan, workdir,"
                          "                   created_at, updated_at)"
                          " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);",
                          -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot create the chat");
      return NULL;
    }

  bind_text (stmt, 1, id);
  bind_text (stmt, 2, folder_id);
  bind_text (stmt, 3, title);
  bind_text (stmt, 4, actual_backend);
  bind_text (stmt, 5, actual_model);
  bind_text (stmt, 6, actual_effort);
  bind_text (stmt, 7, actual_access);
  sqlite3_bind_int (stmt, 8, actual_plan ? 1 : 0);
  bind_text (stmt, 9, workdir);
  sqlite3_bind_int64 (stmt, 10, now);
  sqlite3_bind_int64 (stmt, 11, now);

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
  chat->queued        = column_text (stmt, 13);
  chat->new_worktree  = sqlite3_column_int (stmt, 14) != 0;

  return chat;
}

#define CHAT_COLUMNS \
  "id, folder_id, title, backend, workdir, model, effort, access, plan,"\
  " created_at, updated_at, terminal_open, diff_open, queued, new_worktree"

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
                              "   SET session_id = NULL, last_message_id = 0,"
                              "       context_used = 0, context_window = 0,"
                              "       context_model = NULL"
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

gboolean
xd_storage_set_new_worktree (XdStorage   *self,
                              const char  *chat_id,
                              gboolean     enabled,
                              GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);
  g_return_val_if_fail (chat_id != NULL, FALSE);

  if (sqlite3_prepare_v2 (
        self->db,
        "UPDATE chats SET new_worktree = ?, updated_at = ?"
        " WHERE id = ? AND NOT EXISTS"
        "   (SELECT 1 FROM messages WHERE chat_id = ?);",
        -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot change the workspace");
      return FALSE;
    }

  sqlite3_bind_int (stmt, 1, enabled);
  sqlite3_bind_int64 (stmt, 2, g_get_real_time () / G_USEC_PER_SEC);
  bind_text (stmt, 3, chat_id);
  bind_text (stmt, 4, chat_id);

  ok = sqlite3_step (stmt) == SQLITE_DONE && sqlite3_changes (self->db) == 1;
  if (!ok)
    g_set_error (error, G_IO_ERROR, G_IO_ERROR_FAILED,
                 "The workspace can only be changed before the first message.");

  sqlite3_finalize (stmt);

  return ok;
}

gboolean
xd_storage_use_existing_worktree (XdStorage   *self,
                                  const char  *chat_id,
                                  const char  *workdir,
                                  GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);
  g_return_val_if_fail (chat_id != NULL, FALSE);
  g_return_val_if_fail (workdir != NULL && *workdir != '\0', FALSE);

  if (sqlite3_prepare_v2 (
        self->db,
        "UPDATE chats SET workdir = ?, new_worktree = 0, updated_at = ?"
        " WHERE id = ? AND NOT EXISTS"
        "   (SELECT 1 FROM messages WHERE chat_id = ?);",
        -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot change the workspace");
      return FALSE;
    }

  bind_text (stmt, 1, workdir);
  sqlite3_bind_int64 (stmt, 2, g_get_real_time () / G_USEC_PER_SEC);
  bind_text (stmt, 3, chat_id);
  bind_text (stmt, 4, chat_id);

  ok = sqlite3_step (stmt) == SQLITE_DONE && sqlite3_changes (self->db) == 1;
  if (!ok)
    g_set_error (error, G_IO_ERROR, G_IO_ERROR_FAILED,
                 "The workspace can only be changed before the first message.");

  sqlite3_finalize (stmt);

  return ok;
}

gboolean
xd_storage_use_worktree (XdStorage   *self,
                         const char  *chat_id,
                         const char  *workdir,
                         GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);
  g_return_val_if_fail (chat_id != NULL, FALSE);
  g_return_val_if_fail (workdir != NULL, FALSE);

  if (sqlite3_prepare_v2 (
        self->db,
        "UPDATE chats"
        "   SET workdir = ?, new_worktree = 0, updated_at = ?"
        " WHERE id = ? AND new_worktree = 1 AND NOT EXISTS"
        "   (SELECT 1 FROM messages WHERE chat_id = ?);",
        -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot use the new worktree");
      return FALSE;
    }

  bind_text (stmt, 1, workdir);
  sqlite3_bind_int64 (stmt, 2, g_get_real_time () / G_USEC_PER_SEC);
  bind_text (stmt, 3, chat_id);
  bind_text (stmt, 4, chat_id);

  ok = sqlite3_step (stmt) == SQLITE_DONE && sqlite3_changes (self->db) == 1;
  if (!ok)
    g_set_error (error, G_IO_ERROR, G_IO_ERROR_FAILED,
                 "The workspace changed before the worktree was ready.");

  sqlite3_finalize (stmt);

  return ok;
}

gboolean
xd_storage_set_queued (XdStorage   *self,
                       const char  *chat_id,
                       const char  *text,
                       GError     **error)
{
  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);

  return update_chat_column (self,
                             "UPDATE chats SET queued = ?, updated_at = ? WHERE id = ?;",
                             text, chat_id, "Cannot update the queued message",
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

gboolean
xd_storage_set_context_usage (XdStorage   *self,
                              const char  *chat_id,
                              const char  *backend,
                              const char  *model,
                              guint64      used,
                              guint64      window,
                              GError     **error)
{
  sqlite3_stmt *stmt = NULL;
  gboolean ok;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);
  g_return_val_if_fail (chat_id != NULL, FALSE);
  g_return_val_if_fail (backend != NULL, FALSE);
  g_return_val_if_fail (used <= G_MAXINT64 && window <= G_MAXINT64, FALSE);

  if (sqlite3_prepare_v2 (
        self->db,
        "INSERT INTO chat_sessions"
        "  (chat_id, backend, context_model, context_used, context_window)"
        " VALUES (?, ?, ?, ?, ?)"
        " ON CONFLICT (chat_id, backend) DO UPDATE SET"
        "   context_model = excluded.context_model,"
        "   context_used = excluded.context_used,"
        "   context_window = excluded.context_window;",
        -1, &stmt, NULL) != SQLITE_OK)
    {
      set_sqlite_error (error, self->db, "Cannot store context usage");
      return FALSE;
    }

  bind_text (stmt, 1, chat_id);
  bind_text (stmt, 2, backend);
  bind_text (stmt, 3, model);
  sqlite3_bind_int64 (stmt, 4, (gint64) used);
  sqlite3_bind_int64 (stmt, 5, (gint64) window);

  ok = sqlite3_step (stmt) == SQLITE_DONE;
  if (!ok)
    set_sqlite_error (error, self->db, "Cannot store context usage");

  sqlite3_finalize (stmt);

  return ok;
}

gboolean
xd_storage_get_context_usage (XdStorage  *self,
                              const char *chat_id,
                              const char *backend,
                              const char *model,
                              guint64    *used,
                              guint64    *window)
{
  sqlite3_stmt *stmt = NULL;
  gboolean found = FALSE;

  g_return_val_if_fail (XD_IS_STORAGE (self), FALSE);
  g_return_val_if_fail (chat_id != NULL, FALSE);
  g_return_val_if_fail (backend != NULL, FALSE);

  if (sqlite3_prepare_v2 (
        self->db,
        "SELECT context_model, context_used, context_window"
        " FROM chat_sessions WHERE chat_id = ? AND backend = ?;",
        -1, &stmt, NULL) != SQLITE_OK)
    return FALSE;

  bind_text (stmt, 1, chat_id);
  bind_text (stmt, 2, backend);

  if (sqlite3_step (stmt) == SQLITE_ROW)
    {
      const char *stored_model =
        (const char *) sqlite3_column_text (stmt, 0);
      gint64 stored_used = sqlite3_column_int64 (stmt, 1);
      gint64 stored_window = sqlite3_column_int64 (stmt, 2);

      found = (model == NULL || g_strcmp0 (stored_model, model) == 0) &&
              stored_used > 0 && stored_window > 0;
      if (found)
        {
          if (used != NULL)
            *used = stored_used;
          if (window != NULL)
            *window = stored_window;
        }
    }

  sqlite3_finalize (stmt);

  return found;
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

  g_clear_pointer (&self->path, g_free);

  G_OBJECT_CLASS (xd_storage_parent_class)->finalize (object);
}

static void
xd_storage_dispose (GObject *object)
{
  XdStorage *self = XD_STORAGE (object);

  g_clear_handle_id (&self->settled_id, g_source_remove);
  g_clear_object (&self->watch);

  G_OBJECT_CLASS (xd_storage_parent_class)->dispose (object);
}

static void
xd_storage_class_init (XdStorageClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = xd_storage_dispose;
  G_OBJECT_CLASS (klass)->finalize = xd_storage_finalize;

  /* Something wrote to the database -- this process or another. */
  signals[SIGNAL_CHANGED] =
    g_signal_new ("changed", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 0);
}

static void
xd_storage_init (XdStorage *self)
{
}

const char *
xd_storage_get_path (XdStorage *self)
{
  g_return_val_if_fail (XD_IS_STORAGE (self), NULL);

  return self->path;
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
