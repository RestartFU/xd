#include "chat-view.h"

#include "message-row.h"

struct _HyChatView
{
  AdwBin parent_instance;

  HyStorage *storage;
  HyFsTree *tree;
  HyNode *chat;                 /* unowned; owned by the tree */

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

/* The new row has no allocation yet, so the adjustment only knows its final
 * extent once layout has run. */
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

/* --- composer ------------------------------------------------------------- */

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

static void
send_current_message (HyChatView *self)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *text = NULL;

  if (self->chat == NULL)
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

static void
on_send_clicked (GtkButton *button,
                 gpointer   user_data)
{
  send_current_message (user_data);
}

/* --- public API ----------------------------------------------------------- */

void
hy_chat_view_set_chat (HyChatView *self,
                       HyNode     *chat)
{
  g_return_if_fail (HY_IS_CHAT_VIEW (self));

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
