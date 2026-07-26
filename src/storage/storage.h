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
  gboolean plan;      /* think it through, change nothing */
  gboolean terminal_open;  /* the panes this chat was left with */
  gboolean diff_open;
  gint64 created_at;
  gint64 updated_at;
} HyChat;

typedef struct
{
  gint64 id;
  char *chat_id;
  char *role;         /* "user" | "assistant" | "error" */
  char *content;
  char *raw_json;     /* the backend's own event, when there was one */
  char *label;        /* who produced it: model and effort, for replies */
  gint64 created_at;
} HyMessage;

void hy_chat_free    (HyChat    *self);
void hy_message_free (HyMessage *self);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (HyChat, hy_chat_free)
G_DEFINE_AUTOPTR_CLEANUP_FUNC (HyMessage, hy_message_free)

#define HY_TYPE_STORAGE (hy_storage_get_type ())
G_DECLARE_FINAL_TYPE (HyStorage, hy_storage, HY, STORAGE, GObject)

/*
 * Chats and their messages, in one SQLite database.
 *
 * Chats are attached to a folder by its UUID rather than its path, so folders
 * can be renamed or moved on disk without losing their conversations. Writes
 * happen on the main loop: they are one small row per turn, never per token.
 */
HyStorage  *hy_storage_new             (const char  *db_path,
                                        GError     **error);

/* Returns the new chat's id. @workdir may be NULL to inherit the folder's. */
char       *hy_storage_create_chat     (HyStorage   *self,
                                        const char  *folder_id,
                                        const char  *title,
                                        const char  *backend,
                                        const char  *model,
                                        const char  *effort,
                                        const char  *workdir,
                                        GError     **error);

gboolean    hy_storage_set_workdir     (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *workdir,
                                        GError     **error);

gboolean    hy_storage_set_model       (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *model,
                                        GError     **error);

gboolean    hy_storage_set_effort      (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *effort,
                                        GError     **error);

gboolean    hy_storage_set_access      (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *access,
                                        GError     **error);

/* Plan mode rides alongside the access level rather than replacing it, so
 * leaving plan restores whatever access the chat had. */
/* Which panes a chat is working with. Kept per chat rather than per window:
 * one chat is a repository being edited, the next is a question. */
gboolean    hy_storage_set_panes       (HyStorage   *self,
                                        const char  *chat_id,
                                        gboolean     terminal_open,
                                        gboolean     diff_open,
                                        GError     **error);

gboolean    hy_storage_set_plan        (HyStorage   *self,
                                        const char  *chat_id,
                                        gboolean     plan,
                                        GError     **error);

HyChat     *hy_storage_get_chat        (HyStorage   *self,
                                        const char  *chat_id,
                                        GError     **error);

/* Most recently used first. Elements are HyChat*. */
GPtrArray  *hy_storage_list_chats      (HyStorage   *self,
                                        const char  *folder_id,
                                        GError     **error);

gboolean    hy_storage_set_chat_title  (HyStorage   *self,
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
char       *hy_storage_get_session_id  (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend,
                                        GError     **error);

/* @session_id may be NULL to forget a session that no longer resumes; doing so
 * also resets how much of the conversation that backend is known to have seen,
 * because a session it cannot resume is a session that remembers nothing. */
gboolean    hy_storage_set_session_id  (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend,
                                        const char  *session_id,
                                        GError     **error);

/*
 * How far through the conversation a backend has been brought.
 *
 * Resuming a session only restores what *that* assistant was told. Anything
 * said to the other one in between is missing from it, so hy records the last
 * message each backend has seen and replays whatever came after.
 */
gint64      hy_storage_get_last_seen   (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend);

gboolean    hy_storage_set_last_seen   (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend,
                                        gint64       message_id,
                                        GError     **error);

/* Highest message id in a chat; 0 when it has none. */
gint64      hy_storage_last_message_id (HyStorage   *self,
                                        const char  *chat_id);

/* Messages with an id above @after_id, oldest first. Elements are HyMessage*. */
GPtrArray  *hy_storage_list_messages_since (HyStorage   *self,
                                            const char  *chat_id,
                                            gint64       after_id,
                                            GError     **error);

gboolean    hy_storage_set_backend     (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *backend,
                                        GError     **error);

gboolean    hy_storage_delete_chat     (HyStorage   *self,
                                        const char  *chat_id,
                                        GError     **error);

gboolean    hy_storage_append_message  (HyStorage   *self,
                                        const char  *chat_id,
                                        const char  *role,
                                        const char  *content,
                                        const char  *raw_json,
                                        const char  *label,
                                        GError     **error);

/* Oldest first. Elements are HyMessage*. */
GPtrArray  *hy_storage_list_messages   (HyStorage   *self,
                                        const char  *chat_id,
                                        GError     **error);

/* Paired remote devices: only the token's hash is kept. */
gboolean    hy_storage_add_device      (HyStorage   *self,
                                        const char  *token_hash,
                                        const char  *name,
                                        GError     **error);
char       *hy_storage_device_name     (HyStorage  *self,
                                        const char *token_hash);

/* Full-text search across every message. Elements are HyMessage*. */
GPtrArray  *hy_storage_search          (HyStorage   *self,
                                        const char  *query,
                                        guint        limit,
                                        GError     **error);

/* Folder ids that own at least one chat; used to spot orphaned chats. */
GPtrArray  *hy_storage_list_folder_ids (HyStorage   *self,
                                        GError     **error);

G_END_DECLS
