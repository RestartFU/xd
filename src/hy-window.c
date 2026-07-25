#include "hy-window.h"

#include "chat/chat-view.h"
#include "chat/search-dialog.h"
#include "storage/storage.h"
#include "tree/fs-tree.h"
#include "tree/sidebar.h"

struct _HyWindow
{
  AdwApplicationWindow parent_instance;

  GSettings *settings;
  HyStorage *storage;
  HyFsTree *tree;

  GtkPaned *split_view;
  HyChatView *chat_view;
};

G_DEFINE_FINAL_TYPE (HyWindow, hy_window, ADW_TYPE_APPLICATION_WINDOW)

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
on_node_selected (HySidebar *sidebar,
                  HyNode    *node,
                  gpointer   user_data)
{
  HyWindow *self = user_data;

  if (node != NULL && hy_node_get_kind (node) == HY_NODE_CHAT)
    hy_chat_view_set_chat (self->chat_view, node);
}

static void
on_node_activated (HySidebar *sidebar,
                   HyNode    *node,
                   gpointer   user_data)
{
  HyWindow *self = user_data;

  if (node != NULL && hy_node_get_kind (node) == HY_NODE_CHAT)
    hy_chat_view_set_chat (self->chat_view, node);
}

static void
on_search_result_chosen (HyNode   *chat,
                         gpointer  user_data)
{
  HyWindow *self = user_data;

  hy_chat_view_set_chat (self->chat_view, chat);
}

static void
on_search_action (GtkWidget  *widget,
                  const char *action_name,
                  GVariant   *parameter)
{
  HyWindow *self = HY_WINDOW (widget);

  if (self->storage == NULL)
    return;

  hy_search_dialog_present (widget, self->storage, self->tree,
                            on_search_result_chosen, self);
}

static gboolean
on_close_request (GtkWindow *window,
                  gpointer   user_data)
{
  HyWindow *self = HY_WINDOW (window);
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

HyWindow *
hy_window_new (HyApplication *app)
{
  g_autofree char *workspaces_root = NULL;
  g_autofree char *db_path = NULL;
  g_autoptr (GError) error = NULL;
  HySidebar *sidebar;
  HyWindow *self;

  g_return_val_if_fail (HY_IS_APPLICATION (app), NULL);

  self = g_object_new (HY_TYPE_WINDOW, "application", app, NULL);
  self->settings = g_object_ref (hy_application_get_settings (app));

  gtk_window_set_default_size (GTK_WINDOW (self),
                               g_settings_get_int (self->settings, "window-width"),
                               g_settings_get_int (self->settings, "window-height"));
  if (g_settings_get_boolean (self->settings, "window-maximized"))
    gtk_window_maximize (GTK_WINDOW (self));

  db_path = g_build_filename (g_get_user_data_dir (), "hy", "chats.db", NULL);
  self->storage = hy_storage_new (db_path, &error);
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
  self->tree = hy_fs_tree_new (workspaces_root, self->storage);

  sidebar = hy_sidebar_new (self->tree);
  g_signal_connect (sidebar, "node-selected", G_CALLBACK (on_node_selected), self);
  g_signal_connect (sidebar, "node-activated", G_CALLBACK (on_node_activated), self);

  self->chat_view = hy_chat_view_new (self->storage, self->tree);

  gtk_paned_set_start_child (self->split_view, GTK_WIDGET (sidebar));
  gtk_paned_set_end_child (self->split_view, GTK_WIDGET (self->chat_view));
  gtk_paned_set_position (self->split_view,
                          g_settings_get_int (self->settings, "sidebar-width"));

  g_signal_connect (self, "close-request", G_CALLBACK (on_close_request), NULL);

  return self;
}

static void
hy_window_dispose (GObject *object)
{
  HyWindow *self = HY_WINDOW (object);

  g_clear_object (&self->tree);
  g_clear_object (&self->storage);
  g_clear_object (&self->settings);

  G_OBJECT_CLASS (hy_window_parent_class)->dispose (object);
}

static void
hy_window_class_init (HyWindowClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);
  GtkWidgetClass *widget_class = GTK_WIDGET_CLASS (klass);

  object_class->dispose = hy_window_dispose;

  gtk_widget_class_install_action (widget_class, "win.search", NULL,
                                   on_search_action);
  gtk_widget_class_add_binding_action (widget_class, GDK_KEY_k, GDK_CONTROL_MASK,
                                       "win.search", NULL);
  gtk_widget_class_add_binding_action (widget_class, GDK_KEY_f, GDK_CONTROL_MASK,
                                       "win.search", NULL);
}

static void
hy_window_init (HyWindow *self)
{
  gtk_window_set_title (GTK_WINDOW (self), "hy");

  /*
   * A paned rather than AdwNavigationSplitView, which sizes the sidebar by a
   * fraction of the window and cannot be dragged. The cost is the split
   * view's narrow-window behaviour, where the sidebar becomes a page of its
   * own; hy is a desktop window with a tree that is worth widening.
   */
  self->split_view = GTK_PANED (gtk_paned_new (GTK_ORIENTATION_HORIZONTAL));
  gtk_paned_set_resize_start_child (self->split_view, FALSE);
  gtk_paned_set_shrink_start_child (self->split_view, FALSE);
  gtk_paned_set_resize_end_child (self->split_view, TRUE);
  gtk_paned_set_shrink_end_child (self->split_view, FALSE);

  adw_application_window_set_content (ADW_APPLICATION_WINDOW (self),
                                      GTK_WIDGET (self->split_view));
}
