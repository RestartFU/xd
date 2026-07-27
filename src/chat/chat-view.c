#include "chat-view.h"

#include <string.h>

#include "chat-session.h"
#include "chat-title.h"
#include "ui/dots.h"
#include "handover.h"
#include "message-row.h"
#include "model-picker.h"
#include "option-picker.h"
#include "file-pane.h"
#include "diff-pane.h"
#include "git-actions.h"
#include "terminal-panel.h"
#include "settings/settings-resolver.h"
#include "remote/protocol.h"
#include "util/ask-block.h"
#include "util/git-diff.h"
#include "util/git-head-watch.h"
#include "util/git-info.h"
#include "util/subagent-tool.h"
#include "util/workflow-run.h"
#include "util/worktree.h"

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
  gboolean tool;
  char *text;
} TurnItem;

typedef struct
{
  XdChatView *view;
  XdMessageRow *row;
  char *chat_id;
} RemoteSend;

typedef struct
{
  XdChatView *view;         /* unowned; the view outlives its turns */
  XdChatSession *session;
  char *chat_id;
  char *backend_id;         /* the backend this turn's session id belongs to */
  char *model_id;           /* context usage belongs to this exact model */
  char *prompt;             /* kept so a dead session can be retried */
  char *workdir;            /* where file-change diffs are captured */
  XdGitDiffTracker *diff_tracker;
  char *label;              /* the model and effort this turn actually ran on */
  gint64 started_at;        /* monotonic; how long the work took */
  GtkWidget *anchor;        /* weak: the row just above the turn's output */
  XdNode *node;             /* the row in the tree, so it can show the state */
  GString *text;            /* everything the turn has said, for the ask block */
  GString *segment;         /* what belongs in the row being written now */
  GPtrArray *items;         /* finished speech and tools, in timeline order */
  gboolean resumed;
  gboolean is_retry;
  gboolean had_tool;
  guint64 context_used;
  guint64 context_window;
} Turn;

typedef struct
{
  char *key;
  char *chat_id;
  GtkBox *transcript;       /* owned by transcript_stack */
  XdRemoteClient *remote;
  gint64 message_id;
  guint limit;
} TranscriptPage;

/* Code and paired diffs need more room than prose. Keep the column bounded so
 * it still reads as a conversation, but use the space available on a desktop
 * instead of shrinking two diff sides into a narrow card. */
#define CONTENT_WIDTH 1040
#define TRANSCRIPT_PAGE_SIZE 100
#define TRANSCRIPT_CACHE_SIZE 4

typedef enum
{
  PANE_NONE     = 0,
  PANE_TERMINAL = 1 << 0,
  PANE_FILES    = 1 << 1,
  PANE_DIFF     = 1 << 2,
} PaneState;

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
  GBinding *title_binding;

  /*
   * Set while the chat on screen belongs to a daemon.
   *
   * Everything that writes goes through the storage above, which knows nothing
   * about that chat -- so this doubles as the flag that says so, and the
   * transcript is read over the connection instead.
   */
  XdRemoteClient *remote;
  GCancellable *fetching;       /* the transcript request in flight, if any */
  GPtrArray *pending_remote_messages; /* held until live state arrives */
  guint pending_remote_message_total;
  gint64 pending_remote_message_id;
  gint64 remote_rendered_message_id;
  gboolean restore_remote_panes;

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
  GtkLabel *working_label;
  guint working_timer;

  gboolean remote_working;
  gint64 remote_started_at;       /* monotonic usec on this device */
  GString *remote_said;
  char *remote_label;

  /* The last message the transcript on screen was drawn from, so a write made
   * by something else can be told from one this window just made. */
  gint64 rendered_message_id;

  GHashTable *turns;            /* chat id -> Turn* */

  GtkWidget *header;            /* owned by the toolbar */
  AdwWindowTitle *title;
  GtkStack *stack;
  GtkStack *transcript_stack;
  GtkBox *transcript;
  GtkBox *empty_transcript;
  TranscriptPage *transcript_page;
  GHashTable *transcript_pages; /* local:/remote: chat key -> TranscriptPage */
  GQueue transcript_lru;        /* least recently viewed first */
  GtkScrolledWindow *scroller;
  gboolean follow_bottom;
  guint bottom_jump_tick;
  guint bottom_pin_tick;
  double bottom_jump_upper;
  double bottom_jump_page_size;
  guint bottom_jump_stable_frames;
  gboolean rendering_transcript;
  guint transcript_limit;
  double history_bottom_distance;
  GtkTextView *composer;
  GtkButton *send_button;
  gboolean send_state_set;
  gboolean send_running;
  GtkWidget *composer_area;
  GtkWidget *attachments_bar;
  GtkWidget *queued_bar;
  GtkWidget *choices_bar;
  GtkWidget *commands_bar;
  GtkFlowBox *commands_flow;
  GtkProgressBar *context_meter;
  GtkLabel *queued_label;
  char *queued;             /* typed while a turn was running */
  gboolean syncing_panes;   /* setting the toggles to match the chat */
  GPtrArray *attachments;   /* absolute paths of pasted images */
  GHashTable *command_sets; /* local/remote backend scope -> GStrv */
  char *command_scope;      /* command set used by composer on screen */
  XdModelPicker *model_picker;
  XdOptionPicker *workspace_chooser;
  GPtrArray *workspace_paths; /* choice index -> existing worktree path or NULL */
  XdOptionPicker *effort_chooser;
  XdOptionPicker *access_chooser;
  GtkToggleButton *build_toggle;
  GtkToggleButton *plan_toggle;
  GtkLabel *context_label;
  GtkToggleButton *terminal_button;
  XdTerminalPanel *terminal;
  GtkToggleButton *file_button;
  XdFilePane *files;
  GtkToggleButton *diff_button;
  XdGitActions *git_actions;
  XdDiffPane *diff;
  XdGitHeadWatch *git_head_watch;
  GtkPaned *split;
  GtkPaned *side_split;
  GtkStack *side_stack;
  GSettings *settings;

  /* Set while the choosers are filled in from the chat, so the resulting
   * notify does not read back as the user picking something. */
  gboolean syncing_run_options;
  gboolean syncing_workspace;
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

GtkWidget *
xd_chat_view_get_header (XdChatView *self)
{
  g_return_val_if_fail (XD_IS_CHAT_VIEW (self), NULL);

  return self->header;
}

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
static const char *const workspace_descriptions[] = {
  "Use the checkout this chat currently points at.",
  "Create an isolated branch and checkout for this chat.",
};

G_DEFINE_FINAL_TYPE (XdChatView, xd_chat_view, ADW_TYPE_BIN)

static void send_current_message (XdChatView *self);
static void keep_working_last (XdChatView *self);
static void set_working (XdChatView *self, gboolean working);
static void on_storage_changed (XdStorage *storage, gpointer user_data);
static void load_transcript (XdChatView *self);
static void load_remote_transcript (XdChatView *self);
static void load_remote_options (XdChatView *self);
static void append_tool_line (XdChatView *self, const char *name);
static void show_tool_use (XdChatView *self, const char *summary);
static char *describe_context (const char *workdir);
static void on_remote_sent (GObject *source, GAsyncResult *result, gpointer data);
static void show_queued (XdChatView *self);
static void set_queued_text (XdChatView *self, const char *text);
static Turn *current_turn (XdChatView *self);
static gboolean send_remote_message (XdChatView *self, const char *text);
static void cancel_remote_turn (XdChatView *self);
static void send_queued (XdChatView *self);
static gboolean send_message (XdChatView *self,
                              const char *text);
static void update_send_button (XdChatView *self);
static void update_context_bar (XdChatView *self,
                                const XdChat *chat);
static void update_context_meter (XdChatView *self,
                                  guint64     used,
                                  guint64     window);
static void update_workspace_choice (XdChatView *self,
                                     const XdChat *chat,
                                     const char   *workdir,
                                     gboolean      has_messages,
                                     gboolean      linked_worktree,
                                     GPtrArray    *worktrees);
static guint saved_panes (XdChatView *self, guint fallback);
static void apply_panes (XdChatView *self, guint state);
static void start_turn (XdChatView *self,
                        const char *prompt);
static char *command_scope (XdChatView *self,
                            const char *backend_id);
static void use_command_scope (XdChatView *self,
                               const char *backend_id);
static void store_commands (XdChatView       *self,
                            const char       *scope,
                            const char *const *commands);
static void refresh_command_suggestions (XdChatView *self);
static void on_model_chosen (XdModelPicker *picker,
                             const char    *backend_id,
                             const char    *model_id,
                             gpointer       user_data);
static void on_effort_selected (XdOptionPicker *chooser,
                                GParamSpec     *pspec,
                                gpointer        user_data);
static void on_access_selected (XdOptionPicker *chooser,
                                GParamSpec     *pspec,
                                gpointer        user_data);
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
format_elapsed (const char *verb,
                gint64      seconds)
{
  if (seconds >= 3600)
    return g_strdup_printf ("%s for %dh %02dm", verb, (int) (seconds / 3600),
                            (int) ((seconds % 3600) / 60));
  if (seconds >= 60)
    return g_strdup_printf ("%s for %dm %02ds", verb, (int) (seconds / 60),
                            (int) (seconds % 60));

  return g_strdup_printf ("%s for %ds", verb, (int) seconds);
}

static char *
format_worked_for (gint64 seconds)
{
  return format_elapsed ("Worked", seconds);
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

static void
set_scroll_at_bottom (GtkAdjustment *adjustment)
{
  double bottom =
    MAX (gtk_adjustment_get_lower (adjustment),
         gtk_adjustment_get_upper (adjustment) -
         gtk_adjustment_get_page_size (adjustment));

  if (gtk_adjustment_get_value (adjustment) != bottom)
    gtk_adjustment_set_value (adjustment, bottom);
}

/*
 * Adjustment signals fire while GTK is still negotiating an allocation.
 * Setting the value inside that signal can be overwritten by the scroller's
 * own clamp later in the same frame. Pin again on the next frame, after the
 * allocation that emitted the signal has completed and before the next paint.
 */
static gboolean
pin_transcript_at_bottom (GtkWidget     *widget,
                          GdkFrameClock *frame_clock,
                          gpointer       user_data)
{
  XdChatView *self = user_data;

  self->bottom_pin_tick = 0;
  if (self->follow_bottom)
    set_scroll_at_bottom (
      gtk_scrolled_window_get_vadjustment (self->scroller));

  return G_SOURCE_REMOVE;
}

static void
queue_bottom_pin (XdChatView *self)
{
  if (self->bottom_pin_tick == 0)
    self->bottom_pin_tick =
      gtk_widget_add_tick_callback (GTK_WIDGET (self->scroller),
                                    pin_transcript_at_bottom, self, NULL);
}

static gboolean
reveal_transcript_at_bottom (GtkWidget     *widget,
                             GdkFrameClock *frame_clock,
                             gpointer       user_data)
{
  XdChatView *self = user_data;
  GtkAdjustment *adjustment =
    gtk_scrolled_window_get_vadjustment (self->scroller);
  double upper = gtk_adjustment_get_upper (adjustment);
  double page_size = gtk_adjustment_get_page_size (adjustment);

  set_scroll_at_bottom (adjustment);

  if (upper == self->bottom_jump_upper &&
      page_size == self->bottom_jump_page_size)
    self->bottom_jump_stable_frames++;
  else
    self->bottom_jump_stable_frames = 0;

  self->bottom_jump_upper = upper;
  self->bottom_jump_page_size = page_size;

  /*
   * The adjustment can reach its final value before GTK replaces the old
   * render node. Keep that intermediate frame invisible, then expose the
   * transcript only after two frames agree on its laid-out range.
   */
  if (self->bottom_jump_stable_frames >= 2)
    {
      self->bottom_jump_tick = 0;
      gtk_widget_queue_draw (GTK_WIDGET (self->scroller));
      gtk_widget_set_opacity (GTK_WIDGET (self->scroller), 1.0);
      return G_SOURCE_REMOVE;
    }

  return G_SOURCE_CONTINUE;
}

/*
 * Joining and sending replace the bottom edge. Painting before layout settles
 * either exposes a visible trip down the transcript or leaves GTK's old
 * snapshot on screen until the next wheel event.
 */
static void
begin_bottom_jump (XdChatView *self)
{
  self->follow_bottom = TRUE;
  self->history_bottom_distance = -1;
  self->bottom_jump_upper = -1;
  self->bottom_jump_page_size = -1;
  self->bottom_jump_stable_frames = 0;

  gtk_widget_set_opacity (GTK_WIDGET (self->scroller), 0.0);

  if (self->bottom_jump_tick == 0)
    self->bottom_jump_tick =
      gtk_widget_add_tick_callback (GTK_WIDGET (self->scroller),
                                    reveal_transcript_at_bottom, self, NULL);
}

/*
 * Layout can change the range after any finite number of frames: wrapped text,
 * fonts, and late remote rows all do it. Keep the adjustment pinned until the
 * user scrolls, instead of guessing when layout has finished.
 */
static void
on_scroll_adjustment_changed (GtkAdjustment *adjustment,
                              gpointer       user_data)
{
  XdChatView *self = user_data;

  if (self->follow_bottom)
    queue_bottom_pin (self);
  else if (self->history_bottom_distance >= 0)
    {
      double value =
        MAX (gtk_adjustment_get_lower (adjustment),
             gtk_adjustment_get_upper (adjustment) -
             self->history_bottom_distance);

      if (gtk_adjustment_get_value (adjustment) != value)
        gtk_adjustment_set_value (adjustment, value);
    }
}

static gboolean
on_transcript_scrolled (GtkEventControllerScroll *controller,
                        double                    dx,
                        double                    dy,
                        gpointer                  user_data)
{
  XdChatView *self = user_data;

  self->follow_bottom = FALSE;
  self->history_bottom_distance = -1;
  return GDK_EVENT_PROPAGATE;
}

/* Joining, sending, and new output all explicitly resume bottom following. */
static void
queue_scroll_to_bottom (XdChatView *self)
{
  self->follow_bottom = TRUE;
  self->history_bottom_distance = -1;
  set_scroll_at_bottom (
    gtk_scrolled_window_get_vadjustment (self->scroller));
  queue_bottom_pin (self);
}

static XdMessageRow *
append_row (XdChatView    *self,
            XdMessageKind  kind,
            const char    *text)
{
  XdMessageRow *row = self->remote != NULL
    ? xd_message_row_new_remote (kind, text, self->remote)
    : xd_message_row_new (kind, text);

  gtk_box_append (self->transcript, GTK_WIDGET (row));
  if (!self->rendering_transcript)
    {
      keep_working_last (self);
      queue_scroll_to_bottom (self);
    }

  return row;
}

/*
 * A collapsed tool group keeps only its text, not one hidden GTK widget per
 * call. Large delegated turns can contain hundreds of calls across several
 * cards; constructing and measuring all those labels made merely opening the
 * transcript expensive even though none of them were visible.
 */
static void
free_tool_summaries (gpointer data)
{
  g_string_free (data, TRUE);
}

static void
render_tool_group (GtkExpander *expander)
{
  GString *summaries =
    g_object_get_data (G_OBJECT (expander), "xd-tool-summaries");
  GtkWidget *label = gtk_expander_get_child (expander);

  if (label == NULL)
    {
      label = gtk_label_new (NULL);
      gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
      gtk_label_set_ellipsize (GTK_LABEL (label), PANGO_ELLIPSIZE_MIDDLE);
      gtk_label_set_max_width_chars (GTK_LABEL (label), 100);
      gtk_label_set_selectable (GTK_LABEL (label), TRUE);
      gtk_widget_add_css_class (label, "caption");
      gtk_widget_set_margin_top (label, 4);
      gtk_widget_set_margin_start (label, 12);
      gtk_expander_set_child (expander, label);
    }

  gtk_label_set_text (GTK_LABEL (label), summaries->str);
}

static void
on_tool_group_expanded (GtkExpander *expander,
                        GParamSpec  *pspec,
                        gpointer     user_data)
{
  if (gtk_expander_get_expanded (expander))
    render_tool_group (expander);
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
  GtkWidget *expander;
  GString *summaries;
  g_autofree char *title = NULL;
  int count;

  /* Past the dots, which sit at the end while the turn runs. */
  if (last == self->working_row && last != NULL)
    last = gtk_widget_get_prev_sibling (last);

  if (last != NULL &&
      GTK_IS_EXPANDER (last) &&
      g_object_get_data (G_OBJECT (last), "xd-tool-summaries") != NULL)
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

      g_object_set_data_full (G_OBJECT (expander), "xd-tool-summaries",
                              g_string_new (NULL),
                              free_tool_summaries);
      g_object_set_data (G_OBJECT (expander), "count", GINT_TO_POINTER (0));
      g_signal_connect (expander, "notify::expanded",
                        G_CALLBACK (on_tool_group_expanded), NULL);
      gtk_box_append (self->transcript, expander);
    }

  summaries = g_object_get_data (G_OBJECT (expander), "xd-tool-summaries");
  if (summaries->len > 0)
    g_string_append_c (summaries, '\n');
  g_string_append (summaries, summary);

  /* An open live group stays current. A closed one remains text-only until
   * it is opened again, so hidden content never enters GTK's layout queue. */
  if (gtk_expander_get_expanded (GTK_EXPANDER (expander)))
    render_tool_group (GTK_EXPANDER (expander));

  count = GPOINTER_TO_INT (g_object_get_data (G_OBJECT (expander), "count")) + 1;
  g_object_set_data (G_OBJECT (expander), "count", GINT_TO_POINTER (count));

  title = count == 1 ? g_strdup ("1 tool call")
                     : g_strdup_printf ("%d tool calls", count);
  gtk_expander_set_label (GTK_EXPANDER (expander), title);

  if (!self->rendering_transcript)
    {
      keep_working_last (self);
      queue_scroll_to_bottom (self);
    }
}

static void
show_tool_use (XdChatView *self,
               const char *summary)
{
  const char *diff = xd_git_diff_from_tool (summary);
  g_autofree char *run_id = NULL;
  g_autofree char *url = NULL;
  g_autofree char *subagent = NULL;
  g_autofree char *task = NULL;

  if (diff != NULL)
    {
      g_autofree char *block = g_strdup_printf ("```diff\n%s\n```", diff);

      append_row (self, XD_MESSAGE_ASSISTANT, block);
      return;
    }

  if (xd_workflow_run_from_tool (summary, &run_id, &url))
    {
      g_autofree char *block = g_strdup_printf (
        "**GitHub Actions · Run #%s**\n\n"
        "[Open live status and logs](%s)", run_id, url);
      XdMessageRow *row;

      /* Polling emits the same `gh run view` command repeatedly. The durable
       * tool records may all be useful to the agent, but one live card per run
       * is the useful transcript. This also folds watch-after-view into the
       * card that is already polling that run. */
      for (GtkWidget *child =
             gtk_widget_get_first_child (GTK_WIDGET (self->transcript));
           child != NULL;
           child = gtk_widget_get_next_sibling (child))
        if (g_strcmp0 (
              g_object_get_data (G_OBJECT (child), "xd-workflow-run-id"),
              run_id) == 0)
          return;

      row = append_row (self, XD_MESSAGE_ASSISTANT, block);
      g_object_set_data_full (G_OBJECT (row), "xd-workflow-run-id",
                              g_strdup (run_id), g_free);
      xd_message_row_make_workflow (row, run_id, url);
      return;
    }

  if (xd_subagent_tool_from_tool (summary, &subagent, &task))
    {
      g_autoptr (GString) safe_identity = g_string_new (NULL);
      g_autoptr (GString) safe_task = g_string_new (NULL);
      GtkWidget *activity;
      GtkWidget *last =
        gtk_widget_get_last_child (GTK_WIDGET (self->transcript));

      if (last == self->working_row && last != NULL)
        last = gtk_widget_get_prev_sibling (last);

      /*
       * Older Claude transcripts may already contain each streamed tool-only
       * message twice. Suppress those stored duplicates while the parser fix
       * prevents new ones.
       */
      if (last != NULL &&
          g_strcmp0 (g_object_get_data (G_OBJECT (last),
                                       "xd-subagent-record"),
                     summary) == 0)
        return;

      activity = last != NULL && GTK_IS_EXPANDER (last) ? last : NULL;

      /* Tool prompts are plain text. Keep Markdown punctuation in a task from
       * turning its card into a heading, code span, or accidental link. */
      for (const char *at = subagent; *at != '\0'; at++)
        {
          if (strchr ("\\`*_[]<>#", *at) != NULL)
            g_string_append_c (safe_identity, '\\');
          g_string_append_c (safe_identity, *at);
        }
      for (const char *at = task; *at != '\0'; at++)
        {
          if (strchr ("\\`*_[]<>#", *at) != NULL)
            g_string_append_c (safe_task, '\\');
          g_string_append_c (safe_task, *at);
        }

      g_autofree char *block = g_strdup_printf (
        "**Subagent · %s**\n\n%s", safe_identity->str, safe_task->str);
      XdMessageRow *row =
        append_row (self, XD_MESSAGE_ASSISTANT, block);

      g_object_set_data_full (G_OBJECT (row), "xd-subagent-record",
                              g_strdup (summary), g_free);
      xd_message_row_make_subagent (row, activity);
      return;
    }

  /* The machine-facing event name is not useful prose. A backend may report
   * an edit outside Git, where no patch can be captured; keep that case human
   * instead of leaking "file_change" into the conversation. */
  append_tool_line (self, xd_tool_is_file_change (summary)
                          ? "Files changed" : summary);
}

/*
 * Retires the question attached to the composer.
 *
 * Sending anything answers whatever was outstanding -- by button, by typing
 * something else, or by ignoring it and moving on. Once the conversation has
 * gone past a question, clicking one of its options would send an answer to
 * something nobody is asking any more.
 */
static void
retire_open_questions (XdChatView *self)
{
  GtkWidget *child;

  if (self->choices_bar == NULL)
    return;

  /* Taken away rather than greyed out. The answer is about to appear as a
   * message of its own, so dead buttons would only repeat what was offered. */
  while ((child = gtk_widget_get_first_child (self->choices_bar)) != NULL)
    gtk_box_remove (GTK_BOX (self->choices_bar), child);

  gtk_widget_set_visible (self->choices_bar, FALSE);
}

static void
submit_ask_answer (XdChatView *self,
                   const char *answer)
{
  if (answer == NULL || *answer == '\0' || self->chat == NULL)
    return;

  /* Before the buttons are torn down: when the focused button disappears,
   * focus falls to the first focusable widget -- a selectable message label,
   * which selects its whole text on focus-in. Parking focus on the composer
   * first means it never lands there. */
  gtk_widget_grab_focus (GTK_WIDGET (self->composer));

  if (self->remote != NULL)
    {
      /*
       * Remote nodes deliberately do not exist in local storage. Send through
       * the daemon like composer text; otherwise the local write fails with
       * "Unknown chat <uuid>" before the answer ever leaves this machine.
       *
       * The daemon also owns the one-turn-at-a-time check. If another device
       * started a turn since these choices appeared, it preserves this answer
       * as that chat's queued instruction.
       */
      retire_open_questions (self);
      send_remote_message (self, answer);
    }
  else
    {
      send_message (self, answer);
    }
}

static void
on_choice_clicked (GtkButton *button,
                   gpointer   user_data)
{
  submit_ask_answer (
    user_data, g_object_get_data (G_OBJECT (button), "answer"));
}

static void
submit_ask_input (XdChatView *self,
                  GtkEntry   *entry)
{
  g_autofree char *answer =
    g_strdup (gtk_editable_get_text (GTK_EDITABLE (entry)));

  g_strstrip (answer);
  submit_ask_answer (self, answer);
}

static void
on_ask_input_activated (GtkEntry *entry,
                        gpointer  user_data)
{
  submit_ask_input (user_data, entry);
}

static void
on_ask_input_clicked (GtkButton *button,
                      gpointer   user_data)
{
  GtkEntry *entry = g_object_get_data (G_OBJECT (button), "input");

  submit_ask_input (user_data, entry);
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
  GtkWidget *choices;

  /* Only while it can still be answered. The question itself is part of the
   * reply and stays; these are a way of answering it, and a way of answering
   * something already answered is just something to explain. */
  if (!answerable)
    return;

  retire_open_questions (self);

  if (ask->options[0] != NULL)
    {
      /* One per line: the options are sentences, so side by side they would
       * each be too narrow to read. */
      choices = gtk_flow_box_new ();
      gtk_flow_box_set_selection_mode (
        GTK_FLOW_BOX (choices), GTK_SELECTION_NONE);
      gtk_flow_box_set_row_spacing (GTK_FLOW_BOX (choices), 4);
      gtk_flow_box_set_max_children_per_line (GTK_FLOW_BOX (choices), 1);
      gtk_flow_box_set_homogeneous (GTK_FLOW_BOX (choices), TRUE);

      for (gsize i = 0; ask->options[i] != NULL; i++)
        {
          GtkWidget *button = gtk_button_new_with_label (ask->options[i]);

          gtk_widget_add_css_class (button, "xd-choice");
          gtk_label_set_wrap (
            GTK_LABEL (gtk_button_get_child (GTK_BUTTON (button))), TRUE);
          g_object_set_data_full (G_OBJECT (button), "answer",
                                  g_strdup (ask->options[i]), g_free);
          g_signal_connect (
            button, "clicked", G_CALLBACK (on_choice_clicked), self);

          /* No option is highlighted: which one is right is the user's call,
           * and colouring one of them is xd putting a thumb on the scale. */
          gtk_flow_box_append (GTK_FLOW_BOX (choices), button);
        }

      gtk_box_append (GTK_BOX (self->choices_bar), choices);
    }

  if (ask->accepts_input)
    {
      GtkWidget *input_row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
      GtkWidget *entry = gtk_entry_new ();
      GtkWidget *send = gtk_button_new_with_label ("Send");

      gtk_entry_set_placeholder_text (GTK_ENTRY (entry), "Type your answer");
      gtk_widget_set_hexpand (entry, TRUE);
      gtk_widget_add_css_class (send, "suggested-action");
      g_object_set_data (G_OBJECT (send), "input", entry);
      g_signal_connect (entry, "activate",
                        G_CALLBACK (on_ask_input_activated), self);
      g_signal_connect (send, "clicked",
                        G_CALLBACK (on_ask_input_clicked), self);
      gtk_box_append (GTK_BOX (input_row), entry);
      gtk_box_append (GTK_BOX (input_row), send);
      gtk_box_append (GTK_BOX (self->choices_bar), input_row);
    }

  gtk_widget_set_visible (self->choices_bar, TRUE);
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

/* Duration is turn metadata, not a later answer to a question. */
static gboolean
reply_is_answerable (GPtrArray *messages,
                     guint      position)
{
  for (guint i = position + 1; i < messages->len; i++)
    {
      const XdMessage *message = g_ptr_array_index (messages, i);

      if (g_strcmp0 (message->role, "duration") != 0)
        return FALSE;
    }

  return TRUE;
}

static GtkBox *
new_transcript (void)
{
  GtkBox *transcript =
    GTK_BOX (gtk_box_new (GTK_ORIENTATION_VERTICAL, 8));

  gtk_widget_set_valign (GTK_WIDGET (transcript), GTK_ALIGN_START);
  return transcript;
}

static void
transcript_page_free (TranscriptPage *page)
{
  g_free (page->key);
  g_free (page->chat_id);
  g_clear_object (&page->remote);
  g_free (page);
}

static char *
transcript_page_key (XdNode         *chat,
                     XdRemoteClient *remote)
{
  if (remote == NULL)
    return g_strdup_printf ("local:%s", xd_node_get_chat_id (chat));

  /* A row can hold the connection that loads its remote images. A re-pair to
   * the same host must therefore get fresh rows rather than revive widgets
   * bound to the old client object. */
  return g_strdup_printf ("remote:%p:%s", (void *) remote,
                          xd_node_get_chat_id (chat));
}

static void
touch_transcript_page (XdChatView    *self,
                       TranscriptPage *page)
{
  g_queue_remove (&self->transcript_lru, page);
  g_queue_push_tail (&self->transcript_lru, page);
}

static void
remove_transcript_page (XdChatView     *self,
                        TranscriptPage *page)
{
  g_autofree char *key = NULL;

  if (page == NULL)
    return;

  key = g_strdup (page->key);
  g_queue_remove (&self->transcript_lru, page);
  gtk_stack_remove (self->transcript_stack, GTK_WIDGET (page->transcript));
  g_hash_table_remove (self->transcript_pages, key);
}

static void
activate_empty_transcript (XdChatView *self)
{
  gtk_stack_set_visible_child (
    self->transcript_stack, GTK_WIDGET (self->empty_transcript));
  self->transcript = self->empty_transcript;
  self->transcript_page = NULL;
}

static void
trim_transcript_cache (XdChatView *self)
{
  while (g_hash_table_size (self->transcript_pages) >
         TRANSCRIPT_CACHE_SIZE)
    {
      TranscriptPage *oldest = g_queue_peek_head (&self->transcript_lru);

      if (oldest == NULL)
        return;
      if (oldest == self->transcript_page)
        {
          touch_transcript_page (self, oldest);
          continue;
        }

      remove_transcript_page (self, oldest);
    }
}

/*
 * Makes one chat's already-built rows current.
 *
 * Local pages are reused only while their last durable message id still
 * matches SQLite. Remote pages are shown immediately and then validated by
 * the normal snapshot request, whose revision id decides whether to rebuild.
 */
static gboolean
activate_transcript_page (XdChatView     *self,
                          XdNode         *chat,
                          XdRemoteClient *remote,
                          gint64          message_id)
{
  g_autofree char *key = transcript_page_key (chat, remote);
  TranscriptPage *page = g_hash_table_lookup (self->transcript_pages, key);
  gboolean reused = page != NULL;

  if (page != NULL && remote == NULL && page->message_id != message_id)
    {
      remove_transcript_page (self, page);
      page = NULL;
      reused = FALSE;
    }

  if (page == NULL)
    {
      page = g_new0 (TranscriptPage, 1);
      page->key = g_strdup (key);
      page->chat_id = g_strdup (xd_node_get_chat_id (chat));
      page->transcript = new_transcript ();
      page->remote = remote != NULL ? g_object_ref (remote) : NULL;
      page->message_id = remote != NULL ? -1 : message_id;
      page->limit = TRANSCRIPT_PAGE_SIZE;

      gtk_stack_add_named (self->transcript_stack,
                           GTK_WIDGET (page->transcript), page->key);
      g_hash_table_insert (self->transcript_pages, page->key, page);
    }

  gtk_stack_set_visible_child (
    self->transcript_stack, GTK_WIDGET (page->transcript));
  self->transcript = page->transcript;
  self->transcript_page = page;
  self->transcript_limit = page->limit;
  if (remote != NULL)
    self->remote_rendered_message_id = page->message_id;
  else
    self->rendered_message_id = page->message_id;

  touch_transcript_page (self, page);
  trim_transcript_cache (self);
  return reused;
}

static gboolean
current_transcript_is_cacheable (XdChatView *self)
{
  if (self->transcript_page == NULL)
    return FALSE;

  /*
   * A remote page is the last useful copy when its connection drops. Keep it
   * even during a live turn; reopening validates it against the daemon, while
   * an outage leaves the readable stale copy in place instead of a blank page.
   *
   * Local live fragments can be reconstructed from their in-memory Turn, so
   * they retain the stricter cache rule.
   */
  if (self->remote != NULL)
    return TRUE;

  return current_turn (self) == NULL &&
         !gtk_widget_get_visible (self->choices_bar);
}

static void
leave_current_transcript (XdChatView *self,
                          gboolean    keep)
{
  TranscriptPage *page = self->transcript_page;

  if (page == NULL)
    return;

  page->message_id = self->remote != NULL
    ? (self->remote_working ? -1 : self->remote_rendered_message_id)
    : self->rendered_message_id;
  page->limit = self->transcript_limit;
  touch_transcript_page (self, page);

  if (!keep)
    {
      activate_empty_transcript (self);
      remove_transcript_page (self, page);
    }
}

static void
clear_transcript (XdChatView *self)
{
  GtkWidget *child;

  /* It is about to be taken out with everything else. Stop its timer too. */
  set_working (self, FALSE);
  retire_open_questions (self);

  while ((child = gtk_widget_get_first_child (GTK_WIDGET (self->transcript))) != NULL)
    gtk_box_remove (self->transcript, child);
}

/* Whatever was just added went in under it; the dots belong at the end. */
static void
keep_working_last (XdChatView *self)
{
  if (self->working_row == NULL)
    return;
  if (gtk_widget_get_last_child (GTK_WIDGET (self->transcript)) ==
      self->working_row)
    return;

  g_object_ref (self->working_row);
  gtk_box_remove (self->transcript, self->working_row);
  gtk_box_append (self->transcript, self->working_row);
  g_object_unref (self->working_row);
}

static gint64
working_seconds (XdChatView *self)
{
  Turn *turn = current_turn (self);
  gint64 started_at = turn != NULL ? turn->started_at
                                   : self->remote_started_at;

  if (started_at <= 0)
    return 0;

  return MAX ((g_get_monotonic_time () - started_at) / G_USEC_PER_SEC, 0);
}

static gboolean
update_working_label (gpointer user_data)
{
  XdChatView *self = user_data;
  g_autofree char *text = NULL;

  if (self->working_label == NULL)
    return G_SOURCE_REMOVE;

  text = format_elapsed ("Working", working_seconds (self));
  gtk_label_set_label (self->working_label, text);

  return G_SOURCE_CONTINUE;
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
  if (!working)
    {
      g_clear_handle_id (&self->working_timer, g_source_remove);
      self->working_label = NULL;

      if (self->working_row != NULL)
        gtk_box_remove (self->transcript, self->working_row);
      self->working_row = NULL;
      return;
    }

  if (self->working_row != NULL)
    {
      update_working_label (self);
      return;
    }

  {
    GtkWidget *dots = GTK_WIDGET (xd_dots_new ());

    self->working_row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 4);
    self->working_label = GTK_LABEL (gtk_label_new ("Working for 0s"));
    gtk_label_set_xalign (self->working_label, 0.0f);
    gtk_widget_add_css_class (GTK_WIDGET (self->working_label), "caption");
    gtk_widget_add_css_class (GTK_WIDGET (self->working_label), "dim-label");
    gtk_widget_add_css_class (dots, "caption");
    gtk_widget_add_css_class (dots, "dim-label");
    gtk_box_append (GTK_BOX (self->working_row),
                    GTK_WIDGET (self->working_label));
    gtk_box_append (GTK_BOX (self->working_row), dots);
  }

  gtk_widget_set_halign (self->working_row, GTK_ALIGN_START);
  gtk_widget_set_margin_start (self->working_row, 24);
  gtk_widget_set_margin_top (self->working_row, 6);

  gtk_box_append (self->transcript, self->working_row);
  update_working_label (self);
  self->working_timer = g_timeout_add_seconds (1, update_working_label, self);
}

static void
on_load_earlier_clicked (GtkButton *button,
                         gpointer   user_data)
{
  XdChatView *self = user_data;
  GtkAdjustment *adjustment =
    gtk_scrolled_window_get_vadjustment (self->scroller);

  /*
   * Rebuilding with older rows increases the range above the current view.
   * Preserve its distance from the bottom while GTK settles the new layout.
   */
  self->follow_bottom = FALSE;
  self->history_bottom_distance =
    gtk_adjustment_get_upper (adjustment) -
    gtk_adjustment_get_value (adjustment);
  self->transcript_limit =
    MIN (G_MAXUINT - TRANSCRIPT_PAGE_SIZE, self->transcript_limit) +
    TRANSCRIPT_PAGE_SIZE;

  if (self->remote != NULL)
    {
      self->remote_rendered_message_id = -1;
      load_remote_transcript (self);
    }
  else
    load_transcript (self);
}

static void
append_history_button (XdChatView *self,
                       guint       hidden)
{
  guint count = MIN (hidden, TRANSCRIPT_PAGE_SIZE);
  g_autofree char *label = g_strdup_printf (
    "Load %u earlier message%s", count, count == 1 ? "" : "s");
  GtkWidget *button = gtk_button_new_with_label (label);

  gtk_widget_set_halign (button, GTK_ALIGN_CENTER);
  gtk_widget_set_margin_bottom (button, 8);
  gtk_widget_add_css_class (button, "flat");
  gtk_widget_add_css_class (button, "pill");
  g_signal_connect (button, "clicked",
                    G_CALLBACK (on_load_earlier_clicked), self);
  gtk_box_append (self->transcript, button);
}

/*
 * Draws the recent conversation, oldest first.
 *
 * GtkBox does not virtualize. Rendering an unbounded tool-heavy transcript
 * created thousands of widgets before first paint, so older rows are loaded
 * explicitly in fixed pages.
 */
static void
render_transcript (XdChatView *self,
                   GPtrArray  *messages,
                   guint       total)
{
  guint start =
    messages->len > self->transcript_limit
      ? messages->len - self->transcript_limit : 0;
  guint displayed = messages->len - start;
  guint hidden = total > displayed ? total - displayed : 0;

  self->rendering_transcript = TRUE;
  if (hidden > 0)
    append_history_button (self, hidden);

  for (guint i = start; i < messages->len; i++)
    {
      const XdMessage *message = g_ptr_array_index (messages, i);

      /*
       * New turns store their measured duration explicitly. It lives after
       * the turn's output in the database because that is when it becomes
       * known, but belongs above that output on screen.
       *
       * Older transcripts have no duration row, so retain the timestamp
       * estimate for their assistant-only turns.
       */
      if (i > 0 &&
          g_strcmp0 (((XdMessage *) g_ptr_array_index (messages, i - 1))->role,
                     "user") == 0)
        {
          const XdMessage *before = g_ptr_array_index (messages, i - 1);
          gint64 seconds = -1;

          for (guint j = i; j < messages->len; j++)
            {
              const XdMessage *at = g_ptr_array_index (messages, j);
              char *end = NULL;
              gint64 stored;

              if (g_strcmp0 (at->role, "user") == 0)
                break;

              if (g_strcmp0 (at->role, "duration") != 0)
                continue;

              stored = g_ascii_strtoll (at->content, &end, 10);
              if (end != at->content && *end == '\0' && stored >= 0)
                seconds = stored;
              break;
            }

          if (seconds < 0 && g_strcmp0 (message->role, "assistant") == 0)
            {
              const XdMessage *last = message;

              for (guint j = i; j < messages->len; j++)
                {
                  const XdMessage *at = g_ptr_array_index (messages, j);

                  if (g_strcmp0 (at->role, "assistant") != 0)
                    break;
                  last = at;
                }

              seconds = last->created_at - before->created_at;
            }

          if (seconds >= 1)
            gtk_box_append (self->transcript, worked_for_row (seconds));
        }

      if (g_strcmp0 (message->role, "duration") == 0)
        continue;
      else if (g_strcmp0 (message->role, "assistant") == 0)
        append_reply (self, message->content, message->label,
                      reply_is_answerable (messages, i));
      else if (g_strcmp0 (message->role, "tool") == 0)
        show_tool_use (self, message->content);
      else
        append_row (self, xd_message_kind_from_role (message->role), message->content);
    }

  self->rendering_transcript = FALSE;
  keep_working_last (self);
  if (self->follow_bottom)
    queue_scroll_to_bottom (self);
}

static void
load_transcript (XdChatView *self)
{
  g_autoptr (GPtrArray) messages = NULL;
  g_autoptr (GError) error = NULL;
  guint query_limit =
    self->transcript_limit < G_MAXUINT
      ? self->transcript_limit + 1 : self->transcript_limit;
  guint total = 0;

  clear_transcript (self);

  self->rendered_message_id =
    xd_storage_last_message_id (self->storage, xd_node_get_chat_id (self->chat));

  messages = xd_storage_list_recent_messages (
    self->storage, xd_node_get_chat_id (self->chat),
    query_limit, &total, &error);
  if (messages == NULL)
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return;
    }

  render_transcript (self, messages, total);
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
  g_clear_pointer (&self->pending_remote_messages, g_ptr_array_unref);
  self->pending_remote_message_total = (guint) MIN (
    MAX (json_object_get_int_member_with_default (
           reply, "total_messages", messages->len), 0),
    G_MAXUINT);
  self->pending_remote_message_id =
    json_object_get_int_member_with_default (reply, "last_message_id", 0);

  /* A semantic event already drew this exact database revision. Generic file
   * change broadcasts can arrive afterwards; options may still have changed,
   * but rebuilding identical transcript widgets is pure flicker. */
  if (self->pending_remote_message_id != self->remote_rendered_message_id)
    self->pending_remote_messages = g_steal_pointer (&messages);

  /* Draw only after live state arrives too. Clearing between these two network
   * replies exposed a blank transcript every time a turn stopped. */
  load_remote_options (self);
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
  if (self->remote_said == NULL || self->remote_said->len == 0)
    return;

  {
    gsize visible = xd_ask_visible_length (self->remote_said->str);
    g_autofree char *prose = g_strndup (self->remote_said->str, visible);

    g_strchomp (prose);
    if (*prose != '\0')
      {
        XdMessageRow *row = append_row (self, XD_MESSAGE_ASSISTANT, prose);

        xd_message_row_set_source (row, self->remote_label);
      }
  }

  g_string_truncate (self->remote_said, 0);
}

static void
end_remote_turn (XdChatView *self)
{
  self->remote_working = FALSE;
  self->remote_started_at = 0;
  g_clear_pointer (&self->remote_label, g_free);

  if (self->remote_said != NULL)
    g_string_truncate (self->remote_said, 0);
}

static GStrv
commands_from_json (JsonArray *array)
{
  GStrv commands;
  guint length;
  guint written = 0;

  if (array == NULL || (length = json_array_get_length (array)) == 0)
    return NULL;

  commands = g_new0 (char *, length + 1);
  for (guint i = 0; i < length; i++)
    {
      const char *command = json_array_get_string_element (array, i);

      if (command != NULL && *command != '\0')
        commands[written++] = g_strdup (command[0] == '/' ? command + 1 : command);
    }

  if (written == 0)
    g_clear_pointer (&commands, g_strfreev);

  return commands;
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

  if (g_strcmp0 (name, "commands") == 0 &&
      json_object_has_member (event, "commands"))
    {
      g_auto (GStrv) commands =
        commands_from_json (json_object_get_array_member (event, "commands"));
      g_autofree char *scope = command_scope (
        self, json_object_get_string_member_with_default (
                event, "backend", NULL));

      store_commands (
        self, scope, (const char *const *) commands);
      return;
    }

  if (g_strcmp0 (name, "turn-started") == 0)
    {
      /* Started here or on another device -- there is no difference to draw. */
      load_remote_transcript (self);
      self->remote_working = TRUE;
      self->remote_started_at = g_get_monotonic_time ();
      g_free (self->remote_label);
      self->remote_label =
        g_strdup (json_object_get_string_member_with_default (event, "label", NULL));

      set_working (self, TRUE);
      update_send_button (self);
      return;
    }

  if (g_strcmp0 (name, "queued") == 0)
    {
      set_queued_text (self, text);

      /*
       * A send can race the daemon starting a turn on another device. The
       * immediate row belongs in the queue in that case, so replace it with
       * the daemon's authoritative transcript as soon as that is known.
       */
      for (GtkWidget *child = gtk_widget_get_first_child (
             GTK_WIDGET (self->transcript));
           child != NULL;
           child = gtk_widget_get_next_sibling (child))
        {
          if (g_object_get_data (G_OBJECT (child),
                                 "xd-optimistic-remote") != NULL)
            {
              self->remote_rendered_message_id = -1;
              load_remote_transcript (self);
              break;
            }
        }
      return;
    }

  if (g_strcmp0 (name, "text") == 0 && text != NULL)
    {
      if (self->remote_said == NULL)
        self->remote_said = g_string_new (NULL);

      g_string_append (self->remote_said, text);
      return;
    }

  if (g_strcmp0 (name, "tool") == 0 && text != NULL)
    {
      close_remote_segment (self);
      show_tool_use (self, text);
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
                       const XdChat *chat,
                       gboolean      has_messages,
                       gboolean      linked_worktree,
                       GPtrArray    *worktrees)
{
  g_autofree char *base_description = describe_context (chat->workdir);
  g_autofree char *description =
    chat->new_worktree
      ? g_strdup_printf ("New worktree from %s", base_description)
      : g_strdup (base_description);
  gboolean have_workdir = chat->workdir != NULL && *chat->workdir != '\0';
  g_autofree char *tooltip =
    have_workdir ? g_strdup_printf ("Terminal on %s in %s",
                                    xd_remote_client_get_host (self->remote),
                                    chat->workdir) : NULL;

  gtk_label_set_label (self->context_label, description);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->context_label), description);
  update_workspace_choice (
    self, chat, chat->workdir, has_messages, linked_worktree, worktrees);
  xd_terminal_panel_set_workdir (self->terminal,
                                 have_workdir ? chat->workdir : NULL);
  xd_file_pane_set_workdir (self->files,
                            have_workdir ? chat->workdir : NULL);
  xd_diff_pane_set_workdir (self->diff,
                            have_workdir ? chat->workdir : NULL);
  gtk_widget_set_sensitive (GTK_WIDGET (self->terminal_button), have_workdir);
  gtk_widget_set_sensitive (GTK_WIDGET (self->file_button), have_workdir);
  gtk_widget_set_sensitive (GTK_WIDGET (self->diff_button), have_workdir);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->terminal_button),
                               have_workdir ? tooltip
                                            : "This chat has no working directory");
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->diff_button),
                               have_workdir ? "Changed files"
                                            : "This chat has no working directory");
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->file_button),
                               have_workdir ? "Browse files"
                                            : "This chat has no working directory");
  update_context_meter (self, chat->context_used, chat->context_window);

  xd_model_picker_set_selected (self->model_picker, chat->backend, chat->model);

  self->syncing_run_options = TRUE;

  for (guint i = 0; i < G_N_ELEMENTS (effort_choices); i++)
    {
      if (effort_choices[i] == effort_for (chat))
        xd_option_picker_set_selected (self->effort_chooser, i);
    }

  for (guint i = 0; i < G_N_ELEMENTS (access_choices); i++)
    {
      if (access_choices[i] == ai_access_from_string (chat->access))
        xd_option_picker_set_selected (self->access_chooser, i);
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
  g_autoptr (GPtrArray) worktrees =
    g_ptr_array_new_with_free_func ((GDestroyNotify) xd_worktree_info_free);
  XdChat chat = { 0 };

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (reply == NULL || self->chat == NULL || self->remote == NULL)
    {
      g_clear_pointer (&self->pending_remote_messages, g_ptr_array_unref);
      return;
    }

  /* Borrowed from the reply, which outlives this call: nothing here is kept. */
  chat.backend = (char *) member_string (reply, "backend", NULL);
  chat.model = (char *) member_string (reply, "model", NULL);
  chat.effort = (char *) member_string (reply, "effort", NULL);
  chat.access = (char *) member_string (reply, "access", NULL);
  chat.workdir = (char *) member_string (reply, "workdir", NULL);
  chat.queued = (char *) member_string (reply, "queued", NULL);
  chat.plan = json_object_get_boolean_member_with_default (reply, "plan", FALSE);
  chat.new_worktree =
    json_object_get_boolean_member_with_default (reply, "new_worktree", FALSE);
  chat.context_used =
    MAX (json_object_get_int_member_with_default (reply, "context_used", 0), 0);
  chat.context_window =
    MAX (json_object_get_int_member_with_default (reply, "context_window", 0), 0);

  use_command_scope (self, chat.backend);
  if (json_object_has_member (reply, "commands"))
    {
      g_auto (GStrv) commands =
        commands_from_json (json_object_get_array_member (reply, "commands"));

      store_commands (
        self, self->command_scope, (const char *const *) commands);
    }

  if (json_object_has_member (reply, "worktrees"))
    {
      JsonArray *rows = json_object_get_array_member (reply, "worktrees");

      for (guint i = 0; i < json_array_get_length (rows); i++)
        {
          JsonObject *row = json_array_get_object_element (rows, i);
          const char *path = member_string (row, "path", NULL);
          XdWorktreeInfo *item;

          if (path == NULL)
            continue;

          item = g_new0 (XdWorktreeInfo, 1);
          item->path = g_strdup (path);
          item->branch = g_strdup (member_string (row, "branch", NULL));
          item->detached = json_object_get_boolean_member_with_default (
            row, "detached", FALSE);
          item->main = json_object_get_boolean_member_with_default (
            row, "main", FALSE);
          item->current = json_object_get_boolean_member_with_default (
            row, "current", FALSE);
          g_ptr_array_add (worktrees, item);
        }
    }

  /*
   * Replace the old view as one main-loop operation.
   *
   * The messages and the in-flight turn are two requests because either can
   * change independently. Holding the first answer until the second arrives
   * means GTK never paints the empty state between them.
   */
  if (self->pending_remote_messages != NULL)
    {
      begin_bottom_jump (self);
      clear_transcript (self);
      end_remote_turn (self);
      render_transcript (self, self->pending_remote_messages,
                         self->pending_remote_message_total);
      g_clear_pointer (&self->pending_remote_messages, g_ptr_array_unref);
      self->remote_rendered_message_id = self->pending_remote_message_id;
    }

  update_remote_options (
    self, &chat,
    json_object_get_boolean_member_with_default (reply, "has_messages", FALSE),
    json_object_get_boolean_member_with_default (reply, "linked_worktree", FALSE),
    worktrees);
  if (self->restore_remote_panes)
    {
      apply_panes (self, saved_panes (self, PANE_NONE));
      self->restore_remote_panes = FALSE;
    }
  set_queued_text (self, chat.queued);

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
      const char *segment = member_string (reply, "segment", NULL);
      JsonArray *items = json_object_has_member (reply, "items")
        ? json_object_get_array_member (reply, "items") : NULL;
      gint64 elapsed =
        json_object_get_int_member_with_default (reply, "working_for", 0);

      self->remote_working = TRUE;
      self->remote_started_at =
        g_get_monotonic_time () - MAX (elapsed, 0) * G_USEC_PER_SEC;

      g_free (self->remote_label);
      self->remote_label = g_strdup (member_string (reply, "label", NULL));

      set_working (self, TRUE);

      /* Replayed in order: what it said, and what it reached for in between.
       * The same turn the device that started it is looking at. */
      for (guint i = 0; items != NULL && i < json_array_get_length (items); i++)
        {
          JsonObject *item = json_array_get_object_element (items, i);
          const char *text = member_string (item, "text", "");

          if (json_object_get_boolean_member_with_default (item, "tool", FALSE))
            {
              show_tool_use (self, text);
            }
          else
            {
              XdMessageRow *row = append_row (self, XD_MESSAGE_ASSISTANT, text);

              xd_message_row_set_source (row, self->remote_label);
            }
        }

      /* And what it is in the middle of saying, which the deltas continue. */
      if (segment != NULL && *segment != '\0')
        {
          if (self->remote_said == NULL)
            self->remote_said = g_string_new (NULL);

          g_string_assign (self->remote_said, segment);
        }

      queue_scroll_to_bottom (self);
    }

  queue_scroll_to_bottom (self);
  update_send_button (self);
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

  load_remote_transcript (self);
  xd_file_pane_refresh (self->files);
  xd_diff_pane_refresh (self->diff);
  update_send_button (self);
}

static void
use_chat_node (XdChatView *self,
               XdNode     *chat)
{
  if (self->chat == chat)
    return;

  if (self->chat != NULL)
    xd_node_set_active (self->chat, FALSE);

  g_set_object (&self->chat, chat);

  if (self->chat != NULL)
    xd_node_set_active (self->chat, TRUE);
}

/* Connecting is what makes a turn on the daemon visible here: the events are
 * the same for every device watching. */
static void
set_remote (XdChatView     *self,
            XdRemoteClient *client)
{
  if (self->remote == client)
    {
      xd_diff_pane_set_remote (
        self->diff, client,
        client != NULL && self->chat != NULL
          ? xd_node_get_chat_id (self->chat) : NULL);
      xd_file_pane_set_remote (
        self->files, client,
        client != NULL && self->chat != NULL
          ? xd_node_get_chat_id (self->chat) : NULL);
      return;
    }

  if (self->remote != NULL)
    g_signal_handlers_disconnect_by_data (self->remote, self);

  g_set_object (&self->remote, client);
  if (client != NULL)
    xd_git_head_watch_set_workdir (self->git_head_watch, NULL);
  xd_terminal_panel_set_remote (self->terminal, client);
  xd_diff_pane_set_remote (
    self->diff, client,
    client != NULL && self->chat != NULL
      ? xd_node_get_chat_id (self->chat) : NULL);
  xd_file_pane_set_remote (
    self->files, client,
    client != NULL && self->chat != NULL
      ? xd_node_get_chat_id (self->chat) : NULL);

  if (client != NULL)
    {
      g_signal_connect (client, "event", G_CALLBACK (on_remote_event), self);
      g_signal_connect (client, "opened", G_CALLBACK (on_remote_opened), self);
    }
}

/*
 * Reads the chat back: what has been stored, then what is happening now.
 *
 * Always both, and in that order. A turn is only written down when it ends, so
 * the stored messages are half the picture while one is running -- and every
 * reason to reread the transcript (coming back to the chat, a change made
 * elsewhere, a turn ending) is equally a reason to ask again what the chat is
 * doing. Asking for one without the other is how a reply in progress
 * disappeared: the messages arrived, and nothing put the live part back.
 */
static void
load_remote_transcript (XdChatView *self)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;
  guint query_limit =
    self->transcript_limit < G_MAXUINT
      ? self->transcript_limit + 1 : self->transcript_limit;

  g_cancellable_cancel (self->fetching);
  g_clear_object (&self->fetching);
  g_clear_pointer (&self->pending_remote_messages, g_ptr_array_unref);
  self->fetching = g_cancellable_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "messages");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, xd_node_get_chat_id (self->chat));
  json_builder_set_member_name (builder, "limit");
  json_builder_add_int_value (builder, query_limit);
  json_builder_end_object (builder);
  request = json_builder_get_root (builder);

  xd_remote_client_call_async (self->remote, request,
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
  g_autoptr (XdChat) chat = NULL;
  const char *chat_id;

  if (self->chat == NULL || self->remote != NULL)
    return;

  chat_id = xd_node_get_chat_id (self->chat);
  chat = xd_storage_get_chat (self->storage, chat_id, NULL);
  if (chat != NULL)
    set_queued_text (self, chat->queued);

  if (current_turn (self) != NULL)
    return;

  if (chat != NULL)
    update_context_bar (self, chat);

  /* Another window may have queued this while this one owned the turn. Once
   * that turn is gone, the persisted instruction still runs. */
  if (self->queued != NULL)
    {
      send_queued (self);
      return;
    }

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
turn_item_free (TurnItem *item)
{
  g_free (item->text);
  g_free (item);
}

static void
remember_turn_item (Turn       *turn,
                    gboolean    tool,
                    const char *text)
{
  TurnItem *item = g_new0 (TurnItem, 1);

  item->tool = tool;
  item->text = g_strdup (text);
  g_ptr_array_add (turn->items, item);
}

static void
turn_free (gpointer data)
{
  Turn *turn = data;

  g_clear_object (&turn->session);
  g_clear_pointer (&turn->chat_id, g_free);
  g_clear_pointer (&turn->backend_id, g_free);
  g_clear_pointer (&turn->model_id, g_free);
  g_clear_pointer (&turn->prompt, g_free);
  g_clear_pointer (&turn->workdir, g_free);
  g_clear_pointer (&turn->diff_tracker, xd_git_diff_tracker_free);
  g_clear_pointer (&turn->label, g_free);
  g_clear_object (&turn->node);
  if (turn->anchor != NULL)
    g_object_remove_weak_pointer (G_OBJECT (turn->anchor),
                                  (gpointer *) &turn->anchor);
  g_string_free (turn->text, TRUE);
  g_string_free (turn->segment, TRUE);
  g_clear_pointer (&turn->items, g_ptr_array_unref);
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

static char *
command_scope (XdChatView *self,
               const char *backend_id)
{
  if (backend_id == NULL)
    return NULL;

  return self->remote != NULL
    ? g_strdup_printf ("remote:%s:%s",
                       xd_remote_client_get_host (self->remote), backend_id)
    : g_strdup_printf ("local:%s", backend_id);
}

static void
use_command_scope (XdChatView *self,
                   const char *backend_id)
{
  g_autofree char *scope = command_scope (self, backend_id);

  if (g_strcmp0 (self->command_scope, scope) == 0)
    return;

  g_free (self->command_scope);
  self->command_scope = g_steal_pointer (&scope);
  refresh_command_suggestions (self);
}

static void
store_commands (XdChatView       *self,
                const char       *scope,
                const char *const *commands)
{
  if (scope == NULL || commands == NULL)
    return;

  g_hash_table_replace (self->command_sets, g_strdup (scope),
                        g_strdupv ((char **) commands));

  if (g_strcmp0 (scope, self->command_scope) == 0)
    refresh_command_suggestions (self);
}

static void
on_commands (XdChatSession    *session,
             const char *const *commands,
             gpointer          user_data)
{
  Turn *turn = user_data;
  g_autofree char *scope =
    g_strdup_printf ("local:%s", turn->backend_id);

  store_commands (turn->view, scope, commands);
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
   * The text is not shown, or allocated a row, as it arrives.
   *
   * A message half-written reflows on every token, and Markdown read
   * character by character renders as its own source until the syntax closes.
   * The turn-level working marker already shows progress; a blank message row
   * would only reserve unexplained space until this segment is complete.
   */
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
close_segment (Turn     *turn,
               gboolean  answerable)
{
  if (turn->segment->len == 0)
    return;

  /* Held rather than written until the turn ends, alongside its tool calls,
   * so interruption can store the exact order that happened. */
  remember_turn_item (turn, FALSE, turn->segment->str);

  if (turn_is_visible (turn))
    {
      g_autoptr (XdAsk) ask = answerable
        ? xd_ask_parse (turn->segment->str, NULL) : NULL;

      if (ask != NULL)
        {
          append_reply (turn->view, turn->segment->str, turn->label, TRUE);
        }
      else
        {
          /* Hide a question block until the turn finishes and it can become
           * buttons, rather than briefly showing its machine-facing markup. */
          gsize visible = xd_ask_visible_length (turn->segment->str);
          g_autofree char *prose = g_strndup (turn->segment->str, visible);

          g_strchomp (prose);
          if (*prose != '\0')
            {
              XdMessageRow *row =
                append_row (turn->view, XD_MESSAGE_ASSISTANT, prose);

              xd_message_row_set_source (row, turn->label);
            }
        }
    }

  g_string_truncate (turn->segment, 0);
}

/* Writes everything the turn produced, in the order it happened. */
static void
store_turn_items (Turn *turn)
{
  for (guint i = 0; i < turn->items->len; i++)
    {
      TurnItem *item = g_ptr_array_index (turn->items, i);
      g_autoptr (GError) error = NULL;

      if (!xd_storage_append_message (turn->view->storage, turn->chat_id,
                                      item->tool ? "tool" : "assistant",
                                      item->text, NULL,
                                      item->tool ? NULL : turn->label, &error))
        g_warning ("cannot store turn output: %s", error->message);
    }

  g_ptr_array_set_size (turn->items, 0);
}

static void
store_turn_duration (Turn   *turn,
                     gint64  seconds)
{
  g_autofree char *content = g_strdup_printf ("%" G_GINT64_FORMAT, seconds);
  g_autoptr (GError) error = NULL;

  if (!xd_storage_append_message (turn->view->storage, turn->chat_id,
                                  "duration", content, NULL, NULL, &error))
    g_warning ("cannot store turn duration: %s", error->message);
}

static void
on_tool_use (XdChatSession *session,
             const char    *name,
             gpointer       user_data)
{
  Turn *turn = user_data;
  g_autofree char *diff =
    xd_git_diff_tracker_capture (turn->diff_tracker, name);
  g_autofree char *tool =
    xd_workflow_run_capture_tool (diff, turn->workdir);

  close_segment (turn, FALSE);
  remember_turn_item (turn, TRUE, tool);
  turn->had_tool = TRUE;

  if (turn_is_visible (turn))
    show_tool_use (turn->view, tool);
}

static void
on_usage (XdChatSession *session,
          guint64        used,
          guint64        window,
          gpointer       user_data)
{
  Turn *turn = user_data;

  /* Step-based backends report this more than once. Latest step is current
   * context, not a sum of every request made during the turn. */
  turn->context_used = used;
  turn->context_window = window;
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
  gboolean asked_user;
  gint64 seconds =
    MAX ((g_get_monotonic_time () - turn->started_at) / G_USEC_PER_SEC, 0);

  /*
   * A resumed turn that failed without producing a single token is almost
   * always a session the CLI no longer has -- they are cleaned up over time.
   * Forget it and run the same message again from the transcript instead of
   * making the user retype it. Checked by outcome rather than by matching the
   * CLI's error text, which neither CLI promises to keep stable.
   */
  if (!success && turn->resumed && !turn->is_retry &&
      turn->text->len == 0 && !turn->had_tool)
    {
      retry_prompt = g_strdup (turn->prompt);

      if (!xd_storage_set_session_id (self->storage, chat_id, turn->backend_id,
                                      NULL, &error))
        g_warning ("cannot forget the stale session: %s", error->message);

      if (visible)
        update_context_meter (self, 0, 0);

      /* start_turn() marks it working again; without this the row would sit
       * idle for as long as the retry takes. */
      xd_node_set_state (turn->node, XD_NODE_IDLE);

      g_hash_table_remove (self->turns, chat_id);

      if (visible)
        start_turn (self, retry_prompt);
      return;
    }


  /* Nothing came back at all: say so rather than leaving only the timer. */
  if (turn->text->len == 0 && success && visible)
    {
      XdMessageRow *row =
        append_row (self, XD_MESSAGE_ASSISTANT, "(no reply)");

      xd_message_row_set_source (row, turn->label);
    }

  /* A chat that asked something waits even when it is not on screen. */
  {
    g_autoptr (XdAsk) asked = xd_ask_parse (turn->segment->str, NULL);

    asked_user = asked != NULL;
    xd_node_set_state (
      turn->node,
      asked_user ? XD_NODE_WAITING
      : xd_node_is_active (turn->node) ? XD_NODE_IDLE
                                       : XD_NODE_DONE);
  }

  /* Whatever was still being written when the turn ended is a message like
   * any other. Only now, with all of them final, do they reach the database.
   */
  close_segment (turn, asked_user);
  store_turn_items (turn);
  store_turn_duration (turn, seconds);

  if (turn->context_used > 0 && turn->context_window > 0)
    {
      g_autoptr (GError) context_error = NULL;

      if (!xd_storage_set_context_usage (
            self->storage, chat_id, turn->backend_id, turn->model_id,
            turn->context_used, turn->context_window, &context_error))
        g_warning ("cannot store context usage: %s", context_error->message);
      else if (visible)
        update_context_meter (self, turn->context_used, turn->context_window);
    }

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

  /* The live rows already are this finished turn. When the coalesced database
   * notification arrives, do not clear and reconstruct the same transcript. */
  if (visible)
    self->rendered_message_id =
      xd_storage_last_message_id (self->storage, chat_id);

  /* Frees the turn, so nothing may touch it afterwards. */
  g_hash_table_remove (self->turns, chat_id);

  if (visible)
    update_send_button (self);

  /* Whatever was typed while it was working is what to do next. */
  if (visible && self->queued != NULL)
    send_queued (self);
}

/* --- sending -------------------------------------------------------------- */

static void
on_command_clicked (GtkButton *button,
                    gpointer   user_data)
{
  XdChatView *self = user_data;
  const char *command =
    g_object_get_data (G_OBJECT (button), "xd-agent-command");
  g_autofree char *text = NULL;

  if (command == NULL)
    return;

  text = g_strdup_printf ("/%s ", command);
  gtk_text_buffer_set_text (
    gtk_text_view_get_buffer (self->composer), text, -1);
  gtk_text_view_set_cursor_visible (self->composer, TRUE);
  gtk_widget_grab_focus (GTK_WIDGET (self->composer));
}

static void
refresh_command_suggestions (XdChatView *self)
{
  GtkTextBuffer *buffer;
  GtkTextIter start;
  GtkTextIter end;
  g_autofree char *text = NULL;
  const char *query;
  GStrv commands;
  guint matches = 0;

  if (self->commands_bar == NULL || self->commands_flow == NULL ||
      self->composer == NULL)
    return;

  for (GtkWidget *child =
         gtk_widget_get_first_child (GTK_WIDGET (self->commands_flow));
       child != NULL;
       child = gtk_widget_get_first_child (GTK_WIDGET (self->commands_flow)))
    gtk_flow_box_remove (self->commands_flow, child);

  buffer = gtk_text_view_get_buffer (self->composer);
  gtk_text_buffer_get_bounds (buffer, &start, &end);
  text = gtk_text_buffer_get_text (buffer, &start, &end, FALSE);

  if (text[0] != '/' || self->command_scope == NULL)
    {
      gtk_widget_set_visible (self->commands_bar, FALSE);
      return;
    }

  query = text + 1;
  for (const char *at = query; *at != '\0'; at++)
    {
      if (g_ascii_isspace (*at))
        {
          gtk_widget_set_visible (self->commands_bar, FALSE);
          return;
        }
    }

  commands = g_hash_table_lookup (self->command_sets, self->command_scope);
  for (guint i = 0; commands != NULL && commands[i] != NULL; i++)
    {
      GtkWidget *button;
      g_autofree char *label = NULL;

      if (*query != '\0' &&
          g_ascii_strncasecmp (commands[i], query, strlen (query)) != 0)
        continue;

      label = g_strdup_printf ("/%s", commands[i]);
      button = gtk_button_new_with_label (label);
      gtk_widget_add_css_class (button, "flat");
      gtk_widget_set_halign (button, GTK_ALIGN_FILL);
      g_object_set_data_full (G_OBJECT (button), "xd-agent-command",
                              g_strdup (commands[i]), g_free);
      g_signal_connect (button, "clicked",
                        G_CALLBACK (on_command_clicked), self);
      gtk_flow_box_append (self->commands_flow, button);
      matches++;
    }

  gtk_widget_set_visible (self->commands_bar, matches > 0);
}

static void
on_composer_changed (GtkTextBuffer *buffer,
                     gpointer       user_data)
{
  refresh_command_suggestions (user_data);
}

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

  full_prompt = xd_handover_join (handover, prompt);

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
  turn->items =
    g_ptr_array_new_with_free_func ((GDestroyNotify) turn_item_free);
  turn->session = xd_chat_session_new (backend);
  /* Taken now rather than when the reply lands: the model can be changed
   * while the agent is still working, and what answered is whatever was
   * running when the turn started. */
  turn->label = reply_title (chat);

  g_signal_connect (turn->session, "session-started",
                    G_CALLBACK (on_session_started), turn);
  g_signal_connect (turn->session, "commands",
                    G_CALLBACK (on_commands), turn);
  g_signal_connect (turn->session, "text-delta",
                    G_CALLBACK (on_text_delta), turn);
  g_signal_connect (turn->session, "tool-use",
                    G_CALLBACK (on_tool_use), turn);
  g_signal_connect (turn->session, "usage",
                    G_CALLBACK (on_usage), turn);
  g_signal_connect (turn->session, "finished",
                    G_CALLBACK (on_turn_finished), turn);

  g_hash_table_insert (self->turns, g_strdup (chat->id), turn);
  set_working (self, TRUE);

  /* Resolved per turn rather than at creation, so editing a folder's
   * instructions or model takes effect on the next message instead of only on
   * chats made afterwards. */
  resolved = xd_settings_resolve (xd_node_get_parent (self->chat), chat->backend);

  spec.prompt = full_prompt;
  spec.workdir = workdir_for (chat, resolved);
  turn->workdir = g_strdup (spec.workdir);
  turn->diff_tracker = xd_git_diff_tracker_new (turn->workdir);
  /* The chat's own pick wins; the folder chain is the fallback. */
  spec.model = chat->model != NULL ? chat->model : resolved->model;
  turn->model_id =
    g_strdup (spec.model != NULL ? spec.model : backend->default_model);
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

static gboolean
prepare_new_worktree (XdChatView *self,
                      XdChat     *chat,
                      const char *prompt,
                      GError    **error)
{
  g_autoptr (XdEffectiveSettings) resolved = NULL;
  g_autofree char *worktree = NULL;
  g_autofree char *name = NULL;

  if (!chat->new_worktree)
    return TRUE;

  resolved = xd_settings_resolve (xd_node_get_parent (self->chat), chat->backend);
  name = xd_chat_title_from_prompt (prompt);
  worktree = xd_worktree_create (
    workdir_for (chat, resolved), chat->id, name, error);
  if (worktree == NULL)
    return FALSE;

  if (!xd_storage_use_worktree (
        self->storage, chat->id, worktree, error))
    return FALSE;

  g_free (chat->workdir);
  chat->workdir = g_steal_pointer (&worktree);
  chat->new_worktree = FALSE;
  update_context_bar (self, chat);

  return TRUE;
}

static gboolean
send_message (XdChatView *self,
              const char *text)
{
  g_autoptr (XdChat) chat = NULL;
  g_autoptr (GError) error = NULL;

  if (self->chat == NULL || text == NULL || *text == '\0')
    return FALSE;

  /* One turn at a time per chat; the button is a stop button meanwhile. */
  if (current_turn (self) != NULL)
    return FALSE;

  chat = xd_storage_get_chat (
    self->storage, xd_node_get_chat_id (self->chat), &error);
  if (chat == NULL || !prepare_new_worktree (self, chat, text, &error))
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return FALSE;
    }

  if (!xd_storage_append_message (self->storage, xd_node_get_chat_id (self->chat),
                                  "user", text, NULL, NULL, &error))
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return FALSE;
    }

  gtk_widget_set_sensitive (GTK_WIDGET (self->workspace_chooser), FALSE);
  retire_open_questions (self);
  xd_node_set_state (self->chat, XD_NODE_IDLE);
  begin_bottom_jump (self);
  append_row (self, XD_MESSAGE_USER, text);
  name_chat_after_first_message (self, text);
  xd_fs_tree_bump_chat (self->tree, self->chat);

  start_turn (self, text);

  return TRUE;
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
set_queued_text (XdChatView *self,
                 const char *text)
{
  g_free (self->queued);
  self->queued = g_strdup (text);
  show_queued (self);
}

static void
on_remote_queue_set (GObject      *source,
                     GAsyncResult *result,
                     gpointer      user_data)
{
  g_autoptr (XdChatView) self = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (reply != NULL || g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    return;

  append_row (self, XD_MESSAGE_ERROR, error->message);

  /* Optimistic display failed; read daemon's persisted value back. */
  if (self->remote != NULL && self->chat != NULL)
    load_remote_options (self);
}

static void
set_remote_queue (XdChatView *self,
                  const char *text)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, text != NULL ? "queue" : "drop-queue");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, xd_node_get_chat_id (self->chat));
  if (text != NULL)
    {
      json_builder_set_member_name (builder, "text");
      json_builder_add_string_value (builder, text);
    }
  json_builder_end_object (builder);

  request = json_builder_get_root (builder);
  xd_remote_client_call_async (self->remote, request, NULL,
                               on_remote_queue_set, g_object_ref (self));
}

static gboolean
set_local_queue (XdChatView *self,
                 const char *text)
{
  g_autoptr (GError) error = NULL;

  if (!xd_storage_set_queued (self->storage,
                              xd_node_get_chat_id (self->chat), text, &error))
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return FALSE;
    }

  return TRUE;
}

static void
queue_message (XdChatView *self,
               const char *text)
{
  /* A second message replaces the first rather than piling up: what is meant
   * is the latest instruction, not a list of them to be answered in turn. */
  if (self->remote != NULL)
    set_remote_queue (self, text);
  else if (!set_local_queue (self, text))
    return;

  set_queued_text (self, text);
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
  g_autofree char *text = g_strdup (self->queued);

  if (text == NULL || !set_local_queue (self, NULL))
    return;

  set_queued_text (self, NULL);
  if (!send_message (self, text))
    {
      set_local_queue (self, text);
      set_queued_text (self, text);
    }
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

  /*
   * Always ask the daemon for a remote chat. Its working event may be late or
   * may have been missed, while the queued instruction is already persisted
   * there. Cancel is idempotent, and the daemon also promotes an idle queue.
   */
  if (self->remote != NULL)
    cancel_remote_turn (self);
  else if (turn != NULL)
    xd_chat_session_cancel (turn->session);
  else
    send_queued (self);
  /* The queued text goes out when the turn reports that it has stopped. */
}

static void
on_queued_dropped (GtkButton *button,
                   gpointer   user_data)
{
  XdChatView *self = user_data;

  if (self->remote != NULL)
    set_remote_queue (self, NULL);
  else if (!set_local_queue (self, NULL))
    return;

  set_queued_text (self, NULL);
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

static void
remote_send_free (RemoteSend *send)
{
  g_clear_object (&send->view);
  g_clear_object (&send->row);
  g_free (send->chat_id);
  g_free (send);
}

static void
on_remote_message_sent (GObject      *source,
                        GAsyncResult *result,
                        gpointer      user_data)
{
  RemoteSend *send = user_data;
  XdChatView *self = send->view;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);

  if (reply == NULL &&
      !g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED) &&
      self->remote == XD_REMOTE_CLIENT (source) &&
      self->chat != NULL &&
      g_strcmp0 (send->chat_id, xd_node_get_chat_id (self->chat)) == 0)
    {
      if (gtk_widget_get_parent (GTK_WIDGET (send->row)) ==
          GTK_WIDGET (self->transcript))
        gtk_box_remove (self->transcript, GTK_WIDGET (send->row));

      append_row (self, XD_MESSAGE_ERROR, error->message);
    }

  remote_send_free (send);
}

/*
 * Sends to the daemon, which is where the agent runs.
 *
 * The daemon remains authoritative, but waiting for its broadcast made a sent
 * message feel lost on a latent connection. Draw a temporary row immediately;
 * the normal transcript reload replaces it with what the daemon stored.
 */
static gboolean
send_remote_message (XdChatView *self,
                     const char *text)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;
  g_autoptr (GString) display = g_string_new (text != NULL ? text : "");
  RemoteSend *send;
  gsize total = 0;

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "send");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, xd_node_get_chat_id (self->chat));
  json_builder_set_member_name (builder, "text");
  json_builder_add_string_value (builder, text != NULL ? text : "");

  if (self->attachments->len > 0)
    {
      if (self->attachments->len > XD_REMOTE_MAX_IMAGES)
        {
          append_row (self, XD_MESSAGE_ERROR,
                      "A remote message can contain at most 4 images.");
          return FALSE;
        }

      json_builder_set_member_name (builder, "attachments");
      json_builder_begin_array (builder);

      for (guint i = 0; i < self->attachments->len; i++)
        {
          const char *path = g_ptr_array_index (self->attachments, i);
          g_autofree char *contents = NULL;
          g_autofree char *encoded = NULL;
          g_autofree char *name = g_path_get_basename (path);
          gsize length = 0;

          if (!g_file_get_contents (path, &contents, &length, NULL))
            {
              append_row (self, XD_MESSAGE_ERROR,
                          "Cannot read a pasted image for the remote machine.");
              return FALSE;
            }

          if (length > XD_REMOTE_MAX_IMAGE_BYTES ||
              total > XD_REMOTE_MAX_IMAGES_BYTES - length)
            {
              append_row (self, XD_MESSAGE_ERROR,
                          "The pasted images are too large to send remotely.");
              return FALSE;
            }
          total += length;
          encoded = g_base64_encode ((const guchar *) contents, length);

          json_builder_begin_object (builder);
          json_builder_set_member_name (builder, "name");
          json_builder_add_string_value (builder, name);
          json_builder_set_member_name (builder, "mime");
          json_builder_add_string_value (builder, "image/png");
          json_builder_set_member_name (builder, "data");
          json_builder_add_string_value (builder, encoded);
          json_builder_end_object (builder);

          if (display->len > 0)
            g_string_append_c (display, '\n');
          g_string_append_printf (display, "Image #%u", i + 1);
        }

      json_builder_end_array (builder);
    }

  json_builder_end_object (builder);

  request = json_builder_get_root (builder);

  xd_node_set_state (self->chat, XD_NODE_IDLE);
  begin_bottom_jump (self);
  send = g_new0 (RemoteSend, 1);
  send->view = g_object_ref (self);
  send->chat_id = g_strdup (xd_node_get_chat_id (self->chat));
  send->row = g_object_ref (
    append_row (self, XD_MESSAGE_USER, display->str));
  g_object_set_data (G_OBJECT (send->row), "xd-optimistic-remote",
                     GINT_TO_POINTER (1));

  xd_remote_client_call_async (self->remote, request, NULL,
                               on_remote_message_sent, send);

  if (self->attachments->len > 0)
    forget_attachments (self);

  return TRUE;
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

  /* Enter on an empty composer means "send the instruction already waiting"
   * when there is one. This is the keyboard equivalent of the steer button. */
  if (text == NULL && self->attachments->len == 0 && self->queued != NULL)
    {
      on_steer_clicked (NULL, self);
      return;
    }

  /* A chat on a daemon takes the same composer and the same Enter. */
  if (self->remote != NULL)
    {
      if (text == NULL && self->attachments->len == 0)
        return;

      /*
       * Plain text uses the queue operation while the daemon works. An image
       * has bytes to upload, so it goes through send; the authoritative daemon
       * turns that send into the same single queued instruction.
       */
      if (self->remote_working && self->attachments->len == 0)
        {
          queue_message (self, text);
          return;
        }

      if (!send_remote_message (self, text) && text != NULL)
        gtk_text_buffer_set_text (
          gtk_text_view_get_buffer (self->composer), text, -1);
      return;
    }

  if (self->attachments->len == 0)
    {
      if (text == NULL)
        return;

      /* One turn at a time, so anything typed meanwhile waits for it. */
      if (current_turn (self) != NULL)
        queue_message (self, text);
      else if (!send_message (self, text))
        gtk_text_buffer_set_text (
          gtk_text_view_get_buffer (self->composer), text, -1);

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
  else if (!send_message (self, message->str))
    gtk_text_buffer_set_text (
      gtk_text_view_get_buffer (self->composer), message->str, -1);
}

static void
update_send_button (XdChatView *self)
{
  gboolean running = current_turn (self) != NULL || self->remote_working;

  if (self->send_state_set && self->send_running == running)
    return;

  self->send_state_set = TRUE;
  self->send_running = running;

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

static void
update_workspace_choice (XdChatView   *self,
                         const XdChat *chat,
                         const char   *workdir,
                         gboolean      has_messages,
                         gboolean      linked_worktree,
                         GPtrArray    *worktrees)
{
  g_autoptr (GPtrArray) labels =
    g_ptr_array_new_with_free_func (g_free);
  g_autoptr (GPtrArray) descriptions =
    g_ptr_array_new_with_free_func (g_free);

  self->syncing_workspace = TRUE;

  g_ptr_array_set_size (self->workspace_paths, 0);
  g_ptr_array_add (
    labels, g_strdup (linked_worktree ? "Current worktree"
                                     : "Current checkout"));
  g_ptr_array_add (
    descriptions, g_strdup ("Keep using this chat's current checkout."));
  g_ptr_array_add (self->workspace_paths, NULL);

  g_ptr_array_add (labels, g_strdup ("New worktree"));
  g_ptr_array_add (
    descriptions,
    g_strdup ("Create an isolated branch and checkout for this chat."));
  g_ptr_array_add (self->workspace_paths, NULL);

  for (guint i = 0; worktrees != NULL && i < worktrees->len; i++)
    {
      XdWorktreeInfo *item = g_ptr_array_index (worktrees, i);
      g_autofree char *label = NULL;

      if (item->path == NULL || item->current ||
          xd_worktree_path_equal (item->path, workdir))
        continue;

      if (item->branch != NULL)
        label = item->detached
          ? g_strdup_printf ("Detached at %s", item->branch)
          : g_strdup (item->branch);
      else
        label = g_path_get_basename (item->path);

      g_ptr_array_add (labels, g_steal_pointer (&label));
      g_ptr_array_add (descriptions, g_strdup (item->path));
      g_ptr_array_add (self->workspace_paths, g_strdup (item->path));
    }

  g_ptr_array_add (labels, NULL);
  g_ptr_array_add (descriptions, NULL);
  xd_option_picker_set_choices (
    self->workspace_chooser,
    (const char *const *) labels->pdata,
    (const char *const *) descriptions->pdata);
  xd_option_picker_set_selected (self->workspace_chooser,
                                 chat->new_worktree ? 1 : 0);
  gtk_widget_set_sensitive (GTK_WIDGET (self->workspace_chooser),
                            !has_messages);
  self->syncing_workspace = FALSE;
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

static char *
pane_state_key (XdChatView *self)
{
  const char *chat_id;

  if (self->chat == NULL)
    return NULL;

  chat_id = xd_node_get_chat_id (self->chat);
  if (self->remote == NULL)
    return g_strdup_printf ("local/%s", chat_id);

  return g_strdup_printf (
    "remote/%s:%u/%s",
    xd_remote_client_get_host (self->remote),
    xd_remote_client_get_port (self->remote),
    chat_id);
}

static guint
current_panes (XdChatView *self)
{
  guint state = PANE_NONE;

  if (gtk_toggle_button_get_active (self->terminal_button))
    state |= PANE_TERMINAL;
  if (gtk_toggle_button_get_active (self->file_button))
    state |= PANE_FILES;
  if (gtk_toggle_button_get_active (self->diff_button))
    state |= PANE_DIFF;

  return state;
}

static void
store_panes (XdChatView *self)
{
  g_autofree char *key = pane_state_key (self);
  g_autoptr (GVariant) states = NULL;
  g_autoptr (GVariant) updated = NULL;
  GVariantDict dictionary;

  if (key == NULL)
    return;

  states = g_settings_get_value (self->settings, "pane-state");
  g_variant_dict_init (&dictionary, states);
  g_variant_dict_insert (&dictionary, key, "u", current_panes (self));
  updated = g_variant_dict_end (&dictionary);
  g_settings_set_value (self->settings, "pane-state", updated);
}

static guint
saved_panes (XdChatView *self,
             guint       fallback)
{
  g_autofree char *key = pane_state_key (self);
  g_autoptr (GVariant) states = NULL;
  guint state;

  if (key == NULL)
    return fallback;

  states = g_settings_get_value (self->settings, "pane-state");
  return g_variant_lookup (states, key, "u", &state) ? state : fallback;
}

static void
apply_panes (XdChatView *self,
             guint       state)
{
  if ((state & PANE_FILES) != 0)
    state &= ~PANE_DIFF;

  if (current_panes (self) == state)
    return;

  self->syncing_panes = TRUE;
  gtk_toggle_button_set_active (
    self->terminal_button, (state & PANE_TERMINAL) != 0);

  /* Repository panes share one slot. Clear both first so restoring the same
   * kind after a chat switch still runs its open path against the new chat. */
  gtk_toggle_button_set_active (self->file_button, FALSE);
  gtk_toggle_button_set_active (self->diff_button, FALSE);
  if ((state & PANE_FILES) != 0)
    gtk_toggle_button_set_active (self->file_button, TRUE);
  else if ((state & PANE_DIFF) != 0)
    gtk_toggle_button_set_active (self->diff_button, TRUE);
  self->syncing_panes = FALSE;
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

  store_panes (self);

  if (self->remote != NULL)
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
on_side_dragged (GtkPaned   *paned,
                 GParamSpec *pspec,
                 gpointer    user_data)
{
  XdChatView *self = user_data;
  int width = gtk_widget_get_width (GTK_WIDGET (self->side_stack));

  if (width > 0 && gtk_widget_get_visible (GTK_WIDGET (self->side_stack)))
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
      int width = gtk_widget_get_width (GTK_WIDGET (self->side_stack));

      if (width > 0)
        g_settings_set_int (self->settings, "diff-width", width);

      if (!gtk_toggle_button_get_active (self->file_button))
        gtk_widget_set_visible (GTK_WIDGET (self->side_stack), FALSE);
      return;
    }

  if (gtk_toggle_button_get_active (self->file_button))
    gtk_toggle_button_set_active (self->file_button, FALSE);

  gtk_stack_set_visible_child_name (self->side_stack, "diff");
  gtk_widget_set_visible (GTK_WIDGET (self->side_stack), TRUE);
  set_end_child_size (self->side_split,
                      g_settings_get_int (self->settings, "diff-width"), FALSE);

  xd_diff_pane_refresh (self->diff);
}

static void
on_file_toggled (GtkToggleButton *button,
                 gpointer         user_data)
{
  XdChatView *self = user_data;
  gboolean shown = gtk_toggle_button_get_active (button);

  remember_panes (self);

  if (!shown)
    {
      int width = gtk_widget_get_width (GTK_WIDGET (self->side_stack));

      if (width > 0)
        g_settings_set_int (self->settings, "diff-width", width);

      if (!gtk_toggle_button_get_active (self->diff_button))
        gtk_widget_set_visible (GTK_WIDGET (self->side_stack), FALSE);
      return;
    }

  if (gtk_toggle_button_get_active (self->diff_button))
    gtk_toggle_button_set_active (self->diff_button, FALSE);

  gtk_stack_set_visible_child_name (self->side_stack, "files");
  gtk_widget_set_visible (GTK_WIDGET (self->side_stack), TRUE);
  set_end_child_size (self->side_split,
                      g_settings_get_int (self->settings, "diff-width"), FALSE);
  xd_file_pane_refresh (self->files);
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
  g_autofree char *key =
    g_strdup_printf ("local:%s", xd_node_get_chat_id (chat));
  TranscriptPage *page =
    g_hash_table_lookup (self->transcript_pages, key);

  if (page == self->transcript_page)
    activate_empty_transcript (self);
  remove_transcript_page (self, page);
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

static char *
format_token_count (guint64 tokens)
{
  if (tokens >= 1000000)
    return g_strdup_printf (tokens % 1000000 == 0 ? "%.0fM" : "%.1fM",
                            tokens / 1000000.0);

  if (tokens >= 1000)
    return g_strdup_printf (tokens % 1000 == 0 ? "%.0fk" : "%.1fk",
                            tokens / 1000.0);

  return g_strdup_printf ("%" G_GUINT64_FORMAT, tokens);
}

static void
update_context_meter (XdChatView *self,
                      guint64     used,
                      guint64     window)
{
  g_autofree char *used_text = NULL;
  g_autofree char *window_text = NULL;
  g_autofree char *label = NULL;
  g_autofree char *tooltip = NULL;
  double fraction;

  if (self->context_meter == NULL)
    return;

  if (used == 0 || window == 0)
    {
      gtk_widget_set_visible (GTK_WIDGET (self->context_meter), FALSE);
      return;
    }

  fraction = MIN ((double) used / window, 1.0);
  used_text = format_token_count (used);
  window_text = format_token_count (window);
  label = g_strdup_printf ("%s / %s", used_text, window_text);
  tooltip = g_strdup_printf (
    "Context window: %" G_GUINT64_FORMAT " of %" G_GUINT64_FORMAT
    " tokens (%.0f%%)", used, window, fraction * 100.0);

  gtk_progress_bar_set_fraction (self->context_meter, fraction);
  gtk_progress_bar_set_text (self->context_meter, label);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->context_meter), tooltip);
  gtk_widget_remove_css_class (GTK_WIDGET (self->context_meter), "warning");
  gtk_widget_remove_css_class (GTK_WIDGET (self->context_meter), "error");

  if (fraction >= 0.9)
    gtk_widget_add_css_class (GTK_WIDGET (self->context_meter), "error");
  else if (fraction >= 0.75)
    gtk_widget_add_css_class (GTK_WIDGET (self->context_meter), "warning");

  gtk_widget_set_visible (GTK_WIDGET (self->context_meter), TRUE);
}

static void
update_context_bar (XdChatView   *self,
                    const XdChat *chat)
{
  g_autoptr (XdEffectiveSettings) resolved = NULL;
  g_autoptr (XdGitInfo) git = NULL;
  g_autoptr (GPtrArray) worktrees = NULL;
  g_autofree char *base_description = NULL;
  g_autofree char *description = NULL;
  const char *workdir;
  const char *model;
  guint64 used = 0;
  guint64 window = 0;

  resolved = xd_settings_resolve (xd_node_get_parent (self->chat), chat->backend);
  workdir = workdir_for (chat, resolved);
  xd_git_head_watch_set_workdir (self->git_head_watch, workdir);
  model = chat->model != NULL ? chat->model : resolved->model;
  git = xd_git_info_for_path (workdir);
  if (git != NULL)
    worktrees = xd_worktree_list (workdir, NULL);
  base_description = describe_context (workdir);
  description = chat->new_worktree
    ? g_strdup_printf ("New worktree from %s", base_description)
    : g_strdup (base_description);

  gtk_label_set_label (self->context_label, description);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->context_label), description);
  update_workspace_choice (
    self, chat, workdir,
    xd_storage_last_message_id (self->storage, chat->id) > 0,
    git != NULL && git->linked_worktree, worktrees);
  if (xd_storage_get_context_usage (
        self->storage, chat->id, chat->backend, model, &used, &window))
    update_context_meter (self, used, window);
  else
    update_context_meter (self, 0, 0);

  {
    gboolean have_workdir = workdir != NULL && *workdir != '\0';
    g_autofree char *tooltip =
      have_workdir ? g_strdup_printf ("Terminal in %s", workdir) : NULL;

    xd_terminal_panel_set_chat (self->terminal, xd_node_get_chat_id (self->chat));
    xd_terminal_panel_set_workdir (self->terminal, have_workdir ? workdir : NULL);
    xd_file_pane_set_workdir (self->files, have_workdir ? workdir : NULL);
    xd_diff_pane_set_workdir (self->diff, have_workdir ? workdir : NULL);
    xd_git_actions_set_workdir (self->git_actions, have_workdir ? workdir : NULL);

    gtk_widget_set_sensitive (GTK_WIDGET (self->terminal_button), have_workdir);
    gtk_widget_set_sensitive (GTK_WIDGET (self->file_button), have_workdir);
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->terminal_button),
                                 have_workdir ? tooltip : "This chat has no working directory");
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->file_button),
                                 have_workdir ? "Browse files"
                                              : "This chat has no working directory");
  }

  xd_model_picker_set_selected (self->model_picker, chat->backend,
                                chat->model != NULL ? chat->model : resolved->model);

  self->syncing_run_options = TRUE;

  for (guint i = 0; i < G_N_ELEMENTS (effort_choices); i++)
    {
      if (effort_choices[i] == effort_for (chat))
        xd_option_picker_set_selected (self->effort_chooser, i);
    }

  for (guint i = 0; i < G_N_ELEMENTS (access_choices); i++)
    {
      if (access_choices[i] == ai_access_from_string (chat->access))
        xd_option_picker_set_selected (self->access_chooser, i);
    }

  gtk_toggle_button_set_active (chat->plan ? self->plan_toggle : self->build_toggle,
                                TRUE);

  /* Planning changes nothing, so how much it is allowed to change is moot. */
  gtk_widget_set_sensitive (GTK_WIDGET (self->access_chooser), !chat->plan);

  self->syncing_run_options = FALSE;
}

static void
on_git_head_changed (XdGitHeadWatch *watch,
                     gpointer        user_data)
{
  XdChatView *self = user_data;
  g_autoptr (XdChat) chat = NULL;

  if (self->chat == NULL || self->remote != NULL)
    return;

  chat = xd_storage_get_chat (
    self->storage, xd_node_get_chat_id (self->chat), NULL);
  if (chat == NULL)
    return;

  update_context_bar (self, chat);
  xd_diff_pane_refresh (self->diff);
  xd_git_actions_refresh (self->git_actions);
}

static void
on_workspace_selected (XdOptionPicker *chooser,
                       GParamSpec     *pspec,
                       gpointer        user_data)
{
  XdChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  g_autoptr (XdChat) chat = NULL;
  const char *worktree = NULL;
  guint selected;
  gboolean new_worktree;

  if (self->syncing_workspace || self->chat == NULL)
    return;

  selected = xd_option_picker_get_selected (chooser);
  new_worktree = selected == 1;
  if (selected < self->workspace_paths->len)
    worktree = g_ptr_array_index (self->workspace_paths, selected);

  if (self->remote != NULL)
    {
      if (worktree != NULL)
        set_remote_option (self, "workspace", worktree);
      else
        set_remote_option (self, "new-worktree",
                           new_worktree ? "true" : "false");
      return;
    }

  if (worktree != NULL
        ? !xd_storage_use_existing_worktree (
            self->storage, xd_node_get_chat_id (self->chat), worktree, &error)
        : !xd_storage_set_new_worktree (
            self->storage, xd_node_get_chat_id (self->chat),
            new_worktree, &error))
    {
      append_row (self, XD_MESSAGE_ERROR, error->message);
      return;
    }

  chat = xd_storage_get_chat (
    self->storage, xd_node_get_chat_id (self->chat), NULL);
  if (chat != NULL)
    update_context_bar (self, chat);
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
on_effort_selected (XdOptionPicker *chooser,
                    GParamSpec     *pspec,
                    gpointer        user_data)
{
  XdChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  guint selected = xd_option_picker_get_selected (chooser);

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
on_access_selected (XdOptionPicker *chooser,
                    GParamSpec     *pspec,
                    gpointer        user_data)
{
  XdChatView *self = user_data;
  g_autoptr (GError) error = NULL;
  guint selected = xd_option_picker_get_selected (chooser);

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
      use_command_scope (self, backend_id);
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

  use_command_scope (self, backend_id);

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

/* Git actions mutate a checkout and remain local-only. Terminal, files and
 * diff are read through the daemon when the chat lives remotely. */
static void
set_local_controls_visible (XdChatView *self,
                            gboolean    visible)
{
  gtk_widget_set_visible (GTK_WIDGET (self->git_actions), visible);
}

static void
unbind_chat_title (XdChatView *self)
{
  if (self->title_binding == NULL)
    return;

  g_binding_unbind (self->title_binding);
  self->title_binding = NULL;
}

void
xd_chat_view_show_remote_chat (XdChatView     *self,
                               XdNode         *chat,
                               XdRemoteClient *client)
{
  gboolean changed;
  gboolean keep_previous = FALSE;

  g_return_if_fail (XD_IS_CHAT_VIEW (self));
  g_return_if_fail (XD_IS_NODE (chat));
  g_return_if_fail (XD_IS_REMOTE_CLIENT (client));

  changed = self->chat != chat || self->remote != client;

  /* Remote pages remain useful during an outage. A live page is marked stale
   * as it is left, so the next successful snapshot rebuilds it exactly once. */
  if (changed)
    {
      keep_previous = current_transcript_is_cacheable (self);
      self->follow_bottom = TRUE;
      self->history_bottom_distance = -1;
      self->restore_remote_panes = TRUE;
      g_cancellable_cancel (self->fetching);
      g_clear_object (&self->fetching);
      g_clear_pointer (&self->pending_remote_messages, g_ptr_array_unref);
      set_working (self, FALSE);
      retire_open_questions (self);
      leave_current_transcript (self, keep_previous);
      end_remote_turn (self);
      use_command_scope (self, NULL);
    }

  set_queued_text (self, NULL);
  update_context_meter (self, 0, 0);
  unbind_chat_title (self);
  use_chat_node (self, chat);
  self->title_binding =
    g_object_bind_property (chat, "name", self->title, "title",
                            G_BINDING_SYNC_CREATE);
  set_remote (self, client);
  if (changed)
    {
      activate_transcript_page (self, chat, client, -1);
      begin_bottom_jump (self);
    }

  set_local_controls_visible (self, FALSE);
  xd_terminal_panel_set_chat (self->terminal, xd_node_get_chat_id (chat));

  /* Pane visibility belongs to this device, not daemon state. Keep the old
   * frame during a refresh; a new chat restores after its options arrive. */
  if (changed)
    apply_panes (self, PANE_NONE);
  gtk_widget_set_sensitive (GTK_WIDGET (self->terminal_button), FALSE);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->terminal_button),
                               "Reading the remote working directory");
  gtk_widget_set_sensitive (GTK_WIDGET (self->diff_button), FALSE);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->diff_button),
                               "Reading the remote working directory");
  gtk_widget_set_sensitive (GTK_WIDGET (self->file_button), FALSE);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->file_button),
                               "Reading the remote working directory");

  gtk_stack_set_visible_child_name (self->stack, "chat");
  gtk_widget_set_visible (self->composer_area, TRUE);
  adw_window_title_set_subtitle (self->title, xd_remote_client_get_host (client));

  load_remote_transcript (self);
  update_send_button (self);
  gtk_widget_grab_focus (GTK_WIDGET (self->composer));
}

void
xd_chat_view_set_chat (XdChatView *self,
                       XdNode     *chat)
{
  Turn *turn;
  gboolean changed;
  gboolean cached = FALSE;
  gboolean rebuilt = FALSE;
  gboolean keep_previous = FALSE;
  gint64 last_message_id = 0;

  g_return_if_fail (XD_IS_CHAT_VIEW (self));

  changed = self->chat != chat || self->remote != NULL;
  if (changed)
    {
      keep_previous =
        chat != NULL && current_transcript_is_cacheable (self);
      self->follow_bottom = TRUE;
      self->history_bottom_distance = -1;
      apply_panes (self, PANE_NONE);

      /* Whatever a daemon was still going to say about the last chat is no
       * longer about anything on screen. */
      g_cancellable_cancel (self->fetching);
      g_clear_object (&self->fetching);
      g_clear_pointer (&self->pending_remote_messages, g_ptr_array_unref);
      set_working (self, FALSE);
      retire_open_questions (self);
      leave_current_transcript (self, keep_previous);
      end_remote_turn (self);
    }

  set_remote (self, NULL);
  self->restore_remote_panes = FALSE;
  set_queued_text (self, NULL);
  update_context_meter (self, 0, 0);
  set_local_controls_visible (self, TRUE);
  adw_window_title_set_subtitle (self->title, NULL);

  unbind_chat_title (self);
  use_chat_node (self, chat);

  if (chat == NULL)
    {
      xd_git_head_watch_set_workdir (self->git_head_watch, NULL);
      xd_terminal_panel_set_chat (self->terminal, NULL);
      activate_empty_transcript (self);
      clear_transcript (self);
      gtk_stack_set_visible_child_name (self->stack, "empty");
      gtk_widget_set_visible (self->composer_area, FALSE);
      adw_window_title_set_title (self->title, "xd");
      adw_window_title_set_subtitle (self->title, NULL);
      return;
    }

  gtk_stack_set_visible_child_name (self->stack, "chat");
  gtk_widget_set_visible (self->composer_area, TRUE);
  self->title_binding =
    g_object_bind_property (chat, "name", self->title, "title",
                            G_BINDING_SYNC_CREATE);

  last_message_id =
    xd_storage_last_message_id (self->storage, xd_node_get_chat_id (chat));
  if (changed)
    cached = activate_transcript_page (
      self, chat, NULL, last_message_id);

  {
    g_autoptr (XdChat) record = xd_storage_get_chat (self->storage,
                                                     xd_node_get_chat_id (chat),
                                                     NULL);

    if (record != NULL)
      {
        use_command_scope (self, record->backend);
        update_context_bar (self, record);
        if (changed)
          {
            /* SQLite supplies the existing local default; the per-device
             * state also remembers which repository pane occupied the shared
             * side slot. Restore once, not on every metadata refresh. */
            apply_panes (
              self,
              saved_panes (
                self,
                (record->terminal_open ? PANE_TERMINAL : PANE_NONE) |
                (record->diff_open ? PANE_DIFF : PANE_NONE)));
          }
        set_queued_text (self, record->queued);
      }
    else
      {
        use_command_scope (self, NULL);
      }
  }

  if (changed)
    begin_bottom_jump (self);
  if (!cached &&
      (changed || last_message_id != self->rendered_message_id))
    {
      load_transcript (self);
      rebuilt = TRUE;
    }

  /* Re-attach a reply that kept arriving while another chat was on screen. */
  turn = current_turn (self);
  if (turn != NULL && rebuilt)
    {
      /* The finished parts of this turn live only in memory until it ends,
       * so the rebuilt transcript has to replay them or they vanish until
       * the chat is next reopened. */
      for (guint i = 0; i < turn->items->len; i++)
        {
          TurnItem *item = g_ptr_array_index (turn->items, i);

          if (item->tool)
            {
              show_tool_use (self, item->text);
            }
          else
            {
              XdMessageRow *said =
                append_row (self, XD_MESSAGE_ASSISTANT, item->text);

              xd_message_row_set_source (said, turn->label);
            }
        }

      /* The transcript was rebuilt, and the marker went with it. */
      set_working (self, TRUE);
    }
  else if (turn != NULL)
    {
      set_working (self, TRUE);
    }
  else if (self->queued != NULL)
    {
      /* A restart ended the process, not the instruction waiting behind it. */
      send_queued (self);
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
  GtkWidget *toolbar = gtk_box_new (GTK_ORIENTATION_VERTICAL, 2);
  GtkWidget *identity = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *run = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
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
  g_signal_connect (gtk_text_view_get_buffer (self->composer), "changed",
                    G_CALLBACK (on_composer_changed), self);

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

  self->choices_bar = gtk_box_new (GTK_ORIENTATION_VERTICAL, 6);
  gtk_widget_set_visible (self->choices_bar, FALSE);
  gtk_widget_set_margin_top (self->choices_bar, 6);
  gtk_widget_set_margin_start (self->choices_bar, 10);
  gtk_widget_set_margin_end (self->choices_bar, 10);

  self->commands_flow = GTK_FLOW_BOX (gtk_flow_box_new ());
  gtk_flow_box_set_selection_mode (self->commands_flow, GTK_SELECTION_NONE);
  gtk_flow_box_set_min_children_per_line (self->commands_flow, 1);
  gtk_flow_box_set_max_children_per_line (self->commands_flow, 4);
  gtk_flow_box_set_column_spacing (self->commands_flow, 4);
  gtk_flow_box_set_row_spacing (self->commands_flow, 4);
  gtk_widget_set_halign (GTK_WIDGET (self->commands_flow), GTK_ALIGN_FILL);

  self->commands_bar = gtk_scrolled_window_new ();
  gtk_scrolled_window_set_policy (
    GTK_SCROLLED_WINDOW (self->commands_bar),
    GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_scrolled_window_set_max_content_height (
    GTK_SCROLLED_WINDOW (self->commands_bar), 144);
  gtk_scrolled_window_set_propagate_natural_height (
    GTK_SCROLLED_WINDOW (self->commands_bar), TRUE);
  gtk_scrolled_window_set_child (
    GTK_SCROLLED_WINDOW (self->commands_bar),
    GTK_WIDGET (self->commands_flow));
  gtk_widget_set_visible (self->commands_bar, FALSE);
  gtk_widget_set_margin_top (self->commands_bar, 6);
  gtk_widget_set_margin_start (self->commands_bar, 10);
  gtk_widget_set_margin_end (self->commands_bar, 10);

  self->model_picker = xd_model_picker_new ();
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->model_picker),
                               "Which assistant and model answer in this chat");
  g_signal_connect (self->model_picker, "model-chosen",
                    G_CALLBACK (on_model_chosen), self);

  {
    const char *workspaces[] = {
      "Current checkout",
      "New worktree",
      NULL,
    };

    self->workspace_chooser =
      xd_option_picker_new (workspaces, workspace_descriptions);
    gtk_widget_set_tooltip_text (
      GTK_WIDGET (self->workspace_chooser),
      "Where this chat works; locked after the first message");
    g_signal_connect (
      self->workspace_chooser, "notify::selected",
      G_CALLBACK (on_workspace_selected), self);
  }

  self->context_label = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_ellipsize (self->context_label, PANGO_ELLIPSIZE_MIDDLE);
  gtk_label_set_xalign (self->context_label, 0.0f);
  gtk_widget_set_hexpand (GTK_WIDGET (self->context_label), TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (self->context_label), "dim-label");
  gtk_widget_add_css_class (GTK_WIDGET (self->context_label), "caption");

  self->context_meter = GTK_PROGRESS_BAR (gtk_progress_bar_new ());
  gtk_progress_bar_set_show_text (self->context_meter, TRUE);
  gtk_widget_set_size_request (GTK_WIDGET (self->context_meter), 108, -1);
  gtk_widget_set_valign (GTK_WIDGET (self->context_meter), GTK_ALIGN_CENTER);
  gtk_widget_set_visible (GTK_WIDGET (self->context_meter), FALSE);
  gtk_widget_add_css_class (GTK_WIDGET (self->context_meter), "xd-context-meter");

  self->send_button = GTK_BUTTON (gtk_button_new_from_icon_name ("go-up-symbolic"));
  gtk_widget_add_css_class (GTK_WIDGET (self->send_button), "suggested-action");
  gtk_widget_add_css_class (GTK_WIDGET (self->send_button), "circular");
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->send_button), "Send (Enter)");
  g_signal_connect (self->send_button, "clicked", G_CALLBACK (on_send_clicked), self);

  {
    const char *efforts[G_N_ELEMENTS (effort_choices) + 1] = { NULL };
    const char *accesses[G_N_ELEMENTS (access_choices) + 1] = { NULL };

    for (guint i = 0; i < G_N_ELEMENTS (effort_choices); i++)
      efforts[i] = ai_effort_label (effort_choices[i]);

    for (guint i = 0; i < G_N_ELEMENTS (access_choices); i++)
      accesses[i] = ai_access_label (access_choices[i]);

    self->effort_chooser =
      xd_option_picker_new (efforts, effort_descriptions);
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->effort_chooser),
                                 "How hard the model is asked to think");
    g_signal_connect (self->effort_chooser, "notify::selected",
                      G_CALLBACK (on_effort_selected), self);

    self->access_chooser =
      xd_option_picker_new (accesses, access_descriptions);
    gtk_widget_set_tooltip_text (GTK_WIDGET (self->access_chooser),
                                 "What the assistant may do in the working "
                                 "directory");
    g_signal_connect (self->access_chooser, "notify::selected",
                      G_CALLBACK (on_access_selected), self);
  }

  /* Identity and capacity answer "where and who"; execution controls answer
   * "how". Keeping those groups on their own rows gives every selected value
   * its natural width instead of turning the toolbar into ellipses. */
  gtk_box_append (GTK_BOX (identity), GTK_WIDGET (self->workspace_chooser));
  gtk_box_append (GTK_BOX (identity), GTK_WIDGET (self->model_picker));
  gtk_box_append (GTK_BOX (identity), GTK_WIDGET (self->context_meter));
  gtk_box_append (GTK_BOX (run), GTK_WIDGET (self->effort_chooser));
  gtk_box_append (GTK_BOX (run), GTK_WIDGET (self->access_chooser));

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
    gtk_box_append (GTK_BOX (run), modes);
  }

  self->terminal_button = GTK_TOGGLE_BUTTON (gtk_toggle_button_new ());
  gtk_button_set_icon_name (GTK_BUTTON (self->terminal_button), "utilities-terminal-symbolic");
  gtk_widget_add_css_class (GTK_WIDGET (self->terminal_button), "flat");
  gtk_widget_set_visible (GTK_WIDGET (self->terminal_button), XD_HAS_TERMINAL);
  g_signal_connect (self->terminal_button, "toggled",
                    G_CALLBACK (on_terminal_toggled), self);

  self->file_button = GTK_TOGGLE_BUTTON (gtk_toggle_button_new ());
  gtk_button_set_icon_name (GTK_BUTTON (self->file_button), "folder-symbolic");
  gtk_widget_add_css_class (GTK_WIDGET (self->file_button), "flat");
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->file_button), "Browse files");
  g_signal_connect (self->file_button, "toggled",
                    G_CALLBACK (on_file_toggled), self);

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
    gtk_box_append (GTK_BOX (run), filler);
  }

  gtk_box_append (GTK_BOX (run), GTK_WIDGET (self->send_button));
  gtk_box_append (GTK_BOX (toolbar), identity);
  gtk_box_append (GTK_BOX (toolbar), run);
  /* The controls sit under the text the user is typing, so they need enough
   * clearance not to read as part of it. */
  gtk_widget_set_margin_top (toolbar, 10);
  gtk_widget_set_margin_start (toolbar, 6);
  gtk_widget_set_margin_end (toolbar, 6);
  gtk_widget_set_margin_bottom (toolbar, 6);

  /* Above what is being typed, the way an attachment reads: this is going
   * with the message below it. A pending question sits directly under a
   * queued steer when both exist, so they form one composer attachment stack. */
  gtk_box_append (GTK_BOX (column), self->queued_bar);
  gtk_box_append (GTK_BOX (column), self->choices_bar);
  gtk_box_append (GTK_BOX (column), self->attachments_bar);
  gtk_box_append (GTK_BOX (column), self->commands_bar);
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

  if (self->bottom_jump_tick != 0)
    {
      gtk_widget_remove_tick_callback (GTK_WIDGET (self->scroller),
                                       self->bottom_jump_tick);
      self->bottom_jump_tick = 0;
    }
  if (self->bottom_pin_tick != 0)
    {
      gtk_widget_remove_tick_callback (GTK_WIDGET (self->scroller),
                                       self->bottom_pin_tick);
      self->bottom_pin_tick = 0;
    }

  g_clear_handle_id (&self->working_timer, g_source_remove);
  self->working_label = NULL;
  g_cancellable_cancel (self->fetching);
  g_clear_object (&self->fetching);
  g_clear_pointer (&self->pending_remote_messages, g_ptr_array_unref);
  g_clear_object (&self->remote);
  unbind_chat_title (self);
  if (self->chat != NULL)
    xd_node_set_active (self->chat, FALSE);
  g_clear_object (&self->chat);
  g_clear_pointer (&self->turns, g_hash_table_unref);
  g_queue_clear (&self->transcript_lru);
  g_clear_pointer (&self->transcript_pages, g_hash_table_unref);
  g_clear_pointer (&self->attachments, g_ptr_array_unref);
  g_clear_pointer (&self->command_sets, g_hash_table_unref);
  g_clear_pointer (&self->command_scope, g_free);
  g_clear_pointer (&self->workspace_paths, g_ptr_array_unref);
  g_clear_pointer (&self->queued, g_free);
  g_clear_object (&self->settings);
  g_clear_object (&self->git_head_watch);
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
  GtkWidget *content = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *empty = adw_status_page_new ();

  self->turns = g_hash_table_new_full (g_str_hash, g_str_equal, g_free, turn_free);
  self->transcript_pages = g_hash_table_new_full (
    g_str_hash, g_str_equal, NULL, (GDestroyNotify) transcript_page_free);
  g_queue_init (&self->transcript_lru);
  self->settings = g_settings_new (XD_APP_ID);
  self->attachments = g_ptr_array_new_with_free_func (g_free);
  self->command_sets = g_hash_table_new_full (
    g_str_hash, g_str_equal, g_free, (GDestroyNotify) g_strfreev);
  self->workspace_paths = g_ptr_array_new_with_free_func (g_free);
  self->git_head_watch = xd_git_head_watch_new ();
  g_signal_connect (self->git_head_watch, "changed",
                    G_CALLBACK (on_git_head_changed), self);
  self->transcript_limit = TRANSCRIPT_PAGE_SIZE;
  self->history_bottom_distance = -1;
  self->header = adw_header_bar_new ();

  self->title = ADW_WINDOW_TITLE (adw_window_title_new ("xd", NULL));
  adw_header_bar_set_title_widget (
    ADW_HEADER_BAR (self->header), GTK_WIDGET (self->title));

  /* The sidebar is the leftmost header bar, so whatever the desktop puts on
   * that side of the title bar is its to draw. */
  adw_header_bar_set_show_start_title_buttons (
    ADW_HEADER_BAR (self->header), FALSE);

  /* At the top: these open and close parts of the window, which is what the
   * header bar is for. The row under the composer decides how the next
   * message is answered, which is a different question. */
  self->git_actions = xd_git_actions_new ();
  adw_header_bar_pack_end (
    ADW_HEADER_BAR (self->header), GTK_WIDGET (self->git_actions));

  adw_toolbar_view_add_top_bar (ADW_TOOLBAR_VIEW (toolbar), self->header);

  adw_status_page_set_icon_name (ADW_STATUS_PAGE (empty), XD_CHAT_ICON);
  adw_status_page_set_title (ADW_STATUS_PAGE (empty), "No Chat Selected");
  adw_status_page_set_description (ADW_STATUS_PAGE (empty),
                                   "Pick a chat in the sidebar, or start a new "
                                   "one in a folder.");

  self->transcript_stack = GTK_STACK (gtk_stack_new ());
  gtk_stack_set_hhomogeneous (self->transcript_stack, TRUE);
  gtk_stack_set_vhomogeneous (self->transcript_stack, FALSE);
  gtk_stack_set_transition_type (
    self->transcript_stack, GTK_STACK_TRANSITION_TYPE_NONE);
  self->empty_transcript = new_transcript ();
  self->transcript = self->empty_transcript;
  gtk_stack_add_named (self->transcript_stack,
                       GTK_WIDGET (self->empty_transcript), "empty");

  self->scroller = GTK_SCROLLED_WINDOW (gtk_scrolled_window_new ());
  {
    GtkAdjustment *adjustment =
      gtk_scrolled_window_get_vadjustment (self->scroller);
    GtkEventController *scroll =
      gtk_event_controller_scroll_new (GTK_EVENT_CONTROLLER_SCROLL_VERTICAL);

    g_signal_connect (adjustment, "changed",
                      G_CALLBACK (on_scroll_adjustment_changed), self);
    g_signal_connect (adjustment, "value-changed",
                      G_CALLBACK (on_scroll_adjustment_changed), self);
    gtk_event_controller_set_propagation_phase (scroll, GTK_PHASE_CAPTURE);
    g_signal_connect (scroll, "scroll",
                      G_CALLBACK (on_transcript_scrolled), self);
    gtk_widget_add_controller (GTK_WIDGET (self->scroller), scroll);
  }

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
    adw_clamp_set_child (ADW_CLAMP (clamp),
                         GTK_WIDGET (self->transcript_stack));
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
  adw_header_bar_pack_end (
    ADW_HEADER_BAR (self->header), GTK_WIDGET (self->terminal_button));
  adw_header_bar_pack_end (
    ADW_HEADER_BAR (self->header), GTK_WIDGET (self->file_button));
  adw_header_bar_pack_end (
    ADW_HEADER_BAR (self->header), GTK_WIDGET (self->diff_button));

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

  /* Repository panes sit beside the conversation and terminal together. */
  self->diff = xd_diff_pane_new ();
  self->files = xd_file_pane_new ();

  self->side_stack = GTK_STACK (gtk_stack_new ());
  gtk_stack_add_named (
    self->side_stack, GTK_WIDGET (self->files), "files");
  gtk_stack_add_named (
    self->side_stack, GTK_WIDGET (self->diff), "diff");
  gtk_widget_set_visible (GTK_WIDGET (self->side_stack), FALSE);
  gtk_widget_add_css_class (
    GTK_WIDGET (self->side_stack), "xd-divider-left");

  self->side_split = GTK_PANED (gtk_paned_new (GTK_ORIENTATION_HORIZONTAL));
  g_signal_connect (self->side_split, "notify::position",
                    G_CALLBACK (on_side_dragged), self);
  gtk_paned_set_start_child (self->side_split, GTK_WIDGET (self->split));
  gtk_paned_set_resize_start_child (self->side_split, TRUE);
  gtk_paned_set_shrink_start_child (self->side_split, FALSE);
  gtk_paned_set_end_child (
    self->side_split, GTK_WIDGET (self->side_stack));
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
