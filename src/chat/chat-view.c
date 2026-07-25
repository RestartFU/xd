#include "chat-view.h"

#include "chat-session.h"
#include "message-row.h"

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
  GString *text;
  HyMessageRow *row;        /* NULL while the chat is not on screen */
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
};

G_DEFINE_FINAL_TYPE (HyChatView, hy_chat_view, ADW_TYPE_BIN)

static void send_current_message (HyChatView *self);
static void update_send_button (HyChatView *self);

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

  /* Stored immediately: if hy dies mid-reply the conversation can still be
   * resumed from where the CLI left it. */
  if (!hy_storage_set_session_id (turn->view->storage, turn->chat_id,
                                  session_id, &error))
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
  gboolean visible = turn_is_visible (turn);

  if (turn->text->len > 0)
    {
      if (!hy_storage_append_message (self->storage, chat_id, "assistant",
                                      turn->text->str, NULL, &error))
        g_warning ("cannot store the reply: %s", error->message);
    }

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
 * chat's own choice wins; otherwise it falls back to its folder. */
static const char *
workdir_for (HyChatView *self,
             const HyChat *chat)
{
  HyNode *folder;

  if (chat->workdir != NULL && *chat->workdir != '\0')
    return chat->workdir;

  folder = hy_node_get_parent (self->chat);

  return folder != NULL ? hy_node_get_path (folder) : NULL;
}

static void
start_turn (HyChatView *self,
            const char *prompt)
{
  g_autoptr (HyChat) chat = NULL;
  g_autoptr (GError) error = NULL;
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

  turn = g_new0 (Turn, 1);
  turn->view = self;
  turn->chat_id = g_strdup (chat->id);
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

  spec.prompt = prompt;
  spec.workdir = workdir_for (self, chat);
  spec.resume_session_id = chat->session_id;

  if (!hy_chat_session_start (turn->session, &spec, &error))
    {
      append_row (self, HY_MESSAGE_ERROR, error->message);
      hy_storage_append_message (self->storage, chat->id, "error",
                                 error->message, NULL, NULL);
      g_hash_table_remove (self->turns, chat->id);
    }

  update_send_button (self);
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

static GtkWidget *
build_composer (HyChatView *self)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *frame = gtk_frame_new (NULL);
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

  gtk_frame_set_child (GTK_FRAME (frame), scroller);
  gtk_widget_set_hexpand (frame, TRUE);

  self->send_button = GTK_BUTTON (gtk_button_new_from_icon_name ("go-up-symbolic"));
  gtk_widget_add_css_class (GTK_WIDGET (self->send_button), "suggested-action");
  gtk_widget_add_css_class (GTK_WIDGET (self->send_button), "circular");
  gtk_widget_set_valign (GTK_WIDGET (self->send_button), GTK_ALIGN_END);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->send_button), "Send (Enter)");
  g_signal_connect (self->send_button, "clicked", G_CALLBACK (on_send_clicked), self);

  gtk_box_append (GTK_BOX (box), frame);
  gtk_box_append (GTK_BOX (box), GTK_WIDGET (self->send_button));

  gtk_widget_set_margin_top (box, 6);
  gtk_widget_set_margin_bottom (box, 12);
  gtk_widget_set_margin_start (box, 12);
  gtk_widget_set_margin_end (box, 12);

  return box;
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
