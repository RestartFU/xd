#include "terminal-panel.h"

#include <vte/vte.h>

#include "util/host-launch.h"

/*
 * A shell inside the window, in the directory the agent works in.
 *
 * The point is to see what the agent did without leaving the chat: run the
 * tests it changed, read a diff, undo something. It is a plain shell, not a
 * view of the agent's own commands -- those already appear in the transcript.
 */

struct _HyTerminalPanel
{
  AdwBin parent_instance;

  VteTerminal *terminal;
  char *workdir;
  gboolean running;
  gboolean restarting;
};

enum
{
  SIGNAL_CLOSE_REQUESTED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (HyTerminalPanel, hy_terminal_panel, ADW_TYPE_BIN)

/* Enough to scroll back through a build log, bounded so a runaway command
 * cannot grow the process without limit. */
#define SCROLLBACK_LINES 10000

static void
apply_colours (HyTerminalPanel *self)
{
  AdwStyleManager *style = adw_style_manager_get_default ();
  gboolean dark = adw_style_manager_get_dark (style);
  GdkRGBA foreground, background;

  /* VTE defaults to black on white whatever the rest of the window is doing,
   * so the theme has to be followed by hand. */
  gdk_rgba_parse (&foreground, dark ? "#e4e4e8" : "#1d1d1d");
  /*
   * A step above the window, like every other raised surface.
   *
   * The terminal is a panel sitting on the window, not a hole in it, so it
   * takes a clearer lift than the cards do -- it is a large flat area, and at
   * the same few percent as a button it read as a black rectangle rather than
   * as a surface. Matched by hand because VTE takes a colour rather than
   * following the stylesheet.
   */
  gdk_rgba_parse (&background, dark ? "#17171c" : "#ffffff");

  vte_terminal_set_colors (self->terminal, &foreground, &background, NULL, 0);
}

static void
on_dark_changed (AdwStyleManager *style,
                 GParamSpec      *pspec,
                 gpointer         user_data)
{
  apply_colours (user_data);
}

static void
on_child_exited (VteTerminal *terminal,
                 int          status,
                 gpointer     user_data)
{
  HyTerminalPanel *self = user_data;

  self->running = FALSE;

  /* Asked for: clear what the old shell left and start again. */
  if (self->restarting)
    {
      self->restarting = FALSE;
      vte_terminal_reset (terminal, TRUE, TRUE);
      hy_terminal_panel_start (self);
      return;
    }

  /* Otherwise left as it was rather than restarted: a shell that exits
   * immediately would respawn in a loop with nothing to show for it. The next
   * time the panel is opened starts a fresh one. */
  vte_terminal_feed (terminal, "\r\n\033[2m[exited]\033[0m\r\n", -1);
}

static void
on_spawned (VteTerminal *terminal,
            GPid         pid,
            GError      *error,
            gpointer     user_data)
{
  HyTerminalPanel *self = user_data;

  if (error != NULL)
    {
      g_autofree char *message =
        g_strdup_printf ("\r\n\033[31m%s\033[0m\r\n", error->message);

      self->running = FALSE;
      vte_terminal_feed (terminal, message, -1);
    }
}

static void
spawn_shell (HyTerminalPanel *self)
{
  g_auto (GStrv) env = hy_host_environ ();
  g_autofree char *shell = vte_get_user_shell ();
  char *argv[] = { NULL, NULL };

  if (shell == NULL)
    shell = g_strdup (g_environ_getenv (env, "SHELL"));
  if (shell == NULL)
    shell = g_strdup ("/bin/sh");

  argv[0] = shell;
  self->running = TRUE;

  vte_terminal_spawn_async (self->terminal, VTE_PTY_DEFAULT,
                            self->workdir, argv, env,
                            G_SPAWN_SEARCH_PATH, NULL, NULL, NULL,
                            -1, NULL, on_spawned, self);
}

/*
 * Walks the running shell over to @workdir.
 *
 * One shell is shared by every chat, so switching chats has to move it rather
 * than start another: a pty per chat would multiply for no reason, and
 * killing this one would throw away whatever is in it.
 *
 * The line is cleared first, so a half-typed command does not end up with cd
 * stuck on the front of it. If something is running, both go to that program
 * instead -- there is no way to ask a pty whether it is at a prompt, and
 * waiting for one that may never come is worse than a stray line.
 */
static void
follow_workdir (HyTerminalPanel *self)
{
  g_autofree char *quoted = g_shell_quote (self->workdir);
  g_autofree char *command = g_strdup_printf ("\025cd %s\n", quoted);

  vte_terminal_feed_child (self->terminal, command, -1);
}

void
hy_terminal_panel_set_workdir (HyTerminalPanel *self,
                               const char      *workdir)
{
  gboolean changed;

  g_return_if_fail (HY_IS_TERMINAL_PANEL (self));

  changed = g_strcmp0 (self->workdir, workdir) != 0;
  if (!changed)
    return;

  g_free (self->workdir);
  self->workdir = g_strdup (workdir);

  if (self->workdir == NULL)
    return;

  /* Already talking to a shell: move it. Otherwise the panel may have been
   * waiting for this, since it is restored at startup before any chat is
   * chosen, and the shell starts here in the right place. */
  if (self->running)
    follow_workdir (self);
  else if (gtk_widget_get_visible (GTK_WIDGET (self)))
    hy_terminal_panel_start (self);
}

void
hy_terminal_panel_start (HyTerminalPanel *self)
{
  g_return_if_fail (HY_IS_TERMINAL_PANEL (self));

  if (!self->running && self->workdir != NULL)
    spawn_shell (self);
}

void
hy_terminal_panel_activate (HyTerminalPanel *self)
{
  g_return_if_fail (HY_IS_TERMINAL_PANEL (self));

  hy_terminal_panel_start (self);
  gtk_widget_grab_focus (GTK_WIDGET (self->terminal));
}

/*
 * Replaces the shell with a fresh one.
 *
 * A shell that has been cd'd around, or left inside something, is quicker to
 * replace than to unpick. The old one is killed rather than left running: it
 * has no window to appear in.
 */
static void
on_restart_clicked (GtkButton *button,
                    gpointer   user_data)
{
  HyTerminalPanel *self = user_data;

  if (!self->running)
    {
      vte_terminal_reset (self->terminal, TRUE, TRUE);
      hy_terminal_panel_start (self);
      return;
    }

  /* One child per terminal, so the new shell waits for the old one to be
   * gone rather than racing it. */
  self->restarting = TRUE;
  vte_terminal_feed_child (self->terminal, "\004", -1);
}

static void
on_close_clicked (GtkButton *button,
                  gpointer   user_data)
{
  HyTerminalPanel *self = user_data;

  g_signal_emit (self, signals[SIGNAL_CLOSE_REQUESTED], 0);
}

HyTerminalPanel *
hy_terminal_panel_new (void)
{
  return g_object_new (HY_TYPE_TERMINAL_PANEL, NULL);
}

static void
hy_terminal_panel_finalize (GObject *object)
{
  HyTerminalPanel *self = HY_TERMINAL_PANEL (object);

  g_signal_handlers_disconnect_by_data (adw_style_manager_get_default (), self);
  g_free (self->workdir);

  G_OBJECT_CLASS (hy_terminal_panel_parent_class)->finalize (object);
}

static void
hy_terminal_panel_class_init (HyTerminalPanelClass *klass)
{
  G_OBJECT_CLASS (klass)->finalize = hy_terminal_panel_finalize;

  /* The panel cannot take itself off screen: whoever put it there decides
   * that, and has a button to keep in step. */
  signals[SIGNAL_CLOSE_REQUESTED] =
    g_signal_new ("close-requested", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 0);
}

static void
hy_terminal_panel_init (HyTerminalPanel *self)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
  g_autoptr (PangoFontDescription) font = NULL;

  self->terminal = VTE_TERMINAL (vte_terminal_new ());
  vte_terminal_set_scrollback_lines (self->terminal, SCROLLBACK_LINES);
  vte_terminal_set_scroll_on_output (self->terminal, FALSE);
  vte_terminal_set_scroll_on_keystroke (self->terminal, TRUE);
  vte_terminal_set_mouse_autohide (self->terminal, TRUE);
  vte_terminal_set_cursor_blink_mode (self->terminal, VTE_CURSOR_BLINK_ON);

  /* The bundle carries DejaVu Sans Mono; "Monospace" resolves to it there and
   * to whatever the host prefers when one is installed. */
  font = pango_font_description_from_string ("Monospace 10");
  vte_terminal_set_font (self->terminal, font);

  gtk_widget_set_hexpand (GTK_WIDGET (self->terminal), TRUE);
  gtk_widget_set_vexpand (GTK_WIDGET (self->terminal), TRUE);

  /* No scrollbar: a terminal scrolls with the wheel and with the keys, and
   * the bar was a permanent stripe down a panel that is mostly text. */
  gtk_box_append (GTK_BOX (box), GTK_WIDGET (self->terminal));

  /* Over the terminal rather than above it, so the buttons cost no height
   * and the shell keeps the whole panel. */
  {
    GtkWidget *controls = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 2);
    GtkWidget *restart = gtk_button_new_from_icon_name ("list-add-symbolic");
    GtkWidget *close = gtk_button_new_from_icon_name ("user-trash-symbolic");
    GtkWidget *overlay = gtk_overlay_new ();

    gtk_widget_add_css_class (restart, "flat");
    gtk_widget_set_tooltip_text (restart, "Start a fresh shell");
    g_signal_connect (restart, "clicked", G_CALLBACK (on_restart_clicked), self);

    gtk_widget_add_css_class (close, "flat");
    gtk_widget_set_tooltip_text (close, "Close the terminal");
    g_signal_connect (close, "clicked", G_CALLBACK (on_close_clicked), self);

    gtk_box_append (GTK_BOX (controls), restart);
    gtk_box_append (GTK_BOX (controls), close);
    gtk_widget_set_halign (controls, GTK_ALIGN_END);
    gtk_widget_set_valign (controls, GTK_ALIGN_START);
    gtk_widget_set_margin_top (controls, 4);
    gtk_widget_set_margin_end (controls, 16);

    gtk_overlay_set_child (GTK_OVERLAY (overlay), box);
    gtk_overlay_add_overlay (GTK_OVERLAY (overlay), controls);

    adw_bin_set_child (ADW_BIN (self), overlay);
    return;
  }

  g_signal_connect (self->terminal, "child-exited",
                    G_CALLBACK (on_child_exited), self);
  g_signal_connect (adw_style_manager_get_default (), "notify::dark",
                    G_CALLBACK (on_dark_changed), self);
  apply_colours (self);

  adw_bin_set_child (ADW_BIN (self), box);
}
