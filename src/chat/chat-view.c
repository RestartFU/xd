#include "chat-view.h"

#include "chat-session.h"
#include "chat-title.h"
#include "ui/dots.h"
#include "handover.h"
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
  XdChatView *view;         /* unowned; the view outlives its turns */
  XdChatSession *session;
  char *chat_id;
  char *backend_id;         /* the backend this turn's session id belongs to */
  char *prompt;             /* kept so a dead session can be retried */
  char *label;              /* the model and effort this turn actually ran on */
  gint64 started_at;        /* monotonic; how long the work took */
  GtkWidget *anchor;        /* weak: the row just above the turn's output */
  XdNode *node;             /* the row in the tree, so it can show the state */
  GString *text;            /* everything the turn has said, for the ask block */
  GString *segment;         /* what belongs in the row being written now */
  GPtrArray *said;          /* finished messages, held until the turn ends */
  XdMessageRow *row;        /* NULL until the segment has somewhere to go */
  gboolean resumed;
  gboolean is_retry;
} Turn;

/* Wide enough for code and a diff line, narrow enough that a line of prose
 * is one glance. */
#define CONTENT_WIDTH 860

struct _XdChatView
{
  AdwBin parent_instance;

  XdStorage *storage;
  XdFsTree *tree;
  /*
   * Held, not borrowed.
   *
   * The tree drops and rebuilds chat nodes whenever a folder is rescanned,
   * and the view goes on using this one until it is told otherwise -- so
   * borrowing it means reading freed memory the moment the two disagree.
   */
  XdNode *chat;

  /*
   * Set while the chat on screen belongs to a daemon.
   *
   * Everything that writes goes through the storage above, which knows nothing
   * about that chat -- so this doubles as the flag that says so, and the
   * transcript is read over the connection instead.
   */
  XdRemoteClient *remote;
  GCancellable *fetching;       /* the transcript request in flight, if any */

  /*
   * A turn running on the daemon, as far as this window can see it.
   *
   * The text arrives in pieces like a local one's, and is held the same way --
   * shown when the message is what it is going to be rather than reflowing on
   * every token. What is not held here is the truth: the daemon has written it
   * down by the time the turn ends, and the transcript is read again then.
   */
  /*
   * The dots at the foot of the transcript while a turn is running.
   *
   * On the transcript rather than on the row being written: a turn spends most
   * of its time between messages -- reading, running a command, deciding --
   * and a marker that lived on the row vanished for exactly those stretches,
   * which are the ones where "is it still going?" is the question.
   */
  GtkWidget *working_row;

  gboolean remote_working;
  XdMessageRow *remote_row;
  GString *remote_said;
  char *remote_label;

  /* The last message the transcript on screen was drawn from, so a write made
   * by something else can be told from one this window just made. */
  gint64 rendered_message_id;

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
  XdModelPicker *model_picker;
  GtkDropDown *effort_chooser;
  GtkDropDown *access_chooser;
  GtkToggleButton *build_toggle;
  GtkToggleButton *plan_toggle;
  GtkLabel *context_label;
  GtkToggleButton *terminal_button;
  XdTerminalPanel *terminal;
  GtkToggleButton *diff_button;
  XdGitActions *git_actions;
  XdDiffPane *diff;
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

/* What each option means, one line, shown under its name in the dropdown.
 * Same order as the choice arrays above. */
static const char *const access_descriptions[] = {
  "Look at anything, change nothing.",
  "Edit the working tree; ask before commands.",
  "Run commands and edit without asking.",
};
static const char *const effort_descriptions[] = {
  "Quick answers, little deliberation.",
  "Balanced speed and depth.",
  "Thinks longer before answering.",
  "Extended reasoning for hard problems.",
  "Everything the model has.",
};

/* --- two-line dropdown rows ------------------------------------------------ */

static void
on_option_setup (GtkSignalListItemFactory *factory,
                 GtkListItem              *item,
                 gpointer                  user_data)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 2);
  GtkWidget *title = gtk_label_new (NULL);
  GtkWidget *detail = gtk_label_new (NULL);

  gtk_label_set_xalign (GTK_LABEL (title), 0.0f);
  gtk_label_set_xalign (GTK_LABEL (detail), 0.0f);
  gtk_widget_add_css_class (detail, "caption");
  gtk_widget_add_css_class (detail, "dim-label");

  gtk_box_append (GTK_BOX (box), title);
  gtk_box_append (GTK_BOX (box), detail);
  gtk_list_item_set_child (item, box);
}

static void
on_option_bind (GtkSignalListItemFactory *factory,
                GtkListItem              *item,
                gpointer                  user_data)
{
  const char *const *descriptions = user_data;
  GtkWidget *box = gtk_list_item_get_child (item);
  GtkWidget *title = gtk_widget_get_first_child (box);
  GtkWidget *detail = gtk_widget_get_next_sibling (title);
  GtkStringObject *string = gtk_list_item_get_item (item);

  gtk_label_set_label (GTK_LABEL (title),
                       gtk_string_object_get_string (string));
  gtk_label_set_label (GTK_LABEL (detail),
                       descriptions[gtk_list_item_get_position (item)]);
}

/*
 * The open list explains each option; the closed button only names the one
 * chosen. A description worth a line in the menu would be clutter in the
 * composer bar.
 */
static void
add_option_descriptions (GtkDropDown       *chooser,
                         const char *const *descriptions)
{
  GtkListItemFactory *factory = gtk_signal_list_item_factory_new ();

  g_signal_connect (factory, "setup", G_CALLBACK (on_option_setup), NULL);
  g_signal_connect (factory, "bind", G_CALLBACK (on_option_bind),
                    (gpointer) descriptions);

  gtk_drop_down_set_list_factory (chooser, factory);
  g_object_unref (factory);
}

G_DEFINE_FINAL_TYPE (XdChatView, xd_chat_view, ADW_TYPE_BIN)

static void send_current_message (XdChatView *self);
static void keep_working_last (XdChatView *self);
static void set_working (XdChatView *self, gboolean working);
static void on_storage_changed (XdStorage *storage, gpointer user_data);
static void load_remote_transcript (XdChatView *self);
static void load_remote_options (XdChatView *self);
static void append_tool_line (XdChatView *self, const char *name);
static char *describe_context (const char *workdir);
static void on_remote_sent (GObject *source, GAsyncResult *result, gpointer data);
static void show_queued (XdChatView *self);
static Turn *current_turn (XdChatView *self);
static void send_remote_message (XdChatView *self, const char *text);
static void send_queued (XdChatView *self);
static void send_message (XdChatView *self,
                          const char *text);
static void update_send_button (XdChatView *self);
static void start_turn (XdChatView *self,
                        const char *prompt);
static void on_model_chosen (XdModelPicker *picker,
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
static XdMessageRow *append_row (XdChatView    *self,
                                 XdMessageKind  kind,
                                 const char    *text);
static const char *workdir_for (const XdChat              *chat,
                                const XdEffectiveSettings *resolved);

/* A chat with nothing stored runs at whatever the CLI is configured to use. */
static AiEffort
effort_for (const XdChat *chat)
{
  const AiBackend *backend = ai_backend_lookup (chat->backend);

  if (chat->effort != NULL)
    return ai_effort_from_string (chat->effort);

  return backend != NULL ? ai_backend_default_effort (backend) : AI_EFFORT_HIGH;
}

/* "Claude Opus 5 · High" rather than "Assistant": which model answered, and
 * how hard it was asked to think, are the two things worth knowing. */
static char *
reply_title (const XdChat *chat)
{
  const AiBackend *backend = ai_backend_lookup (chat->backend);

  if (backend == NULL)
    return g_strdup ("Assistant");

  return g_strdup_printf ("%s · %s",
                          ai_backend_model_label (backend, chat->model),
                          ai_effort_label (effort_for (chat)));
}

/*
 * "Worked for 9m 31s", the way t3 puts it.
 *
 * Shown above the turn's output rather than after it, so a long stretch of
 * tools and replies reads as one unit of work with its cost at the top.
 */
static char *
format_worked_for (gint64 seconds)
{
  if (seconds >= 3600)
    return g_strdup_printf ("Worked for %dh %02dm", (int) (seconds / 3600),
                            (int) ((seconds % 3600) / 60));
  if (seconds >= 60)
    return g_strdup_printf ("Worked for %dm %02ds", (int) (seconds / 60),
                            (int) (seconds % 60));

  return g_strdup_printf ("Worked for %ds", (int) seconds);
}

static GtkWidget *
worked_for_row (gint64 seconds)
{
  g_autofree char *text = format_worked_for (seconds);
  GtkWidget *row = gtk_label_new (text);

  gtk_label_set_xalign (GTK_LABEL (row), 0.0f);
  gtk_widget_add_css_class (row, "caption");
  gtk_widget_add_css_class (row, "dim-label");
  gtk_widget_set_margin_start (row, 24);
  gtk_widget_set_margin_top (row, 6);

  return row;
}

/* --- transcript ----------------------------------------------------------- */

static gboolean
scroll_to_bottom (gpointer data)
{
  XdChatView *self = data;
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
queue_scroll_to_bottom (XdChatView *self)
{
  g_idle_add_full (G_PRIORITY_LOW, scroll_to_bottom, g_object_ref (self), NULL);
}

static XdMessageRow *
append_row (XdChatView    *self,
            XdMessageKind  kind,
            const char    *text)
{
  XdMessageRow *row = xd_message_row_new (kind, text);

  gtk_box_append (self->transcript, GTK_WIDGET (row));
  keep_working_last (self);
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
append_tool_line (XdChatView *self,
                  const char *summary)
{
  GtkWidget *last = gtk_widget_get_last_child (GTK_WIDGET (self->transcript));
  GtkWidget *lines;
  GtkWidget *line;
  GtkWidget *expander;
  g_autofree char *title = NULL;
  int count;

  /* Past the dots, which sit at the end while the turn runs. */
  if (last == self->working_row && last != NULL)
    last = gtk_widget_get_prev_sibling (last);

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

  keep_working_last (self);
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
retire_open_questions (XdChatView *self)
{
  GtkWidget *child = gtk_widget_get_first_child (GTK_WIDGET (self->transcript));

  while (child != NULL)
    {
      GtkWidget *next = gtk_widget_get_next_sibling (child);

      /* Taken away rather than greyed out. The answer is about to appear as a
       * message of its own, so a row of dead buttons above it would only be
       * saying what was on offer at the time. */
      if (g_object_get_data (G_OBJECT (child), "xd-choices") != NULL)
        gtk_box_remove (self->transcript, child);

      child = next;
    }
}

static void
on_choice_clicked (GtkButton *button,
                   gpointer   user_data)
{
  XdChatView *self = user_data;
  const char *answer = g_object_get_data (G_OBJECT (button), "answer");

  if (answer == NULL || self->chat == NULL)
    return;

  /* Before the buttons are torn down: when the focused button disappears,
   * focus falls to the first focusable widget -- a selectable message label,
   * which selects its whole text on focus-in. Parking focus on the composer
   * first means it never lands there. */
  gtk_widget_grab_focus (GTK_WIDGET (self->composer));

  send_message (self, answer);
}

/*
 * Renders a question the assistant asked as a row of buttons.
 *
 * Answering by clicking is the point, but the composer stays live: an option
 * the assistant did not think of is usually the interesting one.
 */
static void
append_choices (XdChatView  *self,
                const XdAsk *ask,
                gboolean     answerable)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 6);
  GtkWidget *choices = gtk_flow_box_new ();

  /* Only while it can still be answered. The question itself is part of the
   * reply and stays; these are a way of answering it, and a way of answering
   * something already answered is just something to explain. */
  if (!answerable)
    return;

  /* One per line: the options are sentences, so side by side they would each
   * be too narrow to read. */
  gtk_flow_box_set_selection_mode (GTK_FLOW_BOX (choices), GTK_SELECTION_NONE);
  gtk_flow_box_set_row_spacing (GTK_FLOW_BOX (choices), 4);
  gtk_flow_box_set_max_children_per_line (GTK_FLOW_BOX (choices), 1);
  gtk_flow_box_set_homogeneous (GTK_FLOW_BOX (choices), TRUE);

  for (gsize i = 0; ask->options[i] != NULL; i++)
    {
      GtkWidget *button = gtk_button_new_with_label (ask->options[i]);

      gtk_widget_add_css_class (button, "xd-choice");
      gtk_label_set_wrap (GTK_LABEL (gtk_button_get_child (GTK_BUTTON (button))), TRUE);
      g_object_set_data_full (G_OBJECT (button), "answer",
                              g_strdup (ask->options[i]), g_free);
      g_signal_connect (button, "clicked", G_CALLBACK (on_choice_clicked), self);

      /* No option is highlighted: which one is right is the user's call, and
       * colouring one of them is xd putting a thumb on the scale. */
      gtk_flow_box_append (GTK_FLOW_BOX (choices), button);
    }

  /* A question with something after it has already been answered. It stays
   * on screen as a record of what was offered, but it is not an offer. */
  gtk_widget_set_sensitive (choices, answerable);

  /* Tagged so sending anything takes it away, however it was answered. */
  g_object_set_data (G_OBJECT (box), "xd-choices", choices);

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
append_reply (XdChatView *self,
              const char *text,
              const char *source,
              gboolean    answerable)
{
  g_autoptr (XdAsk) ask = NULL;
  g_autofree char *prose = NULL;
  XdMessageRow *row;

  ask = xd_ask_parse (text, &prose);

  {
    g_autofree char *said = NULL;

    /* The question goes in the message rather than above the buttons, so the
     * transcript still reads as a conversation once they are gone. */
    if (ask != NULL)
      said = *prose != '\0' ? g_strdup_printf ("%s\n\n**%s**", prose, ask->question)
                            : g_strdup_printf ("**%s**", ask->question);

    row = append_row (self, XD_MESSAGE_ASSISTANT, said != NULL ? said : text);
    xd_message_row_set_source (row, source);
  }

  if (ask != NULL)
    append_choices (self, ask, answerable);
}

static void
clear_transcript (XdChatView *self)
{
  GtkWidget *child;

  /* It is about to be taken out with everything else. */
  self->working_row = NULL;

  while ((child = gtk_widget_get_first_child (GTK_WIDGET (self->transcript))) != NULL)
    gtk_box_remove (self->transcript, child);
}

/* Whatever was just added went in under it; the dots belong at the end. */
static void
keep_working_last (XdChatView *self)
{
  if (self->working_row == NULL)
    return;

  g_object_ref (self->working_row);
  gtk_box_remove (self->transcript, self->working_row);
  gtk_box_append (self->transcript, self->working_row);
  g_object_unref (self->working_row);
}

/*
 * Shows that the turn is still going, for as long as it is.
 *
 * One marker for the whole turn rather than one per message: what is being
 * waited for is the turn, and between two things it says there is nothing else
 * on screen that says it has not stopped.
 */
static void
set_working (XdChatView *self,
             gboolean    working)
{
  if (working == (self->working_row != NULL))
    return;

  if (!working)
    {
      gtk_box_remove (self->transcript, self->working_row);
      self->working_row = NULL;
      return;
    }

  self->working_row = GTK_WIDGET (xd_dots_new ());
  gtk_widget_add_css_class (self->working_row, "xd-dots-large");
  gtk_widget_set_halign (self->working_row, GTK_ALIGN_START);
  gtk_widget_set_margin_start (self->working_row, 24);
  gtk_widget_set_margin_top (self->working_row, 4);

  gtk_box_append (self->transcript, self->working_row);
}

/*
 * Draws a conversation, oldest first.
 *
 * Where the messages came from is not this function's business: the local
 * database and a daemon both hand over the same rows, and a remote chat reads
 * like a local one because it is drawn by the same code.
 */
static void
render_transcript (XdChatView *self,
                   GPtrArray  *messages)
{
  for (guint i = 0; i < messages->len; i++)
    {
      const XdMessage *message = g_ptr_array_index (messages, i);
      gboolean starts_run = g_strcmp0 (message->role, "assistant") == 0 &&
        (i == 0 || g_strcmp0 (((XdMessage *) g_ptr_array_index (messages, i - 1))->role,
                              "assistant") != 0);

      /* The work's cost goes above the work: replies are stamped when the
       * turn ends and the user's message when it was sent, so the span
       * between them is how long the agent worked. */
      if (starts_run && i > 0)
        {
          const XdMessage *before = g_ptr_array_index (messages, i - 1);
          const XdMessage *last = message;
          gint64 seconds;

          for (guint j = i; j < messages->len; j++)
            {
              const XdMessage *at = g_ptr_array_index (messages, j);

              if (g_strcmp0 (at->role, "assistant") != 0)
                break;
              last = at;
            }

          seconds = last->created_at - before->created_at;
          if (g_strcmp0 (before->role, "user") == 0 && seconds >= 1)
            gtk_box_append (self->transcript, worked_for_row (seconds));
        }

      if (g_strcmp0 (message->role, "assistant") == 0)
        append_reply (self, message->content, message->label,
                      i + 1 == messages->len);
      else
        append_row (self, xd_message_kind_from_role (message->role), message->content);
    }
}

static void
load_transcript (XdChatView *self)
{
  g_autoptr (GPtrArray) messages = NULL;
  g_autoptr (GError) error = NULL;

  clear_transcript (self);

  self->rendered_message_id =
    xd_storage_last_message_id (self->storage, xd_node_get_chat_id (self->chat));

  messages = xd_storage_list_messages (self->storage,
                                       xd_node_get_chat_id (self->chat), &error);
  if (messages == NULL)
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return;
    }

  render_transcript (self, messages);
}

/* --- transcripts from a daemon -------------------------------------------- */

static const char *
member_string (JsonObject *row,
               const char *name,
               const char *fallback)
{
  return json_object_get_string_member_with_default (row, name, fallback);
}

/* The daemon's messages, as the rows the transcript is drawn from. Only what
 * is drawn is read back: ids and raw events stay on the machine that owns
 * them. */
static GPtrArray *
messages_from_json (JsonArray *rows)
{
  GPtrArray *messages =
    g_ptr_array_new_with_free_func ((GDestroyNotify) xd_message_free);

  for (guint i = 0; rows != NULL && i < json_array_get_length (rows); i++)
    {
      JsonObject *row = json_array_get_object_element (rows, i);
      XdMessage *message = g_new0 (XdMessage, 1);

      message->role = g_strdup (member_string (row, "role", "assistant"));
      message->content = g_strdup (member_string (row, "content", ""));
      message->label = g_strdup (member_string (row, "label", NULL));
      message->created_at = json_object_get_int_member_with_default (row, "at", 0);

      g_ptr_array_add (messages, message);
    }

  return messages;
}

static void
on_remote_messages (GObject      *source,
                    GAsyncResult *result,
                    gpointer      user_data)
{
  g_autoptr (XdChatView) self = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autoptr (GError) error = NULL;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);

  /* Answered after the user moved on, which happens on every click through the
   * tree; the transcript on screen belongs to another chat now. */
  if (g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    return;

  if (reply == NULL)
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return;
    }

  messages = messages_from_json (json_object_has_member (reply, "messages")
                                 ? json_object_get_array_member (reply, "messages")
                                 : NULL);
  render_transcript (self, messages);

  queue_scroll_to_bottom (self);
}

/* --- a turn running on the daemon ------------------------------------------ */

/*
 * Ends the message being streamed, if there is one.
 *
 * The same rule as a local turn: text is shown when the message is finished
 * rather than as it arrives, because Markdown read character by character
 * renders as its own source until the syntax closes.
 */
static void
close_remote_segment (XdChatView *self)
{
  if (self->remote_row == NULL)
    return;

  if (self->remote_said != NULL && self->remote_said->len > 0)
    {
      gsize visible = xd_ask_visible_length (self->remote_said->str);
      g_autofree char *prose = g_strndup (self->remote_said->str, visible);

      g_strchomp (prose);
      xd_message_row_set_text (self->remote_row, prose);
    }

  xd_message_row_set_waiting (self->remote_row, FALSE);
  self->remote_row = NULL;

  if (self->remote_said != NULL)
    g_string_truncate (self->remote_said, 0);
}

static void
end_remote_turn (XdChatView *self)
{
  self->remote_working = FALSE;
  self->remote_row = NULL;
  g_clear_pointer (&self->remote_label, g_free);

  if (self->remote_said != NULL)
    g_string_truncate (self->remote_said, 0);
}

static void
on_remote_event (XdRemoteClient *client,
                 JsonObject     *event,
                 gpointer        user_data)
{
  XdChatView *self = user_data;
  const char *name = json_object_get_string_member_with_default (event, "event",
                                                                 NULL);
  const char *chat_id = json_object_get_string_member_with_default (event, "chat",
                                                                    NULL);
  const char *text = json_object_get_string_member_with_default (event, "text",
                                                                 NULL);

  if (self->chat == NULL || self->remote == NULL)
    return;

  /*
   * Events are broadcast to every device for every chat.
   *
   * One that names a chat is only interesting if it is this one. One that
   * names none -- the daemon noticing its own database was written to -- is
   * about everything, and dropping those was how a change made anywhere else
   * failed to reach an open transcript.
   */
  if (chat_id != NULL &&
      g_strcmp0 (chat_id, xd_node_get_chat_id (self->chat)) != 0)
    return;

  if (g_strcmp0 (name, "turn-started") == 0)
    {
      /* Started here or on another device -- there is no difference to draw. */
      self->remote_working = TRUE;
      g_free (self->remote_label);
      self->remote_label =
        g_strdup (json_object_get_string_member_with_default (event, "label", NULL));

      load_remote_transcript (self);
      set_working (self, TRUE);
      update_send_button (self);
      return;
    }

  if (g_strcmp0 (name, "text") == 0 && text != NULL)
    {
      if (self->remote_said == NULL)
        self->remote_said = g_string_new (NULL);

      g_string_append (self->remote_said, text);

      if (self->remote_row == NULL)
        {
          self->remote_row = append_row (self, XD_MESSAGE_ASSISTANT, NULL);
          xd_message_row_set_source (self->remote_row, self->remote_label);
          xd_message_row_set_waiting (self->remote_row, TRUE);
          queue_scroll_to_bottom (self);
        }

      return;
    }

  if (g_strcmp0 (name, "tool") == 0 && text != NULL)
    {
      close_remote_segment (self);
      append_tool_line (self, text);
      queue_scroll_to_bottom (self);
      return;
    }

  if (g_strcmp0 (name, "turn-finished") == 0)
    {
      end_remote_turn (self);
      set_working (self, FALSE);
      update_send_button (self);

      /* Read back rather than assembled from what arrived: the daemon has
       * written the turn down, and what it wrote is the transcript. */
      load_remote_transcript (self);

      /* Anything typed while it was working waits for exactly this, the way
       * the composer does for a chat running here. */
      if (self->queued != NULL)
        {
          g_autofree char *queued = g_steal_pointer (&self->queued);

          show_queued (self);
          send_remote_message (self, queued);
        }

      return;
    }

  /* Something changed about the chat itself -- edited here, on another device,
   * or in the window open on the daemon's own screen. */
  if (g_strcmp0 (name, "changed") == 0 && !self->remote_working)
    load_remote_transcript (self);
}

/*
 * The run options of a chat on a daemon.
 *
 * Read from over there rather than out of the database: which model answers,
 * how hard it thinks and what it is allowed to touch are the chat's, and the
 * chat is not here. Shown in the same row as a local chat's, because from the
 * composer there is nothing to tell apart.
 */
static void
update_remote_options (XdChatView   *self,
                       const XdChat *chat)
{
  g_autofree char *description = describe_context (chat->workdir);

  gtk_label_set_label (self->context_label, description);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->context_label), description);

  xd_model_picker_set_selected (self->model_picker, chat->backend, chat->model);

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
  gtk_widget_set_sensitive (GTK_WIDGET (self->access_chooser), !chat->plan);

  self->syncing_run_options = FALSE;
}

static void
on_remote_options_received (GObject      *source,
                            GAsyncResult *result,
                            gpointer      user_data)
{
  g_autoptr (XdChatView) self = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;
  XdChat chat = { 0 };

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (reply == NULL)
    return;

  /* Borrowed from the reply, which outlives this call: nothing here is kept. */
  chat.backend = (char *) member_string (reply, "backend", NULL);
  chat.model = (char *) member_string (reply, "model", NULL);
  chat.effort = (char *) member_string (reply, "effort", NULL);
  chat.access = (char *) member_string (reply, "access", NULL);
  chat.workdir = (char *) member_string (reply, "workdir", NULL);
  chat.plan = json_object_get_boolean_member_with_default (reply, "plan", FALSE);

  update_remote_options (self, &chat);

  /*
   * A turn already running when the chat was opened.
   *
   * Started before this window was looking, or on another device entirely.
   * What it has said so far is not in the database yet -- a turn is written
   * down when it ends -- so the daemon hands it over here, and this window
   * joins the reply already in progress rather than showing the message that
   * started it and nothing else.
   */
  if (json_object_get_boolean_member_with_default (reply, "working", FALSE))
    {
      const char *said = member_string (reply, "said", NULL);

      self->remote_working = TRUE;

      g_free (self->remote_label);
      self->remote_label = g_strdup (member_string (reply, "label", NULL));

      set_working (self, TRUE);

      if (said != NULL && *said != '\0')
        {
          if (self->remote_said == NULL)
            self->remote_said = g_string_new (NULL);

          g_string_assign (self->remote_said, said);

          if (self->remote_row == NULL)
            self->remote_row = append_row (self, XD_MESSAGE_ASSISTANT, NULL);

          xd_message_row_set_source (self->remote_row, self->remote_label);
          xd_message_row_set_text (self->remote_row, said);
          xd_message_row_set_waiting (self->remote_row, TRUE);
          queue_scroll_to_bottom (self);
        }

      update_send_button (self);
    }
}

static void
load_remote_options (XdChatView *self)
{
  xd_remote_client_call_op_async (self->remote, "chat", "chat",
                                  xd_node_get_chat_id (self->chat),
                                  self->fetching, on_remote_options_received,
                                  g_object_ref (self));
}

/* One of the run options, changed on the daemon rather than here. */
static void
set_remote_option (XdChatView *self,
                   const char *option,
                   const char *value)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "set-option");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, xd_node_get_chat_id (self->chat));
  json_builder_set_member_name (builder, "option");
  json_builder_add_string_value (builder, option);
  json_builder_set_member_name (builder, "value");
  json_builder_add_string_value (builder, value);
  json_builder_end_object (builder);

  request = json_builder_get_root (builder);

  xd_remote_client_call_async (self->remote, request, NULL, on_remote_sent,
                               g_object_ref (self));
}

/*
 * The line came back.
 *
 * Whatever the daemon said while it was down was said to nobody, so what is on
 * screen is as old as the disconnection. Reading it again is the catching up.
 */
static void
on_remote_opened (XdRemoteClient *client,
                  gpointer        user_data)
{
  XdChatView *self = user_data;

  if (self->chat == NULL || self->remote == NULL)
    return;

  end_remote_turn (self);
  load_remote_transcript (self);
  load_remote_options (self);
  update_send_button (self);
}

/* Connecting is what makes a turn on the daemon visible here: the events are
 * the same for every device watching. */
static void
set_remote (XdChatView     *self,
            XdRemoteClient *client)
{
  if (self->remote == client)
    return;

  if (self->remote != NULL)
    g_signal_handlers_disconnect_by_data (self->remote, self);

  g_set_object (&self->remote, client);

  if (client != NULL)
    {
      g_signal_connect (client, "event", G_CALLBACK (on_remote_event), self);
      g_signal_connect (client, "opened", G_CALLBACK (on_remote_opened), self);
    }
}

static void
load_remote_transcript (XdChatView *self)
{
  clear_transcript (self);

  g_cancellable_cancel (self->fetching);
  g_clear_object (&self->fetching);
  self->fetching = g_cancellable_new ();

  xd_remote_client_call_op_async (self->remote, "messages", "chat",
                                  xd_node_get_chat_id (self->chat),
                                  self->fetching, on_remote_messages,
                                  g_object_ref (self));
}

/*
 * Something wrote to the database that this window did not.
 *
 * The daemon running a turn for another device writes to the same file, and so
 * does a second window -- and neither of them can tell this one. What can is
 * the file itself, so the transcript is read again when what is in it has moved
 * past what is on screen.
 *
 * A turn running here is left alone: it is writing that file, and what it has
 * said is already on screen in the order it was said.
 */
static void
on_storage_changed (XdStorage *storage,
                    gpointer   user_data)
{
  XdChatView *self = user_data;
  const char *chat_id;

  if (self->chat == NULL || self->remote != NULL || current_turn (self) != NULL)
    return;

  chat_id = xd_node_get_chat_id (self->chat);

  if (xd_storage_last_message_id (self->storage, chat_id) ==
      self->rendered_message_id)
    return;

  load_transcript (self);
  queue_scroll_to_bottom (self);
}

/* --- turns ---------------------------------------------------------------- */

static Turn *
current_turn (XdChatView *self)
{
  if (self->chat == NULL)
    return NULL;

  return g_hash_table_lookup (self->turns, xd_node_get_chat_id (self->chat));
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
  if (turn->anchor != NULL)
    g_object_remove_weak_pointer (G_OBJECT (turn->anchor), (gpointer *) &turn->anchor);
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
         g_strcmp0 (xd_node_get_chat_id (turn->view->chat), turn->chat_id) == 0;
}

static void
on_session_started (XdChatSession *session,
                    const char    *session_id,
                    gpointer       user_data)
{
  Turn *turn = user_data;
  g_autoptr (GError) error = NULL;

  /* Stored immediately, and against the backend that issued it: if xd dies
   * mid-reply the conversation can still be resumed from where the CLI left
   * it, and switching assistants does not overwrite the other's session. */
  if (!xd_storage_set_session_id (turn->view->storage, turn->chat_id,
                                  turn->backend_id, session_id, &error))
    g_warning ("cannot store the session id: %s", error->message);
}

static void
on_text_delta (XdChatSession *session,
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
      turn->row = append_row (turn->view, XD_MESSAGE_ASSISTANT, NULL);
      xd_message_row_set_source (turn->row, turn->label);
      xd_message_row_set_waiting (turn->row, TRUE);
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
      gsize visible = xd_ask_visible_length (turn->segment->str);
      g_autofree char *prose = g_strndup (turn->segment->str, visible);

      g_strchomp (prose);
      xd_message_row_set_text (turn->row, prose);
      xd_message_row_set_waiting (turn->row, FALSE);
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

      if (!xd_storage_append_message (turn->view->storage, turn->chat_id,
                                      "assistant", g_ptr_array_index (turn->said, i),
                                      NULL, turn->label, &error))
        g_warning ("cannot store the reply: %s", error->message);
    }

  g_ptr_array_set_size (turn->said, 0);
}

static void
on_tool_use (XdChatSession *session,
             const char    *name,
             gpointer       user_data)
{
  Turn *turn = user_data;

  close_segment (turn);

  if (turn_is_visible (turn))
    append_tool_line (turn->view, name);
}

static void
on_turn_finished (XdChatSession *session,
                  gboolean       success,
                  const char    *message,
                  gpointer       user_data)
{
  Turn *turn = user_data;
  XdChatView *self = turn->view;
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

      if (!xd_storage_set_session_id (self->storage, chat_id, turn->backend_id,
                                      NULL, &error))
        g_warning ("cannot forget the stale session: %s", error->message);

      if (turn->row != NULL)
        gtk_widget_set_visible (GTK_WIDGET (turn->row), FALSE);

      /* start_turn() marks it working again; without this the row would sit
       * idle for as long as the retry takes. */
      xd_node_set_state (turn->node, XD_NODE_IDLE);

      g_hash_table_remove (self->turns, chat_id);

      if (visible)
        start_turn (self, retry_prompt);
      return;
    }


  if (turn->row != NULL)
    {
      g_autoptr (XdAsk) ask = xd_ask_parse (turn->segment->str, NULL);

      xd_message_row_set_waiting (turn->row, FALSE);

      /* Nothing came back at all: say so rather than leaving a blank card. */
      if (turn->text->len == 0 && success)
        xd_message_row_append (turn->row, "(no reply)");

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
    g_autoptr (XdAsk) asked = xd_ask_parse (turn->segment->str, NULL);

    xd_node_set_state (turn->node, asked != NULL ? XD_NODE_WAITING : XD_NODE_IDLE);
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
      !xd_storage_set_last_seen (self->storage, chat_id, turn->backend_id,
                                 xd_storage_last_message_id (self->storage, chat_id),
                                 &error))
    g_warning ("cannot record what the assistant has seen: %s", error->message);

  if (!success)
    {
      const char *text = message != NULL && *message != '\0'
                           ? message : "The backend stopped unexpectedly.";

      if (!xd_storage_append_message (self->storage, chat_id, "error", text,
                                      NULL, NULL, &error))
        g_warning ("cannot store the error: %s", error->message);

      if (visible)
        append_row (self, XD_MESSAGE_ERROR, text);
    }

  if (visible)
    {
      gint64 seconds = (g_get_monotonic_time () - turn->started_at) / G_USEC_PER_SEC;

      if (seconds >= 1)
        {
          GtkWidget *row = worked_for_row (seconds);

          if (turn->anchor != NULL)
            gtk_box_insert_child_after (self->transcript, row, turn->anchor);
          else
            gtk_box_append (self->transcript, row);
        }
    }

  if (visible)
    set_working (self, FALSE);

  /* An agent that edited anything has finished doing so. */
  xd_diff_pane_refresh (self->diff);
  xd_git_actions_refresh (self->git_actions);

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
take_composer_text (XdChatView *self)
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
workdir_for (const XdChat              *chat,
             const XdEffectiveSettings *resolved)
{
  if (chat->workdir != NULL && *chat->workdir != '\0')
    return chat->workdir;

  return resolved->workdir;
}

static void
start_turn (XdChatView *self,
            const char *prompt)
{
  g_autoptr (XdChat) chat = NULL;
  g_autoptr (GError) error = NULL;
  g_autoptr (XdEffectiveSettings) resolved = NULL;
  g_autofree char *resume_session_id = NULL;
  g_autofree char *handover = NULL;
  g_autofree char *full_prompt = NULL;
  g_autofree char *system_prompt = NULL;
  const AiBackend *backend;
  AiRunSpec spec = { 0 };
  Turn *turn;

  chat = xd_storage_get_chat (self->storage, xd_node_get_chat_id (self->chat),
                              &error);
  if (chat == NULL)
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return;
    }

  backend = ai_backend_lookup (chat->backend);
  if (backend == NULL)
    {
      g_autofree char *text = g_strdup_printf ("Unknown backend “%s”.",
                                               chat->backend);

      append_row (self, XD_MESSAGE_ERROR, text);
      return;
    }

  resume_session_id = xd_storage_get_session_id (self->storage, chat->id,
                                                 backend->id, NULL);

  /* Whatever this backend has not been told -- because the chat is new, or
   * because those turns went to the other assistant. */
  handover = xd_handover_build (self->storage, chat->id,
                                xd_storage_get_last_seen (self->storage, chat->id,
                                                          backend->id));

  full_prompt = handover != NULL ? g_strdup_printf ("%s\n\n%s", handover, prompt)
                                 : g_strdup (prompt);

  turn = g_new0 (Turn, 1);
  turn->view = self;
  turn->node = g_object_ref (self->chat);
  turn->started_at = g_get_monotonic_time ();

  /* Whatever is last right now sits just above this turn's output, which is
   * where the "worked for" line belongs when the turn ends. Weak, since the
   * transcript can be rebuilt while the turn runs. */
  turn->anchor = gtk_widget_get_last_child (GTK_WIDGET (self->transcript));
  if (turn->anchor != NULL)
    g_object_add_weak_pointer (G_OBJECT (turn->anchor), (gpointer *) &turn->anchor);

  xd_node_set_state (turn->node, XD_NODE_WORKING);
  turn->chat_id = g_strdup (chat->id);
  turn->backend_id = g_strdup (backend->id);
  turn->prompt = g_strdup (prompt);
  turn->resumed = resume_session_id != NULL;
  turn->text = g_string_new (NULL);
  turn->segment = g_string_new (NULL);
  turn->said = g_ptr_array_new_with_free_func (g_free);
  turn->session = xd_chat_session_new (backend);
  /* Taken now rather than when the reply lands: the model can be changed
   * while the agent is still working, and what answered is whatever was
   * running when the turn started. */
  turn->label = reply_title (chat);
  turn->row = append_row (self, XD_MESSAGE_ASSISTANT, NULL);
  xd_message_row_set_source (turn->row, turn->label);
  xd_message_row_set_waiting (turn->row, TRUE);

  set_working (self, TRUE);

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
  resolved = xd_settings_resolve (xd_node_get_parent (self->chat), chat->backend);

  spec.prompt = full_prompt;
  spec.workdir = workdir_for (chat, resolved);
  /* The chat's own pick wins; the folder chain is the fallback. */
  spec.model = chat->model != NULL ? chat->model : resolved->model;
  {
    g_autofree char *place =
      xd_settings_describe_place (xd_node_get_parent (self->chat),
                                  workdir_for (chat, resolved));
    g_autofree char *instructions =
      resolved->instructions != NULL
        ? g_strdup_printf ("%s\n\n%s", resolved->instructions, xd_ask_instructions ())
        : g_strdup (xd_ask_instructions ());

    spec.system_prompt = place != NULL
      ? g_strdup_printf ("%s\n\n%s", place, instructions)
      : g_strdup (instructions);

    g_free (system_prompt);
    system_prompt = (char *) spec.system_prompt;
  }
  spec.resume_session_id = resume_session_id;
  spec.effort = effort_for (chat);
  /* Plan overrides the access level for as long as it is on, without
   * overwriting it. */
  spec.access = chat->plan ? AI_ACCESS_PLAN
                           : ai_access_from_string (chat->access);

  if (!xd_chat_session_start (turn->session, &spec, &error))
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      xd_storage_append_message (self->storage, chat->id, "error",
                                 error->message, NULL, NULL, NULL);
      g_hash_table_remove (self->turns, chat->id);
      set_working (self, FALSE);
    }

  update_send_button (self);
}

/*
 * An unnamed chat takes its name from what was asked first. Deriving it from
 * the text costs nothing, where asking the model for a title would cost a
 * whole extra round trip before the answer even starts.
 */
static void
name_chat_after_first_message (XdChatView *self,
                               const char *prompt)
{
  g_autoptr (GPtrArray) messages = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *title = NULL;

  if (g_strcmp0 (xd_node_get_name (self->chat), XD_CHAT_UNTITLED) != 0)
    return;

  messages = xd_storage_list_messages (self->storage,
                                       xd_node_get_chat_id (self->chat), &error);
  if (messages == NULL || messages->len > 1)
    return;

  title = xd_chat_title_from_prompt (prompt);
  if (title == NULL)
    return;

  if (!xd_fs_tree_rename_chat (self->tree, self->chat, title, &error))
    {
      g_warning ("cannot name the chat: %s", error->message);
      return;
    }

  adw_window_title_set_title (self->title, title);
}

static void
send_message (XdChatView *self,
              const char *text)
{
  g_autoptr (GError) error = NULL;

  if (self->chat == NULL || text == NULL || *text == '\0')
    return;

  /* One turn at a time per chat; the button is a stop button meanwhile. */
  if (current_turn (self) != NULL)
    return;

  if (!xd_storage_append_message (self->storage, xd_node_get_chat_id (self->chat),
                                  "user", text, NULL, NULL, &error))
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return;
    }

  retire_open_questions (self);
  xd_node_set_state (self->chat, XD_NODE_IDLE);
  append_row (self, XD_MESSAGE_USER, text);
  name_chat_after_first_message (self, text);
  xd_fs_tree_bump_chat (self->tree, self->chat);

  start_turn (self, text);
}

/* --- pasted images --------------------------------------------------------- */

/* Big enough to recognise a screenshot, small enough to leave the composer
 * where it was. */
#define PREVIEW_HEIGHT 96
#define PREVIEW_MAX_WIDTH 168

static void
forget_attachments (XdChatView *self)
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
  XdChatView *self = user_data;
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
add_attachment (XdChatView *self,
                GdkTexture *texture)
{
  g_autofree char *directory = g_build_filename (g_get_user_cache_dir (), "xd",
                                                 "pasted", NULL);
  g_autofree char *name = NULL;
  g_autofree char *path = NULL;
  g_autoptr (GError) error = NULL;
  GtkWidget *chip;
  GtkWidget *picture;
  GtkWidget *remove;

  if (g_mkdir_with_parents (directory, 0700) != 0)
    {
      append_row (self, XD_MESSAGE_ERROR, "Cannot write the pasted image.");
      return;
    }

  /* The CLIs read the image off disk, so it needs a real file with a name
   * that will not collide with the next paste. */
  name = g_strdup_printf ("paste-%" G_GINT64_FORMAT ".png", g_get_real_time ());
  path = g_build_filename (directory, name, NULL);

  if (!gdk_texture_save_to_png (texture, path))
    {
      append_row (self, XD_MESSAGE_ERROR, "Cannot write the pasted image.");
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
  XdChatView *self = user_data;
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
paste_image (XdChatView *self)
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
show_queued (XdChatView *self)
{
  gtk_widget_set_visible (self->queued_bar, self->queued != NULL);

  if (self->queued != NULL)
    gtk_label_set_label (self->queued_label, self->queued);
}

static void
queue_message (XdChatView *self,
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
send_queued (XdChatView *self)
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
  XdChatView *self = user_data;
  Turn *turn = current_turn (self);

  if (turn != NULL)
    xd_chat_session_cancel (turn->session);
  /* The queued text goes out when the turn reports that it has stopped. */
}

static void
on_queued_dropped (GtkButton *button,
                   gpointer   user_data)
{
  XdChatView *self = user_data;

  g_clear_pointer (&self->queued, g_free);
  show_queued (self);
}

/* A refusal -- the chat already working, the daemon gone -- is the only part of
 * this worth showing: what worked comes back as an event. */
static void
on_remote_sent (GObject      *source,
                GAsyncResult *result,
                gpointer      user_data)
{
  g_autoptr (XdChatView) self = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);

  if (reply == NULL && !g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    append_row (self, XD_MESSAGE_ERROR, error->message);
}

/*
 * Sends to the daemon, which is where the agent runs.
 *
 * Nothing is written here and nothing is drawn: the daemon stores the message
 * and broadcasts what it did, and this window redraws from that like every
 * other device watching -- including the one the daemon is running on.
 */
static void
send_remote_message (XdChatView *self,
                     const char *text)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "send");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, xd_node_get_chat_id (self->chat));
  json_builder_set_member_name (builder, "text");
  json_builder_add_string_value (builder, text);
  json_builder_end_object (builder);

  request = json_builder_get_root (builder);

  xd_remote_client_call_async (self->remote, request, NULL,
                               on_remote_sent, g_object_ref (self));
}

static void
cancel_remote_turn (XdChatView *self)
{
  xd_remote_client_call_op_async (self->remote, "cancel", "chat",
                                  xd_node_get_chat_id (self->chat),
                                  NULL, on_remote_sent, g_object_ref (self));
}

static void
send_current_message (XdChatView *self)
{
  g_autofree char *text = take_composer_text (self);
  g_autoptr (GString) message = NULL;

  /*
   * A chat on a daemon takes the same composer and the same Enter.
   *
   * Attachments do not travel: they are files on this machine, and the agent
   * reading them is on another one.
   */
  if (self->remote != NULL)
    {
      if (self->remote_working)
        {
          if (text != NULL)
            queue_message (self, text);
          return;
        }

      if (text != NULL)
        send_remote_message (self, text);
      return;
    }

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
update_send_button (XdChatView *self)
{
  gboolean running = current_turn (self) != NULL || self->remote_working;

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
  g_autoptr (XdGitInfo) git = xd_git_info_for_path (workdir);
  g_autoptr (GString) text = g_string_new (NULL);
  g_autofree char *shown = NULL;

  if (workdir == NULL)
    return g_strdup ("No working directory");

  /*
   * The directory, written the way it would be typed.
   *
   * Two chats in the same folder can run in different checkouts, and a chat on
   * a daemon runs somewhere that is not on this machine at all -- so which
   * directory this is answers a question the branch cannot.
   */
  if (g_str_has_prefix (workdir, g_get_home_dir ()))
    shown = g_strconcat ("~", workdir + strlen (g_get_home_dir ()), NULL);
  else
    shown = g_strdup (workdir);

  if (git == NULL)
    return g_strdup_printf ("%s — not a repository", shown);

  if (git->branch != NULL)
    g_string_append_printf (text, "%s %s", git->detached ? "detached at" : "⎇",
                            git->branch);

  g_string_append_printf (text, "%s%s", text->len > 0 ? " · " : "", git->name);

  if (git->linked_worktree)
    g_string_append (text, " (worktree)");

  /* The remote is not shown: it is the longest thing in the bar and the least
   * likely to be in question, and it crowded out the branch, which changes. */

  g_string_append_printf (text, " · %s", shown);

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
set_terminal_height (XdChatView *self,
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
remember_panes (XdChatView *self)
{
  g_autoptr (GError) error = NULL;

  if (self->syncing_panes || self->chat == NULL)
    return;

  if (!xd_storage_set_panes (self->storage, xd_node_get_chat_id (self->chat),
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
  XdChatView *self = user_data;
  int height = gtk_widget_get_height (GTK_WIDGET (self->terminal));

  if (height > 0 && gtk_widget_get_visible (GTK_WIDGET (self->terminal)))
    g_settings_set_int (self->settings, "terminal-height", height);
}

static void
on_diff_dragged (GtkPaned   *paned,
                 GParamSpec *pspec,
                 gpointer    user_data)
{
  XdChatView *self = user_data;
  int width = gtk_widget_get_width (GTK_WIDGET (self->diff));

  if (width > 0 && gtk_widget_get_visible (GTK_WIDGET (self->diff)))
    g_settings_set_int (self->settings, "diff-width", width);
}

static void
on_diff_toggled (GtkToggleButton *button,
                 gpointer         user_data)
{
  XdChatView *self = user_data;
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

  xd_diff_pane_refresh (self->diff);
}

/* The panel asked to go away; the button has to agree, or it would take two
 * clicks to open it again. */
static void
close_terminal (XdChatView *self)
{
  gtk_toggle_button_set_active (self->terminal_button, FALSE);
}

/* A chat that no longer exists has no business keeping shells alive. */
static void
forget_chat_sessions (XdChatView *self,
                      XdNode     *chat)
{
  xd_terminal_panel_forget_chat (self->terminal, xd_node_get_chat_id (chat));
}

static int
terminal_height (XdChatView *self)
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
  XdChatView *self = user_data;
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
    xd_terminal_panel_start (self->terminal);
  else
    xd_terminal_panel_activate (self->terminal);
}

static void
update_context_bar (XdChatView   *self,
                    const XdChat *chat)
{
  g_autoptr (XdEffectiveSettings) resolved = NULL;
  g_autofree char *description = NULL;

  resolved = xd_settings_resolve (xd_node_get_parent (self->chat), chat->backend);
  description = describe_context (workdir_for (chat, resolved));

  gtk_label_set_label (self->context_label, description);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->context_label), description);

  {
    const char *workdir = workdir_for (chat, resolved);
    gboolean have_workdir = workdir != NULL && *workdir != '\0';
    g_autofree char *tooltip =
      have_workdir ? g_strdup_printf ("Terminal in %s", workdir) : NULL;

    xd_terminal_panel_set_chat (self->terminal, xd_node_get_chat_id (self->chat));
    xd_terminal_panel_set_workdir (self->terminal, have_workdir ? workdir : NULL);
    xd_diff_pane_set_workdir (self->diff, have_workdir ? workdir : NULL);
    xd_git_actions_set_workdir (self->git_actions, have_workdir ? workdir : NULL);

    /* The panes belong to the chat, so opening one brings them back. */
    self->syncing_panes = TRUE;
    gtk_toggle_button_set_active (self->terminal_button, chat->terminal_open);
    gtk_toggle_button_set_active (self->diff_button, chat->diff_open);
    self->syncing_panes = FALSE;

    gtk_widget_set_sensitive (GTK_WIDGET (self->terminal_button), have_workdir);
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->terminal_button),
                                 have_workdir ? tooltip : "This chat has no working directory");
  }

  xd_model_picker_set_selected (self->model_picker, chat->backend,
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
  XdChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  gboolean plan = gtk_toggle_button_get_active (self->plan_toggle);

  if (self->syncing_run_options || self->chat == NULL)
    return;

  if (self->remote != NULL)
    {
      set_remote_option (self, "plan", plan ? "true" : "false");
      gtk_widget_set_sensitive (GTK_WIDGET (self->access_chooser), !plan);
      return;
    }

  if (!xd_storage_set_plan (self->storage, xd_node_get_chat_id (self->chat),
                            plan, &error))
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return;
    }

  gtk_widget_set_sensitive (GTK_WIDGET (self->access_chooser), !plan);
}

static void
on_effort_selected (GtkDropDown *chooser,
                    GParamSpec  *pspec,
                    gpointer     user_data)
{
  XdChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  guint selected = gtk_drop_down_get_selected (chooser);

  if (self->syncing_run_options || self->chat == NULL ||
      selected >= G_N_ELEMENTS (effort_choices))
    return;

  if (self->remote != NULL)
    {
      set_remote_option (self, "effort",
                         ai_effort_to_string (effort_choices[selected]));
      return;
    }

  if (!xd_storage_set_effort (self->storage, xd_node_get_chat_id (self->chat),
                              ai_effort_to_string (effort_choices[selected]),
                              &error))
    append_row (self, XD_MESSAGE_ERROR, error->message);
}

static void
on_access_selected (GtkDropDown *chooser,
                    GParamSpec  *pspec,
                    gpointer     user_data)
{
  XdChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  guint selected = gtk_drop_down_get_selected (chooser);

  if (self->syncing_run_options || self->chat == NULL ||
      selected >= G_N_ELEMENTS (access_choices))
    return;

  if (self->remote != NULL)
    {
      set_remote_option (self, "access",
                         ai_access_to_string (access_choices[selected]));
      return;
    }

  if (!xd_storage_set_access (self->storage, xd_node_get_chat_id (self->chat),
                              ai_access_to_string (access_choices[selected]),
                              &error))
    append_row (self, XD_MESSAGE_ERROR, error->message);
}

static void
on_model_chosen (XdModelPicker *picker,
                 const char    *backend_id,
                 const char    *model_id,
                 gpointer       user_data)
{
  XdChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  g_autoptr (XdChat) chat = NULL;
  const char *chat_id;
  gboolean backend_changed;

  if (self->chat == NULL)
    return;

  if (self->remote != NULL)
    {
      /* Both, and in that order: the backend decides which models mean
       * anything, so a model set against the old one would be nonsense. */
      set_remote_option (self, "backend", backend_id);
      set_remote_option (self, "model", model_id);
      return;
    }

  chat_id = xd_node_get_chat_id (self->chat);
  chat = xd_storage_get_chat (self->storage, chat_id, NULL);
  if (chat == NULL)
    return;

  backend_changed = g_strcmp0 (chat->backend, backend_id) != 0;

  if (backend_changed &&
      !xd_storage_set_backend (self->storage, chat_id, backend_id, &error))
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return;
    }

  if (!xd_storage_set_model (self->storage, chat_id, model_id, &error))
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return;
    }

  /* The tree shows which assistant a chat belongs to, so it follows. */
  if (backend_changed)
    {
      const AiBackend *backend = ai_backend_lookup (backend_id);

      if (backend != NULL)
        xd_node_set_icon_name (self->chat, backend->icon_name);
    }

  /* Said in the chat, and stored with it: a transcript where the voice
   * changes mid-way should say so where it happened. */
  if (backend_changed || g_strcmp0 (chat->model, model_id) != 0)
    {
      const AiBackend *backend = ai_backend_lookup (backend_id);
      g_autofree char *event = NULL;

      if (backend != NULL)
        event = g_strdup_printf ("Switched to %s",
                                 ai_backend_model_label (backend, model_id));

      if (event != NULL)
        {
          if (!xd_storage_append_message (self->storage, chat_id, "event",
                                          event, NULL, NULL, &error))
            g_warning ("cannot store the switch: %s", error->message);

          append_row (self, XD_MESSAGE_TOOL, event);
        }
    }

  /* Nothing is discarded here. Sessions are kept per backend, so switching
   * assistants resumes that assistant's own session when it has one, and
   * otherwise the next turn replays the transcript to it. Either way the
   * conversation carries over. */

  {
    g_autoptr (XdChat) updated = xd_storage_get_chat (self->storage, chat_id, NULL);

    if (updated != NULL)
      update_context_bar (self, updated);
  }
}

static void
on_send_clicked (GtkButton *button,
                 gpointer   user_data)
{
  XdChatView *self = user_data;
  Turn *turn = current_turn (self);

  if (self->remote != NULL && self->remote_working)
    cancel_remote_turn (self);
  else if (turn != NULL)
    xd_chat_session_cancel (turn->session);
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
  XdChatView *self = user_data;

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

/*
 * The parts of the window that act on this machine.
 *
 * A remote chat has none of them: the terminal it would open is a shell here,
 * the changed files are this checkout, and the composer sends through a session
 * started here. All of that belongs to the daemon, so it is taken away rather
 * than left on screen doing the wrong thing quietly.
 */
static void
set_local_controls_visible (XdChatView *self,
                            gboolean    visible)
{
  gtk_widget_set_visible (GTK_WIDGET (self->terminal_button), visible);
  gtk_widget_set_visible (GTK_WIDGET (self->diff_button), visible);
  gtk_widget_set_visible (GTK_WIDGET (self->git_actions), visible);

  if (visible)
    return;

  /* Closing them without writing the panes back: they are being closed
   * because of where the chat lives, not because the user shut them. */
  self->syncing_panes = TRUE;
  gtk_toggle_button_set_active (self->terminal_button, FALSE);
  gtk_toggle_button_set_active (self->diff_button, FALSE);
  self->syncing_panes = FALSE;

  xd_terminal_panel_set_chat (self->terminal, NULL);
}

void
xd_chat_view_show_remote_chat (XdChatView     *self,
                               XdNode         *chat,
                               XdRemoteClient *client)
{
  Turn *turn;

  g_return_if_fail (XD_IS_CHAT_VIEW (self));
  g_return_if_fail (XD_IS_NODE (chat));
  g_return_if_fail (XD_IS_REMOTE_CLIENT (client));

  turn = current_turn (self);
  if (turn != NULL)
    turn->row = NULL;

  g_set_object (&self->chat, chat);
  set_remote (self, client);

  set_local_controls_visible (self, FALSE);

  gtk_stack_set_visible_child_name (self->stack, "chat");
  gtk_widget_set_visible (self->composer_area, TRUE);
  adw_window_title_set_title (self->title, xd_node_get_name (chat));
  adw_window_title_set_subtitle (self->title, xd_remote_client_get_host (client));

  end_remote_turn (self);
  load_remote_transcript (self);
  load_remote_options (self);
  update_send_button (self);
  gtk_widget_grab_focus (GTK_WIDGET (self->composer));
}

void
xd_chat_view_set_chat (XdChatView *self,
                       XdNode     *chat)
{
  Turn *turn;

  g_return_if_fail (XD_IS_CHAT_VIEW (self));

  /* The outgoing chat's row is about to be destroyed with the transcript. */
  turn = current_turn (self);
  if (turn != NULL)
    turn->row = NULL;

  /* Whatever a daemon was still going to say about the last chat is no longer
   * about anything on screen. */
  g_cancellable_cancel (self->fetching);
  g_clear_object (&self->fetching);
  set_remote (self, NULL);
  end_remote_turn (self);
  self->working_row = NULL;
  set_local_controls_visible (self, TRUE);
  adw_window_title_set_subtitle (self->title, NULL);

  g_set_object (&self->chat, chat);

  if (chat == NULL)
    {
      xd_terminal_panel_set_chat (self->terminal, NULL);
      clear_transcript (self);
      gtk_stack_set_visible_child_name (self->stack, "empty");
      gtk_widget_set_visible (self->composer_area, FALSE);
      adw_window_title_set_title (self->title, "xd");
      adw_window_title_set_subtitle (self->title, NULL);
      return;
    }

  gtk_stack_set_visible_child_name (self->stack, "chat");
  gtk_widget_set_visible (self->composer_area, TRUE);
  adw_window_title_set_title (self->title, xd_node_get_name (chat));

  {
    g_autoptr (XdChat) record = xd_storage_get_chat (self->storage,
                                                     xd_node_get_chat_id (chat),
                                                     NULL);

    if (record != NULL)
      update_context_bar (self, record);
  }

  load_transcript (self);

  /* Re-attach a reply that kept arriving while another chat was on screen. */
  turn = current_turn (self);
  if (turn != NULL)
    {
      /* The finished parts of this turn live only in memory until it ends,
       * so the rebuilt transcript has to replay them or they vanish until
       * the chat is next reopened. */
      for (guint i = 0; i < turn->said->len; i++)
        {
          XdMessageRow *said =
            append_row (self, XD_MESSAGE_ASSISTANT,
                        g_ptr_array_index (turn->said, i));

          xd_message_row_set_source (said, turn->label);
        }

      turn->row = append_row (self, XD_MESSAGE_ASSISTANT, turn->segment->str);
      xd_message_row_set_source (turn->row, turn->label);
      xd_message_row_set_waiting (turn->row, TRUE);
    }

  update_send_button (self);
  gtk_widget_grab_focus (GTK_WIDGET (self->composer));
}

XdNode *
xd_chat_view_get_chat (XdChatView *self)
{
  g_return_val_if_fail (XD_IS_CHAT_VIEW (self), NULL);

  return self->chat;
}

XdChatView *
xd_chat_view_new (XdStorage *storage,
                  XdFsTree  *tree)
{
  XdChatView *self;

  g_return_val_if_fail (XD_IS_STORAGE (storage), NULL);
  g_return_val_if_fail (XD_IS_FS_TREE (tree), NULL);

  self = g_object_new (XD_TYPE_CHAT_VIEW, NULL);
  self->storage = g_object_ref (storage);
  self->tree = g_object_ref (tree);

  /* Writes made by anything else on this machine, the daemon included. */
  xd_storage_watch (storage);
  g_signal_connect (storage, "changed", G_CALLBACK (on_storage_changed), self);

  /* Here rather than in init: the tree does not exist yet at init time. */
  g_signal_connect_swapped (self->tree, "chat-removed",
                            G_CALLBACK (forget_chat_sessions), self);

  g_signal_connect (self->storage, "changed",
                    G_CALLBACK (on_storage_changed), self);

  xd_chat_view_set_chat (self, NULL);

  return self;
}

/* --- construction --------------------------------------------------------- */

/*
 * The composer is a bar rather than a bare entry: the two things worth knowing
 * before pressing Enter are which assistant will answer and which checkout it
 * will be looking at, so both sit next to the text.
 */
static GtkWidget *
build_composer (XdChatView *self)
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

  self->model_picker = xd_model_picker_new ();
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
    add_option_descriptions (self->effort_chooser, effort_descriptions);
    gtk_widget_add_css_class (GTK_WIDGET (self->effort_chooser), "flat");
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->effort_chooser),
                                 "How hard the model is asked to think");
    g_signal_connect (self->effort_chooser, "notify::selected",
                      G_CALLBACK (on_effort_selected), self);

    self->access_chooser = GTK_DROP_DOWN (gtk_drop_down_new (G_LIST_MODEL (accesses), NULL));
    add_option_descriptions (self->access_chooser, access_descriptions);
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

  /* The send button sits at the end of the row it belongs to, with the
   * controls that decide what gets sent. */
  {
    GtkWidget *filler = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);

    gtk_widget_set_hexpand (filler, TRUE);
    gtk_box_append (GTK_BOX (toolbar), filler);
  }

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
  gtk_widget_add_css_class (toolbar, "xd-composer");
  gtk_box_append (GTK_BOX (column), toolbar);

  gtk_frame_set_child (GTK_FRAME (frame), column);
  gtk_widget_set_margin_top (frame, 6);
  gtk_widget_set_margin_start (frame, 12);
  gtk_widget_set_margin_end (frame, 12);

  /*
   * What is being worked on goes under the box, not in it.
   *
   * The row inside decides how the next message is answered; the branch and
   * directory are what it will be answered about. Keeping them in one row
   * left both cramped enough to be truncated.
   */
  {
    GtkWidget *stack = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
    GtkWidget *context = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);

    gtk_box_append (GTK_BOX (context), GTK_WIDGET (self->context_label));
    gtk_widget_add_css_class (context, "xd-context");
    gtk_widget_add_css_class (context, "dim-label");
    gtk_widget_set_margin_start (context, 26);
    gtk_widget_set_margin_end (context, 26);
    gtk_widget_set_margin_bottom (context, 12);

    gtk_box_append (GTK_BOX (stack), frame);
    gtk_box_append (GTK_BOX (stack), context);

    return stack;
  }
}

static void
xd_chat_view_dispose (GObject *object)
{
  XdChatView *self = XD_CHAT_VIEW (object);

  if (self->storage != NULL)
    g_signal_handlers_disconnect_by_data (self->storage, self);

  g_cancellable_cancel (self->fetching);
  g_clear_object (&self->fetching);
  g_clear_object (&self->remote);
  g_clear_object (&self->chat);
  g_clear_pointer (&self->turns, g_hash_table_unref);
  g_clear_pointer (&self->attachments, g_ptr_array_unref);
  g_clear_pointer (&self->queued, g_free);
  g_clear_object (&self->settings);
  g_clear_object (&self->storage);
  g_clear_object (&self->tree);

  G_OBJECT_CLASS (xd_chat_view_parent_class)->dispose (object);
}

static void
xd_chat_view_class_init (XdChatViewClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = xd_chat_view_dispose;
}

static void
xd_chat_view_init (XdChatView *self)
{
  GtkWidget *toolbar = adw_toolbar_view_new ();
  GtkWidget *header = adw_header_bar_new ();
  GtkWidget *content = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *empty = adw_status_page_new ();

  self->turns = g_hash_table_new_full (g_str_hash, g_str_equal, g_free, turn_free);
  self->settings = g_settings_new (XD_APP_ID);
  self->attachments = g_ptr_array_new_with_free_func (g_free);

  self->title = ADW_WINDOW_TITLE (adw_window_title_new ("xd", NULL));
  adw_header_bar_set_title_widget (ADW_HEADER_BAR (header), GTK_WIDGET (self->title));

  /* The sidebar is the leftmost header bar, so whatever the desktop puts on
   * that side of the title bar is its to draw. */
  adw_header_bar_set_show_start_title_buttons (ADW_HEADER_BAR (header), FALSE);

  /* At the top: these open and close parts of the window, which is what the
   * header bar is for. The row under the composer decides how the next
   * message is answered, which is a different question. */
  self->git_actions = xd_git_actions_new ();
  adw_header_bar_pack_end (ADW_HEADER_BAR (header), GTK_WIDGET (self->git_actions));

  adw_toolbar_view_add_top_bar (ADW_TOOLBAR_VIEW (toolbar), header);

  adw_status_page_set_icon_name (ADW_STATUS_PAGE (empty), XD_CHAT_ICON);
  adw_status_page_set_title (ADW_STATUS_PAGE (empty), "No Chat Selected");
  adw_status_page_set_description (ADW_STATUS_PAGE (empty),
                                   "Pick a chat in the sidebar, or start a new "
                                   "one in a folder.");

  self->transcript = GTK_BOX (gtk_box_new (GTK_ORIENTATION_VERTICAL, 8));
  gtk_widget_set_valign (GTK_WIDGET (self->transcript), GTK_ALIGN_START);

  self->scroller = GTK_SCROLLED_WINDOW (gtk_scrolled_window_new ());

  /* No scrollbar here either: GTK's overlay bar never quite goes away, and a
   * line beside the conversation was the first thing the eye caught. The
   * wheel still scrolls; position is something the content itself shows. */
  gtk_scrolled_window_set_policy (self->scroller, GTK_POLICY_NEVER,
                                  GTK_POLICY_EXTERNAL);

  /*
   * A column, not the whole window.
   *
   * Text set across a wide window is hard to read -- the eye loses the line
   * on the way back -- and a conversation pinned to both edges reads as a log
   * rather than as something being said. The composer is clamped to match, so
   * a message lines up with the box it was written in.
   */
  {
    GtkWidget *clamp = adw_clamp_new ();

    adw_clamp_set_maximum_size (ADW_CLAMP (clamp), CONTENT_WIDTH);
    adw_clamp_set_tightening_threshold (ADW_CLAMP (clamp), CONTENT_WIDTH);
    adw_clamp_set_child (ADW_CLAMP (clamp), GTK_WIDGET (self->transcript));
    gtk_widget_set_margin_top (clamp, 12);
    gtk_widget_set_margin_bottom (clamp, 12);
    gtk_scrolled_window_set_child (self->scroller, clamp);
  }
  gtk_widget_set_vexpand (GTK_WIDGET (self->scroller), TRUE);

  self->stack = GTK_STACK (gtk_stack_new ());
  gtk_stack_add_named (self->stack, empty, "empty");
  gtk_stack_add_named (self->stack, GTK_WIDGET (self->scroller), "chat");
  gtk_widget_set_vexpand (GTK_WIDGET (self->stack), TRUE);

  self->composer_area = build_composer (self);

  /* Packed here rather than with the rest of the header: the toggles are
   * built with the composer, so they do not exist until it has been. */
  adw_header_bar_pack_end (ADW_HEADER_BAR (header), GTK_WIDGET (self->terminal_button));
  adw_header_bar_pack_end (ADW_HEADER_BAR (header), GTK_WIDGET (self->diff_button));

  {
    GtkWidget *clamp = adw_clamp_new ();

    adw_clamp_set_maximum_size (ADW_CLAMP (clamp), CONTENT_WIDTH);
    adw_clamp_set_tightening_threshold (ADW_CLAMP (clamp), CONTENT_WIDTH);
    adw_clamp_set_child (ADW_CLAMP (clamp), self->composer_area);
    self->composer_area = clamp;
  }

  gtk_box_append (GTK_BOX (content), GTK_WIDGET (self->stack));
  gtk_box_append (GTK_BOX (content), self->composer_area);

  /* The terminal shares the window with the conversation rather than covering
   * it: the reason to open one is usually to check something the agent just
   * said, which means reading both at once. */
  self->terminal = xd_terminal_panel_new ();
  gtk_widget_set_visible (GTK_WIDGET (self->terminal), FALSE);
  gtk_widget_add_css_class (GTK_WIDGET (self->terminal), "xd-divider-top");
  g_signal_connect_swapped (self->terminal, "close-requested",
                            G_CALLBACK (close_terminal), self);

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
  self->diff = xd_diff_pane_new ();
  gtk_widget_set_visible (GTK_WIDGET (self->diff), FALSE);
  gtk_widget_add_css_class (GTK_WIDGET (self->diff), "xd-divider-left");

  self->side_split = GTK_PANED (gtk_paned_new (GTK_ORIENTATION_HORIZONTAL));
  g_signal_connect (self->side_split, "notify::position",
                    G_CALLBACK (on_diff_dragged), self);
  gtk_paned_set_start_child (self->side_split, GTK_WIDGET (self->split));
  gtk_paned_set_resize_start_child (self->side_split, TRUE);
  gtk_paned_set_shrink_start_child (self->side_split, FALSE);
  gtk_paned_set_end_child (self->side_split, GTK_WIDGET (self->diff));
  gtk_paned_set_resize_end_child (self->side_split, FALSE);
  gtk_paned_set_shrink_end_child (self->side_split, FALSE);

  /* Named so the stylesheet can reach them; see XD_STYLE. */
  gtk_widget_add_css_class (toolbar, "xd-surface");
  gtk_widget_add_css_class (content, "xd-surface");
  gtk_widget_add_css_class (GTK_WIDGET (self->scroller), "xd-surface");
  gtk_widget_add_css_class (GTK_WIDGET (self->stack), "xd-surface");

  adw_toolbar_view_set_content (ADW_TOOLBAR_VIEW (toolbar), GTK_WIDGET (self->side_split));


  adw_bin_set_child (ADW_BIN (self), toolbar);
}
