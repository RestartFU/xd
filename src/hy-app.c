#include "hy-app.h"
#include "hy-window.h"

struct _HyApplication
{
  AdwApplication parent_instance;

  GSettings *settings;
};

G_DEFINE_FINAL_TYPE (HyApplication, hy_application, ADW_TYPE_APPLICATION)

HyApplication *
hy_application_new (void)
{
  return g_object_new (HY_TYPE_APPLICATION,
                       "application-id", HY_APP_ID,
                       "flags", G_APPLICATION_DEFAULT_FLAGS,
                       NULL);
}

GSettings *
hy_application_get_settings (HyApplication *self)
{
  g_return_val_if_fail (HY_IS_APPLICATION (self), NULL);

  return self->settings;
}

static void
hy_application_activate (GApplication *app)
{
  GtkWindow *window = gtk_application_get_active_window (GTK_APPLICATION (app));

  if (window == NULL)
    window = GTK_WINDOW (hy_window_new (HY_APPLICATION (app)));

  gtk_window_present (window);
}

static void
on_about_action (GSimpleAction *action,
                 GVariant      *param,
                 gpointer       user_data)
{
  HyApplication *self = user_data;
  GtkWindow *parent = gtk_application_get_active_window (GTK_APPLICATION (self));

  adw_show_about_dialog (GTK_WIDGET (parent),
                         "application-name", "hy",
                         "application-icon", HY_APP_ID,
                         "version", HY_VERSION,
                         "comments", "Workspace-organized AI conversations",
                         "developer-name", "restartfu",
                         "license-type", GTK_LICENSE_MIT_X11,
                         NULL);
}

static void
on_quit_action (GSimpleAction *action,
                GVariant      *param,
                gpointer       user_data)
{
  g_application_quit (G_APPLICATION (user_data));
}

static const GActionEntry app_actions[] = {
  { "about", on_about_action, NULL, NULL, NULL, { 0 } },
  { "quit",  on_quit_action,  NULL, NULL, NULL, { 0 } },
};

static void
hy_application_startup (GApplication *app)
{
  HyApplication *self = HY_APPLICATION (app);

  G_APPLICATION_CLASS (hy_application_parent_class)->startup (app);

  self->settings = g_settings_new (HY_APP_ID);

  g_action_map_add_action_entries (G_ACTION_MAP (self), app_actions,
                                   G_N_ELEMENTS (app_actions), self);

  gtk_application_set_accels_for_action (GTK_APPLICATION (self), "app.quit",
                                         (const char *[]) { "<primary>q", NULL });
}

static void
hy_application_dispose (GObject *object)
{
  HyApplication *self = HY_APPLICATION (object);

  g_clear_object (&self->settings);

  G_OBJECT_CLASS (hy_application_parent_class)->dispose (object);
}

static void
hy_application_class_init (HyApplicationClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);
  GApplicationClass *app_class = G_APPLICATION_CLASS (klass);

  object_class->dispose = hy_application_dispose;
  app_class->activate = hy_application_activate;
  app_class->startup = hy_application_startup;
}

static void
hy_application_init (HyApplication *self)
{
}
