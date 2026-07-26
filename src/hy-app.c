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

/*
 * Tightens the default GNOME scale.
 *
 * Adwaita is sized for touch-friendly desktop apps: a tool window that is
 * mostly a tree and a transcript gets far less on screen than it has room
 * for. This trims the type and the row heights to roughly what t3 shows,
 * without imposing a fixed pixel size on the text itself -- em keeps it
 * following whatever the desktop font is set to.
 */
static const char *HY_STYLE =
  /*
   * A near-black palette rather than Adwaita's grey.
   *
   * These are libadwaita's own colour names, so overriding them here reaches
   * every widget that follows the theme -- including ones hy never touches --
   * instead of restyling each in turn and missing some.
   */
  "@define-color window_bg_color #000000;\n"
  "@define-color view_bg_color #000000;\n"
  "@define-color headerbar_bg_color #000000;\n"
  "@define-color sidebar_bg_color #050506;\n"
  "@define-color popover_bg_color #0e0e10;\n"
  "@define-color dialog_bg_color #0e0e10;\n"
  "@define-color card_bg_color #0e0e10;\n"
  "@define-color window_fg_color #f2f2f4;\n"
  "@define-color view_fg_color #f2f2f4;\n"

  "window { font-size: 0.9em; }\n"

  /* Flat: the window is one surface, so the bars that divide it are told
   * apart by spacing rather than by lines and shading. */
  "headerbar { min-height: 38px; background: transparent; box-shadow: none;"
  " border: none; }\n"
  "headerbar button { min-height: 26px; min-width: 26px; padding: 2px 6px; }\n"
  "paned > separator { background: alpha(currentColor, 0.06); }\n"

  /* The tree: rows sized to their text, and rounded so a selection reads as
   * a highlight rather than as a band across the pane. */
  "listview > row { min-height: 0; padding: 4px 8px; margin: 0 6px;"
  " border-radius: 8px; }\n"
  "listview > row label { padding: 0; }\n"
  "listview > row:selected { background: alpha(currentColor, 0.10); }\n"
  "listview > row:hover:not(:selected) { background: alpha(currentColor, 0.05); }\n"

  /* Controls: pills, sized for a toolbar rather than a dialog. */
  "button, dropdown > button, togglebutton { border-radius: 8px; }\n"
  "button.flat, dropdown > button, togglebutton { min-height: 24px; }\n"
  "togglebutton:checked { background: alpha(@accent_bg_color, 0.22);"
  " color: @accent_fg_color; }\n"
  "button.circular { border-radius: 9999px; }\n"

  /* What the user typed, and the box they type into: the same rounded shape,
   * so a message looks like what the composer produces. */
  ".card { border-radius: 12px; }\n"
  "frame, frame > border { border-radius: 16px; border-color:"
  " alpha(currentColor, 0.08); }\n"
  /* The composer is the one thing on screen the user acts on, so it gets
   * room rather than being another thin bar. */
  "frame > box { padding: 4px; }\n"
  "textview, textview text { background: transparent; }\n"

  "popover > contents { border-radius: 12px; padding: 4px; }\n"
  "popover menuitem { border-radius: 8px; }\n"

  /* Out of the way until used, which keeps a long transcript from being
   * framed by a bar down its side. */
  "scrollbar { background: transparent; border: none; }\n"
  "scrollbar slider { min-width: 6px; min-height: 6px;"
  " background: alpha(currentColor, 0.18); }\n"
  "scrollbar slider:hover { background: alpha(currentColor, 0.32); }\n"

  /* A chat waiting to be answered, in a tree the user may not be looking at.
   * Slow enough to notice without being the thing you look at. */
  "@keyframes hy-pulse { from { opacity: 1; } to { opacity: 0.25; } }\n"
  ".hy-waiting { color: @accent_color;"
  " animation: hy-pulse 1.4s ease-in-out infinite alternate; }\n";

static void
load_style (void)
{
  g_autoptr (GtkCssProvider) provider = gtk_css_provider_new ();

  gtk_css_provider_load_from_string (provider, HY_STYLE);
  gtk_style_context_add_provider_for_display (gdk_display_get_default (),
                                              GTK_STYLE_PROVIDER (provider),
                                              GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
}

static void
hy_application_startup (GApplication *app)
{
  HyApplication *self = HY_APPLICATION (app);

  G_APPLICATION_CLASS (hy_application_parent_class)->startup (app);

  self->settings = g_settings_new (HY_APP_ID);

  /* The palette above is hand-picked for a dark window; in light it would be
   * black text on black. */
  adw_style_manager_set_color_scheme (adw_style_manager_get_default (),
                                      ADW_COLOR_SCHEME_FORCE_DARK);

  load_style ();

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
