#include "chat-view.h"

#include "chat-session.h"
#include "message-row.h"
#include "model-picker.h"
#include "settings/settings-resolver.h"
#include "util/git-info.h"

/*
 * A turn in flight.
 *
 * Turns are tracked per chat rather than per view so that switching to another
 * chat while the model is answering does not throw the answer away: the
 * subprocess keeps running, the text keeps accumulating, and the reply is
 * still written to the database. Coming back to the chat re-attaches the live
 * row to what has arrived so far.
 */
typedef struct
{
  HyChatView *view;         /* unowned; the view outlives its turns */
  HyChatSession *session;
  char *chat_id;
  char *backend_id;         /* the backend this turn's session id belongs to */
  char *prompt;             /* kept so a dead session can be retried */
  GString *text;
  HyMessageRow *row;        /* NULL while the chat is not on screen */
  gboolean resumed;
  gboolean is_retry;
} Turn;

struct _HyChatView
{
  AdwBin parent_instance;

  HyStorage *storage;
  HyFsTree *tree;
  HyNode *chat;                 /* unowned; owned by the tree */

  GHashTable *turns;            /* chat id -> Turn* */

  AdwWindowTitle *title;
  GtkStack *stack;
  GtkBox *transcript;
  GtkScrolledWindow *scroller;
  GtkTextView *composer;
  GtkButton *send_button;
  GtkWidget *composer_area;
  HyModelPicker *model_picker;
  GtkLabel *context_label;
};

G_DEFINE_FINAL_TYPE (HyChatView, hy_chat_view, ADW_TYPE_BIN)

static void send_current_message (HyChatView *self);
static void update_send_button (HyChatView *self);
static void start_turn (HyChatView *self,
                        const char *prompt);
static void on_model_chosen (HyModelPicker *picker,
                             const char    *backend_id,
                             const char    *model_id,
                             gpointer       user_data);
static HyMessageRow *append_row (HyChatView    *self,
                                 HyMessageKind  kind,
                                 const char    *text);
static const char *workdir_for (const HyChat              *chat,
                                const HyEffectiveSettings *resolved);

/* --- transcript ----------------------------------------------------------- */

static gboolean
scroll_to_bottom (gpointer data)
{
  HyChatView *self = data;
  GtkAdjustment *adjustment = gtk_scrolled_window_get_vadjustment (self->scroller);

  gtk_adjustment_set_value (adjustment,
                            gtk_adjustment_get_upper (adjustment) -
                            gtk_adjustment_get_page_size (adjustment));

  g_object_unref (self);

  return G_SOURCE_REMOVE;
}

/* A freshly appended row has no allocation yet, so the adjustment only knows
 * its final extent once layout has run. */
static void
queue_scroll_to_bottom (HyChatView *self)
{
  g_idle_add_full (G_PRIORITY_LOW, scroll_to_bottom, g_object_ref (self), NULL);
}

static HyMessageRow *
append_row (HyChatView    *self,
            HyMessageKind  kind,
            const char    *text)
{
  HyMessageRow *row = hy_message_row_new (kind, text);

  gtk_box_append (self->transcript, GTK_WIDGET (row));
  queue_scroll_to_bottom (self);

  return row;
}

static void
clear_transcript (HyChatView *self)
{
  GtkWidget *child;

  while ((child = gtk_widget_get_first_child (GTK_WIDGET (self->transcript))) != NULL)
    gtk_box_remove (self->transcript, child);
}

static void
load_transcript (HyChatView *self)
{
  g_autoptr (GPtrArray) messages = NULL;
  g_autoptr (GError) error = NULL;

  clear_transcript (self);

  messages = hy_storage_list_messages (self->storage,
                                       hy_node_get_chat_id (self->chat), &error);
  if (messages == NULL)
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      return;
    }

  for (guint i = 0; i < messages->len; i++)
    {
      const HyMessage *message = g_ptr_array_index (messages, i);

      append_row (self, hy_message_kind_from_role (message->role), message->content);
    }
}

/* --- turns ---------------------------------------------------------------- */

static Turn *
current_turn (HyChatView *self)
{
  if (self->chat == NULL)
    return NULL;

  return g_hash_table_lookup (self->turns, hy_node_get_chat_id (self->chat));
}

static void
turn_free (gpointer data)
{
  Turn *turn = data;

  g_clear_object (&turn->session);
  g_clear_pointer (&turn->chat_id, g_free);
  g_clear_pointer (&turn->backend_id, g_free);
  g_clear_pointer (&turn->prompt, g_free);
  g_string_free (turn->text, TRUE);
  g_free (turn);
}

/* True when @turn's chat is the one currently on screen. */
static gboolean
turn_is_visible (Turn *turn)
{
  return turn->view->chat != NULL &&
         g_strcmp0 (hy_node_get_chat_id (turn->view->chat), turn->chat_id) == 0;
}

static void
on_session_started (HyChatSession *session,
                    const char    *session_id,
                    gpointer       user_data)
{
  Turn *turn = user_data;
  g_autoptr (GError) error = NULL;

  /* Stored immediately, and against the backend that issued it: if hy dies
   * mid-reply the conversation can still be resumed from where the CLI left
   * it, and switching assistants does not overwrite the other's session. */
  if (!hy_storage_set_session_id (turn->view->storage, turn->chat_id,
                                  turn->backend_id, session_id, &error))
    g_warning ("cannot store the session id: %s", error->message);
}

static void
on_text_delta (HyChatSession *session,
               const char    *delta,
               gpointer       user_data)
{
  Turn *turn = user_data;

  g_string_append (turn->text, delta);

  if (turn->row != NULL)
    {
      hy_message_row_append (turn->row, delta);
      queue_scroll_to_bottom (turn->view);
    }
}

static void
on_tool_use (HyChatSession *session,
             const char    *name,
             gpointer       user_data)
{
  Turn *turn = user_data;

  if (turn_is_visible (turn))
    {
      g_autofree char *text = g_strdup_printf ("Using %s…", name);

      append_row (turn->view, HY_MESSAGE_TOOL, text);
    }
}

static void
on_turn_finished (HyChatSession *session,
                  gboolean       success,
                  const char    *message,
                  gpointer       user_data)
{
  Turn *turn = user_data;
  HyChatView *self = turn->view;
  g_autoptr (GError) error = NULL;
  g_autofree char *chat_id = g_strdup (turn->chat_id);
  g_autofree char *retry_prompt = NULL;
  gboolean visible = turn_is_visible (turn);

  /*
   * A resumed turn that failed without producing a single token is almost
   * always a session the CLI no longer has -- they are cleaned up over time.
   * Forget it and run the same message again from the transcript instead of
   * making the user retype it. Checked by outcome rather than by matching the
   * CLI's error text, which neither CLI promises to keep stable.
   */
  if (!success && turn->resumed && !turn->is_retry && turn->text->len == 0)
    {
      retry_prompt = g_strdup (turn->prompt);

      if (!hy_storage_set_session_id (self->storage, chat_id, turn->backend_id,
                                      NULL, &error))
        g_warning ("cannot forget the stale session: %s", error->message);

      if (turn->row != NULL)
        gtk_widget_set_visible (GTK_WIDGET (turn->row), FALSE);

      g_hash_table_remove (self->turns, chat_id);

      if (visible)
        start_turn (self, retry_prompt);
      return;
    }

  if (turn->text->len > 0)
    {
      if (!hy_storage_append_message (self->storage, chat_id, "assistant",
                                      turn->text->str, NULL, &error))
        g_warning ("cannot store the reply: %s", error->message);
    }

  /* This backend has now been told everything up to and including its own
   * reply, so the next turn only has to replay what comes after. */
  if (success &&
      !hy_storage_set_last_seen (self->storage, chat_id, turn->backend_id,
                                 hy_storage_last_message_id (self->storage, chat_id),
                                 &error))
    g_warning ("cannot record what the assistant has seen: %s", error->message);

  if (!success)
    {
      const char *text = message != NULL && *message != '\0'
                           ? message : "The backend stopped unexpectedly.";

      if (!hy_storage_append_message (self->storage, chat_id, "error", text,
                                      NULL, &error))
        g_warning ("cannot store the error: %s", error->message);

      if (visible)
        append_row (self, HY_MESSAGE_ERROR, text);
    }

  if (turn->row != NULL)
    {
      hy_message_row_set_waiting (turn->row, FALSE);

      /* Nothing came back at all: say so rather than leaving a blank card. */
      if (turn->text->len == 0 && success)
        hy_message_row_append (turn->row, "(no reply)");
    }

  /* Frees the turn, so nothing may touch it afterwards. */
  g_hash_table_remove (self->turns, chat_id);

  if (visible)
    update_send_button (self);
}

/* --- sending -------------------------------------------------------------- */

static char *
take_composer_text (HyChatView *self)
{
  GtkTextBuffer *buffer = gtk_text_view_get_buffer (self->composer);
  GtkTextIter start, end;
  g_autofree char *text = NULL;
  char *trimmed;

  gtk_text_buffer_get_bounds (buffer, &start, &end);
  text = gtk_text_buffer_get_text (buffer, &start, &end, FALSE);

  trimmed = g_strdup (text);
  g_strstrip (trimmed);

  if (*trimmed == '\0')
    {
      g_free (trimmed);
      return NULL;
    }

  gtk_text_buffer_set_text (buffer, "", -1);

  return trimmed;
}

/* Where the backend runs, which is how project context reaches the CLI. The
 * chat's own choice wins; otherwise the folder chain decides. */
static const char *
workdir_for (const HyChat              *chat,
             const HyEffectiveSettings *resolved)
{
  if (chat->workdir != NULL && *chat->workdir != '\0')
    return chat->workdir;

  return resolved->workdir;
}

/* Roughly how much earlier conversation to hand to a backend that has no
 * session of its own. Enough to carry the thread, not so much that it crowds
 * out the message the user actually asked. */
#define HANDOVER_LIMIT_BYTES 12000

/*
 * Retells whatever this backend has not been told.
 *
 * Resuming a session restores only what *that* assistant was sent, so
 * anything said to the other one in between is missing from it. Replaying
 * those messages is what keeps one conversation coherent across two CLIs --
 * and it matters on every turn, not only the first after a switch. The
 * message being sent right now is already stored, so the last entry is
 * skipped; it travels as the prompt.
 */
static char *
build_handover (HyChatView *self,
                const char *chat_id,
                gint64      last_seen)
{
  g_autoptr (GPtrArray) messages = NULL;
  g_autoptr (GString) text = NULL;
  gsize budget = 0;
  guint first;

  messages = hy_storage_list_messages_since (self->storage, chat_id, last_seen, NULL);
  if (messages == NULL || messages->len < 2)
    return NULL;

  /* Walk back from the most recent, keeping what fits. */
  for (first = messages->len - 1; first > 0; first--)
    {
      const HyMessage *message = g_ptr_array_index (messages, first - 1);

      budget += strlen (message->content) + 16;
      if (budget > HANDOVER_LIMIT_BYTES)
        break;
    }

  text = g_string_new ("[Part of this conversation happened with a different "
                       "assistant, so you have not seen it. It is reproduced "
                       "below verbatim. Treat it as part of the conversation "
                       "you are already in: continue from it, and do not greet "
                       "the user again or re-introduce yourself.]\n\n");

  for (guint i = first; i + 1 < messages->len; i++)
    {
      const HyMessage *message = g_ptr_array_index (messages, i);
      const char *who;

      if (g_strcmp0 (message->role, "user") == 0)
        who = "User";
      else if (g_strcmp0 (message->role, "assistant") == 0)
        who = "Assistant";
      else
        continue;   /* errors and tool notes are ours, not the conversation */

      g_string_append_printf (text, "%s: %s\n\n", who, message->content);
    }

  g_string_append (text, "[End of earlier conversation. The user's new message "
                         "follows.]");

  return g_string_free (g_steal_pointer (&text), FALSE);
}

static void
start_turn (HyChatView *self,
            const char *prompt)
{
  g_autoptr (HyChat) chat = NULL;
  g_autoptr (GError) error = NULL;
  g_autoptr (HyEffectiveSettings) resolved = NULL;
  g_autofree char *resume_session_id = NULL;
  g_autofree char *handover = NULL;
  g_autofree char *full_prompt = NULL;
  const AiBackend *backend;
  AiRunSpec spec = { 0 };
  Turn *turn;

  chat = hy_storage_get_chat (self->storage, hy_node_get_chat_id (self->chat),
                              &error);
  if (chat == NULL)
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      return;
    }

  backend = ai_backend_lookup (chat->backend);
  if (backend == NULL)
    {
      g_autofree char *text = g_strdup_printf ("Unknown backend “%s”.",
                                               chat->backend);

      append_row (self, HY_MESSAGE_ERROR, text);
      return;
    }

  resume_session_id = hy_storage_get_session_id (self->storage, chat->id,
                                                 backend->id, NULL);

  /* Whatever this backend has not been told -- because the chat is new, or
   * because those turns went to the other assistant. */
  handover = build_handover (self, chat->id,
                             hy_storage_get_last_seen (self->storage, chat->id,
                                                       backend->id));

  full_prompt = handover != NULL ? g_strdup_printf ("%s\n\n%s", handover, prompt)
                                 : g_strdup (prompt);

  turn = g_new0 (Turn, 1);
  turn->view = self;
  turn->chat_id = g_strdup (chat->id);
  turn->backend_id = g_strdup (backend->id);
  turn->prompt = g_strdup (prompt);
  turn->resumed = resume_session_id != NULL;
  turn->text = g_string_new (NULL);
  turn->session = hy_chat_session_new (backend);
  turn->row = append_row (self, HY_MESSAGE_ASSISTANT, NULL);
  hy_message_row_set_waiting (turn->row, TRUE);

  g_signal_connect (turn->session, "session-started",
                    G_CALLBACK (on_session_started), turn);
  g_signal_connect (turn->session, "text-delta",
                    G_CALLBACK (on_text_delta), turn);
  g_signal_connect (turn->session, "tool-use",
                    G_CALLBACK (on_tool_use), turn);
  g_signal_connect (turn->session, "finished",
                    G_CALLBACK (on_turn_finished), turn);

  g_hash_table_insert (self->turns, g_strdup (chat->id), turn);

  /* Resolved per turn rather than at creation, so editing a folder's
   * instructions or model takes effect on the next message instead of only on
   * chats made afterwards. */
  resolved = hy_settings_resolve (hy_node_get_parent (self->chat), chat->backend);

  spec.prompt = full_prompt;
  spec.workdir = workdir_for (chat, resolved);
  /* The chat's own pick wins; the folder chain is the fallback. */
  spec.model = chat->model != NULL ? chat->model : resolved->model;
  spec.system_prompt = resolved->instructions;
  spec.resume_session_id = resume_session_id;

  if (!hy_chat_session_start (turn->session, &spec, &error))
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      hy_storage_append_message (self->storage, chat->id, "error",
                                 error->message, NULL, NULL);
      g_hash_table_remove (self->turns, chat->id);
    }

  update_send_button (self);
}

/* How much of the first message becomes the chat's name. */
#define TITLE_LENGTH 48

/*
 * An unnamed chat takes its name from what was asked first. Deriving it from
 * the text costs nothing, where asking the model for a title would cost a
 * whole extra round trip before the answer even starts.
 */
static void
name_chat_after_first_message (HyChatView *self,
                               const char *prompt)
{
  g_autoptr (GPtrArray) messages = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *title = NULL;
  const char *newline;
  glong length;

  if (g_strcmp0 (hy_node_get_name (self->chat), "New Chat") != 0)
    return;

  messages = hy_storage_list_messages (self->storage,
                                       hy_node_get_chat_id (self->chat), &error);
  if (messages == NULL || messages->len > 1)
    return;

  /* First line only: a pasted stack trace should not become the title. */
  newline = strchr (prompt, '\n');
  title = newline != NULL ? g_strndup (prompt, newline - prompt)
                          : g_strdup (prompt);
  g_strstrip (title);

  length = g_utf8_strlen (title, -1);
  if (length > TITLE_LENGTH)
    {
      g_autofree char *shortened = g_utf8_substring (title, 0, TITLE_LENGTH);

      g_free (title);
      title = g_strconcat (shortened, "…", NULL);
    }

  if (*title == '\0')
    return;

  if (!hy_fs_tree_rename_chat (self->tree, self->chat, title, &error))
    {
      g_warning ("cannot name the chat: %s", error->message);
      return;
    }

  adw_window_title_set_title (self->title, title);
}

static void
send_current_message (HyChatView *self)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *text = NULL;

  if (self->chat == NULL)
    return;

  /* One turn at a time per chat; the button is a stop button meanwhile. */
  if (current_turn (self) != NULL)
    return;

  text = take_composer_text (self);
  if (text == NULL)
    return;

  if (!hy_storage_append_message (self->storage, hy_node_get_chat_id (self->chat),
                                  "user", text, NULL, &error))
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      return;
    }

  append_row (self, HY_MESSAGE_USER, text);
  name_chat_after_first_message (self, text);
  hy_fs_tree_bump_chat (self->tree, self->chat);

  start_turn (self, text);
}

static void
update_send_button (HyChatView *self)
{
  gboolean running = current_turn (self) != NULL;

  gtk_button_set_icon_name (self->send_button,
                            running ? "media-playback-stop-symbolic"
                                    : "go-up-symbolic");
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->send_button),
                               running ? "Stop" : "Send (Enter)");

  if (running)
    {
      gtk_widget_remove_css_class (GTK_WIDGET (self->send_button), "suggested-action");
      gtk_widget_add_css_class (GTK_WIDGET (self->send_button), "destructive-action");
    }
  else
    {
      gtk_widget_remove_css_class (GTK_WIDGET (self->send_button), "destructive-action");
      gtk_widget_add_css_class (GTK_WIDGET (self->send_button), "suggested-action");
    }
}

/* --- the context bar ------------------------------------------------------ */

/*
 * Describes the checkout the next message will run against: branch, the
 * directory's name, whether it is a linked worktree, and where it came from.
 */
static char *
describe_context (const char *workdir)
{
  g_autoptr (HyGitInfo) git = hy_git_info_for_path (workdir);
  g_autoptr (GString) text = g_string_new (NULL);

  if (workdir == NULL)
    return g_strdup ("No working directory");

  if (git == NULL)
    {
      g_autofree char *name = g_path_get_basename (workdir);

      return g_strdup_printf ("%s — not a repository", name);
    }

  if (git->branch != NULL)
    g_string_append_printf (text, "%s %s", git->detached ? "detached at" : "⎇",
                            git->branch);

  g_string_append_printf (text, "%s%s", text->len > 0 ? " · " : "", git->name);

  if (git->linked_worktree)
    g_string_append (text, " (worktree)");

  if (git->remote_url != NULL)
    g_string_append_printf (text, " · %s", git->remote_url);

  return g_string_free (g_steal_pointer (&text), FALSE);
}

static void
update_context_bar (HyChatView   *self,
                    const HyChat *chat)
{
  g_autoptr (HyEffectiveSettings) resolved = NULL;
  g_autofree char *description = NULL;

  resolved = hy_settings_resolve (hy_node_get_parent (self->chat), chat->backend);
  description = describe_context (workdir_for (chat, resolved));

  gtk_label_set_label (self->context_label, description);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->context_label), description);

  hy_model_picker_set_selected (self->model_picker, chat->backend,
                                chat->model != NULL ? chat->model : resolved->model);
}

static void
on_model_chosen (HyModelPicker *picker,
                 const char    *backend_id,
                 const char    *model_id,
                 gpointer       user_data)
{
  HyChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  g_autoptr (HyChat) chat = NULL;
  const char *chat_id;
  gboolean backend_changed;

  if (self->chat == NULL)
    return;

  chat_id = hy_node_get_chat_id (self->chat);
  chat = hy_storage_get_chat (self->storage, chat_id, NULL);
  if (chat == NULL)
    return;

  backend_changed = g_strcmp0 (chat->backend, backend_id) != 0;

  if (backend_changed &&
      !hy_storage_set_backend (self->storage, chat_id, backend_id, &error))
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      return;
    }

  if (!hy_storage_set_model (self->storage, chat_id, model_id, &error))
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      return;
    }

  /* Nothing is discarded here. Sessions are kept per backend, so switching
   * assistants resumes that assistant's own session when it has one, and
   * otherwise the next turn replays the transcript to it. Either way the
   * conversation carries over. */

  {
    g_autoptr (HyChat) updated = hy_storage_get_chat (self->storage, chat_id, NULL);

    if (updated != NULL)
      update_context_bar (self, updated);
  }
}

static void
on_send_clicked (GtkButton *button,
                 gpointer   user_data)
{
  HyChatView *self = user_data;
  Turn *turn = current_turn (self);

  if (turn != NULL)
    hy_chat_session_cancel (turn->session);
  else
    send_current_message (self);
}

static gboolean
on_composer_key (GtkEventControllerKey *controller,
                 guint                  keyval,
                 guint                  keycode,
                 GdkModifierType        state,
                 gpointer               user_data)
{
  HyChatView *self = user_data;

  /* Enter sends, Shift+Enter inserts a newline. */
  if ((keyval == GDK_KEY_Return || keyval == GDK_KEY_KP_Enter) &&
      !(state & GDK_SHIFT_MASK))
    {
      send_current_message (self);
      return GDK_EVENT_STOP;
    }

  return GDK_EVENT_PROPAGATE;
}

/* --- public API ----------------------------------------------------------- */

void
hy_chat_view_set_chat (HyChatView *self,
                       HyNode     *chat)
{
  Turn *turn;

  g_return_if_fail (HY_IS_CHAT_VIEW (self));

  /* The outgoing chat's row is about to be destroyed with the transcript. */
  turn = current_turn (self);
  if (turn != NULL)
    turn->row = NULL;

  self->chat = chat;

  if (chat == NULL)
    {
      clear_transcript (self);
      gtk_stack_set_visible_child_name (self->stack, "empty");
      gtk_widget_set_visible (self->composer_area, FALSE);
      adw_window_title_set_title (self->title, "hy");
      adw_window_title_set_subtitle (self->title, NULL);
      return;
    }

  gtk_stack_set_visible_child_name (self->stack, "chat");
  gtk_widget_set_visible (self->composer_area, TRUE);
  adw_window_title_set_title (self->title, hy_node_get_name (chat));

  {
    g_autoptr (HyChat) record = hy_storage_get_chat (self->storage,
                                                     hy_node_get_chat_id (chat),
                                                     NULL);

    if (record != NULL)
      update_context_bar (self, record);
  }

  load_transcript (self);

  /* Re-attach a reply that kept arriving while another chat was on screen. */
  turn = current_turn (self);
  if (turn != NULL)
    {
      turn->row = append_row (self, HY_MESSAGE_ASSISTANT, turn->text->str);
      hy_message_row_set_waiting (turn->row, TRUE);
    }

  update_send_button (self);
  gtk_widget_grab_focus (GTK_WIDGET (self->composer));
}

HyNode *
hy_chat_view_get_chat (HyChatView *self)
{
  g_return_val_if_fail (HY_IS_CHAT_VIEW (self), NULL);

  return self->chat;
}

HyChatView *
hy_chat_view_new (HyStorage *storage,
                  HyFsTree  *tree)
{
  HyChatView *self;

  g_return_val_if_fail (HY_IS_STORAGE (storage), NULL);
  g_return_val_if_fail (HY_IS_FS_TREE (tree), NULL);

  self = g_object_new (HY_TYPE_CHAT_VIEW, NULL);
  self->storage = g_object_ref (storage);
  self->tree = g_object_ref (tree);

  hy_chat_view_set_chat (self, NULL);

  return self;
}

/* --- construction --------------------------------------------------------- */

/*
 * The composer is a bar rather than a bare entry: the two things worth knowing
 * before pressing Enter are which assistant will answer and which checkout it
 * will be looking at, so both sit next to the text.
 */
static GtkWidget *
build_composer (HyChatView *self)
{
  GtkWidget *frame = gtk_frame_new (NULL);
  GtkWidget *column = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *toolbar = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *scroller = gtk_scrolled_window_new ();
  GtkEventController *keys;

  self->composer = GTK_TEXT_VIEW (gtk_text_view_new ());
  gtk_text_view_set_wrap_mode (self->composer, GTK_WRAP_WORD_CHAR);
  gtk_text_view_set_top_margin (self->composer, 8);
  gtk_text_view_set_bottom_margin (self->composer, 8);
  gtk_text_view_set_left_margin (self->composer, 8);
  gtk_text_view_set_right_margin (self->composer, 8);

  keys = gtk_event_controller_key_new ();
  g_signal_connect (keys, "key-pressed", G_CALLBACK (on_composer_key), self);
  gtk_widget_add_controller (GTK_WIDGET (self->composer), keys);

  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scroller),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_scrolled_window_set_max_content_height (GTK_SCROLLED_WINDOW (scroller), 180);
  gtk_scrolled_window_set_propagate_natural_height (GTK_SCROLLED_WINDOW (scroller), TRUE);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scroller),
                                 GTK_WIDGET (self->composer));

  self->model_picker = hy_model_picker_new ();
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->model_picker),
                               "Which assistant and model answer in this chat");
  g_signal_connect (self->model_picker, "model-chosen",
                    G_CALLBACK (on_model_chosen), self);

  self->context_label = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_ellipsize (self->context_label, PANGO_ELLIPSIZE_MIDDLE);
  gtk_label_set_xalign (self->context_label, 0.0f);
  gtk_widget_set_hexpand (GTK_WIDGET (self->context_label), TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (self->context_label), "dim-label");
  gtk_widget_add_css_class (GTK_WIDGET (self->context_label), "caption");

  self->send_button = GTK_BUTTON (gtk_button_new_from_icon_name ("go-up-symbolic"));
  gtk_widget_add_css_class (GTK_WIDGET (self->send_button), "suggested-action");
  gtk_widget_add_css_class (GTK_WIDGET (self->send_button), "circular");
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->send_button), "Send (Enter)");
  g_signal_connect (self->send_button, "clicked", G_CALLBACK (on_send_clicked), self);

  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->model_picker));
  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->context_label));
  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->send_button));
  gtk_widget_set_margin_start (toolbar, 6);
  gtk_widget_set_margin_end (toolbar, 6);
  gtk_widget_set_margin_bottom (toolbar, 6);

  gtk_box_append (GTK_BOX (column), scroller);
  gtk_box_append (GTK_BOX (column), toolbar);

  gtk_frame_set_child (GTK_FRAME (frame), column);
  gtk_widget_set_margin_top (frame, 6);
  gtk_widget_set_margin_bottom (frame, 12);
  gtk_widget_set_margin_start (frame, 12);
  gtk_widget_set_margin_end (frame, 12);

  return frame;
}

static void
hy_chat_view_dispose (GObject *object)
{
  HyChatView *self = HY_CHAT_VIEW (object);

  g_clear_pointer (&self->turns, g_hash_table_unref);
  g_clear_object (&self->storage);
  g_clear_object (&self->tree);

  G_OBJECT_CLASS (hy_chat_view_parent_class)->dispose (object);
}

static void
hy_chat_view_class_init (HyChatViewClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = hy_chat_view_dispose;
}

static void
hy_chat_view_init (HyChatView *self)
{
  GtkWidget *toolbar = adw_toolbar_view_new ();
  GtkWidget *header = adw_header_bar_new ();
  GtkWidget *content = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *empty = adw_status_page_new ();
  GtkWidget *menu_button;
  GMenu *menu;

  self->turns = g_hash_table_new_full (g_str_hash, g_str_equal, g_free, turn_free);

  self->title = ADW_WINDOW_TITLE (adw_window_title_new ("hy", NULL));
  adw_header_bar_set_title_widget (ADW_HEADER_BAR (header), GTK_WIDGET (self->title));

  menu = g_menu_new ();
  g_menu_append (menu, "Search…", "win.search");
  g_menu_append (menu, "About hy", "app.about");
  g_menu_append (menu, "Quit", "app.quit");

  menu_button = gtk_menu_button_new ();
  gtk_menu_button_set_icon_name (GTK_MENU_BUTTON (menu_button), "open-menu-symbolic");
  gtk_menu_button_set_menu_model (GTK_MENU_BUTTON (menu_button), G_MENU_MODEL (menu));
  g_object_unref (menu);
  adw_header_bar_pack_end (ADW_HEADER_BAR (header), menu_button);

  adw_toolbar_view_add_top_bar (ADW_TOOLBAR_VIEW (toolbar), header);

  adw_status_page_set_icon_name (ADW_STATUS_PAGE (empty), "chat-bubble-text-symbolic");
  adw_status_page_set_title (ADW_STATUS_PAGE (empty), "No Chat Selected");
  adw_status_page_set_description (ADW_STATUS_PAGE (empty),
                                   "Pick a chat in the sidebar, or start a new "
                                   "one in a folder.");

  self->transcript = GTK_BOX (gtk_box_new (GTK_ORIENTATION_VERTICAL, 0));
  gtk_widget_set_valign (GTK_WIDGET (self->transcript), GTK_ALIGN_START);

  self->scroller = GTK_SCROLLED_WINDOW (gtk_scrolled_window_new ());
  gtk_scrolled_window_set_policy (self->scroller, GTK_POLICY_NEVER,
                                  GTK_POLICY_AUTOMATIC);
  gtk_scrolled_window_set_child (self->scroller, GTK_WIDGET (self->transcript));
  gtk_widget_set_vexpand (GTK_WIDGET (self->scroller), TRUE);

  self->stack = GTK_STACK (gtk_stack_new ());
  gtk_stack_add_named (self->stack, empty, "empty");
  gtk_stack_add_named (self->stack, GTK_WIDGET (self->scroller), "chat");
  gtk_widget_set_vexpand (GTK_WIDGET (self->stack), TRUE);

  self->composer_area = build_composer (self);

  gtk_box_append (GTK_BOX (content), GTK_WIDGET (self->stack));
  gtk_box_append (GTK_BOX (content), self->composer_area);

  adw_toolbar_view_set_content (ADW_TOOLBAR_VIEW (toolbar), content);
  adw_bin_set_child (ADW_BIN (self), toolbar);
}
