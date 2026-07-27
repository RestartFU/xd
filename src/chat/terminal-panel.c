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
  XdRemoteClient *remote;
  GCancellable *remote_loading;
  GHashTable *pending_kills;  /* terminal ids awaiting daemon acknowledgement */
  gboolean focus_next_remote;
  gboolean chat_is_remote;

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

/*
 * VTE understands OSC 8 hyperlinks itself. This match adds ordinary URLs
 * printed by tools which do not emit OSC 8, stopping before common sentence
 * punctuation while retaining URL path and query characters.
 */
#define TERMINAL_URL_PATTERN \
  "(?i)\\b(?:https?|ftp)://[^[:space:]<>\"']*[-[:alnum:]_~/#?&=%+]"

/* One Dark's hues, toned to sit on this window: VTE's stock palette is the
 * primaries of a 1990s xterm, and one saturated #00ff00 prompt undoes a
 * theme. */
static const char *TERMINAL_PALETTE[16] = {
  "#23232a", "#e06c75", "#98c379", "#d19a66",
  "#61afef", "#c678dd", "#56b6c2", "#b8bcc8",
  "#5c6370", "#e06c75", "#98c379", "#d19a66",
  "#61afef", "#c678dd", "#56b6c2", "#e6e6ec",
};

static char *view_key (XdTerminalPanel *self, const char *chat_id);
static AdwTabView *ensure_view (XdTerminalPanel *self, const char *chat_id);
static void load_remote_sessions (XdTerminalPanel *self);

static VteTerminal *
terminal_for_page (AdwTabPage *page)
{
  GtkWidget *child;
  VteTerminal *terminal;

  if (page == NULL)
    return NULL;

  child = adw_tab_page_get_child (page);
  if (VTE_IS_TERMINAL (child))
    return VTE_TERMINAL (child);

  terminal = g_object_get_data (G_OBJECT (child), "remote-terminal-widget");
  return VTE_IS_TERMINAL (terminal) ? terminal : NULL;
}

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
  g_autofree char *key = NULL;

  if (self->chat_id == NULL)
    return NULL;

  key = view_key (self, self->chat_id);
  return g_hash_table_lookup (self->views, key);
}

static VteTerminal *
current_terminal (XdTerminalPanel *self)
{
  AdwTabView *view = current_view (self);
  AdwTabPage *page;

  if (view == NULL)
    return NULL;

  page = adw_tab_view_get_selected_page (view);

  return terminal_for_page (page);
}

static gboolean
view_has_terminal (AdwTabView *view)
{
  for (int i = 0; i < adw_tab_view_get_n_pages (view); i++)
    {
      if (terminal_for_page (adw_tab_view_get_nth_page (view, i)) != NULL)
        return TRUE;
    }

  return FALSE;
}

static char *
view_key (XdTerminalPanel *self,
          const char      *chat_id)
{
  return g_strdup_printf ("%s:%s", self->remote != NULL ? "remote" : "local",
                          chat_id);
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
on_terminal_link_pressed (GtkGestureClick *gesture,
                          int              n_press,
                          double           x,
                          double           y,
                          gpointer         user_data)
{
  VteTerminal *terminal = VTE_TERMINAL (user_data);
  g_autofree char *uri = NULL;

  if (n_press != 1)
    return;

  uri = vte_terminal_check_hyperlink_at (terminal, x, y);
  if (uri == NULL)
    uri = vte_terminal_check_match_at (terminal, x, y, NULL);
  if (uri == NULL)
    return;

  gtk_gesture_set_state (GTK_GESTURE (gesture), GTK_EVENT_SEQUENCE_CLAIMED);
  xd_host_open_uri (uri);
}

static gboolean
on_terminal_key (GtkEventControllerKey *controller,
                 guint                  keyval,
                 guint                  keycode,
                 GdkModifierType        state,
                 gpointer               user_data)
{
  VteTerminal *terminal = VTE_TERMINAL (user_data);
  GdkModifierType copy_modifiers = GDK_CONTROL_MASK | GDK_SHIFT_MASK;

  if ((state & copy_modifiers) == copy_modifiers &&
      gdk_keyval_to_lower (keyval) == GDK_KEY_c)
    {
      vte_terminal_copy_clipboard_format (terminal, VTE_FORMAT_TEXT);
      return GDK_EVENT_STOP;
    }

  if ((state & copy_modifiers) == copy_modifiers &&
      gdk_keyval_to_lower (keyval) == GDK_KEY_v)
    {
      vte_terminal_paste_clipboard (terminal);
      return GDK_EVENT_STOP;
    }

  return GDK_EVENT_PROPAGATE;
}

static void
configure_terminal (VteTerminal *terminal)
{
  g_autoptr (PangoFontDescription) font = NULL;
  g_autoptr (VteRegex) url_regex = NULL;
  g_autoptr (GError) error = NULL;
  GtkEventController *keys;
  GtkGesture *links;
  int tag;

  vte_terminal_set_scrollback_lines (terminal, SCROLLBACK_LINES);
  vte_terminal_set_scroll_on_output (terminal, FALSE);
  vte_terminal_set_scroll_on_keystroke (terminal, TRUE);
  vte_terminal_set_mouse_autohide (terminal, TRUE);
  vte_terminal_set_cursor_blink_mode (terminal, VTE_CURSOR_BLINK_ON);
  vte_terminal_set_allow_hyperlink (terminal, TRUE);

  url_regex = vte_regex_new_for_match (TERMINAL_URL_PATTERN, -1,
                                       VTE_REGEX_FLAGS_DEFAULT, &error);
  if (url_regex != NULL)
    {
      tag = vte_terminal_match_add_regex (terminal, url_regex, 0);
      vte_terminal_match_set_cursor_name (terminal, tag, "pointer");
    }
  else
    {
      g_warning ("cannot enable terminal URL matching: %s", error->message);
    }

  font = pango_font_description_from_string ("JetBrains Mono, Monospace 10");
  vte_terminal_set_font (terminal, font);
  apply_colours (terminal);

  keys = gtk_event_controller_key_new ();
  gtk_event_controller_set_propagation_phase (keys, GTK_PHASE_CAPTURE);
  g_signal_connect (keys, "key-pressed", G_CALLBACK (on_terminal_key), terminal);
  gtk_widget_add_controller (GTK_WIDGET (terminal), keys);

  links = gtk_gesture_click_new ();
  gtk_gesture_single_set_button (GTK_GESTURE_SINGLE (links),
                                 GDK_BUTTON_PRIMARY);
  gtk_event_controller_set_propagation_phase (GTK_EVENT_CONTROLLER (links),
                                              GTK_PHASE_CAPTURE);
  g_signal_connect (links, "pressed",
                    G_CALLBACK (on_terminal_link_pressed), terminal);
  gtk_widget_add_controller (GTK_WIDGET (terminal),
                             GTK_EVENT_CONTROLLER (links));

  gtk_widget_set_hexpand (GTK_WIDGET (terminal), TRUE);
  gtk_widget_set_vexpand (GTK_WIDGET (terminal), TRUE);
}

static void
add_session (XdTerminalPanel *self,
             AdwTabView      *view)
{
  VteTerminal *terminal = VTE_TERMINAL (vte_terminal_new ());
  AdwTabPage *page;
  g_autofree char *title = NULL;

  configure_terminal (terminal);

  g_signal_connect (terminal, "child-exited",
                    G_CALLBACK (on_child_exited), self);

  page = adw_tab_view_append (view, GTK_WIDGET (terminal));
  title = self->workdir != NULL ? g_path_get_basename (self->workdir)
                                : g_strdup ("shell");
  adw_tab_page_set_title (page, title);

  spawn_shell (self, terminal);
  adw_tab_view_set_selected_page (view, page);
}

/* --- terminals whose pty is on the daemon -------------------------------- */

static VteTerminal *
find_remote_terminal (AdwTabView *view,
                      const char *id)
{
  for (int i = 0; i < adw_tab_view_get_n_pages (view); i++)
    {
      AdwTabPage *page = adw_tab_view_get_nth_page (view, i);
      VteTerminal *terminal = terminal_for_page (page);
      const char *at = terminal != NULL
        ? g_object_get_data (G_OBJECT (terminal), "remote-terminal") : NULL;

      if (g_strcmp0 (at, id) == 0)
        return terminal;
    }

  return NULL;
}

static AdwStatusPage *
remote_status_page (AdwTabView *view)
{
  for (int i = 0; i < adw_tab_view_get_n_pages (view); i++)
    {
      AdwTabPage *page = adw_tab_view_get_nth_page (view, i);
      GtkWidget *child = adw_tab_page_get_child (page);

      if (g_object_get_data (G_OBJECT (child), "remote-terminal-status") != NULL)
        return ADW_STATUS_PAGE (child);
    }

  return NULL;
}

static void
show_remote_status (XdTerminalPanel *self,
                    const char      *title,
                    const char      *description)
{
  AdwTabView *view = ensure_view (self, self->chat_id);
  AdwStatusPage *status = remote_status_page (view);

  if (status == NULL)
    {
      AdwTabPage *page;

      status = ADW_STATUS_PAGE (adw_status_page_new ());
      g_object_set_data (G_OBJECT (status), "remote-terminal-status",
                         GINT_TO_POINTER (TRUE));
      adw_status_page_set_icon_name (status, "utilities-terminal-symbolic");
      page = adw_tab_view_append (view, GTK_WIDGET (status));
      adw_tab_page_set_title (page, "Terminal");
      adw_tab_view_set_selected_page (view, page);
    }

  adw_status_page_set_title (status, title);
  adw_status_page_set_description (status, description);
}

/*
 * Called only after a real terminal exists, so removing the selected status
 * page cannot leave the panel empty or trigger its last-page close path.
 */
static void
clear_remote_status (AdwTabView *view)
{
  AdwStatusPage *status = remote_status_page (view);

  if (status != NULL)
    {
      AdwTabPage *page =
        adw_tab_view_get_page (view, GTK_WIDGET (status));

      if (page != NULL)
        adw_tab_view_close_page (view, page);
    }
}

static void
finish_remote_call (GObject      *source,
                    GAsyncResult *result,
                    gpointer      user_data)
{
  g_autoptr (XdTerminalPanel) self = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (reply == NULL &&
      !g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED) &&
      !g_error_matches (error, XD_REMOTE_ERROR, XD_REMOTE_ERROR_DISCONNECTED))
    g_warning ("remote terminal request failed: %s", error->message);
}

typedef struct
{
  XdTerminalPanel *panel;
  char *id;
} PendingKill;

static void
pending_kill_free (PendingKill *kill)
{
  g_object_unref (kill->panel);
  g_free (kill->id);
  g_free (kill);
}

static void
on_remote_killed (GObject      *source,
                  GAsyncResult *result,
                  gpointer      user_data)
{
  PendingKill *kill = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (reply != NULL)
    g_hash_table_remove (kill->panel->pending_kills, kill->id);

  pending_kill_free (kill);
}

static void
send_remote_kill (XdTerminalPanel *self,
                  const char      *id)
{
  PendingKill *kill;

  if (self->remote == NULL || !xd_remote_client_is_connected (self->remote))
    return;

  kill = g_new0 (PendingKill, 1);
  kill->panel = g_object_ref (self);
  kill->id = g_strdup (id);

  xd_remote_client_call_op_async (self->remote, "terminal-kill", "terminal",
                                  id, NULL, on_remote_killed, kill);
}

static void
retry_remote_kills (XdTerminalPanel *self)
{
  GHashTableIter iter;
  gpointer key;

  g_hash_table_iter_init (&iter, self->pending_kills);
  while (g_hash_table_iter_next (&iter, &key, NULL))
    send_remote_kill (self, key);
}

static void
send_terminal_bytes (XdTerminalPanel *self,
                     const char      *id,
                     const guint8    *data,
                     gsize            length)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;
  g_autofree char *encoded = NULL;

  if (self->remote == NULL || !xd_remote_client_is_connected (self->remote))
    return;

  encoded = g_base64_encode (data, length);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "terminal-input");
  json_builder_set_member_name (builder, "terminal");
  json_builder_add_string_value (builder, id);
  json_builder_set_member_name (builder, "data");
  json_builder_add_string_value (builder, encoded);
  json_builder_end_object (builder);
  request = json_builder_get_root (builder);

  xd_remote_client_call_async (self->remote, request, NULL, finish_remote_call,
                               g_object_ref (self));
}

static void
on_remote_commit (VteTerminal *terminal,
                  const char  *text,
                  guint        length,
                  gpointer     user_data)
{
  XdTerminalPanel *self = user_data;
  const char *id = g_object_get_data (G_OBJECT (terminal), "remote-terminal");

  if (id != NULL)
    send_terminal_bytes (self, id, (const guint8 *) text, length);
}

static void
send_remote_resize (XdTerminalPanel *self,
                    const char      *id,
                    guint            columns,
                    guint            rows)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;

  if (self->remote == NULL || !xd_remote_client_is_connected (self->remote))
    return;

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "terminal-resize");
  json_builder_set_member_name (builder, "terminal");
  json_builder_add_string_value (builder, id);
  json_builder_set_member_name (builder, "columns");
  json_builder_add_int_value (builder, columns);
  json_builder_set_member_name (builder, "rows");
  json_builder_add_int_value (builder, rows);
  json_builder_end_object (builder);
  request = json_builder_get_root (builder);

  xd_remote_client_call_async (self->remote, request, NULL, finish_remote_call,
                               g_object_ref (self));
}

static gboolean
watch_remote_size (GtkWidget     *widget,
                   GdkFrameClock *clock,
                   gpointer       user_data)
{
  XdTerminalPanel *self = user_data;
  VteTerminal *terminal = VTE_TERMINAL (widget);
  guint columns = (guint) vte_terminal_get_column_count (terminal);
  guint rows = (guint) vte_terminal_get_row_count (terminal);
  guint old_columns =
    GPOINTER_TO_UINT (g_object_get_data (G_OBJECT (terminal), "remote-columns"));
  guint old_rows =
    GPOINTER_TO_UINT (g_object_get_data (G_OBJECT (terminal), "remote-rows"));
  const char *id = g_object_get_data (G_OBJECT (terminal), "remote-terminal");
  GtkWidget *container =
    g_object_get_data (G_OBJECT (terminal), "remote-container");

  /*
   * Terminal itself is fixed to daemon's canonical geometry. Its scrollable
   * viewport says how many cells this device would prefer when the pane is
   * resized; broadcasting that choice then fixes every emulator to it.
   */
  if (container != NULL)
    {
      guint width = (guint) MAX (gtk_widget_get_width (container), 0);
      guint height = (guint) MAX (gtk_widget_get_height (container), 0);
      guint char_width = (guint) MAX (vte_terminal_get_char_width (terminal), 1);
      guint char_height = (guint) MAX (vte_terminal_get_char_height (terminal), 1);

      if (width > 0 && height > 0)
        {
          columns = MAX (width / char_width, 1);
          rows = MAX (height / char_height, 1);
        }
    }

  /* A canonical size received from the daemon can itself cause a GTK
   * allocation. Observe that allocation without echoing it back as a new
   * device resize. */
  if (g_object_get_data (G_OBJECT (terminal), "remote-applying-size") != NULL)
    {
      g_object_set_data (G_OBJECT (terminal), "remote-applying-size", NULL);
      g_object_set_data (G_OBJECT (terminal), "remote-columns",
                         GUINT_TO_POINTER (columns));
      g_object_set_data (G_OBJECT (terminal), "remote-rows",
                         GUINT_TO_POINTER (rows));
      return G_SOURCE_CONTINUE;
    }

  if (id != NULL && columns > 0 && rows > 0 &&
      (columns != old_columns || rows != old_rows))
    {
      g_object_set_data (G_OBJECT (terminal), "remote-columns",
                         GUINT_TO_POINTER (columns));
      g_object_set_data (G_OBJECT (terminal), "remote-rows",
                         GUINT_TO_POINTER (rows));
      send_remote_resize (self, id, columns, rows);
    }

  return G_SOURCE_CONTINUE;
}

static VteTerminal *
add_remote_session (XdTerminalPanel *self,
                    AdwTabView      *view,
                    const char      *id,
                    const char      *title,
                    guint            columns,
                    guint            rows,
                    JsonArray       *replay)
{
  VteTerminal *terminal = find_remote_terminal (view, id);

  if (terminal == NULL)
    {
      AdwTabPage *page;
      GtkWidget *scroller = gtk_scrolled_window_new ();

      terminal = VTE_TERMINAL (vte_terminal_new ());
      configure_terminal (terminal);
      gtk_widget_set_hexpand (GTK_WIDGET (terminal), FALSE);
      gtk_widget_set_vexpand (GTK_WIDGET (terminal), FALSE);
      gtk_widget_set_halign (GTK_WIDGET (terminal), GTK_ALIGN_START);
      gtk_widget_set_valign (GTK_WIDGET (terminal), GTK_ALIGN_START);
      vte_terminal_set_size (terminal, columns, rows);
      g_object_set_data_full (G_OBJECT (terminal), "remote-terminal",
                              g_strdup (id), g_free);
      g_object_set_data (G_OBJECT (terminal), "remote-columns",
                         GUINT_TO_POINTER (columns));
      g_object_set_data (G_OBJECT (terminal), "remote-rows",
                         GUINT_TO_POINTER (rows));
      g_signal_connect (terminal, "commit", G_CALLBACK (on_remote_commit), self);
      gtk_widget_add_tick_callback (GTK_WIDGET (terminal), watch_remote_size,
                                    self, NULL);

      gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scroller),
                                      GTK_POLICY_AUTOMATIC,
                                      GTK_POLICY_AUTOMATIC);
      gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scroller),
                                     GTK_WIDGET (terminal));
      g_object_set_data (G_OBJECT (scroller), "remote-terminal-widget", terminal);
      g_object_set_data (G_OBJECT (terminal), "remote-container", scroller);

      page = adw_tab_view_append (view, scroller);
      g_object_set_data (G_OBJECT (terminal), "remote-page", page);
      adw_tab_page_set_title (page, title != NULL ? title : "shell");
      adw_tab_view_set_selected_page (view, page);
    }
  else
    {
      AdwTabPage *page =
        g_object_get_data (G_OBJECT (terminal), "remote-page");

      if (page != NULL && title != NULL)
        adw_tab_page_set_title (page, title);

      g_object_set_data (G_OBJECT (terminal), "remote-applying-size",
                         GINT_TO_POINTER (TRUE));
      g_object_set_data (G_OBJECT (terminal), "remote-columns",
                         GUINT_TO_POINTER (columns));
      g_object_set_data (G_OBJECT (terminal), "remote-rows",
                         GUINT_TO_POINTER (rows));
      vte_terminal_set_size (terminal, columns, rows);

      if (replay != NULL)
        {
          /* A reconnect hands over an authoritative replay. */
          vte_terminal_reset (terminal, TRUE, TRUE);
        }
    }

  for (guint i = 0; replay != NULL && i < json_array_get_length (replay); i++)
    {
      JsonObject *item = json_array_get_object_element (replay, i);

      if (json_object_has_member (item, "data"))
        {
          const char *encoded =
            json_object_get_string_member_with_default (item, "data", "");
          g_autofree guchar *data = NULL;
          gsize length = 0;

          data = g_base64_decode (encoded, &length);
          if (length > 0)
            vte_terminal_feed (terminal, (const char *) data, (gssize) length);
        }
      else
        {
          guint at_columns = (guint)
            json_object_get_int_member_with_default (item, "columns", columns);
          guint at_rows = (guint)
            json_object_get_int_member_with_default (item, "rows", rows);

          vte_terminal_set_size (terminal, at_columns, at_rows);
        }
    }

  /* Replay ends at daemon's current canonical geometry. */
  if (replay != NULL)
    {
      g_object_set_data (G_OBJECT (terminal), "remote-applying-size",
                         GINT_TO_POINTER (TRUE));
      g_object_set_data (G_OBJECT (terminal), "remote-columns",
                         GUINT_TO_POINTER (columns));
      g_object_set_data (G_OBJECT (terminal), "remote-rows",
                         GUINT_TO_POINTER (rows));
      vte_terminal_set_size (terminal, columns, rows);
    }

  return terminal;
}

typedef struct
{
  XdTerminalPanel *panel;
  char *chat_id;
} RemoteLoad;

static void
remote_load_free (RemoteLoad *load)
{
  g_object_unref (load->panel);
  g_free (load->chat_id);
  g_free (load);
}

static void
on_remote_listed (GObject      *source,
                  GAsyncResult *result,
                  gpointer      user_data)
{
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;
  g_autoptr (GHashTable) seen =
    g_hash_table_new_full (g_str_hash, g_str_equal, g_free, NULL);
  RemoteLoad *load = user_data;
  XdTerminalPanel *self = load->panel;
  AdwTabView *view;
  JsonArray *rows;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (reply == NULL ||
      self->remote != XD_REMOTE_CLIENT (source) ||
      g_strcmp0 (self->chat_id, load->chat_id) != 0)
    {
      remote_load_free (load);
      return;
    }

  view = ensure_view (self, load->chat_id);
  rows = json_object_has_member (reply, "terminals")
    ? json_object_get_array_member (reply, "terminals") : NULL;

  for (guint i = 0; rows != NULL && i < json_array_get_length (rows); i++)
    {
      JsonObject *row = json_array_get_object_element (rows, i);
      const char *id =
        json_object_get_string_member_with_default (row, "id", NULL);
      const char *title =
        json_object_get_string_member_with_default (row, "title", "shell");
      guint columns = (guint)
        json_object_get_int_member_with_default (row, "columns", 80);
      guint rows_count = (guint)
        json_object_get_int_member_with_default (row, "rows", 24);
      JsonArray *replay = json_object_has_member (row, "replay")
        ? json_object_get_array_member (row, "replay") : NULL;

      if (id == NULL)
        continue;

      add_remote_session (self, view, id, title, columns, rows_count,
                          replay);
      g_hash_table_add (seen, g_strdup (id));
    }

  if (g_hash_table_size (seen) > 0)
    clear_remote_status (view);

  /* Anything absent from daemon's list ended while this device was away. */
  for (gint i = (gint) adw_tab_view_get_n_pages (view) - 1; i >= 0; i--)
    {
      AdwTabPage *page = adw_tab_view_get_nth_page (view, (guint) i);
      VteTerminal *terminal = terminal_for_page (page);
      const char *id = terminal != NULL
        ? g_object_get_data (G_OBJECT (terminal), "remote-terminal") : NULL;

      if (id != NULL && !g_hash_table_contains (seen, id))
        {
          g_object_set_data (G_OBJECT (terminal), "remote-removing",
                             GINT_TO_POINTER (TRUE));
          adw_tab_view_close_page (view, page);
        }
    }

  if (self->focus_next_remote)
    {
      VteTerminal *terminal = current_terminal (self);

      if (terminal != NULL)
        {
          self->focus_next_remote = FALSE;
          gtk_widget_grab_focus (GTK_WIDGET (terminal));
        }
    }

  remote_load_free (load);
}

static void
on_remote_opened_reply (GObject      *source,
                        GAsyncResult *result,
                        gpointer      user_data)
{
  RemoteLoad *load = user_data;
  XdTerminalPanel *self = load->panel;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);

  if (self->remote == XD_REMOTE_CLIENT (source) &&
      g_strcmp0 (self->chat_id, load->chat_id) == 0)
    {
      if (reply != NULL)
        {
          /*
           * The event normally made the tab already. Listing as well makes
           * the request's own answer authoritative if that event was missed,
           * and replays prompt bytes emitted immediately after the open.
           */
          load_remote_sessions (self);
        }
      else if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        {
          const char *message = error != NULL
            ? error->message : "The remote machine did not answer.";
          const char *description =
            g_strcmp0 (message, "Unknown op") == 0
              ? "Update and restart xd on the remote machine, then try again."
              : message;

          self->focus_next_remote = FALSE;
          show_remote_status (self, "Could Not Open Remote Terminal",
                              description);
        }
    }

  remote_load_free (load);
}

static void
load_remote_sessions (XdTerminalPanel *self)
{
  RemoteLoad *load;

  if (self->remote == NULL || self->chat_id == NULL ||
      !xd_remote_client_is_connected (self->remote))
    return;

  if (self->remote_loading != NULL)
    g_cancellable_cancel (self->remote_loading);
  g_clear_object (&self->remote_loading);
  self->remote_loading = g_cancellable_new ();

  load = g_new0 (RemoteLoad, 1);
  load->panel = g_object_ref (self);
  load->chat_id = g_strdup (self->chat_id);

  xd_remote_client_call_op_async (self->remote, "terminal-list", "chat",
                                  self->chat_id, self->remote_loading,
                                  on_remote_listed, load);
}

static void
on_remote_terminal_event (XdRemoteClient *client,
                          JsonObject     *event,
                          gpointer        user_data)
{
  XdTerminalPanel *self = user_data;
  const char *name =
    json_object_get_string_member_with_default (event, "event", NULL);
  const char *chat =
    json_object_get_string_member_with_default (event, "chat", NULL);
  const char *id =
    json_object_get_string_member_with_default (event, "terminal", NULL);
  AdwTabView *view;
  VteTerminal *terminal;

  if (self->remote != client || self->chat_id == NULL || id == NULL ||
      g_strcmp0 (chat, self->chat_id) != 0)
    return;

  view = current_view (self);
  if (view == NULL)
    view = ensure_view (self, self->chat_id);

  if (g_strcmp0 (name, "terminal-opened") == 0)
    {
      const char *title =
        json_object_get_string_member_with_default (event, "title", "shell");
      guint columns = (guint)
        json_object_get_int_member_with_default (event, "columns", 80);
      guint rows = (guint)
        json_object_get_int_member_with_default (event, "rows", 24);

      terminal = add_remote_session (self, view, id, title, columns, rows, NULL);
      clear_remote_status (view);
      if (self->focus_next_remote)
        {
          self->focus_next_remote = FALSE;
          gtk_widget_grab_focus (GTK_WIDGET (terminal));
        }
      return;
    }

  terminal = find_remote_terminal (view, id);
  if (terminal == NULL)
    return;

  if (g_strcmp0 (name, "terminal-output") == 0)
    {
      const char *encoded =
        json_object_get_string_member_with_default (event, "data", NULL);
      g_autofree guchar *data = NULL;
      gsize length = 0;

      if (encoded == NULL)
        return;

      data = g_base64_decode (encoded, &length);
      vte_terminal_feed (terminal, (const char *) data, (gssize) length);
    }
  else if (g_strcmp0 (name, "terminal-resized") == 0)
    {
      guint columns = (guint)
        json_object_get_int_member_with_default (event, "columns", 80);
      guint rows = (guint)
        json_object_get_int_member_with_default (event, "rows", 24);

      g_object_set_data (G_OBJECT (terminal), "remote-applying-size",
                         GINT_TO_POINTER (TRUE));
      g_object_set_data (G_OBJECT (terminal), "remote-columns",
                         GUINT_TO_POINTER (columns));
      g_object_set_data (G_OBJECT (terminal), "remote-rows",
                         GUINT_TO_POINTER (rows));
      vte_terminal_set_size (terminal, columns, rows);
    }
  else if (g_strcmp0 (name, "terminal-closed") == 0)
    {
      AdwTabPage *page =
        g_object_get_data (G_OBJECT (terminal), "remote-page");

      g_hash_table_remove (self->pending_kills, id);
      if (page != NULL)
        {
          g_object_set_data (G_OBJECT (terminal), "remote-removing",
                             GINT_TO_POINTER (TRUE));
          adw_tab_view_close_page (view, page);
        }
    }
}

static void
on_remote_reopened (XdRemoteClient *client,
                    gpointer        user_data)
{
  XdTerminalPanel *self = user_data;

  if (self->remote == client)
    {
      retry_remote_kills (self);
      load_remote_sessions (self);
    }
}

/* The chat's last session is gone, so the panel has nothing to show. */
static gboolean
on_close_page (AdwTabView *view,
               AdwTabPage *page,
               gpointer    user_data)
{
  XdTerminalPanel *self = user_data;
  VteTerminal *terminal = terminal_for_page (page);
  const char *id = terminal != NULL
    ? g_object_get_data (G_OBJECT (terminal), "remote-terminal") : NULL;

  if (id != NULL &&
      g_object_get_data (G_OBJECT (terminal), "remote-removing") == NULL &&
      self->remote != NULL)
    {
      g_hash_table_add (self->pending_kills, g_strdup (id));
      send_remote_kill (self, id);
    }

  if (view == current_view (self) && adw_tab_view_get_n_pages (view) == 1)
    g_signal_emit (self, signals[SIGNAL_CLOSE_REQUESTED], 0);

  /* Non-pinned terminal pages need no confirmation. */
  return GDK_EVENT_PROPAGATE;
}

static AdwTabView *
ensure_view (XdTerminalPanel *self,
             const char      *chat_id)
{
  g_autofree char *key = view_key (self, chat_id);
  AdwTabView *view = g_hash_table_lookup (self->views, key);

  if (view == NULL)
    {
      view = ADW_TAB_VIEW (adw_tab_view_new ());
      g_signal_connect (view, "close-page", G_CALLBACK (on_close_page), self);
      gtk_stack_add_named (self->stack, GTK_WIDGET (view), key);
      g_hash_table_insert (self->views, g_strdup (key), view);
    }

  return view;
}

void
xd_terminal_panel_set_remote (XdTerminalPanel *self,
                              XdRemoteClient  *client)
{
  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));
  g_return_if_fail (client == NULL || XD_IS_REMOTE_CLIENT (client));

  if (self->remote == client)
    return;

  if (self->remote_loading != NULL)
    g_cancellable_cancel (self->remote_loading);
  g_clear_object (&self->remote_loading);

  if (self->remote != NULL)
    g_signal_handlers_disconnect_by_data (self->remote, self);

  g_set_object (&self->remote, client);

  if (self->remote != NULL)
    {
      g_signal_connect (self->remote, "event",
                        G_CALLBACK (on_remote_terminal_event), self);
      g_signal_connect (self->remote, "opened",
                        G_CALLBACK (on_remote_reopened), self);
      if (xd_remote_client_is_connected (self->remote))
        retry_remote_kills (self);
    }
}

void
xd_terminal_panel_set_chat (XdTerminalPanel *self,
                            const char      *chat_id)
{
  AdwTabView *view;
  gboolean is_remote;

  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));

  is_remote = self->remote != NULL;
  if (g_strcmp0 (self->chat_id, chat_id) == 0 &&
      self->chat_is_remote == is_remote)
    return;

  g_free (self->chat_id);
  self->chat_id = g_strdup (chat_id);
  self->chat_is_remote = is_remote;

  if (chat_id == NULL)
    {
      adw_tab_bar_set_view (self->bar, NULL);
      return;
    }

  view = ensure_view (self, chat_id);
  gtk_stack_set_visible_child (self->stack, GTK_WIDGET (view));
  adw_tab_bar_set_view (self->bar, view);

  if (self->remote != NULL)
    load_remote_sessions (self);
}

void
xd_terminal_panel_set_workdir (XdTerminalPanel *self,
                               const char      *workdir)
{
  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));

  g_free (self->workdir);
  self->workdir = g_strdup (workdir);
}

static void
open_remote_session (XdTerminalPanel *self,
                     gboolean         reuse)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;
  g_autofree char *description = NULL;
  RemoteLoad *load;

  if (self->remote == NULL || self->chat_id == NULL)
    return;

  description =
    g_strdup_printf ("Waiting for %s",
                     xd_remote_client_get_host (self->remote));
  show_remote_status (self, "Opening Remote Terminal…", description);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "terminal-open");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, self->chat_id);
  json_builder_set_member_name (builder, "columns");
  json_builder_add_int_value (builder, 80);
  json_builder_set_member_name (builder, "rows");
  json_builder_add_int_value (builder, 24);
  json_builder_set_member_name (builder, "reuse");
  json_builder_add_boolean_value (builder, reuse);
  json_builder_end_object (builder);
  request = json_builder_get_root (builder);

  load = g_new0 (RemoteLoad, 1);
  load->panel = g_object_ref (self);
  load->chat_id = g_strdup (self->chat_id);

  xd_remote_client_call_async (self->remote, request, NULL,
                               on_remote_opened_reply, load);
}

void
xd_terminal_panel_start (XdTerminalPanel *self)
{
  AdwTabView *view;

  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));

  if (self->chat_id == NULL || self->workdir == NULL)
    return;

  view = ensure_view (self, self->chat_id);
  if (!view_has_terminal (view))
    {
      if (self->remote == NULL)
        add_session (self, view);
      else
        open_remote_session (self, TRUE);
    }
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
  else if (self->remote != NULL)
    self->focus_next_remote = TRUE;
}

void
xd_terminal_panel_forget_chat (XdTerminalPanel *self,
                               const char      *chat_id)
{
  const char *prefixes[] = { "local", "remote" };

  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));

  for (guint i = 0; i < G_N_ELEMENTS (prefixes); i++)
    {
      g_autofree char *key = g_strdup_printf ("%s:%s", prefixes[i], chat_id);
      AdwTabView *view = g_hash_table_lookup (self->views, key);

      if (view == NULL)
        continue;

      /* Destroying a local view destroys its ptys. Remote chat deletion is
       * sent by the tree and the daemon closes its ptys itself. */
      g_hash_table_remove (self->views, key);
      gtk_stack_remove (self->stack, GTK_WIDGET (view));
    }

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
  if (self->remote == NULL)
    add_session (self, view);
  else
    open_remote_session (self, FALSE);
}

/*
 * Kills the session on screen.
 *
 * Closing the page destroys the terminal; the pty closes with it and the
 * kernel hangs up everything attached. If it was the last one, on_close_page
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

  if (self->remote_loading != NULL)
    g_cancellable_cancel (self->remote_loading);
  g_clear_object (&self->remote_loading);
  if (self->remote != NULL)
    g_signal_handlers_disconnect_by_data (self->remote, self);
  g_clear_object (&self->remote);
  g_clear_pointer (&self->pending_kills, g_hash_table_unref);
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
  GtkWidget *tabs = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
  GtkWidget *controls = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 2);
  GtkWidget *new_button = gtk_button_new_from_icon_name ("list-add-symbolic");
  GtkWidget *kill_button = gtk_button_new_from_icon_name ("user-trash-symbolic");

  self->views = g_hash_table_new_full (g_str_hash, g_str_equal, g_free, NULL);
  self->pending_kills =
    g_hash_table_new_full (g_str_hash, g_str_equal, g_free, NULL);

  self->bar = ADW_TAB_BAR (adw_tab_bar_new ());
  adw_tab_bar_set_autohide (self->bar, TRUE);

  self->stack = GTK_STACK (gtk_stack_new ());
  gtk_widget_set_vexpand (GTK_WIDGET (self->stack), TRUE);
  gtk_widget_set_hexpand (GTK_WIDGET (self->bar), TRUE);

  gtk_widget_add_css_class (new_button, "flat");
  gtk_widget_set_tooltip_text (new_button, "New session");
  g_signal_connect (new_button, "clicked", G_CALLBACK (on_new_session), self);

  gtk_widget_add_css_class (kill_button, "flat");
  gtk_widget_set_tooltip_text (kill_button, "Kill this session");
  g_signal_connect (kill_button, "clicked", G_CALLBACK (on_kill_session), self);

  gtk_box_append (GTK_BOX (controls), new_button);
  gtk_box_append (GTK_BOX (controls), kill_button);
  gtk_widget_set_valign (controls, GTK_ALIGN_CENTER);
  gtk_widget_set_margin_top (controls, 4);
  gtk_widget_set_margin_start (controls, 4);
  gtk_widget_set_margin_end (controls, 8);

  gtk_box_append (GTK_BOX (tabs), GTK_WIDGET (self->bar));
  gtk_box_append (GTK_BOX (tabs), controls);
  gtk_box_append (GTK_BOX (box), tabs);
  gtk_box_append (GTK_BOX (box), GTK_WIDGET (self->stack));

  adw_bin_set_child (ADW_BIN (self), box);
}
