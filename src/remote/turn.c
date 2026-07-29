#include "turn.h"

#include "chat/chat-session.h"
#include "chat/handover.h"
#include "settings/folder-settings.h"
#include "settings/settings-resolver.h"
#include "tree/xd-node.h"
#include "util/ask-block.h"
#include "util/git-diff.h"
#include "util/workflow-run.h"
#include "util/workspace-block.h"
#include "util/worktree.h"

#include <string.h>

#define DIFF_INIT_BUDGET_SECONDS 5

/*
 * The daemon running a turn, which is the same work the window does with the
 * same pieces: resolve what the folder chain says, start the CLI, and write
 * down what it said.
 *
 * What is different is who watches. Nothing here draws anything, and nothing
 * waits for a client: a turn started from a laptop that then closes its lid
 * finishes anyway, and the transcript is complete when it does.
 */

struct _XdDaemonTurn
{
  GObject parent_instance;

  XdStorage *storage;
  char *root_path;

  XdChatSession *session;
  char *chat_id;
  char *backend_id;
  char *model_id;
  char *label;
  gint64 transcript_message_id; /* user row; live output follows it */
  char *workdir;             /* where file-change diffs are captured */
  XdGitDiffTracker *diff_tracker;
  GCancellable *diff_cancellable;
  guint diff_timeout_id;
  guint diff_generation;
  GString *text;            /* everything said this turn */
  GString *segment;         /* what belongs to the message being written */
  gint64 segment_message_id; /* live row extended as this segment streams */

  /* Speech and tools in the order they happened. Storage is updated live so a
   * daemon stop cannot erase progress; these copies let devices join a turn
   * already in flight. XdTurnItem*. */
  GPtrArray *items;
  GStrv commands;              /* installed commands reported by this backend */
  gint64 started_at;           /* monotonic usec */
  gboolean resumed;
  gboolean asked_user;
  guint64 context_used;
  guint64 context_window;
};

enum
{
  SIGNAL_COMMANDS,
  SIGNAL_TEXT,
  SIGNAL_TOOL,
  SIGNAL_FINISHED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (XdDaemonTurn, xd_daemon_turn, G_TYPE_OBJECT)

typedef struct
{
  char *workdir;
  guint generation;
} DiffInit;

static void
diff_init_free (DiffInit *init)
{
  g_free (init->workdir);
  g_free (init);
}

static void
build_diff_tracker (GTask        *task,
                    gpointer      source_object,
                    gpointer      task_data,
                    GCancellable *cancellable)
{
  DiffInit *init = task_data;
  XdGitDiffTracker *tracker =
    xd_git_diff_tracker_new_cancellable (init->workdir, cancellable);

  if (g_cancellable_is_cancelled (cancellable))
    {
      g_clear_pointer (&tracker, xd_git_diff_tracker_free);
      g_task_return_new_error (task, G_IO_ERROR, G_IO_ERROR_CANCELLED,
                               "Git snapshot exceeded its time budget");
      return;
    }

  g_task_return_pointer (
    task, tracker, (GDestroyNotify) xd_git_diff_tracker_free);
}

static gboolean
cancel_slow_diff_tracker (gpointer user_data)
{
  XdDaemonTurn *self = user_data;

  self->diff_timeout_id = 0;
  g_cancellable_cancel (self->diff_cancellable);
  return G_SOURCE_REMOVE;
}

static void
on_diff_tracker_ready (GObject      *source,
                       GAsyncResult *result,
                       gpointer      user_data)
{
  g_autoptr (XdDaemonTurn) self = user_data;
  g_autoptr (GError) error = NULL;
  g_autoptr (XdGitDiffTracker) tracker =
    g_task_propagate_pointer (G_TASK (result), &error);
  DiffInit *init = g_task_get_task_data (G_TASK (result));

  /* A workspace report can start a newer snapshot while this one is still
   * running. An old checkout must never replace the new tracker. */
  if (init->generation != self->diff_generation)
    return;

  g_clear_handle_id (&self->diff_timeout_id, g_source_remove);
  g_clear_object (&self->diff_cancellable);

  if (tracker != NULL)
    {
      g_clear_pointer (&self->diff_tracker, xd_git_diff_tracker_free);
      self->diff_tracker = g_steal_pointer (&tracker);
    }
}

static void
start_diff_tracker (XdDaemonTurn *self)
{
  g_autoptr (GTask) task = NULL;
  DiffInit *init = g_new0 (DiffInit, 1);

  self->diff_generation++;
  self->diff_cancellable = g_cancellable_new ();
  task = g_task_new (NULL, self->diff_cancellable,
                     on_diff_tracker_ready, g_object_ref (self));
  init->workdir = g_strdup (self->workdir);
  init->generation = self->diff_generation;
  g_task_set_task_data (task, init, (GDestroyNotify) diff_init_free);
  g_task_run_in_thread (task, build_diff_tracker);

  /*
   * Inline diffs are an enhancement. A repository expensive enough to miss
   * this budget would make every later snapshot expensive too, so abandon the
   * tracker rather than consuming a worker indefinitely.
   */
  self->diff_timeout_id = g_timeout_add_seconds_full (
    G_PRIORITY_DEFAULT, DIFF_INIT_BUDGET_SECONDS,
    cancel_slow_diff_tracker, g_object_ref (self), g_object_unref);
}

/* --- what the folder chain says -------------------------------------------- */

/* Reads a folder's id out of its dotfile. */
static char *
folder_id_at (const char *path)
{
  g_autoptr (XdFolderSettings) settings = xd_folder_settings_load (path, NULL);

  return settings != NULL ? g_strdup (settings->id) : NULL;
}

static char *
find_folder_path (const char *path,
                  const char *id)
{
  g_autofree char *here = folder_id_at (path);
  g_autoptr (GDir) dir = NULL;
  const char *entry;

  if (here != NULL && g_strcmp0 (here, id) == 0)
    return g_strdup (path);

  dir = g_dir_open (path, 0, NULL);
  while (dir != NULL && (entry = g_dir_read_name (dir)) != NULL)
    {
      g_autofree char *child = g_build_filename (path, entry, NULL);
      char *found;

      if (entry[0] == '.' || !g_file_test (child, G_FILE_TEST_IS_DIR))
        continue;

      found = find_folder_path (child, id);
      if (found != NULL)
        return found;
    }

  return NULL;
}

/*
 * A chat may point at a linked checkout which was removed after its last turn.
 * Restore the checkout it moved from; old rows without that history fall back
 * to folder settings. Persist before launch so later turns and every client
 * see the same recovered workspace.
 */
static char *
resolve_workdir (XdDaemonTurn              *self,
                 const XdChat              *chat,
                 const XdEffectiveSettings *resolved)
{
  const char *inherited = resolved != NULL ? resolved->workdir : NULL;
  const char *original =
    chat->original_workdir != NULL &&
    g_file_test (chat->original_workdir, G_FILE_TEST_IS_DIR)
      ? chat->original_workdir : inherited;

  if (chat->workdir == NULL || *chat->workdir == '\0')
    return g_strdup (original);

  if (g_file_test (chat->workdir, G_FILE_TEST_IS_DIR) ||
      original == NULL || *original == '\0' ||
      !g_file_test (original, G_FILE_TEST_IS_DIR))
    return g_strdup (chat->workdir);

  if (!xd_storage_restore_workdir (
        self->storage, chat->id, original, NULL))
    return g_strdup (chat->workdir);

  return g_strdup (original);
}

/*
 * The folder chain as nodes, so the resolver the window uses can be used here.
 *
 * The settings it reads are files on this machine, and the rules for which one
 * wins are worth having in one place rather than two -- a folder's model
 * meaning something different depending on which side asked would be its own
 * kind of bug.
 *
 * @keep holds the chain alive: a node's parent is a weak reference, so nothing
 * else does.
 */
static XdNode *
folder_chain (XdDaemonTurn *self,
              const char   *folder_id,
              GPtrArray    *keep)
{
  g_autofree char *path = NULL;
  g_autoptr (GPtrArray) paths = g_ptr_array_new_with_free_func (g_free);
  XdNode *parent = NULL;
  XdNode *node = NULL;

  if (folder_id == NULL)
    return NULL;

  path = find_folder_path (self->root_path, folder_id);
  if (path == NULL)
    return NULL;

  /* Up to the workspace root, then built back down so each node has its
   * parent. */
  for (char *at = g_strdup (path); at != NULL; )
    {
      char *up;

      g_ptr_array_insert (paths, 0, at);

      if (g_strcmp0 (at, self->root_path) == 0)
        break;

      up = g_path_get_dirname (at);
      if (g_strcmp0 (up, at) == 0)
        {
          g_free (up);
          break;
        }

      at = up;
    }

  for (guint i = 0; i < paths->len; i++)
    {
      const char *at = g_ptr_array_index (paths, i);
      g_autofree char *id = folder_id_at (at);
      g_autofree char *name = g_path_get_basename (at);

      node = xd_node_new_folder (at, name, id);
      xd_node_set_parent (node, parent);
      g_ptr_array_add (keep, node);
      parent = node;
    }

  return node;
}

/* --- the turn -------------------------------------------------------------- */

static void
on_commands (XdChatSession    *session,
             const char *const *commands,
             gpointer          user_data)
{
  XdDaemonTurn *self = user_data;

  g_strfreev (self->commands);
  self->commands = g_strdupv ((char **) commands);
  g_signal_emit (self, signals[SIGNAL_COMMANDS], 0, commands);
}

static void
on_session_started (XdChatSession *session,
                    const char    *session_id,
                    gpointer       user_data)
{
  XdDaemonTurn *self = user_data;
  g_autoptr (GError) error = NULL;

  /* Stored immediately, and against the backend that issued it: if the daemon
   * dies mid-reply the conversation can still be resumed from where the CLI
   * left it. */
  if (!xd_storage_set_session_id (self->storage, self->chat_id,
                                  self->backend_id, session_id, &error))
    g_warning ("cannot store the session id: %s", error->message);
}

static void
on_text_delta (XdChatSession *session,
               const char    *delta,
               gpointer       user_data)
{
  XdDaemonTurn *self = user_data;
  g_autoptr (GError) error = NULL;

  g_string_append (self->text, delta);
  g_string_append (self->segment, delta);

  if (self->segment_message_id == 0)
    {
      if (!xd_storage_append_message_with_id (
            self->storage, self->chat_id, "assistant", self->segment->str,
            NULL, self->label, &self->segment_message_id, &error))
        g_warning ("cannot store live turn output: %s", error->message);
    }
  else if (!xd_storage_update_message (
             self->storage, self->segment_message_id,
             self->segment->str, &error))
    {
      g_warning ("cannot update live turn output: %s", error->message);
    }

  g_signal_emit (self, signals[SIGNAL_TEXT], 0, delta);
}

/*
 * Ends the message being written, if there is one.
 *
 * An agent that works in steps says something, uses a tool, then says
 * something about what it found. Each stretch of speech is its own message, so
 * the transcript reads the way the work went.
 */
static void
item_free (XdTurnItem *item)
{
  g_free (item->text);
  g_free (item);
}

static void
remember (XdDaemonTurn *self,
          gboolean      tool,
          const char   *text)
{
  XdTurnItem *item = g_new0 (XdTurnItem, 1);

  item->tool = tool;
  item->text = g_strdup (text);

  g_ptr_array_add (self->items, item);
}

static gboolean
switch_workspace (XdDaemonTurn *self,
                  const char   *reported)
{
  g_autofree char *workdir =
    xd_worktree_registered_path (self->workdir, reported);
  g_autoptr (GError) error = NULL;

  if (workdir == NULL)
    {
      g_warning ("ignoring unregistered workspace reported by agent: %s",
                 reported);
      return FALSE;
    }

  if (xd_worktree_path_equal (self->workdir, workdir))
    return TRUE;

  if (!xd_storage_switch_workdir (
        self->storage, self->chat_id, workdir, self->workdir, &error))
    {
      g_warning ("cannot switch to the agent's workspace: %s", error->message);
      return FALSE;
    }

  g_free (self->workdir);
  self->workdir = g_steal_pointer (&workdir);

  g_clear_handle_id (&self->diff_timeout_id, g_source_remove);
  if (self->diff_cancellable != NULL)
    g_cancellable_cancel (self->diff_cancellable);
  g_clear_object (&self->diff_cancellable);
  g_clear_pointer (&self->diff_tracker, xd_git_diff_tracker_free);
  start_diff_tracker (self);

  return TRUE;
}

static void
close_segment (XdDaemonTurn *self)
{
  g_autofree char *reported = NULL;
  g_autofree char *without_workspace = NULL;
  g_autoptr (GError) error = NULL;

  if (self->segment->len == 0)
    return;

  reported =
    xd_workspace_block_parse (self->segment->str, &without_workspace);
  if (reported != NULL && switch_workspace (self, reported))
    {
      g_string_assign (self->segment, without_workspace);

      if (self->segment_message_id != 0 &&
          !(self->segment->len > 0
              ? xd_storage_update_message (
                  self->storage, self->segment_message_id,
                  self->segment->str, &error)
              : xd_storage_delete_message (
                  self->storage, self->segment_message_id, &error)))
        g_warning ("cannot hide workspace control markup: %s", error->message);
    }

  if (self->segment->len == 0)
    {
      self->segment_message_id = 0;
      return;
    }

  remember (self, FALSE, self->segment->str);
  g_string_truncate (self->segment, 0);
  self->segment_message_id = 0;
}

static void
on_tool_use (XdChatSession *session,
             const char    *name,
             gpointer       user_data)
{
  XdDaemonTurn *self = user_data;
  g_autoptr (GError) error = NULL;
  g_autofree char *diff =
    xd_git_diff_tracker_capture (self->diff_tracker, name);
  g_autofree char *tool =
    xd_workflow_run_capture_tool (diff, self->workdir);

  close_segment (self);
  remember (self, TRUE, tool);

  if (!xd_storage_append_message (self->storage, self->chat_id, "tool", tool,
                                  NULL, NULL, &error))
    g_warning ("cannot store live tool output: %s", error->message);

  g_signal_emit (self, signals[SIGNAL_TOOL], 0, tool);
}

static void
on_usage (XdChatSession *session,
          guint64        used,
          guint64        window,
          gpointer       user_data)
{
  XdDaemonTurn *self = user_data;

  self->context_used = used;
  self->context_window = window;
}

static void
store_turn_duration (XdDaemonTurn *self)
{
  g_autofree char *content =
    g_strdup_printf ("%" G_GINT64_FORMAT, xd_daemon_turn_get_elapsed (self));
  g_autoptr (GError) error = NULL;

  if (!xd_storage_append_message (self->storage, self->chat_id, "duration",
                                  content, NULL, NULL, &error))
    g_warning ("cannot store turn duration: %s", error->message);
}

static void
on_finished (XdChatSession *session,
             gboolean       success,
             const char    *message,
             gpointer       user_data)
{
  XdDaemonTurn *self = user_data;
  g_autoptr (GError) error = NULL;
  g_autoptr (XdAsk) asked =
    xd_ask_parse (self->segment != NULL ? self->segment->str : NULL, NULL);

  self->asked_user = asked != NULL;
  close_segment (self);
  store_turn_duration (self);

  if (self->context_used > 0 && self->context_window > 0 &&
      !xd_storage_set_context_usage (
        self->storage, self->chat_id, self->backend_id, self->model_id,
        self->context_used, self->context_window, &error))
    {
      g_warning ("cannot store context usage: %s", error->message);
      g_clear_error (&error);
    }

  if (!success && message != NULL)
    {
      xd_storage_append_message (self->storage, self->chat_id, "error",
                                 message, NULL, NULL, NULL);
    }

  /* How far this backend has been brought, so the next turn knows what it
   * still has to be told. */
  if (!xd_storage_set_last_seen (self->storage, self->chat_id, self->backend_id,
                                 xd_storage_last_message_id (self->storage,
                                                             self->chat_id),
                                 &error))
    g_warning ("cannot record what the backend has seen: %s", error->message);

  g_signal_emit (self, signals[SIGNAL_FINISHED], 0, success, message);
}

/* What answered, for the label under a reply. */
static char *
reply_label (const XdChat    *chat,
             const AiBackend *backend,
             const char      *model)
{
  const char *effort = chat->effort != NULL
    ? ai_effort_label (ai_effort_from_string (chat->effort))
    : ai_effort_label (ai_backend_default_effort (backend));

  return g_strdup_printf ("%s · %s", ai_backend_model_label (backend, model),
                          effort);
}

char *
xd_daemon_turn_resolve_workdir (XdDaemonTurn *self,
                                const XdChat  *chat)
{
  g_autoptr (GPtrArray) chain = g_ptr_array_new_with_free_func (g_object_unref);
  g_autoptr (XdEffectiveSettings) resolved = NULL;

  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), NULL);
  g_return_val_if_fail (chat != NULL, NULL);

  resolved = xd_settings_resolve (folder_chain (self, chat->folder_id, chain),
                                  chat->backend);

  return resolve_workdir (self, chat, resolved);
}

gboolean
xd_daemon_turn_start (XdDaemonTurn  *self,
                      const char    *chat_id,
                      const char    *prompt,
                      gboolean       user_submitted,
                      GError       **error)
{
  g_autoptr (XdChat) chat = NULL;
  g_autoptr (XdEffectiveSettings) resolved = NULL;
  g_autoptr (GPtrArray) chain = g_ptr_array_new_with_free_func (g_object_unref);
  g_autofree char *resume_session_id = NULL;
  g_autofree char *handover = NULL;
  g_autofree char *full_prompt = NULL;
  g_autofree char *instructions = NULL;
  g_auto (GStrv) folder_ids = NULL;
  const AiBackend *backend;
  g_autofree char *workdir = NULL;
  const char *model;
  XdNode *folder;
  AiRunSpec spec = { 0 };

  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), FALSE);
  g_return_val_if_fail (chat_id != NULL && prompt != NULL, FALSE);
  g_return_val_if_fail (self->session == NULL, FALSE);

  self->started_at = g_get_monotonic_time ();

  chat = xd_storage_get_chat (self->storage, chat_id, error);
  if (chat == NULL)
    return FALSE;

  backend = ai_backend_lookup (chat->backend);
  if (backend == NULL)
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                   "Unknown backend “%s”.", chat->backend);
      return FALSE;
    }

  /* Stored before anything can go wrong, so the message is in the transcript
   * even if the CLI never starts. */
  if (!(user_submitted
        ? xd_storage_append_message (
            self->storage, chat_id, "user", prompt, NULL, NULL, error)
        : xd_storage_append_message_without_recency (
            self->storage, chat_id, "user", prompt, NULL, NULL, error)))
    return FALSE;

  self->transcript_message_id =
    xd_storage_last_message_id (self->storage, chat_id);

  resume_session_id = xd_storage_get_session_id (self->storage, chat_id,
                                                 backend->id, NULL);
  handover = xd_handover_build (self->storage, chat_id,
                                xd_storage_get_last_seen (self->storage, chat_id,
                                                          backend->id));
  full_prompt = xd_handover_join (handover, prompt);

  /* Resolved per turn rather than at creation, so editing a folder's
   * instructions or model takes effect on the next message. */
  folder = folder_chain (self, chat->folder_id, chain);
  resolved = xd_settings_resolve (folder, chat->backend);

  workdir = resolve_workdir (self, chat, resolved);
  model = chat->model != NULL ? chat->model : resolved->model;

  instructions = resolved->instructions != NULL
    ? g_strdup_printf ("%s\n\n%s", resolved->instructions, xd_ask_instructions ())
    : g_strdup (xd_ask_instructions ());

  {
    g_autofree char *place = xd_settings_describe_place (folder, workdir);

    if (place != NULL)
      {
        char *with_place = g_strdup_printf ("%s\n\n%s", place, instructions);

        g_free (instructions);
        instructions = with_place;
      }
  }

  self->chat_id = g_strdup (chat_id);
  self->backend_id = g_strdup (backend->id);
  self->model_id =
    g_strdup (model != NULL ? model : backend->default_model);
  self->label = reply_label (chat, backend, model);
  self->workdir = g_strdup (workdir);
  self->resumed = resume_session_id != NULL;
  self->text = g_string_new (NULL);
  self->segment = g_string_new (NULL);
  self->items = g_ptr_array_new_with_free_func ((GDestroyNotify) item_free);
  self->session = xd_chat_session_new (backend);

  g_signal_connect (self->session, "session-started",
                    G_CALLBACK (on_session_started), self);
  g_signal_connect (self->session, "commands", G_CALLBACK (on_commands), self);
  g_signal_connect (self->session, "text-delta", G_CALLBACK (on_text_delta), self);
  g_signal_connect (self->session, "tool-use", G_CALLBACK (on_tool_use), self);
  g_signal_connect (self->session, "usage", G_CALLBACK (on_usage), self);
  g_signal_connect (self->session, "finished", G_CALLBACK (on_finished), self);

  spec.prompt = full_prompt;
  spec.workdir = workdir;
  folder_ids = xd_node_folder_ids (folder);
  spec.folder_ids = (const char *const *) folder_ids;
  spec.model = model;
  spec.system_prompt = instructions;
  spec.resume_session_id = resume_session_id;
  spec.effort = chat->effort != NULL ? ai_effort_from_string (chat->effort)
                                     : ai_backend_default_effort (backend);
  /* Plan overrides the access level for as long as it is on, without
   * overwriting it. */
  spec.access = chat->plan ? AI_ACCESS_PLAN : ai_access_from_string (chat->access);

  if (!xd_chat_session_start (self->session, &spec, error))
    {
      xd_storage_append_message (self->storage, chat_id, "error",
                                 (*error)->message, NULL, NULL, NULL);
      g_clear_object (&self->session);
      return FALSE;
    }

  start_diff_tracker (self);

  return TRUE;
}

void
xd_daemon_turn_cancel (XdDaemonTurn *self)
{
  g_return_if_fail (XD_IS_DAEMON_TURN (self));

  if (self->session != NULL)
    xd_chat_session_cancel (self->session);
}

gboolean
xd_daemon_turn_is_running (XdDaemonTurn *self)
{
  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), FALSE);

  return self->session != NULL && xd_chat_session_is_running (self->session);
}

gint64
xd_daemon_turn_get_elapsed (XdDaemonTurn *self)
{
  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), 0);

  if (self->started_at <= 0)
    return 0;

  return MAX ((g_get_monotonic_time () - self->started_at) / G_USEC_PER_SEC, 0);
}

const char *
xd_daemon_turn_get_label (XdDaemonTurn *self)
{
  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), NULL);

  return self->label;
}

const char *
xd_daemon_turn_get_workdir (XdDaemonTurn *self)
{
  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), NULL);

  return self->workdir;
}

gint64
xd_daemon_turn_get_transcript_id (XdDaemonTurn *self)
{
  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), 0);

  return self->transcript_message_id;
}

GPtrArray *
xd_daemon_turn_get_items (XdDaemonTurn *self)
{
  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), NULL);

  return self->items;
}

const char *
xd_daemon_turn_get_segment (XdDaemonTurn *self)
{
  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), NULL);

  return self->segment != NULL ? self->segment->str : NULL;
}

gboolean
xd_daemon_turn_asked_user (XdDaemonTurn *self)
{
  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), FALSE);

  return self->asked_user;
}

const char *const *
xd_daemon_turn_get_commands (XdDaemonTurn *self)
{
  g_return_val_if_fail (XD_IS_DAEMON_TURN (self), NULL);

  return (const char *const *) self->commands;
}

XdDaemonTurn *
xd_daemon_turn_new (XdStorage  *storage,
                    const char *root_path)
{
  XdDaemonTurn *self;

  g_return_val_if_fail (XD_IS_STORAGE (storage), NULL);

  self = g_object_new (XD_TYPE_DAEMON_TURN, NULL);
  self->storage = g_object_ref (storage);
  self->root_path = g_strdup (root_path);

  return self;
}

static void
xd_daemon_turn_dispose (GObject *object)
{
  XdDaemonTurn *self = XD_DAEMON_TURN (object);

  if (self->session != NULL)
    g_signal_handlers_disconnect_by_data (self->session, self);

  g_clear_handle_id (&self->diff_timeout_id, g_source_remove);
  if (self->diff_cancellable != NULL)
    g_cancellable_cancel (self->diff_cancellable);
  g_clear_object (&self->diff_cancellable);
  g_clear_object (&self->session);
  g_clear_object (&self->storage);

  G_OBJECT_CLASS (xd_daemon_turn_parent_class)->dispose (object);
}

static void
xd_daemon_turn_finalize (GObject *object)
{
  XdDaemonTurn *self = XD_DAEMON_TURN (object);

  g_clear_pointer (&self->root_path, g_free);
  g_clear_pointer (&self->chat_id, g_free);
  g_clear_pointer (&self->backend_id, g_free);
  g_clear_pointer (&self->model_id, g_free);
  g_clear_pointer (&self->label, g_free);
  g_clear_pointer (&self->workdir, g_free);
  g_clear_pointer (&self->diff_tracker, xd_git_diff_tracker_free);
  g_clear_pointer (&self->items, g_ptr_array_unref);
  g_clear_pointer (&self->commands, g_strfreev);

  if (self->text != NULL)
    g_string_free (self->text, TRUE);
  if (self->segment != NULL)
    g_string_free (self->segment, TRUE);

  G_OBJECT_CLASS (xd_daemon_turn_parent_class)->finalize (object);
}

static void
xd_daemon_turn_class_init (XdDaemonTurnClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = xd_daemon_turn_dispose;
  object_class->finalize = xd_daemon_turn_finalize;

  signals[SIGNAL_COMMANDS] =
    g_signal_new ("commands", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRV);

  signals[SIGNAL_TEXT] =
    g_signal_new ("text", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRING);

  signals[SIGNAL_TOOL] =
    g_signal_new ("tool", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRING);

  signals[SIGNAL_FINISHED] =
    g_signal_new ("finished", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 2,
                  G_TYPE_BOOLEAN, G_TYPE_STRING);
}

static void
xd_daemon_turn_init (XdDaemonTurn *self)
{
}
