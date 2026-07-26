#pragma once

#include <gio/gio.h>

G_BEGIN_DECLS

typedef struct
{
  char *id;
  char *folder_id;
  char *title;
  char *backend;
  char *workdir;      /* NULL: inherit the folder's */
  char *model;        /* NULL: the backend's default */
  char *effort;       /* NULL: the CLI's own setting */
  char *access;       /* NULL: read-only */
  char *queued;       /* next message, kept until its turn starts */
  gboolean new_worktree; /* create an isolated checkout before first turn */
  gboolean plan;      /* think it through, change nothing */
  gboolean terminal_open;  /* the panes this chat was left with */
  gboolean diff_open;
  gint64 created_at;
  gint64 updated_at;
} XdChat;

typedef struct
{
  gint64 id;
  char *chat_id;
  char *role;         /* "user" | "assistant" | "error" */
  char *content;
  char *raw_json;     /* the backend's own event, when there was one */
  char *label;        /* who produced it: model and effort, for replies */
  gint64 created_at;
} XdMessage;

void xd_chat_free    (XdChat    *self);
void xd_message_free (XdMessage *self);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (XdChat, xd_chat_free)
G_DEFINE_AUTOPTR_CLEANUP_FUNC (XdMessage, xd_message_free)

#define XD_TYPE_STORAGE (xd_storage_get_type ())
G_DECLARE_FINAL_TYPE (XdStorage, xd_storage, XD, STORAGE, GObject)

/*
 * Chats and their messages, in one SQLite database.
 *
 * Chats are attached to a folder by its UUID rather than its path, so folders
 * can be renamed or moved on disk without losing their conversations. Writes
 * happen on the main loop: they are one small row per turn, never per token.
 */
XdStorage  *xd_storage_new             (const char  *db_path,
                                        GError     **error);

/* The file it was opened from. Anything watching for writes made by another
 * process on this machine needs it; SQLite says nothing about those. */
const char *xd_storage_get_path        (XdStorage   *self);

/*
 * ::changed -- something wrote to this database.
 *
 * Including this process: it is a file being watched, not a journal of who did
 * what. What it is for is the writes nobody here made -- the daemon running a
 * turn for another device, or a second window -- because SQLite has no way to
 * mention those, and without it a window shows a conversation that stopped
 * being true while it was on screen.
 *
 * Coalesced, since one statement is several writes.
 */

/*
 * Watches the database for writes, and emits ::changed when it settles.
 *
 * More than one process works on this file: a window and the daemon serving
 * the same chats to other devices, at least. SQLite tells a connection nothing
 * about what another one wrote, so the file itself is watched -- the directory
 * really, since in WAL mode the writes land beside it.
 *
 * The signal fires for this process's own writes too. Whoever listens knows
 * what it just did; nothing else can tell the difference.
 */
void        xd_storage_watch           (XdStorage   *self);

/* Returns the new chat's id. @workdir may be NULL to inherit the folder's. */
char       *xd_storage_create_chat     (XdStorage   *self,
                                        const char  *folder_id,
                                        const char  *title,
                                        const char  *backend,
                                        const char  *model,
                                        const char  *effort,
                                        const char  *workdir,
                                        GError     **error);

gboolean    xd_storage_set_workdir     (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *workdir,
                                        GError     **error);

/* A pending new-worktree choice is consumed when the first turn starts. */
gboolean    xd_storage_set_new_worktree (XdStorage  *self,
                                         const char *chat_id,
                                         gboolean    enabled,
                                         GError    **error);

/* Atomically consumes the pending choice and points the chat at its checkout. */
gboolean    xd_storage_use_worktree     (XdStorage   *self,
                                         const char  *chat_id,
                                         const char  *workdir,
                                         GError     **error);

gboolean    xd_storage_set_model       (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *model,
                                        GError     **error);

gboolean    xd_storage_set_effort      (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *effort,
                                        GError     **error);

gboolean    xd_storage_set_access      (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *access,
                                        GError     **error);

/* @text may be NULL to discard or consume the queued message. */
gboolean    xd_storage_set_queued      (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *text,
                                        GError     **error);

/* Plan mode rides alongside the access level rather than replacing it, so
 * leaving plan restores whatever access the chat had. */
/* Which panes a chat is working with. Kept per chat rather than per window:
 * one chat is a repository being edited, the next is a question. */
gboolean    xd_storage_set_panes       (XdStorage   *self,
                                        const char  *chat_id,
                                        gboolean     terminal_open,
                                        gboolean     diff_open,
                                        GError     **error);

gboolean    xd_storage_set_plan        (XdStorage   *self,
                                        const char  *chat_id,
                                        gboolean     plan,
                                        GError     **error);

XdChat     *xd_storage_get_chat        (XdStorage   *self,
                                        const char  *chat_id,
                                        GError     **error);

/* Most recently used first. Elements are XdChat*. */
GPtrArray  *xd_storage_list_chats      (XdStorage   *self,
                                        const char  *folder_id,
                                        GError     **error);

gboolean    xd_storage_set_chat_title  (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *title,
                                        GError     **error);

/*
 * Resumable sessions are tracked per backend.
 *
 * A session id only means something to the CLI that issued it, so a chat that
 * has talked to both keeps one of each — switching assistants and switching
 * back resumes each side where it was left rather than starting over.
 */
char       *xd_storage_get_session_id  (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend,
                                        GError     **error);

/* @session_id may be NULL to forget a session that no longer resumes; doing so
 * also resets how much of the conversation that backend is known to have seen,
 * because a session it cannot resume is a session that remembers nothing. */
gboolean    xd_storage_set_session_id  (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend,
                                        const char  *session_id,
                                        GError     **error);

/*
 * How far through the conversation a backend has been brought.
 *
 * Resuming a session only restores what *that* assistant was told. Anything
 * said to the other one in between is missing from it, so xd records the last
 * message each backend has seen and replays whatever came after.
 */
gint64      xd_storage_get_last_seen   (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend);

gboolean    xd_storage_set_last_seen   (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend,
                                        gint64       message_id,
                                        GError     **error);

/* Highest message id in a chat; 0 when it has none. */
gint64      xd_storage_last_message_id (XdStorage   *self,
                                        const char  *chat_id);

/* Messages with an id above @after_id, oldest first. Elements are XdMessage*. */
GPtrArray  *xd_storage_list_messages_since (XdStorage   *self,
                                            const char  *chat_id,
                                            gint64       after_id,
                                            GError     **error);

gboolean    xd_storage_set_backend     (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend,
                                        GError     **error);

gboolean    xd_storage_delete_chat     (XdStorage   *self,
                                        const char  *chat_id,
                                        GError     **error);

gboolean    xd_storage_append_message  (XdStorage   *self,
                                        const char  *chat_id,
                                        const char  *role,
                                        const char  *content,
                                        const char  *raw_json,
                                        const char  *label,
                                        GError     **error);

/* Oldest first. Elements are XdMessage*. */
GPtrArray  *xd_storage_list_messages   (XdStorage   *self,
                                        const char  *chat_id,
                                        GError     **error);

/* Paired remote devices: only the token's hash is kept. */
gboolean    xd_storage_add_device      (XdStorage   *self,
                                        const char  *token_hash,
                                        const char  *name,
                                        GError     **error);
char       *xd_storage_device_name     (XdStorage  *self,
                                        const char *token_hash);

/* Full-text search across every message. Elements are XdMessage*. */
GPtrArray  *xd_storage_search          (XdStorage   *self,
                                        const char  *query,
                                        guint        limit,
                                        GError     **error);

/* Folder ids that own at least one chat; used to spot orphaned chats. */
GPtrArray  *xd_storage_list_folder_ids (XdStorage   *self,
                                        GError     **error);

G_END_DECLS
