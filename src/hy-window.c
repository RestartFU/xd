#include "hy-window.h"

struct _HyWindow
{
  AdwApplicationWindow parent_instance;

  GSettings *settings;

  AdwNavigationSplitView *split_view;
  AdwNavigationPage *content_page;
};

G_DEFINE_FINAL_TYPE (HyWindow, hy_window, ADW_TYPE_APPLICATION_WINDOW)

static AdwNavigationPage *
build_sidebar_page (HyWindow *self)
{
  GtkWidget *toolbar = adw_toolbar_view_new ();
  GtkWidget *header = adw_header_bar_new ();
  GtkWidget *placeholder;

  adw_toolbar_view_add_top_bar (ADW_TOOLBAR_VIEW (toolbar), header);

  placeholder = adw_status_page_new ();
  adw_status_page_set_icon_name (ADW_STATUS_PAGE (placeholder), "folder-symbolic");
  adw_status_page_set_title (ADW_STATUS_PAGE (placeholder), "No Workspaces");
  adw_status_page_set_description (ADW_STATUS_PAGE (placeholder),
                                   "The workspace tree lands here.");
  adw_toolbar_view_set_content (ADW_TOOLBAR_VIEW (toolbar), placeholder);

  return adw_navigation_page_new (toolbar, "Workspaces");
}

static AdwNavigationPage *
build_content_page (HyWindow *self)
{
  GtkWidget *toolbar = adw_toolbar_view_new ();
  GtkWidget *header = adw_header_bar_new ();
  GtkWidget *status;
  GtkWidget *menu_button;
  GMenu *menu;

  menu = g_menu_new ();
  g_menu_append (menu, "About hy", "app.about");
  g_menu_append (menu, "Quit", "app.quit");

  menu_button = gtk_menu_button_new ();
  gtk_menu_button_set_icon_name (GTK_MENU_BUTTON (menu_button), "open-menu-symbolic");
  gtk_menu_button_set_menu_model (GTK_MENU_BUTTON (menu_button), G_MENU_MODEL (menu));
  g_object_unref (menu);

  adw_header_bar_pack_end (ADW_HEADER_BAR (header), menu_button);
  adw_toolbar_view_add_top_bar (ADW_TOOLBAR_VIEW (toolbar), header);

  status = adw_status_page_new ();
  adw_status_page_set_icon_name (ADW_STATUS_PAGE (status), "user-available-symbolic");
  adw_status_page_set_title (ADW_STATUS_PAGE (status), "hy");
  adw_status_page_set_description (ADW_STATUS_PAGE (status),
                                   "Pick a chat in the sidebar to get started.");
  adw_toolbar_view_set_content (ADW_TOOLBAR_VIEW (toolbar), status);

  return adw_navigation_page_new (toolbar, "Chat");
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
  g_settings_set_boolean (self->settings, "window-maximized",
                          gtk_window_is_maximized (window));

  return GDK_EVENT_PROPAGATE;
}

HyWindow *
hy_window_new (HyApplication *app)
{
  HyWindow *self;

  g_return_val_if_fail (HY_IS_APPLICATION (app), NULL);

  self = g_object_new (HY_TYPE_WINDOW, "application", app, NULL);
  self->settings = g_object_ref (hy_application_get_settings (app));

  gtk_window_set_default_size (GTK_WINDOW (self),
                               g_settings_get_int (self->settings, "window-width"),
                               g_settings_get_int (self->settings, "window-height"));
  if (g_settings_get_boolean (self->settings, "window-maximized"))
    gtk_window_maximize (GTK_WINDOW (self));

  adw_navigation_split_view_set_sidebar (self->split_view, build_sidebar_page (self));

  self->content_page = build_content_page (self);
  adw_navigation_split_view_set_content (self->split_view, self->content_page);

  g_signal_connect (self, "close-request", G_CALLBACK (on_close_request), NULL);

  return self;
}

static void
hy_window_dispose (GObject *object)
{
  HyWindow *self = HY_WINDOW (object);

  g_clear_object (&self->settings);

  G_OBJECT_CLASS (hy_window_parent_class)->dispose (object);
}

static void
hy_window_class_init (HyWindowClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = hy_window_dispose;
}

static void
hy_window_init (HyWindow *self)
{
  gtk_window_set_title (GTK_WINDOW (self), "hy");

  self->split_view = ADW_NAVIGATION_SPLIT_VIEW (adw_navigation_split_view_new ());
  adw_navigation_split_view_set_min_sidebar_width (self->split_view, 200);
  adw_navigation_split_view_set_max_sidebar_width (self->split_view, 420);
  adw_navigation_split_view_set_sidebar_width_fraction (self->split_view, 0.28);

  adw_application_window_set_content (ADW_APPLICATION_WINDOW (self),
                                      GTK_WIDGET (self->split_view));
}
