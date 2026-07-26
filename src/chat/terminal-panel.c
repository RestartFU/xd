#include "terminal-panel.h"

#include <vte/vte.h>

#include "util/host-launch.h"

/*
 * Shell sessions, grouped per chat.
 *
 * Each chat owns an AdwTabView of terminals; the views live in a stack and
 * survive chat switches, so coming back to a chat finds its shells where
 * they were. Killing is done by the pty: closing a tab destroys its
 * terminal, the pty goes with it, and the kernel hangs up the shell.
 */

struct _XdTerminalPanel
{
  AdwBin parent_instance;

  char *chat_id;
  char *workdir;

  AdwTabBar *bar;
  GtkStack *stack;
  GHashTable *views;        /* chat id -> AdwTabView, owned by the stack */
};

G_DEFINE_FINAL_TYPE (XdTerminalPanel, xd_terminal_panel, ADW_TYPE_BIN)

enum
{
  SIGNAL_CLOSE_REQUESTED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

#define SCROLLBACK_LINES 10000

/* One Dark's hues, toned to sit on this window: VTE's stock palette is the
 * primaries of a 1990s xterm, and one saturated #00ff00 prompt undoes a
 * theme. */
static const char *TERMINAL_PALETTE[16] = {
  "#23232a", "#e06c75", "#98c379", "#d19a66",
  "#61afef", "#c678dd", "#56b6c2", "#b8bcc8",
  "#5c6370", "#e06c75", "#98c379", "#d19a66",
  "#61afef", "#c678dd", "#56b6c2", "#e6e6ec",
};

static void
apply_colours (VteTerminal *terminal)
{
  gboolean dark = adw_style_manager_get_dark (adw_style_manager_get_default ());
  GdkRGBA foreground, background;
  GdkRGBA palette[16];

  for (gsize i = 0; i < 16; i++)
    gdk_rgba_parse (&palette[i], TERMINAL_PALETTE[i]);

  gdk_rgba_parse (&foreground, dark ? "#d4d4d4" : "#1d1d1d");
  gdk_rgba_parse (&background, dark ? "#0a0a0c" : "#ffffff");

  vte_terminal_set_colors (terminal, &foreground, &background,
                           palette, G_N_ELEMENTS (palette));
}

static AdwTabView *
current_view (XdTerminalPanel *self)
{
  if (self->chat_id == NULL)
    return NULL;

  return g_hash_table_lookup (self->views, self->chat_id);
}

static VteTerminal *
current_terminal (XdTerminalPanel *self)
{
  AdwTabView *view = current_view (self);
  AdwTabPage *page;

  if (view == NULL)
    return NULL;

  page = adw_tab_view_get_selected_page (view);

  return page != NULL ? VTE_TERMINAL (adw_tab_page_get_child (page)) : NULL;
}

/* The shell ended on its own; its tab has nothing left to show. */
static void
on_child_exited (VteTerminal *terminal,
                 int          status,
                 gpointer     user_data)
{
  XdTerminalPanel *self = user_data;
  GHashTableIter iter;
  gpointer view;

  g_hash_table_iter_init (&iter, self->views);
  while (g_hash_table_iter_next (&iter, NULL, &view))
    {
      AdwTabPage *page = adw_tab_view_get_page (ADW_TAB_VIEW (view),
                                                GTK_WIDGET (terminal));

      if (page != NULL)
        {
          adw_tab_view_close_page (ADW_TAB_VIEW (view), page);
          return;
        }
    }
}

static void
spawn_shell (XdTerminalPanel *self,
             VteTerminal     *terminal)
{
  g_auto (GStrv) env = xd_host_environ ();
  g_autofree char *shell = vte_get_user_shell ();
  char *argv[] = { NULL, NULL };

  if (shell == NULL)
    shell = g_strdup (g_environ_getenv (env, "SHELL"));
  if (shell == NULL)
    shell = g_strdup ("/bin/sh");

  argv[0] = shell;

  vte_terminal_spawn_async (terminal, VTE_PTY_DEFAULT,
                            self->workdir, argv, env,
                            G_SPAWN_SEARCH_PATH, NULL, NULL, NULL,
                            -1, NULL, NULL, NULL);
}

static void
add_session (XdTerminalPanel *self,
             AdwTabView      *view)
{
  VteTerminal *terminal = VTE_TERMINAL (vte_terminal_new ());
  AdwTabPage *page;
  g_autoptr (PangoFontDescription) font = NULL;
  g_autofree char *title = NULL;

  vte_terminal_set_scrollback_lines (terminal, SCROLLBACK_LINES);
  vte_terminal_set_scroll_on_output (terminal, FALSE);
  vte_terminal_set_scroll_on_keystroke (terminal, TRUE);
  vte_terminal_set_mouse_autohide (terminal, TRUE);
  vte_terminal_set_cursor_blink_mode (terminal, VTE_CURSOR_BLINK_ON);

  font = pango_font_description_from_string ("JetBrains Mono, Monospace 10");
  vte_terminal_set_font (terminal, font);
  apply_colours (terminal);

  gtk_widget_set_hexpand (GTK_WIDGET (terminal), TRUE);
  gtk_widget_set_vexpand (GTK_WIDGET (terminal), TRUE);

  g_signal_connect (terminal, "child-exited",
                    G_CALLBACK (on_child_exited), self);

  page = adw_tab_view_append (view, GTK_WIDGET (terminal));
  title = self->workdir != NULL ? g_path_get_basename (self->workdir)
                                : g_strdup ("shell");
  adw_tab_page_set_title (page, title);

  spawn_shell (self, terminal);
  adw_tab_view_set_selected_page (view, page);
}

/* The chat's last session is gone, so the panel has nothing to show. */
static void
on_page_closed (AdwTabView *view,
                AdwTabPage *page,
                gpointer    user_data)
{
  XdTerminalPanel *self = user_data;

  if (view == current_view (self) && adw_tab_view_get_n_pages (view) == 1)
    g_signal_emit (self, signals[SIGNAL_CLOSE_REQUESTED], 0);
}

static AdwTabView *
ensure_view (XdTerminalPanel *self,
             const char      *chat_id)
{
  AdwTabView *view = g_hash_table_lookup (self->views, chat_id);

  if (view == NULL)
    {
      view = ADW_TAB_VIEW (adw_tab_view_new ());
      g_signal_connect (view, "close-page", G_CALLBACK (on_page_closed), self);
      gtk_stack_add_named (self->stack, GTK_WIDGET (view), chat_id);
      g_hash_table_insert (self->views, g_strdup (chat_id), view);
    }

  return view;
}

void
xd_terminal_panel_set_chat (XdTerminalPanel *self,
                            const char      *chat_id)
{
  AdwTabView *view;

  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));

  if (g_strcmp0 (self->chat_id, chat_id) == 0)
    return;

  g_free (self->chat_id);
  self->chat_id = g_strdup (chat_id);

  if (chat_id == NULL)
    {
      adw_tab_bar_set_view (self->bar, NULL);
      return;
    }

  view = ensure_view (self, chat_id);
  gtk_stack_set_visible_child (self->stack, GTK_WIDGET (view));
  adw_tab_bar_set_view (self->bar, view);
}

void
xd_terminal_panel_set_workdir (XdTerminalPanel *self,
                               const char      *workdir)
{
  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));

  g_free (self->workdir);
  self->workdir = g_strdup (workdir);
}

void
xd_terminal_panel_start (XdTerminalPanel *self)
{
  AdwTabView *view;

  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));

  if (self->chat_id == NULL || self->workdir == NULL)
    return;

  view = ensure_view (self, self->chat_id);
  if (adw_tab_view_get_n_pages (view) == 0)
    add_session (self, view);
}

void
xd_terminal_panel_activate (XdTerminalPanel *self)
{
  VteTerminal *terminal;

  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));

  xd_terminal_panel_start (self);

  terminal = current_terminal (self);
  if (terminal != NULL)
    gtk_widget_grab_focus (GTK_WIDGET (terminal));
}

void
xd_terminal_panel_forget_chat (XdTerminalPanel *self,
                               const char      *chat_id)
{
  AdwTabView *view;

  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));

  view = g_hash_table_lookup (self->views, chat_id);
  if (view == NULL)
    return;

  /* Destroying the view destroys the terminals, whose ptys close, which is
   * what actually kills the shells. */
  g_hash_table_remove (self->views, chat_id);
  gtk_stack_remove (self->stack, GTK_WIDGET (view));

  if (g_strcmp0 (self->chat_id, chat_id) == 0)
    adw_tab_bar_set_view (self->bar, NULL);
}

/* --- the buttons ----------------------------------------------------------- */

static void
on_new_session (GtkButton *button,
                gpointer   user_data)
{
  XdTerminalPanel *self = user_data;
  AdwTabView *view;

  if (self->chat_id == NULL)
    return;

  view = ensure_view (self, self->chat_id);
  add_session (self, view);
}

/*
 * Kills the session on screen.
 *
 * Closing the page destroys the terminal; the pty closes with it and the
 * kernel hangs up everything attached. If it was the last one, on_page_closed
 * asks for the panel to go too.
 */
static void
on_kill_session (GtkButton *button,
                 gpointer   user_data)
{
  XdTerminalPanel *self = user_data;
  AdwTabView *view = current_view (self);
  AdwTabPage *page;

  if (view == NULL)
    return;

  page = adw_tab_view_get_selected_page (view);
  if (page != NULL)
    adw_tab_view_close_page (view, page);
}

XdTerminalPanel *
xd_terminal_panel_new (void)
{
  return g_object_new (XD_TYPE_TERMINAL_PANEL, NULL);
}

static void
xd_terminal_panel_finalize (GObject *object)
{
  XdTerminalPanel *self = XD_TERMINAL_PANEL (object);

  g_clear_pointer (&self->views, g_hash_table_unref);
  g_free (self->chat_id);
  g_free (self->workdir);

  G_OBJECT_CLASS (xd_terminal_panel_parent_class)->finalize (object);
}

static void
xd_terminal_panel_class_init (XdTerminalPanelClass *klass)
{
  G_OBJECT_CLASS (klass)->finalize = xd_terminal_panel_finalize;

  signals[SIGNAL_CLOSE_REQUESTED] =
    g_signal_new ("close-requested", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 0);
}

static void
xd_terminal_panel_init (XdTerminalPanel *self)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *controls = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 2);
  GtkWidget *new_button = gtk_button_new_from_icon_name ("list-add-symbolic");
  GtkWidget *kill_button = gtk_button_new_from_icon_name ("user-trash-symbolic");
  GtkWidget *overlay = gtk_overlay_new ();

  self->views = g_hash_table_new_full (g_str_hash, g_str_equal, g_free, NULL);

  self->bar = ADW_TAB_BAR (adw_tab_bar_new ());
  adw_tab_bar_set_autohide (self->bar, TRUE);

  self->stack = GTK_STACK (gtk_stack_new ());
  gtk_widget_set_vexpand (GTK_WIDGET (self->stack), TRUE);

  gtk_box_append (GTK_BOX (box), GTK_WIDGET (self->bar));
  gtk_box_append (GTK_BOX (box), GTK_WIDGET (self->stack));

  gtk_widget_add_css_class (new_button, "flat");
  gtk_widget_set_tooltip_text (new_button, "New session");
  g_signal_connect (new_button, "clicked", G_CALLBACK (on_new_session), self);

  gtk_widget_add_css_class (kill_button, "flat");
  gtk_widget_set_tooltip_text (kill_button, "Kill this session");
  g_signal_connect (kill_button, "clicked", G_CALLBACK (on_kill_session), self);

  gtk_box_append (GTK_BOX (controls), new_button);
  gtk_box_append (GTK_BOX (controls), kill_button);
  gtk_widget_set_halign (controls, GTK_ALIGN_END);
  gtk_widget_set_valign (controls, GTK_ALIGN_START);
  gtk_widget_set_margin_top (controls, 4);
  gtk_widget_set_margin_end (controls, 16);

  gtk_overlay_set_child (GTK_OVERLAY (overlay), box);
  gtk_overlay_add_overlay (GTK_OVERLAY (overlay), controls);

  adw_bin_set_child (ADW_BIN (self), overlay);
}
