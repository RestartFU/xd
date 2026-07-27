#include "xd-app.h"

#include <glib/gstdio.h>
#include "xd-window.h"

struct _XdApplication
{
  AdwApplication parent_instance;

  GSettings *settings;
};

G_DEFINE_FINAL_TYPE (XdApplication, xd_application, ADW_TYPE_APPLICATION)

XdApplication *
xd_application_new (void)
{
  return g_object_new (XD_TYPE_APPLICATION,
                       "application-id", XD_APP_ID,
                       "flags", G_APPLICATION_DEFAULT_FLAGS,
                       NULL);
}

GSettings *
xd_application_get_settings (XdApplication *self)
{
  g_return_val_if_fail (XD_IS_APPLICATION (self), NULL);

  return self->settings;
}

static void
xd_application_activate (GApplication *app)
{
  GtkWindow *window = gtk_application_get_active_window (GTK_APPLICATION (app));

  if (window == NULL)
    window = GTK_WINDOW (xd_window_new (XD_APPLICATION (app)));

  gtk_window_present (window);
}

static void
on_about_action (GSimpleAction *action,
                 GVariant      *param,
                 gpointer       user_data)
{
  XdApplication *self = user_data;
  GtkWindow *parent = gtk_application_get_active_window (GTK_APPLICATION (self));

  adw_show_about_dialog (GTK_WIDGET (parent),
                         "application-name", "xd",
                         "application-icon", XD_APP_ID,
                         "version", XD_VERSION_STRING,
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
static const char *XD_STYLE =
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
   * The surfaces, by a class xd puts on its own widgets.
   *
   * Overriding libadwaita's colours has now failed twice -- once because the
   * name it reads changed, once because the widget painting the background is
   * not the one the selector names. A class on a widget xd created is the one
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
  ".xd-surface, .xd-surface > *, .xd-sidebar, .xd-sidebar > *,"
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
  "button, dropdown > button, entry, .osd"
  " { background-color: alpha(#ffffff, 0.05); border-color:"
  " alpha(#ffffff, 0.07); }\n"
  "button:hover, dropdown > button:hover"
  " { background-color: alpha(#ffffff, 0.09); }\n"

  /* The bar under the composer: what is being worked on, not a control. */
  ".xd-context { background-color: alpha(#ffffff, 0.025); border-radius: 0 0 14px 14px;"
  " padding: 4px 12px; }\n"
  ".xd-context label { font-size: 0.85em; }\n"

  /*
   * DM Sans, which is what t3code itself is set in; Inter and Cantarell
   * behind it as the fallbacks the bundle already carried. Emoji fonts stay
   * out of this explicit list: Pango can otherwise choose their keycap
   * components for ordinary digits. Fontconfig still finds the bundled emoji
   * font for glyphs these text faces do not cover.
   */
  "window { font-family: \"DM Sans\", \"Inter\", \"Cantarell\", sans-serif;"
  " font-size: 0.95em; }\n"

  /* Flat: the window is one surface, so the bars that divide it are told
   * apart by spacing rather than by lines and shading. */
  /* The line under the title runs the full width, which is what lets the
   * vertical dividers stop at it instead of looking cut short. */
  /*
   * One height for every header bar, or their bottom borders land on
   * different rows: each bar sizes to its own content, and the sidebar's is
   * not the chat's. The height is fixed and the children are kept small
   * enough to fit inside it.
   */
  "headerbar { min-height: 42px; padding-top: 5px; padding-bottom: 5px;"
  " background: transparent; box-shadow: none;"
  " border: none; border-bottom: 1px solid #2a2a2d; }\n"
  "headerbar button, headerbar menubutton > button { min-height: 26px;"
  " min-width: 26px; padding: 2px 6px; margin-top: 0; margin-bottom: 0; }\n"
  /* A hairline, not a gutter. Two pixels rather than one so it can still be
   * grabbed -- the separator is its own drag handle, and a line you cannot
   * catch would mean the panes stop being resizable. */
  /* Invisible until reached for. The panes already end where they end; the
   * line only mattered as somewhere to grab, so it appears when hovered and
   * keeps its width the rest of the time. */
  /*
   * A visible line at every pane boundary, running the full height of the
   * split -- past the composer and down the terminal, so the division stays
   * clear where the panes are darkest.
   */
  /*
   * The separator itself is never drawn. Give it a forgiving hit target,
   * though: a one-pixel invisible handle made pane resizing impractical.
   * The visible line remains a border on the pane beside it.
   */
  "paned > separator { min-width: 8px; min-height: 8px; border: none;"
  " opacity: 0; }\n"
  ".xd-divider-left { border-left: 1px solid #2a2a2d; }\n"
  ".xd-divider-top { border-top: 1px solid #2a2a2d; }\n"

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
  ".xd-composer button, .xd-composer togglebutton,"
  " .xd-composer dropdown > button { background: none; border: none;"
  " box-shadow: none; padding: 4px 10px; }\n"
  /* AdwButtonContent's icon and label sit in this internal box. Keep their
   * gap explicit: theme defaults differ, and on macOS they can touch. */
  ".xd-composer button buttoncontent > box { border-spacing: 6px; }\n"
  /* Settings, not statements: they say how the next message will be handled,
   * which is worth reading once and then ignoring. */
  ".xd-composer button label, .xd-composer dropdown label"
  " { color: alpha(#ffffff, 0.6); }\n"
  ".xd-composer button:hover label { color: alpha(#ffffff, 0.85); }\n"
  /* The active mode in blue, like t3: a GtkToggleButton's CSS node is named
   * plain "button", so earlier togglebutton selectors matched nothing at
   * all -- pressed state included. */
  ".xd-composer button:checked { background: alpha(#3584e4, 0.22);"
  " border-radius: 8px; }\n"
  ".xd-composer button:checked label, .xd-composer button:checked image"
  " { color: #6bb2f8; }\n"
  /* Elsewhere -- the terminal and diff toggles in the header -- checked is a
   * plain lift, which the flat background rules above would otherwise
   * swallow. */
  "button:checked { background-color: alpha(#ffffff, 0.14); }\n"
  ".xd-composer button:hover, .xd-composer togglebutton:hover,"
  " .xd-composer dropdown > button:hover"
  " { background: alpha(currentColor, 0.08); }\n"

  /* Except the one that sends or stops, which is the action rather than a
   * setting. Keep both states the same size so changing state cannot move the
   * rest of the composer. */
  ".xd-composer button.suggested-action,"
  " .xd-composer button.destructive-action { color: #ffffff;"
  " border-radius: 9999px; min-width: 28px; min-height: 28px; padding: 4px; }\n"
  ".xd-composer button.suggested-action { background: #3584e4; }\n"
  ".xd-composer button.destructive-action { background: #e01b24; }\n"
  ".xd-composer button.destructive-action:hover { background: #c01c28; }\n"

  /* Controls: pills, sized for a toolbar rather than a dialog. */
  "button, dropdown > button { border-radius: 8px; }\n"
  "button.flat, dropdown > button { min-height: 24px; }\n"
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

  /*
   * The panel is a child widget, never the popover's own chrome.
   *
   * The surface and contents are rendered at the window system's mercy --
   * surface alpha, renderer, scale -- and every sharp-corner report traced
   * back to them. A child widget's rounded background is ordinary scene
   * geometry, identical under every renderer. Contents carry nothing, and
   * zero padding makes the surface exactly the panel's rectangle, so there
   * is no ring around it left to mispaint.
   */
  "popover > contents { background: none; border: none; box-shadow: none;"
  " padding: 0; }\n"
  "popover listview { background-color: #16161b;"
  " border: 1px solid alpha(#ffffff, 0.10); border-radius: 12px;"
  " padding: 5px; }\n"
  ".xd-menu { background-color: #16161b;"
  " border: 1px solid alpha(#ffffff, 0.10); border-radius: 12px;"
  " padding: 6px; }\n"
  ".xd-menu-popover > contents { background-color: #16161b;"
  " border: 1px solid alpha(#ffffff, 0.10); border-radius: 12px;"
  " padding: 5px; }\n"
  "popover menuitem { border-radius: 8px; padding: 6px 10px; }\n"
  /*
   * Dialogs, in the app's own register rather than the system's.
   *
   * An alert dialog arrives as a pale rounded sheet with heavy padding and
   * full-width buttons -- correct for GNOME, foreign here. Same panel tone
   * as the menus, same radius, buttons that read as the composer's do.
   */
  "dialog, .dialog-content, alertdialog > * { background-color: #16161b; }\n"
  ".dialog-content, alertdialog { border-radius: 14px;"
  " border: 1px solid alpha(#ffffff, 0.10); }\n"
  "alertdialog .title { font-size: 1.05em; font-weight: 700; }\n"
  "alertdialog .response-area button { min-height: 30px; border-radius: 9px;"
  " background-color: alpha(#ffffff, 0.06); }\n"
  "alertdialog .response-area button.suggested-action"
  " { background-color: #3584e4; color: #ffffff; }\n"
  "alertdialog .response-area button.destructive-action"
  " { background-color: alpha(#e01b24, 0.85); color: #ffffff; }\n"
  /* The rows inside a dialog: the same card the transcript uses, not the
   * system's white-ish list. */
  "row.entry, row.combo, row.action, preferencesgroup listview > row"
  " { background-color: alpha(#ffffff, 0.05); border-radius: 10px; }\n"
  "row.entry:focus-within { background-color: alpha(#ffffff, 0.08); }\n"
  ".xd-inline-image picture { border-radius: 10px; }\n"
  /* The dropdown's open list: room for the two lines, a rounded hover, and
   * no band of selection colour behind the one already chosen. */
  /* The list widgets inside paint their own lighter slab over the popover's
   * fill unless told not to; the rows should sit on the popover itself. The
   * model picker is a GtkListBox, whose node is "list", beside the
   * GtkListView the dropdowns use. */
  "popover list, popover scrolledwindow, popover viewport,"
  " popover box { background: none; }\n"
  /* Chosen with the pointer, so the keyboard focus ring is drawn where no
   * keyboard is involved; hover and selection already say where you are. */
  "popover listview > row, popover list > row { outline: none; }\n"
  "popover list > row { border-radius: 10px; }\n"
  "popover list > row:selected { background: alpha(#ffffff, 0.07); }\n"
  "popover list > row:hover:not(:selected)"
  " { background: alpha(#ffffff, 0.05); }\n"
  "popover listview > row { border-radius: 10px; padding: 8px 12px; }\n"
  "popover listview > row:selected { background: alpha(#ffffff, 0.07); }\n"
  "popover listview > row:hover:not(:selected)"
  " { background: alpha(#ffffff, 0.05); }\n"

  /* Nothing has a scrollbar except the transcript, and that one is a thin
   * overlay: everywhere else the content says how long it is. */
  /* GTK fades the edge of a scrolled view to say there is more past it. The
   * transcript already ends in the composer, so the fade only ever appeared
   * as a smear across the last line. */
  "scrolledwindow > undershoot.top, scrolledwindow > undershoot.bottom,"
  " scrolledwindow > undershoot.left, scrolledwindow > undershoot.right,"
  " scrolledwindow > overshoot.top, scrolledwindow > overshoot.bottom"
  " { background: none; box-shadow: none; }\n"

  /*
   * The trough as well as the slider.
   *
   * A scrollbar is scrollbar > trough > slider, and styling only the slider
   * left the trough drawing a full-height band beside the transcript -- wider
   * than the slider inside it, and always there.
   */
  "scrollbar, scrollbar > range, scrollbar > range > trough,"
  " scrollbar trough { background: none; background-image: none;"
  " border: none; box-shadow: none; min-width: 0; margin: 0; padding: 0; }\n"
  "scrollbar slider { min-width: 4px; min-height: 4px; border: none;"
  " margin: 2px; border-radius: 4px; background: alpha(#ffffff, 0.14); }\n"
  "scrollbar slider:hover { background: alpha(#ffffff, 0.28); }\n"

  /*
   * The options under a question: outlined, not filled.
   *
   * As solid slabs they carried more visual weight than the reply above
   * them; a choice is an offer, and an offer reads better as a quiet
   * outline that lifts when approached.
   */
  ".xd-choice { background: none; border: 1px solid alpha(#ffffff, 0.10);"
  " border-radius: 10px; padding: 7px 14px; }\n"
  ".xd-choice label { color: alpha(#ffffff, 0.65); }\n"
  ".xd-choice:hover { background: alpha(#ffffff, 0.05);"
  " border-color: alpha(#ffffff, 0.18); }\n"
  ".xd-choice:hover label { color: alpha(#ffffff, 0.95); }\n"

  /* Compact context usage beside the selected model. */
  "progressbar.xd-context-meter { font-size: 0.82em; }\n"
  "progressbar.xd-context-meter > trough,"
  " progressbar.xd-context-meter > trough > progress"
  " { min-height: 5px; border-radius: 5px; }\n"

  /* The terminal's tabs: the chosen one carries a fill, and every tab keeps
   * enough width that the title and its close button stop fighting. */
  "tabbar { background: none; box-shadow: none; }\n"
  "tabbar tabbox { background: none; margin: 0; padding: 0; }\n"
  "tabbar tab { border-radius: 0; margin: 0; padding: 5px 8px;"
  " min-width: 110px; }\n"
  "tabbar tab:selected, tabbar tab:checked"
  " { background: alpha(#ffffff, 0.10); }\n"
  /* No X on the tabs: the trash can kills the selected session, and one way
   * to do a destructive thing is enough. */
  "tabbar tab button { opacity: 0; min-width: 0; min-height: 0;"
  " padding: 0; margin: 0; border: none; }\n"

  /* Code blocks: a card of their own, since Pango cannot draw a padded
   * rounded background behind a run of text. */
  ".xd-code { background-color: alpha(#ffffff, 0.04);"
  " border: 1px solid alpha(#ffffff, 0.06); border-radius: 10px;"
  " padding: 10px 12px; }\n"
  /* Durable external work: visibly separate from agent prose, but quiet
   * enough to stay in the timeline after the run has finished. */
  ".xd-status { background-color: alpha(#3584e4, 0.08);"
  " border: 1px solid alpha(#3584e4, 0.22); border-radius: 10px; }\n"
  ".xd-workflow-log { padding: 8px 10px;"
  " background: alpha(#000000, 0.18); border-radius: 7px;"
  " font-family: \"JetBrains Mono\", monospace; font-size: 0.90em; }\n"
  ".xd-subagent { background-color: alpha(#a56de2, 0.07);"
  " border: 1px solid alpha(#a56de2, 0.22);"
  " border-left: 3px solid alpha(#a56de2, 0.72); border-radius: 10px; }\n"
  ".xd-code label { font-family: \"JetBrains Mono\", monospace;"
  " font-size: 1em; }\n"
  ".xd-code textview.xd-diff, .xd-code textview.xd-diff text"
  " { background: transparent; font-size: 1em; }\n"
  ".xd-code.xd-inline-diff { padding: 0; }\n"

  /* Structured diff pane. One label holds the complete patch: hundreds of
   * child widgets made scrolling stutter on every frame. */
  ".xd-diff-text { min-width: 480px; padding: 7px 10px;"
  " font-family: \"JetBrains Mono\", monospace; font-size: 1em; }\n"
  ".xd-diff-expander > title { padding: 9px 12px; }\n"

  /* Repository browser: the list and preview share the same quiet side pane
   * as changes, so files read like navigation rather than message cards. */
  ".xd-file-list { padding: 5px; background: transparent; }\n"
  ".xd-file-list > row { border-radius: 8px; margin: 2px 0; }\n"
  ".xd-file-list > row:selected { background: alpha(#ffffff, 0.09); }\n"
  ".xd-file-list > row:hover:not(:selected)"
  " { background: alpha(#ffffff, 0.05); }\n"
  "textview.xd-file-preview, textview.xd-file-preview text"
  " { background: transparent; font-family: \"JetBrains Mono\", monospace;"
  " font-size: 0.94em; }\n"

  /* Selectable, but not editable-looking: a caret blinking in a message
   * suggests typing somewhere nothing can be typed. Selection keeps its
   * colour; only the caret goes. */
  ".xd-body { caret-color: transparent; }\n"

  /* A chat waiting to be answered, in a tree the user may not be looking at.
   * Slow enough to notice without being the thing you look at. */
  "@keyframes xd-pulse { from { opacity: 1; } to { opacity: 0.25; } }\n"
  ".xd-waiting { color: @accent_color;"
  " animation: xd-pulse 1.4s ease-in-out infinite alternate; }\n"
  ".xd-done { color: @success_color;"
  " animation: xd-pulse 1.4s ease-in-out infinite alternate; }\n"

  /* The blue sidebar update control keeps its icon readable. Its download
   * icon fades while an update is available or moving; restart settles into
   * a still reload icon. */
  ".xd-update image { color: #ffffff; }\n"
  ".xd-update:disabled image { color: alpha(#ffffff, 0.35); }\n"
  ".xd-update-fade image {"
  " animation: xd-pulse 1.4s ease-in-out infinite alternate; }\n"

  /* A remote that is not answering. Still, not pulsing: it is a state the row
   * may sit in for hours, and nothing is being waited on. */
  ".xd-offline { color: @error_color; }\n"

  /* The first row sits directly under the header, and on a desktop with larger
   * text its ascenders met that edge. A row of clearance costs nothing. */
  ".xd-sidebar listview { padding-top: 4px; }\n"

  /* The entry a row becomes while it is being named. Sized to the row rather
   * than to a form, so naming something does not make the tree jump. */
  ".xd-inline-entry { min-height: 0; padding: 0 4px; }\n";

static void
load_style (void)
{
  g_autoptr (GtkCssProvider) provider = gtk_css_provider_new ();

  gtk_css_provider_load_from_string (provider, XD_STYLE);
  gtk_style_context_add_provider_for_display (gdk_display_get_default (),
                                              GTK_STYLE_PROVIDER (provider),
                                              GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
}

static gboolean
is_button (GtkWidget *widget)
{
  return GTK_IS_BUTTON (widget) || GTK_IS_MENU_BUTTON (widget);
}

static void
update_pointer_cursor (GtkEventControllerMotion *controller,
                       double                    x,
                       double                    y,
                       gpointer                  user_data)
{
  GtkWidget *root = gtk_event_controller_get_widget (
    GTK_EVENT_CONTROLLER (controller));
  GtkWidget *target = gtk_widget_pick (root, x, y, GTK_PICK_DEFAULT);
  gboolean over_button = FALSE;

  for (GtkWidget *widget = target;
       widget != NULL;
       widget = gtk_widget_get_parent (widget))
    {
      if (is_button (widget))
        {
          over_button = gtk_widget_is_sensitive (widget);
          break;
        }
    }

  gtk_widget_set_cursor_from_name (root, over_button ? "pointer" : NULL);
}

static void
clear_pointer_cursor (GtkEventControllerMotion *controller,
                      gpointer                  user_data)
{
  gtk_widget_set_cursor (
    gtk_event_controller_get_widget (GTK_EVENT_CONTROLLER (controller)), NULL);
}

/*
 * GTK themes deliberately keep the default arrow over buttons. xd uses the
 * same pointer affordance as its web-shaped composer and choice controls.
 *
 * Watch each application window at capture phase rather than setting every
 * button one by one: buttons are also created later for choices, attachments
 * and dialogs, and those should not quietly miss the rule.
 */
static void
on_window_added (GtkApplication *application,
                 GtkWindow      *window,
                 gpointer        user_data)
{
  GtkEventController *motion = gtk_event_controller_motion_new ();

  gtk_event_controller_set_propagation_phase (motion, GTK_PHASE_CAPTURE);
  g_signal_connect (motion, "enter",
                    G_CALLBACK (update_pointer_cursor), NULL);
  g_signal_connect (motion, "motion",
                    G_CALLBACK (update_pointer_cursor), NULL);
  g_signal_connect (motion, "leave",
                    G_CALLBACK (clear_pointer_cursor), NULL);
  gtk_widget_add_controller (GTK_WIDGET (window), motion);
}

static void
xd_application_startup (GApplication *app)
{
  XdApplication *self = XD_APPLICATION (app);

  G_APPLICATION_CLASS (xd_application_parent_class)->startup (app);

  /*
   * The project was called hy first. Anything it left behind is moved
   * across rather than abandoned: a rename should not read as data loss.
   */
  if (g_strcmp0 (XD_DATA_NAME, "xd") == 0)
    {
      g_autofree char *was = g_build_filename (g_get_user_data_dir (), "hy", NULL);
      g_autofree char *now = g_build_filename (g_get_user_data_dir (), "xd", NULL);

      if (g_file_test (was, G_FILE_TEST_IS_DIR) && !g_file_test (now, G_FILE_TEST_EXISTS))
        g_rename (was, now);
    }

  self->settings = g_settings_new (XD_APP_ID);

  /* The palette above is hand-picked for a dark window; in light it would be
   * black text on black. */
  adw_style_manager_set_color_scheme (adw_style_manager_get_default (),
                                      ADW_COLOR_SCHEME_FORCE_DARK);

  load_style ();

  g_signal_connect (self, "window-added", G_CALLBACK (on_window_added), NULL);

  g_action_map_add_action_entries (G_ACTION_MAP (self), app_actions,
                                   G_N_ELEMENTS (app_actions), self);

  gtk_application_set_accels_for_action (GTK_APPLICATION (self), "app.quit",
                                         (const char *[]) { "<primary>q", NULL });
}

static void
xd_application_dispose (GObject *object)
{
  XdApplication *self = XD_APPLICATION (object);

  g_clear_object (&self->settings);

  G_OBJECT_CLASS (xd_application_parent_class)->dispose (object);
}

static void
xd_application_class_init (XdApplicationClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);
  GApplicationClass *app_class = G_APPLICATION_CLASS (klass);

  object_class->dispose = xd_application_dispose;
  app_class->activate = xd_application_activate;
  app_class->startup = xd_application_startup;
}

static void
xd_application_init (XdApplication *self)
{
}
