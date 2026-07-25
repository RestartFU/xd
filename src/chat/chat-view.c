#include "chat-view.h"

#include "chat-session.h"
#include "message-row.h"
#include "model-picker.h"
#include "diff-pane.h"
#include "git-actions.h"
#include "terminal-panel.h"
#include "settings/settings-resolver.h"
#include "util/ask-block.h"
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
  char *label;              /* the model and effort this turn actually ran on */
  HyNode *node;             /* the row in the tree, so it can show the state */
  GString *text;            /* everything the turn has said, for the ask block */
  GString *segment;         /* what belongs in the row being written now */
  GPtrArray *said;          /* finished messages, held until the turn ends */
  HyMessageRow *row;        /* NULL until the segment has somewhere to go */
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
  GtkWidget *attachments_bar;
  GtkWidget *queued_bar;
  GtkLabel *queued_label;
  char *queued;             /* typed while a turn was running */
  gboolean syncing_panes;   /* setting the toggles to match the chat */
  GPtrArray *attachments;   /* absolute paths of pasted images */
  HyModelPicker *model_picker;
  GtkDropDown *effort_chooser;
  GtkDropDown *access_chooser;
  GtkToggleButton *build_toggle;
  GtkToggleButton *plan_toggle;
  GtkLabel *context_label;
  GtkToggleButton *terminal_button;
  HyTerminalPanel *terminal;
  GtkToggleButton *diff_button;
  HyGitActions *git_actions;
  HyDiffPane *diff;
  GtkPaned *split;
  GtkPaned *side_split;
  GSettings *settings;

  /* Set while the choosers are filled in from the chat, so the resulting
   * notify does not read back as the user picking something. */
  gboolean syncing_run_options;
};

/*
 * Offered in the composer bar, least permissive first.
 *
 * Plan is not on this list: it is a mode rather than a rung, and it sits on
 * its own toggle so leaving it restores whatever access the chat had.
 */
static const AiAccess access_choices[] = {
  AI_ACCESS_READ_ONLY, AI_ACCESS_EDIT, AI_ACCESS_FULL,
};

static const AiEffort effort_choices[] = {
  AI_EFFORT_LOW, AI_EFFORT_MEDIUM, AI_EFFORT_HIGH,
  AI_EFFORT_XHIGH, AI_EFFORT_MAX,
};

G_DEFINE_FINAL_TYPE (HyChatView, hy_chat_view, ADW_TYPE_BIN)

static void send_current_message (HyChatView *self);
static void send_queued (HyChatView *self);
static void send_message (HyChatView *self,
                          const char *text);
static void update_send_button (HyChatView *self);
static void start_turn (HyChatView *self,
                        const char *prompt);
static void on_model_chosen (HyModelPicker *picker,
                             const char    *backend_id,
                             const char    *model_id,
                             gpointer       user_data);
static void on_effort_selected (GtkDropDown *chooser,
                                GParamSpec  *pspec,
                                gpointer     user_data);
static void on_access_selected (GtkDropDown *chooser,
                                GParamSpec  *pspec,
                                gpointer     user_data);
static void on_plan_toggled (GtkToggleButton *toggle,
                             gpointer         user_data);
static HyMessageRow *append_row (HyChatView    *self,
                                 HyMessageKind  kind,
                                 const char    *text);
static const char *workdir_for (const HyChat              *chat,
                                const HyEffectiveSettings *resolved);

/* A chat with nothing stored runs at whatever the CLI is configured to use. */
static AiEffort
effort_for (const HyChat *chat)
{
  const AiBackend *backend = ai_backend_lookup (chat->backend);

  if (chat->effort != NULL)
    return ai_effort_from_string (chat->effort);

  return backend != NULL ? ai_backend_default_effort (backend) : AI_EFFORT_HIGH;
}

/* "Claude Opus 5 · High" rather than "Assistant": which model answered, and
 * how hard it was asked to think, are the two things worth knowing. */
static char *
reply_title (const HyChat *chat)
{
  const AiBackend *backend = ai_backend_lookup (chat->backend);

  if (backend == NULL)
    return g_strdup ("Assistant");

  return g_strdup_printf ("%s · %s",
                          ai_backend_model_label (backend, chat->model),
                          ai_effort_label (effort_for (chat)));
}

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

/*
 * Tool calls collapse into one row.
 *
 * A turn can make a dozen of them, and expanded they push the reply off the
 * screen. Consecutive calls join the group that is already open; anything
 * else closes it, so the grouping follows the shape of the turn.
 */
static void
append_tool_line (HyChatView *self,
                  const char *summary)
{
  GtkWidget *last = gtk_widget_get_last_child (GTK_WIDGET (self->transcript));
  GtkWidget *lines;
  GtkWidget *line;
  GtkWidget *expander;
  g_autofree char *title = NULL;
  int count;

  if (last != NULL && GTK_IS_EXPANDER (last))
    {
      expander = last;
    }
  else
    {
      expander = gtk_expander_new (NULL);
      gtk_expander_set_expanded (GTK_EXPANDER (expander), FALSE);
      gtk_widget_add_css_class (expander, "dim-label");
      gtk_widget_set_margin_top (expander, 4);
      gtk_widget_set_margin_bottom (expander, 4);
      gtk_widget_set_margin_start (expander, 24);
      gtk_widget_set_margin_end (expander, 24);

      lines = gtk_box_new (GTK_ORIENTATION_VERTICAL, 2);
      gtk_widget_set_margin_top (lines, 4);
      gtk_widget_set_margin_start (lines, 12);
      gtk_expander_set_child (GTK_EXPANDER (expander), lines);

      g_object_set_data (G_OBJECT (expander), "count", GINT_TO_POINTER (0));
      gtk_box_append (self->transcript, expander);
    }

  lines = gtk_expander_get_child (GTK_EXPANDER (expander));

  line = gtk_label_new (summary);
  gtk_label_set_xalign (GTK_LABEL (line), 0.0f);
  gtk_label_set_ellipsize (GTK_LABEL (line), PANGO_ELLIPSIZE_MIDDLE);
  gtk_label_set_selectable (GTK_LABEL (line), TRUE);
  gtk_widget_add_css_class (line, "caption");
  gtk_box_append (GTK_BOX (lines), line);

  count = GPOINTER_TO_INT (g_object_get_data (G_OBJECT (expander), "count")) + 1;
  g_object_set_data (G_OBJECT (expander), "count", GINT_TO_POINTER (count));

  title = count == 1 ? g_strdup ("1 tool call")
                     : g_strdup_printf ("%d tool calls", count);
  gtk_expander_set_label (GTK_EXPANDER (expander), title);

  queue_scroll_to_bottom (self);
}

/*
 * Retires every question on screen.
 *
 * Sending anything answers whatever was outstanding -- by button, by typing
 * something else, or by ignoring it and moving on. Once the conversation has
 * gone past a question, clicking one of its options would send an answer to
 * something nobody is asking any more.
 */
static void
retire_open_questions (HyChatView *self)
{
  for (GtkWidget *child = gtk_widget_get_first_child (GTK_WIDGET (self->transcript));
       child != NULL;
       child = gtk_widget_get_next_sibling (child))
    {
      GtkWidget *choices = g_object_get_data (G_OBJECT (child), "hy-choices");

      if (choices != NULL)
        gtk_widget_set_sensitive (choices, FALSE);
    }
}

static void
on_choice_clicked (GtkButton *button,
                   gpointer   user_data)
{
  HyChatView *self = user_data;
  const char *answer = g_object_get_data (G_OBJECT (button), "answer");

  if (answer == NULL || self->chat == NULL)
    return;

  send_message (self, answer);
}

/*
 * Renders a question the assistant asked as a row of buttons.
 *
 * Answering by clicking is the point, but the composer stays live: an option
 * the assistant did not think of is usually the interesting one.
 */
static void
append_choices (HyChatView  *self,
                const HyAsk *ask,
                gboolean     answerable)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 6);
  GtkWidget *question = gtk_label_new (ask->question);
  GtkWidget *choices = gtk_flow_box_new ();

  gtk_label_set_wrap (GTK_LABEL (question), TRUE);
  gtk_label_set_xalign (GTK_LABEL (question), 0.0f);
  gtk_widget_add_css_class (question, "heading");

  /* One per line: the options are sentences, so side by side they would each
   * be too narrow to read. */
  gtk_flow_box_set_selection_mode (GTK_FLOW_BOX (choices), GTK_SELECTION_NONE);
  gtk_flow_box_set_row_spacing (GTK_FLOW_BOX (choices), 4);
  gtk_flow_box_set_max_children_per_line (GTK_FLOW_BOX (choices), 1);
  gtk_flow_box_set_homogeneous (GTK_FLOW_BOX (choices), TRUE);

  for (gsize i = 0; ask->options[i] != NULL; i++)
    {
      GtkWidget *button = gtk_button_new_with_label (ask->options[i]);

      gtk_label_set_wrap (GTK_LABEL (gtk_button_get_child (GTK_BUTTON (button))), TRUE);
      g_object_set_data_full (G_OBJECT (button), "answer",
                              g_strdup (ask->options[i]), g_free);
      g_signal_connect (button, "clicked", G_CALLBACK (on_choice_clicked), self);

      /* No option is highlighted: which one is right is the user's call, and
       * colouring one of them is hy putting a thumb on the scale. */
      gtk_flow_box_append (GTK_FLOW_BOX (choices), button);
    }

  /* A question with something after it has already been answered. It stays
   * on screen as a record of what was offered, but it is not an offer. */
  gtk_widget_set_sensitive (choices, answerable);

  /* Tagged so sending anything can retire it, however it was answered. */
  g_object_set_data (G_OBJECT (box), "hy-choices", choices);

  gtk_box_append (GTK_BOX (box), question);
  gtk_box_append (GTK_BOX (box), choices);
  gtk_widget_set_margin_top (box, 4);
  gtk_widget_set_margin_start (box, 12);
  gtk_widget_set_margin_end (box, 12);

  gtk_box_append (self->transcript, box);
  queue_scroll_to_bottom (self);
}

/*
 * Assistant text may carry a question; the block itself is never shown.
 *
 * @answerable is false when the transcript continues past this message: the
 * question was answered when it was asked, and reopening the chat must not
 * offer it again.
 */
static void
append_reply (HyChatView *self,
              const char *text,
              const char *source,
              gboolean    answerable)
{
  g_autoptr (HyAsk) ask = NULL;
  g_autofree char *prose = NULL;
  HyMessageRow *row;

  ask = hy_ask_parse (text, &prose);

  row = append_row (self, HY_MESSAGE_ASSISTANT, ask != NULL ? prose : text);
  hy_message_row_set_source (row, source);

  if (ask != NULL)
    append_choices (self, ask, answerable);
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

      if (g_strcmp0 (message->role, "assistant") == 0)
        append_reply (self, message->content, message->label,
                      i + 1 == messages->len);
      else
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
  g_clear_pointer (&turn->label, g_free);
  g_clear_object (&turn->node);
  g_string_free (turn->text, TRUE);
  g_string_free (turn->segment, TRUE);
  g_clear_pointer (&turn->said, g_ptr_array_unref);
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
  g_string_append (turn->segment, delta);

  /*
   * The text is not shown as it arrives.
   *
   * A message half-written reflows on every token, and Markdown read
   * character by character renders as its own source until the syntax closes.
   * The row holds a spinner until the message is what it is going to be. The
   * previous one ended at a tool call, so this needs a row of its own --
   * below the tools, where it was said.
   */
  if (turn->row == NULL && turn_is_visible (turn))
    {
      turn->row = append_row (turn->view, HY_MESSAGE_ASSISTANT, NULL);
      hy_message_row_set_source (turn->row, turn->label);
      hy_message_row_set_waiting (turn->row, TRUE);
      queue_scroll_to_bottom (turn->view);
    }
}

/*
 * Ends the message being written, if there is one.
 *
 * An agent that works in steps says something, uses a tool, then says
 * something about what it found. Kept in one row, all of that text collapses
 * into a single paragraph sitting above every tool it ever called, in an
 * order that never happened. Each stretch of speech is its own message, so
 * the transcript reads the way the work went.
 */
static void
close_segment (Turn *turn)
{
  if (turn->segment->len == 0)
    return;

  /* Held rather than written: a message is worth storing once it is what it
   * is going to be, and the turn is still running. */
  g_ptr_array_add (turn->said, g_strdup (turn->segment->str));

  if (turn->row != NULL)
    {
      /* Show only what is safe to show: a question block becomes buttons when
       * the turn ends and must not appear as markup first. */
      gsize visible = hy_ask_visible_length (turn->segment->str);
      g_autofree char *prose = g_strndup (turn->segment->str, visible);

      g_strchomp (prose);
      hy_message_row_set_text (turn->row, prose);
      hy_message_row_set_waiting (turn->row, FALSE);
      queue_scroll_to_bottom (turn->view);
    }

  g_string_truncate (turn->segment, 0);
  turn->row = NULL;
}

/* Writes what the turn said, in the order it said it. */
static void
store_what_was_said (Turn *turn)
{
  for (guint i = 0; i < turn->said->len; i++)
    {
      g_autoptr (GError) error = NULL;

      if (!hy_storage_append_message (turn->view->storage, turn->chat_id,
                                      "assistant", g_ptr_array_index (turn->said, i),
                                      NULL, turn->label, &error))
        g_warning ("cannot store the reply: %s", error->message);
    }

  g_ptr_array_set_size (turn->said, 0);
}

static void
on_tool_use (HyChatSession *session,
             const char    *name,
             gpointer       user_data)
{
  Turn *turn = user_data;

  close_segment (turn);

  if (turn_is_visible (turn))
    append_tool_line (turn->view, name);
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

      /* start_turn() marks it working again; without this the row would sit
       * idle for as long as the retry takes. */
      hy_node_set_state (turn->node, HY_NODE_IDLE);

      g_hash_table_remove (self->turns, chat_id);

      if (visible)
        start_turn (self, retry_prompt);
      return;
    }


  if (turn->row != NULL)
    {
      g_autoptr (HyAsk) ask = hy_ask_parse (turn->segment->str, NULL);

      hy_message_row_set_waiting (turn->row, FALSE);

      /* Nothing came back at all: say so rather than leaving a blank card. */
      if (turn->text->len == 0 && success)
        hy_message_row_append (turn->row, "(no reply)");

      /* The block streamed in as text; re-render it as a question. */
      if (ask != NULL && visible)
        {
          g_autofree char *said = g_strdup (turn->segment->str);

          gtk_box_remove (self->transcript, GTK_WIDGET (turn->row));
          turn->row = NULL;
          append_reply (self, said, turn->label, TRUE);
        }
    }

  /*
   * A chat that asked something is waiting for the user, not finished. Read
   * before the segment is cleared, and from the text rather than from whether
   * buttons were drawn -- a chat the user is not looking at has no row to
   * draw them on, and is exactly the one worth marking.
   */
  {
    g_autoptr (HyAsk) asked = hy_ask_parse (turn->segment->str, NULL);

    hy_node_set_state (turn->node, asked != NULL ? HY_NODE_WAITING : HY_NODE_IDLE);
  }

  /* Whatever was still being written when the turn ended is a message like
   * any other. Only now, with all of them final, do they reach the database.
   */
  close_segment (turn);
  store_what_was_said (turn);

  /* This backend has now been told everything up to and including its own
   * reply, so the next turn only has to replay what comes after. Read after
   * the reply is stored, or it would not count. */
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
                                      NULL, NULL, &error))
        g_warning ("cannot store the error: %s", error->message);

      if (visible)
        append_row (self, HY_MESSAGE_ERROR, text);
    }

  /* An agent that edited anything has finished doing so. */
  hy_diff_pane_refresh (self->diff);
  hy_git_actions_refresh (self->git_actions);

  /* Frees the turn, so nothing may touch it afterwards. */
  g_hash_table_remove (self->turns, chat_id);

  if (visible)
    update_send_button (self);

  /* Whatever was typed while it was working is what to do next. */
  if (visible && self->queued != NULL)
    send_queued (self);
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
  g_autofree char *system_prompt = NULL;
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
  turn->node = g_object_ref (self->chat);

  hy_node_set_state (turn->node, HY_NODE_WORKING);
  turn->chat_id = g_strdup (chat->id);
  turn->backend_id = g_strdup (backend->id);
  turn->prompt = g_strdup (prompt);
  turn->resumed = resume_session_id != NULL;
  turn->text = g_string_new (NULL);
  turn->segment = g_string_new (NULL);
  turn->said = g_ptr_array_new_with_free_func (g_free);
  turn->session = hy_chat_session_new (backend);
  /* Taken now rather than when the reply lands: the model can be changed
   * while the agent is still working, and what answered is whatever was
   * running when the turn started. */
  turn->label = reply_title (chat);
  turn->row = append_row (self, HY_MESSAGE_ASSISTANT, NULL);
  hy_message_row_set_source (turn->row, turn->label);
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
  {
    g_autofree char *instructions =
      resolved->instructions != NULL
        ? g_strdup_printf ("%s\n\n%s", resolved->instructions, hy_ask_instructions ())
        : g_strdup (hy_ask_instructions ());

    spec.system_prompt = g_strdup (instructions);
    g_free (system_prompt);
    system_prompt = (char *) spec.system_prompt;
  }
  spec.resume_session_id = resume_session_id;
  spec.effort = effort_for (chat);
  /* Plan overrides the access level for as long as it is on, without
   * overwriting it. */
  spec.access = chat->plan ? AI_ACCESS_PLAN
                           : ai_access_from_string (chat->access);

  if (!hy_chat_session_start (turn->session, &spec, &error))
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      hy_storage_append_message (self->storage, chat->id, "error",
                                 error->message, NULL, NULL, NULL);
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
send_message (HyChatView *self,
              const char *text)
{
  g_autoptr (GError) error = NULL;

  if (self->chat == NULL || text == NULL || *text == '\0')
    return;

  /* One turn at a time per chat; the button is a stop button meanwhile. */
  if (current_turn (self) != NULL)
    return;

  if (!hy_storage_append_message (self->storage, hy_node_get_chat_id (self->chat),
                                  "user", text, NULL, NULL, &error))
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      return;
    }

  retire_open_questions (self);
  hy_node_set_state (self->chat, HY_NODE_IDLE);
  append_row (self, HY_MESSAGE_USER, text);
  name_chat_after_first_message (self, text);
  hy_fs_tree_bump_chat (self->tree, self->chat);

  start_turn (self, text);
}

/* --- pasted images --------------------------------------------------------- */

/* Big enough to recognise a screenshot, small enough to leave the composer
 * where it was. */
#define PREVIEW_HEIGHT 96
#define PREVIEW_MAX_WIDTH 168

static void
forget_attachments (HyChatView *self)
{
  g_ptr_array_set_size (self->attachments, 0);

  gtk_widget_set_visible (self->attachments_bar, FALSE);

  for (GtkWidget *child = gtk_widget_get_first_child (self->attachments_bar);
       child != NULL;
       child = gtk_widget_get_first_child (self->attachments_bar))
    gtk_box_remove (GTK_BOX (self->attachments_bar), child);
}

static void
on_attachment_removed (GtkButton *button,
                       gpointer   user_data)
{
  HyChatView *self = user_data;
  GtkWidget *chip = gtk_widget_get_ancestor (GTK_WIDGET (button), GTK_TYPE_OVERLAY);
  const char *path = g_object_get_data (G_OBJECT (chip), "path");

  for (guint i = 0; i < self->attachments->len; i++)
    {
      if (g_strcmp0 (g_ptr_array_index (self->attachments, i), path) == 0)
        {
          g_ptr_array_remove_index (self->attachments, i);
          break;
        }
    }

  gtk_box_remove (GTK_BOX (self->attachments_bar), chip);

  if (self->attachments->len == 0)
    gtk_widget_set_visible (self->attachments_bar, FALSE);
}

/*
 * Shows a pasted image before it is sent.
 *
 * A thumbnail rather than the filename: the whole point of pasting a
 * screenshot is that it is quicker than describing it, and a path says
 * nothing about whether the right thing was captured.
 */
static void
add_attachment (HyChatView *self,
                GdkTexture *texture)
{
  g_autofree char *directory = g_build_filename (g_get_user_cache_dir (), "hy",
                                                 "pasted", NULL);
  g_autofree char *name = NULL;
  g_autofree char *path = NULL;
  g_autoptr (GError) error = NULL;
  GtkWidget *chip;
  GtkWidget *picture;
  GtkWidget *remove;

  if (g_mkdir_with_parents (directory, 0700) != 0)
    {
      append_row (self, HY_MESSAGE_ERROR, "Cannot write the pasted image.");
      return;
    }

  /* The CLIs read the image off disk, so it needs a real file with a name
   * that will not collide with the next paste. */
  name = g_strdup_printf ("paste-%" G_GINT64_FORMAT ".png", g_get_real_time ());
  path = g_build_filename (directory, name, NULL);

  if (!gdk_texture_save_to_png (texture, path))
    {
      append_row (self, HY_MESSAGE_ERROR, "Cannot write the pasted image.");
      return;
    }

  /*
   * A thumbnail, not the image scaled down by the widget.
   *
   * GtkPicture asks for the size of what it holds, and a size request is a
   * minimum rather than a cap, so a screenshot of the screen wants the height
   * of the screen. Shrinking the pixels themselves is the only thing that
   * actually makes it small. The file on disk stays full size -- that is what
   * gets read.
   */
  {
    /* Read back from the file just written, at the size it will be shown:
     * the loader does the scaling, and the full-size copy never has to be
     * held in memory a second time. */
    g_autoptr (GdkPixbuf) small =
      gdk_pixbuf_new_from_file_at_scale (path, PREVIEW_MAX_WIDTH, PREVIEW_HEIGHT,
                                         TRUE, NULL);
    g_autoptr (GdkTexture) thumbnail = NULL;

    if (small == NULL)
      return;

    thumbnail = gdk_texture_new_for_pixbuf (small);
    picture = gtk_picture_new_for_paintable (GDK_PAINTABLE (thumbnail));
  }

  gtk_widget_set_halign (picture, GTK_ALIGN_CENTER);
  gtk_widget_set_valign (picture, GTK_ALIGN_CENTER);

  remove = gtk_button_new_from_icon_name ("window-close-symbolic");
  gtk_widget_add_css_class (remove, "circular");
  gtk_widget_set_halign (remove, GTK_ALIGN_END);
  gtk_widget_set_valign (remove, GTK_ALIGN_START);
  gtk_widget_set_tooltip_text (remove, "Remove");
  g_signal_connect (remove, "clicked", G_CALLBACK (on_attachment_removed), self);

  /* A card with the name under it, so it reads as a file going with the
   * message rather than as part of what is being typed. */
  {
    GtkWidget *card = gtk_box_new (GTK_ORIENTATION_VERTICAL, 4);
    GtkWidget *label = gtk_label_new (name);

    gtk_label_set_ellipsize (GTK_LABEL (label), PANGO_ELLIPSIZE_MIDDLE);
    gtk_label_set_max_width_chars (GTK_LABEL (label), 18);
    gtk_widget_add_css_class (label, "caption");
    gtk_widget_add_css_class (label, "dim-label");

    gtk_box_append (GTK_BOX (card), picture);
    gtk_box_append (GTK_BOX (card), label);
    gtk_widget_add_css_class (card, "card");
    gtk_widget_set_margin_top (card, 6);
    gtk_widget_set_margin_bottom (card, 6);
    gtk_widget_set_margin_start (card, 6);
    gtk_widget_set_margin_end (card, 6);
    gtk_widget_set_halign (card, GTK_ALIGN_START);

    chip = gtk_overlay_new ();
    gtk_overlay_set_child (GTK_OVERLAY (chip), card);
    gtk_overlay_add_overlay (GTK_OVERLAY (chip), remove);
    gtk_widget_set_halign (chip, GTK_ALIGN_START);
    g_object_set_data_full (G_OBJECT (chip), "path", g_strdup (path), g_free);
  }

  gtk_box_append (GTK_BOX (self->attachments_bar), chip);
  gtk_widget_set_visible (self->attachments_bar, TRUE);

  g_ptr_array_add (self->attachments, g_steal_pointer (&path));
}

static void
on_texture_pasted (GObject      *source,
                   GAsyncResult *result,
                   gpointer      user_data)
{
  HyChatView *self = user_data;
  g_autoptr (GdkTexture) texture = NULL;
  g_autoptr (GError) error = NULL;

  texture = gdk_clipboard_read_texture_finish (GDK_CLIPBOARD (source), result, &error);
  if (texture == NULL)
    {
      g_debug ("nothing pasteable in the clipboard: %s",
               error != NULL ? error->message : "no image");
      return;
    }

  add_attachment (self, texture);
}

/* True when the clipboard holds an image, which is pasted as one. */
static gboolean
paste_image (HyChatView *self)
{
  GdkClipboard *clipboard = gtk_widget_get_clipboard (GTK_WIDGET (self));
  GdkContentFormats *formats = gdk_clipboard_get_formats (clipboard);

  if (!gdk_content_formats_contain_gtype (formats, GDK_TYPE_TEXTURE))
    return FALSE;

  gdk_clipboard_read_texture_async (clipboard, NULL, on_texture_pasted, self);

  return TRUE;
}

/* --- messages typed while the agent is working ------------------------------ */

static void
show_queued (HyChatView *self)
{
  gtk_widget_set_visible (self->queued_bar, self->queued != NULL);

  if (self->queued != NULL)
    gtk_label_set_label (self->queued_label, self->queued);
}

static void
queue_message (HyChatView *self,
               const char *text)
{
  /* A second message replaces the first rather than piling up: what is meant
   * is the latest instruction, not a list of them to be answered in turn. */
  g_free (self->queued);
  self->queued = g_strdup (text);

  show_queued (self);
}

/*
 * Sends what is queued as soon as it can be sent.
 *
 * Called when a turn ends. The queued text goes through the normal path, so
 * it is stored, shown and answered like anything else.
 */
static void
send_queued (HyChatView *self)
{
  g_autofree char *text = g_steal_pointer (&self->queued);

  show_queued (self);

  if (text != NULL)
    send_message (self, text);
}

/*
 * Interrupts the turn to say this now.
 *
 * Neither CLI takes input mid-turn -- they are given a prompt and run to
 * completion -- so steering means stopping the turn and starting another.
 * What was said up to the stop is kept, and the next turn replays it, so the
 * agent knows what it had got to before being redirected.
 */
static void
on_steer_clicked (GtkButton *button,
                  gpointer   user_data)
{
  HyChatView *self = user_data;
  Turn *turn = current_turn (self);

  if (turn != NULL)
    hy_chat_session_cancel (turn->session);
  /* The queued text goes out when the turn reports that it has stopped. */
}

static void
on_queued_dropped (GtkButton *button,
                   gpointer   user_data)
{
  HyChatView *self = user_data;

  g_clear_pointer (&self->queued, g_free);
  show_queued (self);
}

static void
send_current_message (HyChatView *self)
{
  g_autofree char *text = take_composer_text (self);
  g_autoptr (GString) message = NULL;

  if (self->attachments->len == 0)
    {
      if (text == NULL)
        return;

      /* One turn at a time, so anything typed meanwhile waits for it. */
      if (current_turn (self) != NULL)
        queue_message (self, text);
      else
        send_message (self, text);

      return;
    }

  /*
   * Images travel as paths.
   *
   * Neither CLI takes an image on the command line, but both can read a file
   * they are told about, so the message names them. They are written where
   * they will still be there afterwards, since the transcript refers to them.
   */
  message = g_string_new (text != NULL ? text : "");

  for (guint i = 0; i < self->attachments->len; i++)
    {
      if (message->len > 0)
        g_string_append (message, "\n");

      g_string_append_printf (message, "[image: %s]",
                              (const char *) g_ptr_array_index (self->attachments, i));
    }

  forget_attachments (self);

  if (current_turn (self) != NULL)
    queue_message (self, message->str);
  else
    send_message (self, message->str);
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

  /* The remote is not shown: it is the longest thing in the bar and the least
   * likely to be in question, and it crowded out the branch, which changes. */

  return g_string_free (g_steal_pointer (&text), FALSE);
}

/* Used only the first time the panel is opened, before there is a remembered
 * height: enough for a build log, little enough that the conversation is
 * still the thing on screen. */
#define TERMINAL_DEFAULT_HEIGHT 260

typedef struct
{
  GtkPaned *paned;
  int size;                 /* how big the end child should be */
  gboolean vertical;
} PendingSplit;

/*
 * Puts the divider where it leaves the end child @size big.
 *
 * A window that has not been laid out yet has nothing to divide, which is
 * exactly the case when a chat is opened at startup -- so instead of giving
 * up, it waits for the first frame that has a size. Getting this wrong is
 * silent: the pane simply comes back the default size, as though nothing had
 * ever been remembered.
 */
static gboolean
apply_split (GtkWidget     *widget,
             GdkFrameClock *clock,
             gpointer       user_data)
{
  PendingSplit *pending = user_data;
  int available = pending->vertical ? gtk_widget_get_height (GTK_WIDGET (pending->paned))
                                    : gtk_widget_get_width (GTK_WIDGET (pending->paned));

  if (available <= 0)
    return G_SOURCE_CONTINUE;

  gtk_paned_set_position (pending->paned, MAX (available - pending->size, 0));

  return G_SOURCE_REMOVE;
}

static void
set_end_child_size (GtkPaned *paned,
                    int       size,
                    gboolean  vertical)
{
  PendingSplit *pending;

  if (size <= 0)
    return;

  pending = g_new0 (PendingSplit, 1);
  pending->paned = paned;
  pending->size = size;
  pending->vertical = vertical;

  gtk_widget_add_tick_callback (GTK_WIDGET (paned), apply_split, pending, g_free);
}

static void
set_terminal_height (HyChatView *self,
                     int         height)
{
  set_end_child_size (self->split, height, TRUE);
}

/*
 * Writes which panes this chat is working with.
 *
 * Skipped while the toggles are being set to match a chat being opened,
 * which would otherwise write the chat's own state back over itself -- and,
 * worse, write it against whichever chat was open a moment ago.
 */
static void
remember_panes (HyChatView *self)
{
  g_autoptr (GError) error = NULL;

  if (self->syncing_panes || self->chat == NULL)
    return;

  if (!hy_storage_set_panes (self->storage, hy_node_get_chat_id (self->chat),
                             gtk_toggle_button_get_active (self->terminal_button),
                             gtk_toggle_button_get_active (self->diff_button),
                             &error))
    g_warning ("cannot remember the open panes: %s", error->message);
}

/*
 * Shows or hides what the working tree looks like now.
 *
 * Read when opened rather than watched: the answer only changes when
 * something writes to the repository, which here means an agent finishing a
 * turn or the user running something in the terminal.
 */
/* The divider is where the user put it, so it is written when it moves
 * rather than only when a pane is closed -- a window shut with both open
 * would otherwise lose both sizes. */
static void
on_terminal_dragged (GtkPaned   *paned,
                     GParamSpec *pspec,
                     gpointer    user_data)
{
  HyChatView *self = user_data;
  int height = gtk_widget_get_height (GTK_WIDGET (self->terminal));

  if (height > 0 && gtk_widget_get_visible (GTK_WIDGET (self->terminal)))
    g_settings_set_int (self->settings, "terminal-height", height);
}

static void
on_diff_dragged (GtkPaned   *paned,
                 GParamSpec *pspec,
                 gpointer    user_data)
{
  HyChatView *self = user_data;
  int width = gtk_widget_get_width (GTK_WIDGET (self->diff));

  if (width > 0 && gtk_widget_get_visible (GTK_WIDGET (self->diff)))
    g_settings_set_int (self->settings, "diff-width", width);
}

static void
on_diff_toggled (GtkToggleButton *button,
                 gpointer         user_data)
{
  HyChatView *self = user_data;
  gboolean shown = gtk_toggle_button_get_active (button);

  remember_panes (self);

  if (!shown)
    {
      int width = gtk_widget_get_width (GTK_WIDGET (self->diff));

      if (width > 0)
        g_settings_set_int (self->settings, "diff-width", width);

      gtk_widget_set_visible (GTK_WIDGET (self->diff), FALSE);
      return;
    }

  gtk_widget_set_visible (GTK_WIDGET (self->diff), TRUE);

  set_end_child_size (self->side_split,
                      g_settings_get_int (self->settings, "diff-width"), FALSE);

  hy_diff_pane_refresh (self->diff);
}

static int
terminal_height (HyChatView *self)
{
  int height = gtk_widget_get_height (GTK_WIDGET (self->terminal));

  return height > 0 ? height : g_settings_get_int (self->settings, "terminal-height");
}

/*
 * Shows or hides the shell that runs where the agent does.
 *
 * The shell is only started the first time it is opened -- an embedded
 * terminal nobody looks at is a process and a pty for nothing -- and is left
 * running when the panel is hidden, so closing it does not throw away what is
 * on screen or interrupt a command.
 */
static void
on_terminal_toggled (GtkToggleButton *button,
                     gpointer         user_data)
{
  HyChatView *self = user_data;
  gboolean shown = gtk_toggle_button_get_active (button);

  remember_panes (self);

  if (!shown)
    {
      /* Kept before the panel loses its allocation and reports zero. */
      g_settings_set_int (self->settings, "terminal-height", terminal_height (self));
      gtk_widget_set_visible (GTK_WIDGET (self->terminal), FALSE);
      return;
    }

  gtk_widget_set_visible (GTK_WIDGET (self->terminal), TRUE);
  set_terminal_height (self, g_settings_get_int (self->settings, "terminal-height"));

  /* Only take the keyboard when the user asked for the panel, not when it is
   * being reopened because a chat had it open. */
  if (self->syncing_panes)
    hy_terminal_panel_start (self->terminal);
  else
    hy_terminal_panel_activate (self->terminal);
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

  {
    const char *workdir = workdir_for (chat, resolved);
    gboolean have_workdir = workdir != NULL && *workdir != '\0';
    g_autofree char *tooltip =
      have_workdir ? g_strdup_printf ("Terminal in %s", workdir) : NULL;

    hy_terminal_panel_set_workdir (self->terminal, have_workdir ? workdir : NULL);
    hy_diff_pane_set_workdir (self->diff, have_workdir ? workdir : NULL);
    hy_git_actions_set_workdir (self->git_actions, have_workdir ? workdir : NULL);

    /* The panes belong to the chat, so opening one brings them back. */
    self->syncing_panes = TRUE;
    gtk_toggle_button_set_active (self->terminal_button, chat->terminal_open);
    gtk_toggle_button_set_active (self->diff_button, chat->diff_open);
    self->syncing_panes = FALSE;

    gtk_widget_set_sensitive (GTK_WIDGET (self->terminal_button), have_workdir);
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->terminal_button),
                                 have_workdir ? tooltip : "This chat has no working directory");
  }

  hy_model_picker_set_selected (self->model_picker, chat->backend,
                                chat->model != NULL ? chat->model : resolved->model);

  self->syncing_run_options = TRUE;

  for (guint i = 0; i < G_N_ELEMENTS (effort_choices); i++)
    {
      if (effort_choices[i] == effort_for (chat))
        gtk_drop_down_set_selected (self->effort_chooser, i);
    }

  for (guint i = 0; i < G_N_ELEMENTS (access_choices); i++)
    {
      if (access_choices[i] == ai_access_from_string (chat->access))
        gtk_drop_down_set_selected (self->access_chooser, i);
    }

  gtk_toggle_button_set_active (chat->plan ? self->plan_toggle : self->build_toggle,
                                TRUE);

  /* Planning changes nothing, so how much it is allowed to change is moot. */
  gtk_widget_set_sensitive (GTK_WIDGET (self->access_chooser), !chat->plan);

  self->syncing_run_options = FALSE;
}

static void
on_plan_toggled (GtkToggleButton *toggle,
                 gpointer         user_data)
{
  HyChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  gboolean plan = gtk_toggle_button_get_active (self->plan_toggle);

  if (self->syncing_run_options || self->chat == NULL)
    return;

  if (!hy_storage_set_plan (self->storage, hy_node_get_chat_id (self->chat),
                            plan, &error))
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      return;
    }

  gtk_widget_set_sensitive (GTK_WIDGET (self->access_chooser), !plan);
}

static void
on_effort_selected (GtkDropDown *chooser,
                    GParamSpec  *pspec,
                    gpointer     user_data)
{
  HyChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  guint selected = gtk_drop_down_get_selected (chooser);

  if (self->syncing_run_options || self->chat == NULL ||
      selected >= G_N_ELEMENTS (effort_choices))
    return;

  if (!hy_storage_set_effort (self->storage, hy_node_get_chat_id (self->chat),
                              ai_effort_to_string (effort_choices[selected]),
                              &error))
    append_row (self, HY_MESSAGE_ERROR, error->message);
}

static void
on_access_selected (GtkDropDown *chooser,
                    GParamSpec  *pspec,
                    gpointer     user_data)
{
  HyChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  guint selected = gtk_drop_down_get_selected (chooser);

  if (self->syncing_run_options || self->chat == NULL ||
      selected >= G_N_ELEMENTS (access_choices))
    return;

  if (!hy_storage_set_access (self->storage, hy_node_get_chat_id (self->chat),
                              ai_access_to_string (access_choices[selected]),
                              &error))
    append_row (self, HY_MESSAGE_ERROR, error->message);
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

  /* The tree shows which assistant a chat belongs to, so it follows. */
  if (backend_changed)
    {
      const AiBackend *backend = ai_backend_lookup (backend_id);

      if (backend != NULL)
        hy_node_set_icon_name (self->chat, backend->icon_name);
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

  /* An image in the clipboard is pasted as an image; anything else falls
   * through to the text view's own handling. */
  if (keyval == GDK_KEY_v && (state & GDK_CONTROL_MASK) && paste_image (self))
    return GDK_EVENT_STOP;

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
      turn->row = append_row (self, HY_MESSAGE_ASSISTANT, turn->segment->str);
      hy_message_row_set_source (turn->row, turn->label);
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
  gtk_text_view_set_top_margin (self->composer, 10);
  gtk_text_view_set_bottom_margin (self->composer, 10);
  gtk_text_view_set_left_margin (self->composer, 10);
  gtk_text_view_set_right_margin (self->composer, 10);

  keys = gtk_event_controller_key_new ();
  g_signal_connect (keys, "key-pressed", G_CALLBACK (on_composer_key), self);
  gtk_widget_add_controller (GTK_WIDGET (self->composer), keys);

  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scroller),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_scrolled_window_set_max_content_height (GTK_SCROLLED_WINDOW (scroller), 180);
  gtk_scrolled_window_set_propagate_natural_height (GTK_SCROLLED_WINDOW (scroller), TRUE);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scroller),
                                 GTK_WIDGET (self->composer));

  /*
   * What is waiting to be sent, with a way to send it now.
   *
   * Queued rather than refused: a message typed while the agent is working is
   * still what the user wants next, and making them retype it after the turn
   * ends wastes the time they spent typing it.
   */
  {
    GtkWidget *icon = gtk_image_new_from_icon_name ("document-send-symbolic");
    GtkWidget *steer = gtk_button_new_from_icon_name ("media-skip-forward-symbolic");
    GtkWidget *drop = gtk_button_new_from_icon_name ("window-close-symbolic");

    self->queued_label = GTK_LABEL (gtk_label_new (NULL));
    gtk_label_set_ellipsize (self->queued_label, PANGO_ELLIPSIZE_END);
    gtk_label_set_xalign (self->queued_label, 0.0f);
    gtk_widget_set_hexpand (GTK_WIDGET (self->queued_label), TRUE);
    gtk_widget_add_css_class (GTK_WIDGET (self->queued_label), "dim-label");

    gtk_widget_add_css_class (steer, "flat");
    gtk_widget_set_tooltip_text (steer, "Send now, interrupting the agent");
    g_signal_connect (steer, "clicked", G_CALLBACK (on_steer_clicked), self);

    gtk_widget_add_css_class (drop, "flat");
    gtk_widget_set_tooltip_text (drop, "Discard");
    g_signal_connect (drop, "clicked", G_CALLBACK (on_queued_dropped), self);

    self->queued_bar = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
    gtk_box_append (GTK_BOX (self->queued_bar), icon);
    gtk_box_append (GTK_BOX (self->queued_bar), GTK_WIDGET (self->queued_label));
    gtk_box_append (GTK_BOX (self->queued_bar), steer);
    gtk_box_append (GTK_BOX (self->queued_bar), drop);
    gtk_widget_set_visible (self->queued_bar, FALSE);
    gtk_widget_set_margin_top (self->queued_bar, 6);
    gtk_widget_set_margin_start (self->queued_bar, 10);
    gtk_widget_set_margin_end (self->queued_bar, 6);
  }

  self->attachments_bar = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  gtk_widget_set_visible (self->attachments_bar, FALSE);
  gtk_widget_set_margin_top (self->attachments_bar, 8);
  gtk_widget_set_margin_start (self->attachments_bar, 10);
  gtk_widget_set_margin_end (self->attachments_bar, 10);

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

  {
    GtkStringList *efforts = gtk_string_list_new (NULL);
    GtkStringList *accesses = gtk_string_list_new (NULL);

    for (guint i = 0; i < G_N_ELEMENTS (effort_choices); i++)
      gtk_string_list_append (efforts, ai_effort_label (effort_choices[i]));

    for (guint i = 0; i < G_N_ELEMENTS (access_choices); i++)
      gtk_string_list_append (accesses, ai_access_label (access_choices[i]));

    self->effort_chooser = GTK_DROP_DOWN (gtk_drop_down_new (G_LIST_MODEL (efforts), NULL));
    gtk_widget_add_css_class (GTK_WIDGET (self->effort_chooser), "flat");
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->effort_chooser),
                                 "How hard the model is asked to think");
    g_signal_connect (self->effort_chooser, "notify::selected",
                      G_CALLBACK (on_effort_selected), self);

    self->access_chooser = GTK_DROP_DOWN (gtk_drop_down_new (G_LIST_MODEL (accesses), NULL));
    gtk_widget_add_css_class (GTK_WIDGET (self->access_chooser), "flat");
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->access_chooser),
                                 "What the assistant may do in the working "
                                 "directory");
    g_signal_connect (self->access_chooser, "notify::selected",
                      G_CALLBACK (on_access_selected), self);
  }

  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->model_picker));
  gtk_box_append (GTK_BOX (toolbar), gtk_separator_new (GTK_ORIENTATION_VERTICAL));
  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->effort_chooser));
  gtk_box_append (GTK_BOX (toolbar), gtk_separator_new (GTK_ORIENTATION_VERTICAL));
  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->access_chooser));
  gtk_box_append (GTK_BOX (toolbar), gtk_separator_new (GTK_ORIENTATION_VERTICAL));

  /* Build and Plan are two states of one choice, so they read as one control
   * rather than two independent buttons. */
  {
    GtkWidget *modes = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);

    self->build_toggle = GTK_TOGGLE_BUTTON (gtk_toggle_button_new ());
    gtk_button_set_child (GTK_BUTTON (self->build_toggle),
                          adw_button_content_new ());
    adw_button_content_set_icon_name (
      ADW_BUTTON_CONTENT (gtk_button_get_child (GTK_BUTTON (self->build_toggle))),
      "package-x-generic-symbolic");
    adw_button_content_set_label (
      ADW_BUTTON_CONTENT (gtk_button_get_child (GTK_BUTTON (self->build_toggle))),
      "Build");
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->build_toggle),
                                 "Carry the work out");

    self->plan_toggle = GTK_TOGGLE_BUTTON (gtk_toggle_button_new ());
    gtk_button_set_child (GTK_BUTTON (self->plan_toggle),
                          adw_button_content_new ());
    adw_button_content_set_icon_name (
      ADW_BUTTON_CONTENT (gtk_button_get_child (GTK_BUTTON (self->plan_toggle))),
      "view-list-bullet-symbolic");
    adw_button_content_set_label (
      ADW_BUTTON_CONTENT (gtk_button_get_child (GTK_BUTTON (self->plan_toggle))),
      "Plan");
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->plan_toggle),
                                 "Work out an approach without changing anything");

    gtk_toggle_button_set_group (self->plan_toggle, self->build_toggle);
    gtk_toggle_button_set_active (self->build_toggle, TRUE);
    g_signal_connect (self->plan_toggle, "toggled",
                      G_CALLBACK (on_plan_toggled), self);

    gtk_box_append (GTK_BOX (modes), GTK_WIDGET (self->build_toggle));
    gtk_box_append (GTK_BOX (modes), GTK_WIDGET (self->plan_toggle));
    gtk_widget_add_css_class (modes, "linked");
    gtk_box_append (GTK_BOX (toolbar), modes);

    gtk_box_append (GTK_BOX (toolbar), gtk_separator_new (GTK_ORIENTATION_VERTICAL));
  }
  self->terminal_button = GTK_TOGGLE_BUTTON (gtk_toggle_button_new ());
  gtk_button_set_icon_name (GTK_BUTTON (self->terminal_button), "utilities-terminal-symbolic");
  gtk_widget_add_css_class (GTK_WIDGET (self->terminal_button), "flat");
  g_signal_connect (self->terminal_button, "toggled",
                    G_CALLBACK (on_terminal_toggled), self);

  self->diff_button = GTK_TOGGLE_BUTTON (gtk_toggle_button_new ());
  gtk_button_set_icon_name (GTK_BUTTON (self->diff_button), "view-list-ordered-symbolic");
  gtk_widget_add_css_class (GTK_WIDGET (self->diff_button), "flat");
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->diff_button), "Changed files");
  g_signal_connect (self->diff_button, "toggled",
                    G_CALLBACK (on_diff_toggled), self);

  self->git_actions = hy_git_actions_new ();

  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->context_label));
  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->git_actions));
  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->diff_button));
  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->terminal_button));
  gtk_box_append (GTK_BOX (toolbar), GTK_WIDGET (self->send_button));
  /* The controls sit under the text the user is typing, so they need enough
   * clearance not to read as part of it. */
  gtk_widget_set_margin_top (toolbar, 10);
  gtk_widget_set_margin_start (toolbar, 6);
  gtk_widget_set_margin_end (toolbar, 6);
  gtk_widget_set_margin_bottom (toolbar, 6);

  /* Above what is being typed, the way an attachment reads: this is going
   * with the message below it. */
  gtk_box_append (GTK_BOX (column), self->queued_bar);
  gtk_box_append (GTK_BOX (column), self->attachments_bar);
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
  g_clear_pointer (&self->attachments, g_ptr_array_unref);
  g_clear_pointer (&self->queued, g_free);
  g_clear_object (&self->settings);
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
  self->settings = g_settings_new (HY_APP_ID);
  self->attachments = g_ptr_array_new_with_free_func (g_free);

  self->title = ADW_WINDOW_TITLE (adw_window_title_new ("hy", NULL));
  adw_header_bar_set_title_widget (ADW_HEADER_BAR (header), GTK_WIDGET (self->title));

  /* The sidebar is the leftmost header bar, so whatever the desktop puts on
   * that side of the title bar is its to draw. */
  adw_header_bar_set_show_start_title_buttons (ADW_HEADER_BAR (header), FALSE);

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

  /* The terminal shares the window with the conversation rather than covering
   * it: the reason to open one is usually to check something the agent just
   * said, which means reading both at once. */
  self->terminal = hy_terminal_panel_new ();
  gtk_widget_set_visible (GTK_WIDGET (self->terminal), FALSE);

  self->split = GTK_PANED (gtk_paned_new (GTK_ORIENTATION_VERTICAL));
  g_signal_connect (self->split, "notify::position",
                    G_CALLBACK (on_terminal_dragged), self);
  gtk_paned_set_start_child (self->split, content);
  gtk_paned_set_resize_start_child (self->split, TRUE);
  gtk_paned_set_shrink_start_child (self->split, FALSE);
  gtk_paned_set_end_child (self->split, GTK_WIDGET (self->terminal));
  gtk_paned_set_resize_end_child (self->split, FALSE);
  gtk_paned_set_shrink_end_child (self->split, FALSE);

  /* The diff sits beside the conversation and the terminal together, since
   * it is about the repository rather than about either of them. */
  self->diff = hy_diff_pane_new ();
  gtk_widget_set_visible (GTK_WIDGET (self->diff), FALSE);

  self->side_split = GTK_PANED (gtk_paned_new (GTK_ORIENTATION_HORIZONTAL));
  g_signal_connect (self->side_split, "notify::position",
                    G_CALLBACK (on_diff_dragged), self);
  gtk_paned_set_start_child (self->side_split, GTK_WIDGET (self->split));
  gtk_paned_set_resize_start_child (self->side_split, TRUE);
  gtk_paned_set_shrink_start_child (self->side_split, FALSE);
  gtk_paned_set_end_child (self->side_split, GTK_WIDGET (self->diff));
  gtk_paned_set_resize_end_child (self->side_split, FALSE);
  gtk_paned_set_shrink_end_child (self->side_split, FALSE);

  adw_toolbar_view_set_content (ADW_TOOLBAR_VIEW (toolbar), GTK_WIDGET (self->side_split));


  adw_bin_set_child (ADW_BIN (self), toolbar);
}
