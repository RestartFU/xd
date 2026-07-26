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
   * A black palette rather than Adwaita's grey.
   *
   * Set as custom properties, not with @define-color. libadwaita 1.6 moved
   * its colours to custom properties and kept the @define-color names as
   * aliases it no longer reads itself -- so overriding those parses cleanly,
   * reports nothing, and changes nothing.
   */
  ":root {"
  " --window-bg-color: #000000;"
  " --window-fg-color: #f2f2f4;"
  " --view-bg-color: #000000;"
  " --view-fg-color: #f2f2f4;"
  " --headerbar-bg-color: #000000;"
  " --headerbar-fg-color: #f2f2f4;"
  " --headerbar-backdrop-color: #000000;"
  " --sidebar-bg-color: #060607;"
  " --sidebar-fg-color: #f2f2f4;"
  " --sidebar-backdrop-color: #060607;"
  " --secondary-sidebar-bg-color: #060607;"
  " --popover-bg-color: #101013;"
  " --dialog-bg-color: #101013;"
  " --card-bg-color: #101013;"
  "}\n"

  /*
   * The surfaces, by a class hy puts on its own widgets.
   *
   * Overriding libadwaita's colours has now failed twice -- once because the
   * name it reads changed, once because the widget painting the background is
   * not the one the selector names. A class on a widget hy created is the one
   * thing neither can move out from under.
   */
  /*
   * Layered rather than uniformly black.
   *
   * Everything at #000 leaves nothing to tell the panes apart, and the
   * borders end up doing work that a change of shade does better. Each
   * surface sits a little above the one behind it, with a hairline edge
   * where they meet -- which is what reads as glass.
   */
  ".hy-surface, .hy-surface > *, .hy-sidebar, .hy-sidebar > *,"
  " window, .background, headerbar, .toolbar"
  " { background-color: #0a0a0c; }\n"

  /*
   * Everything above the base is white over it, never a colour of its own.
   *
   * Three hand-picked near-blacks did not sit together -- the terminal read
   * as a hole and the buttons as grey borrowed from somewhere else. One base
   * with everything else a percentage of white over it cannot clash with
   * itself, and stays right if the base moves.
   */
  "button, togglebutton, dropdown > button, entry, .osd"
  " { background-color: alpha(#ffffff, 0.05); border-color:"
  " alpha(#ffffff, 0.07); }\n"
  "button:hover, togglebutton:hover, dropdown > button:hover"
  " { background-color: alpha(#ffffff, 0.09); }\n"

  /* The bar under the composer: what is being worked on, not a control. */
  ".hy-context { background-color: alpha(#ffffff, 0.025); border-radius: 0 0 14px 14px;"
  " padding: 4px 12px; }\n"
  ".hy-context label { font-size: 0.85em; }\n"

  /* Inter, which is what interfaces of this kind are set in and what the
   * bundle now carries; Cantarell is the fallback it shipped with. */
  "window { font-family: \"Inter\", \"Cantarell\", sans-serif; font-size: 0.9em; }\n"

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

  /*
   * The composer's controls are text, not boxes.
   *
   * Each one framed made a row of five buttons read as five separate things
   * to decide; flat, they read as one line of settings with the send button
   * at the end of it.
   */
  ".hy-composer button, .hy-composer togglebutton,"
  " .hy-composer dropdown > button { background: none; border: none;"
  " box-shadow: none; padding: 4px 10px; }\n"
  ".hy-composer togglebutton:checked { background: alpha(#3584e4, 0.20);"
  " color: #7db3f0; }\n"
  ".hy-composer button:hover, .hy-composer togglebutton:hover,"
  " .hy-composer dropdown > button:hover"
  " { background: alpha(currentColor, 0.08); }\n"
  ".hy-composer separator { margin: 6px 2px; }\n"

  /* Except the one that sends, which is the action rather than a setting. */
  ".hy-composer button.suggested-action { background: #3584e4; color: #ffffff;"
  " border-radius: 9999px; min-width: 28px; min-height: 28px; padding: 4px; }\n"

  /* Controls: pills, sized for a toolbar rather than a dialog. */
  "button, dropdown > button, togglebutton { border-radius: 8px; }\n"
  "button.flat, dropdown > button, togglebutton { min-height: 24px; }\n"
  "togglebutton:checked { background: alpha(@accent_bg_color, 0.22);"
  " color: @accent_fg_color; }\n"
  "button.circular { border-radius: 9999px; }\n"

  /* What the user typed, and the box they type into: the same rounded shape,
   * so a message looks like what the composer produces. */
  ".card { border-radius: 12px; background-color: alpha(#ffffff, 0.06);"
  " border: 1px solid alpha(#ffffff, 0.05); }\n"
  "frame, frame > border { border-radius: 16px;"
  " background-color: alpha(#ffffff, 0.04);"
  " border-color: alpha(#ffffff, 0.07); }\n"
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
