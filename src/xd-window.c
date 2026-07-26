#include "xd-window.h"

#include "chat/chat-view.h"
#include "chat/search-dialog.h"
#include "storage/storage.h"
#include "tree/fs-tree.h"
#include "tree/sidebar.h"

struct _XdWindow
{
  AdwApplicationWindow parent_instance;

  GSettings *settings;
  XdStorage *storage;
  XdFsTree *tree;

  GtkPaned *split_view;
  XdChatView *chat_view;
};

G_DEFINE_FINAL_TYPE (XdWindow, xd_window, ADW_TYPE_APPLICATION_WINDOW)

/* An empty setting means "use the default", which keeps this user's home
 * directory out of the stored configuration. */
static char *
resolve_root (GSettings  *settings,
              const char *key,
              const char *fallback_name)
{
  g_autofree char *configured = g_settings_get_string (settings, key);

  if (configured != NULL && *configured != '\0')
    return g_steal_pointer (&configured);

  return g_build_filename (g_get_home_dir (), fallback_name, NULL);
}

/* Selecting a chat opens it; selecting a folder leaves the current chat alone,
 * so browsing the tree does not throw away what you were reading. */
static void
on_node_selected (XdSidebar *sidebar,
                  XdNode    *node,
                  gpointer   user_data)
{
  XdWindow *self = user_data;

  if (node != NULL && xd_node_get_kind (node) == XD_NODE_CHAT)
    xd_chat_view_set_chat (self->chat_view, node);
}

/*
 * The chat on screen has been deleted.
 *
 * Without this the view keeps showing a chat that is no longer in the
 * database -- readable, and worse, still able to be typed into.
 */
static void
on_chat_removed (XdFsTree *tree,
                 XdNode   *chat,
                 gpointer  user_data)
{
  XdWindow *self = user_data;

  if (xd_chat_view_get_chat (self->chat_view) == chat)
    xd_chat_view_set_chat (self->chat_view, NULL);
}

static void
on_node_activated (XdSidebar *sidebar,
                   XdNode    *node,
                   gpointer   user_data)
{
  XdWindow *self = user_data;

  if (node != NULL && xd_node_get_kind (node) == XD_NODE_CHAT)
    xd_chat_view_set_chat (self->chat_view, node);
}

static void
on_search_result_chosen (XdNode   *chat,
                         gpointer  user_data)
{
  XdWindow *self = user_data;

  xd_chat_view_set_chat (self->chat_view, chat);
}

static void
on_search_action (GtkWidget  *widget,
                  const char *action_name,
                  GVariant   *parameter)
{
  XdWindow *self = XD_WINDOW (widget);

  if (self->storage == NULL)
    return;

  xd_search_dialog_present (widget, self->storage, self->tree,
                            on_search_result_chosen, self);
}

/*
 * Clicking anywhere else drops a message's text selection.
 *
 * A selectable label holds its selection when focus leaves, so highlighted
 * text lingered all over the transcript. Watched in the capture phase at the
 * window, which sees the press wherever it lands -- another message, the
 * composer, the sidebar, or dead space.
 */
static void
on_press_anywhere (GtkGestureClick *gesture,
                   int              n_press,
                   double           x,
                   double           y,
                   gpointer         user_data)
{
  GtkWindow *window = user_data;
  GtkWidget *focus = gtk_window_get_focus (window);
  GtkWidget *target;

  if (focus == NULL || !GTK_IS_LABEL (focus) ||
      !gtk_widget_has_css_class (focus, "xd-body"))
    return;

  target = gtk_widget_pick (GTK_WIDGET (window), x, y, GTK_PICK_DEFAULT);
  if (target != focus)
    gtk_label_select_region (GTK_LABEL (focus), 0, 0);
}

static gboolean
on_close_request (GtkWindow *window,
                  gpointer   user_data)
{
  XdWindow *self = XD_WINDOW (window);
  int width, height;

  gtk_window_get_default_size (window, &width, &height);
  g_settings_set_int (self->settings, "window-width", width);
  g_settings_set_int (self->settings, "window-height", height);
  g_settings_set_int (self->settings, "sidebar-width",
                      gtk_paned_get_position (self->split_view));
  g_settings_set_boolean (self->settings, "window-maximized",
                          gtk_window_is_maximized (window));

  return GDK_EVENT_PROPAGATE;
}

XdWindow *
xd_window_new (XdApplication *app)
{
  g_autofree char *workspaces_root = NULL;
  g_autofree char *db_path = NULL;
  g_autoptr (GError) error = NULL;
  XdSidebar *sidebar;
  XdWindow *self;

  g_return_val_if_fail (XD_IS_APPLICATION (app), NULL);

  self = g_object_new (XD_TYPE_WINDOW, "application", app, NULL);
  self->settings = g_object_ref (xd_application_get_settings (app));

  gtk_window_set_default_size (GTK_WINDOW (self),
                               g_settings_get_int (self->settings, "window-width"),
                               g_settings_get_int (self->settings, "window-height"));
  if (g_settings_get_boolean (self->settings, "window-maximized"))
    gtk_window_maximize (GTK_WINDOW (self));

  db_path = g_build_filename (g_get_user_data_dir (), "xd", "chats.db", NULL);
  self->storage = xd_storage_new (db_path, &error);
  if (self->storage == NULL)
    {
      /* Without storage there is nothing to show, so say so plainly rather
       * than starting up half-working. */
      AdwAlertDialog *dialog =
        ADW_ALERT_DIALOG (adw_alert_dialog_new ("Cannot Open the Chat Database",
                                                error->message));

      adw_alert_dialog_add_response (dialog, "quit", "Quit");
      g_signal_connect_swapped (dialog, "response", G_CALLBACK (gtk_window_destroy), self);
      adw_dialog_present (ADW_DIALOG (dialog), GTK_WIDGET (self));

      return self;
    }

  workspaces_root = resolve_root (self->settings, "workspaces-root", "Workspaces");
  self->tree = xd_fs_tree_new (workspaces_root, self->storage);

  sidebar = xd_sidebar_new (self->tree);
  g_signal_connect (sidebar, "node-selected", G_CALLBACK (on_node_selected), self);
  g_signal_connect (sidebar, "node-activated", G_CALLBACK (on_node_activated), self);

  self->chat_view = xd_chat_view_new (self->storage, self->tree);
  gtk_widget_add_css_class (GTK_WIDGET (self->chat_view), "xd-divider-left");
  g_signal_connect (self->tree, "chat-removed", G_CALLBACK (on_chat_removed), self);

  gtk_paned_set_start_child (self->split_view, GTK_WIDGET (sidebar));
  gtk_paned_set_end_child (self->split_view, GTK_WIDGET (self->chat_view));
  gtk_paned_set_position (self->split_view,
                          g_settings_get_int (self->settings, "sidebar-width"));

  g_signal_connect (self, "close-request", G_CALLBACK (on_close_request), NULL);

  {
    GtkGesture *press = gtk_gesture_click_new ();

    gtk_event_controller_set_propagation_phase (GTK_EVENT_CONTROLLER (press),
                                                GTK_PHASE_CAPTURE);
    g_signal_connect (press, "pressed", G_CALLBACK (on_press_anywhere), self);
    gtk_widget_add_controller (GTK_WIDGET (self), GTK_EVENT_CONTROLLER (press));
  }

  return self;
}

static void
xd_window_dispose (GObject *object)
{
  XdWindow *self = XD_WINDOW (object);

  g_clear_object (&self->tree);
  g_clear_object (&self->storage);
  g_clear_object (&self->settings);

  G_OBJECT_CLASS (xd_window_parent_class)->dispose (object);
}

static void
xd_window_class_init (XdWindowClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);
  GtkWidgetClass *widget_class = GTK_WIDGET_CLASS (klass);

  object_class->dispose = xd_window_dispose;

  gtk_widget_class_install_action (widget_class, "win.search", NULL,
                                   on_search_action);
  gtk_widget_class_add_binding_action (widget_class, GDK_KEY_k, GDK_CONTROL_MASK,
                                       "win.search", NULL);
  gtk_widget_class_add_binding_action (widget_class, GDK_KEY_f, GDK_CONTROL_MASK,
                                       "win.search", NULL);
}

static void
xd_window_init (XdWindow *self)
{
  gtk_window_set_title (GTK_WINDOW (self), "xd");

  /*
   * A paned rather than AdwNavigationSplitView, which sizes the sidebar by a
   * fraction of the window and cannot be dragged. The cost is the split
   * view's narrow-window behaviour, where the sidebar becomes a page of its
   * own; xd is a desktop window with a tree that is worth widening.
   */
  self->split_view = GTK_PANED (gtk_paned_new (GTK_ORIENTATION_HORIZONTAL));
  gtk_paned_set_resize_start_child (self->split_view, FALSE);
  gtk_paned_set_shrink_start_child (self->split_view, FALSE);
  gtk_paned_set_resize_end_child (self->split_view, TRUE);
  gtk_paned_set_shrink_end_child (self->split_view, FALSE);

  adw_application_window_set_content (ADW_APPLICATION_WINDOW (self),
                                      GTK_WIDGET (self->split_view));
}
