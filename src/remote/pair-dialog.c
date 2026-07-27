#include "pair-dialog.h"

#include "ui/panel-style.h"

#include <errno.h>

/*
 * Pairing, which is the one moment a client trusts a daemon it has never seen.
 *
 * The code is short-lived and single-use, so this is asked for and answered in
 * one go rather than kept anywhere: what is worth keeping is the token the
 * daemon hands back, and the certificate it offered while doing so.
 */

typedef struct
{
  grefcount refs;
  GtkWindow *window;             /* weak */
  GtkEntry *host;
  GtkEntry *port;
  GtkEntry *code;
  GtkLabel *trouble;
  GtkButton *connect;
  XdRemoteClient *client;
  GCancellable *cancellable;
  XdRemotePairedCallback callback;
  gpointer user_data;
} PairPrompt;

static PairPrompt *
pair_prompt_ref (PairPrompt *prompt)
{
  g_ref_count_inc (&prompt->refs);
  return prompt;
}

static void
pair_prompt_unref (PairPrompt *prompt)
{
  if (!g_ref_count_dec (&prompt->refs))
    return;

  g_clear_object (&prompt->cancellable);
  g_clear_object (&prompt->client);
  g_free (prompt);
}

static void
on_window_gone (gpointer user_data,
                GObject *where_window_was)
{
  PairPrompt *prompt = user_data;

  prompt->window = NULL;
  if (prompt->cancellable != NULL)
    g_cancellable_cancel (prompt->cancellable);

  pair_prompt_unref (prompt);
}

/*
 * Disconnects the weak callback before destroying explicitly. The window's
 * data owns another prompt reference, and an in-flight pair owns one more, so
 * cancellation cannot leave its completion callback holding freed memory.
 */
static void
close_prompt (PairPrompt *prompt)
{
  GtkWindow *window = prompt->window;

  if (window == NULL)
    return;

  prompt->window = NULL;
  if (prompt->cancellable != NULL)
    g_cancellable_cancel (prompt->cancellable);

  g_object_weak_unref (G_OBJECT (window), on_window_gone, prompt);
  pair_prompt_unref (prompt);
  gtk_window_destroy (window);
}

static void
show_error (PairPrompt *prompt,
            const char *message)
{
  if (prompt->window == NULL)
    return;

  gtk_label_set_label (prompt->trouble, message);
  gtk_widget_set_visible (GTK_WIDGET (prompt->trouble), TRUE);
}

static void
set_busy (PairPrompt *prompt,
          gboolean    busy)
{
  if (prompt->window == NULL)
    return;

  gtk_widget_set_sensitive (GTK_WIDGET (prompt->host), !busy);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->port), !busy);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->code), !busy);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->connect), !busy);
  gtk_button_set_label (prompt->connect, busy ? "Connecting…" : "Connect");
}

static void
on_paired (GObject      *source,
           GAsyncResult *result,
           gpointer      user_data)
{
  PairPrompt *prompt = user_data;
  g_autoptr (GError) error = NULL;
  gboolean paired;

  paired = xd_remote_client_pair_finish (XD_REMOTE_CLIENT (source),
                                         result, &error);
  g_clear_object (&prompt->cancellable);

  if (!paired)
    {
      if (prompt->window != NULL)
        {
          show_error (prompt, error->message);
          set_busy (prompt, FALSE);
          gtk_widget_grab_focus (GTK_WIDGET (prompt->code));
        }

      g_clear_object (&prompt->client);
      pair_prompt_unref (prompt);
      return;
    }

  if (prompt->window != NULL)
    {
      prompt->callback (prompt->client, prompt->user_data);
      close_prompt (prompt);
    }

  pair_prompt_unref (prompt);
}

/*
 * The code as the daemon printed it.
 *
 * It is compared exactly, and it was read off a terminal and typed back in, so
 * the spaces and lower case that come with doing that are taken out here
 * rather than turned into "no such pairing code".
 */
static char *
normalise_code (const char *text)
{
  GString *code = g_string_new (NULL);

  for (const char *at = text; *at != '\0'; at++)
    {
      if (!g_ascii_isspace (*at))
        g_string_append_c (code, g_ascii_toupper (*at));
    }

  return g_string_free (code, FALSE);
}

static gboolean
parse_port (const char *text,
            guint16    *port)
{
  char *end = NULL;
  guint64 value;

  if (text == NULL || *text == '\0')
    return FALSE;

  errno = 0;
  value = g_ascii_strtoull (text, &end, 10);
  if (errno == ERANGE || end == text || *end != '\0' ||
      value == 0 || value > G_MAXUINT16)
    return FALSE;

  *port = (guint16) value;
  return TRUE;
}

static void
begin_pairing (PairPrompt *prompt)
{
  g_autofree char *host = NULL;
  g_autofree char *code = NULL;
  guint16 port;

  if (prompt->window == NULL || prompt->cancellable != NULL)
    return;

  host = g_strdup (gtk_editable_get_text (GTK_EDITABLE (prompt->host)));
  g_strstrip (host);
  code = normalise_code (
    gtk_editable_get_text (GTK_EDITABLE (prompt->code)));

  if (*host == '\0')
    {
      show_error (prompt, "Enter the remote machine’s address.");
      gtk_widget_grab_focus (GTK_WIDGET (prompt->host));
      return;
    }

  if (!parse_port (
        gtk_editable_get_text (GTK_EDITABLE (prompt->port)), &port))
    {
      show_error (prompt, "Port must be a number from 1 to 65535.");
      gtk_widget_grab_focus (GTK_WIDGET (prompt->port));
      return;
    }

  if (*code == '\0')
    {
      show_error (prompt, "Enter the code printed by “xd serve --pair”.");
      gtk_widget_grab_focus (GTK_WIDGET (prompt->code));
      return;
    }

  gtk_widget_set_visible (GTK_WIDGET (prompt->trouble), FALSE);
  g_clear_object (&prompt->client);
  prompt->client = xd_remote_client_new (host, port);
  prompt->cancellable = g_cancellable_new ();
  set_busy (prompt, TRUE);

  xd_remote_client_pair_async (prompt->client, code, NULL,
                               prompt->cancellable, on_paired,
                               pair_prompt_ref (prompt));
}

static void
on_connect_clicked (GtkButton *button,
                    gpointer   user_data)
{
  begin_pairing (user_data);
}

static void
on_cancel_clicked (GtkButton *button,
                   gpointer   user_data)
{
  close_prompt (user_data);
}

static gboolean
on_key (GtkEventControllerKey *controller,
        guint                  keyval,
        guint                  keycode,
        GdkModifierType        state,
        gpointer               user_data)
{
  if (keyval != GDK_KEY_Escape)
    return GDK_EVENT_PROPAGATE;

  close_prompt (user_data);
  return GDK_EVENT_STOP;
}

static GtkEntry *
field (GtkBox          *body,
       const char      *title,
       const char      *text,
       GtkInputPurpose  purpose)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 5);
  GtkWidget *label = gtk_label_new (title);
  GtkWidget *entry = gtk_entry_new ();

  gtk_label_set_xalign (GTK_LABEL (label), 0.0f);
  gtk_widget_add_css_class (label, "caption");
  gtk_widget_add_css_class (label, "dim-label");

  gtk_editable_set_text (GTK_EDITABLE (entry), text != NULL ? text : "");
  gtk_entry_set_input_purpose (GTK_ENTRY (entry), purpose);
  gtk_entry_set_activates_default (GTK_ENTRY (entry), TRUE);
  gtk_widget_set_hexpand (entry, TRUE);

  gtk_box_append (GTK_BOX (box), label);
  gtk_box_append (GTK_BOX (box), entry);
  gtk_box_append (body, box);

  return GTK_ENTRY (entry);
}

static GtkWidget *
hint (const char *key,
      const char *what)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  GtkWidget *label = gtk_label_new (key);
  GtkWidget *text = gtk_label_new (what);

  gtk_widget_add_css_class (label, "xd-key");
  gtk_widget_add_css_class (text, "dim-label");
  gtk_widget_add_css_class (text, "caption");
  gtk_box_append (GTK_BOX (box), label);
  gtk_box_append (GTK_BOX (box), text);

  return box;
}

void
xd_remote_pair_dialog_present (GtkWidget              *parent,
                               GSettings              *settings,
                               XdRemotePairedCallback  callback,
                               gpointer                user_data)
{
  g_autofree char *known_host = NULL;
  g_autofree char *known_port = NULL;
  GtkWindow *parent_window;
  PairPrompt *prompt;
  GtkWidget *window;
  GtkWidget *column;
  GtkWidget *header;
  GtkWidget *title;
  GtkWidget *description;
  GtkWidget *body;
  GtkWidget *footer;
  GtkWidget *spacer;
  GtkWidget *cancel;
  GtkEventController *keys;

  g_return_if_fail (GTK_IS_WIDGET (parent));
  g_return_if_fail (callback != NULL);

  xd_panel_style_ensure ();

  known_host = g_settings_get_string (settings, "remote-host");
  known_port = g_strdup_printf (
    "%d", g_settings_get_int (settings, "remote-port"));
  parent_window = GTK_WINDOW (gtk_widget_get_root (parent));

  prompt = g_new0 (PairPrompt, 1);
  g_ref_count_init (&prompt->refs);
  prompt->callback = callback;
  prompt->user_data = user_data;

  window = gtk_window_new ();
  prompt->window = GTK_WINDOW (window);
  gtk_window_set_transient_for (GTK_WINDOW (window), parent_window);
  gtk_window_set_application (
    GTK_WINDOW (window), gtk_window_get_application (parent_window));
  gtk_window_set_destroy_with_parent (GTK_WINDOW (window), TRUE);
  gtk_window_set_modal (GTK_WINDOW (window), TRUE);
  gtk_window_set_decorated (GTK_WINDOW (window), FALSE);
  gtk_window_set_default_size (GTK_WINDOW (window), 620, 460);
  gtk_widget_add_css_class (window, "xd-panel");

  title = gtk_label_new ("Connect to a Remote");
  gtk_label_set_xalign (GTK_LABEL (title), 0.0f);
  gtk_widget_add_css_class (title, "title-3");

  description = gtk_label_new (
    "Run “xd serve --pair” on the other machine, then enter the short-lived "
    "code it prints.");
  gtk_label_set_xalign (GTK_LABEL (description), 0.0f);
  gtk_label_set_wrap (GTK_LABEL (description), TRUE);
  gtk_widget_add_css_class (description, "dim-label");

  header = gtk_box_new (GTK_ORIENTATION_VERTICAL, 5);
  gtk_box_append (GTK_BOX (header), title);
  gtk_box_append (GTK_BOX (header), description);
  gtk_widget_add_css_class (header, "xd-panel-bar");
  gtk_widget_add_css_class (header, "xd-panel-head");

  body = gtk_box_new (GTK_ORIENTATION_VERTICAL, 14);
  gtk_widget_set_margin_top (body, 22);
  gtk_widget_set_margin_bottom (body, 22);
  gtk_widget_set_margin_start (body, 22);
  gtk_widget_set_margin_end (body, 22);
  gtk_widget_set_vexpand (body, TRUE);
  gtk_widget_set_valign (body, GTK_ALIGN_START);

  prompt->host = field (GTK_BOX (body), "Host", known_host,
                        GTK_INPUT_PURPOSE_URL);
  prompt->port = field (GTK_BOX (body), "Port", known_port,
                        GTK_INPUT_PURPOSE_DIGITS);
  prompt->code = field (GTK_BOX (body), "Pairing Code", NULL,
                        GTK_INPUT_PURPOSE_PIN);

  prompt->trouble = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_xalign (prompt->trouble, 0.0f);
  gtk_label_set_wrap (prompt->trouble, TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (prompt->trouble), "error");
  gtk_widget_set_visible (GTK_WIDGET (prompt->trouble), FALSE);
  gtk_box_append (GTK_BOX (body), GTK_WIDGET (prompt->trouble));

  footer = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 12);
  gtk_box_append (GTK_BOX (footer), hint ("Esc", "Cancel"));
  gtk_box_append (GTK_BOX (footer), hint ("Enter", "Connect"));
  spacer = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
  gtk_widget_set_hexpand (spacer, TRUE);
  gtk_box_append (GTK_BOX (footer), spacer);

  cancel = gtk_button_new_with_label ("Cancel");
  gtk_widget_add_css_class (cancel, "flat");
  gtk_box_append (GTK_BOX (footer), cancel);

  prompt->connect = GTK_BUTTON (gtk_button_new_with_label ("Connect"));
  gtk_widget_add_css_class (GTK_WIDGET (prompt->connect), "xd-panel-action");
  gtk_box_append (GTK_BOX (footer), GTK_WIDGET (prompt->connect));
  gtk_widget_add_css_class (footer, "xd-panel-bar");
  gtk_widget_add_css_class (footer, "xd-panel-foot");

  column = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  gtk_box_append (GTK_BOX (column), header);
  gtk_box_append (GTK_BOX (column), body);
  gtk_box_append (GTK_BOX (column), footer);
  gtk_window_set_child (GTK_WINDOW (window), column);
  gtk_window_set_default_widget (GTK_WINDOW (window),
                                 GTK_WIDGET (prompt->connect));

  g_signal_connect (prompt->connect, "clicked",
                    G_CALLBACK (on_connect_clicked), prompt);
  g_signal_connect (cancel, "clicked",
                    G_CALLBACK (on_cancel_clicked), prompt);
  g_signal_connect_swapped (prompt->host, "activate",
                            G_CALLBACK (begin_pairing), prompt);
  g_signal_connect_swapped (prompt->port, "activate",
                            G_CALLBACK (begin_pairing), prompt);
  g_signal_connect_swapped (prompt->code, "activate",
                            G_CALLBACK (begin_pairing), prompt);

  keys = gtk_event_controller_key_new ();
  gtk_event_controller_set_propagation_phase (keys, GTK_PHASE_CAPTURE);
  g_signal_connect (keys, "key-pressed", G_CALLBACK (on_key), prompt);
  gtk_widget_add_controller (window, keys);

  g_object_set_data_full (G_OBJECT (window), "pair-prompt", prompt,
                          (GDestroyNotify) pair_prompt_unref);
  g_object_weak_ref (G_OBJECT (window), on_window_gone,
                     pair_prompt_ref (prompt));

  gtk_window_present (GTK_WINDOW (window));
  gtk_widget_grab_focus (GTK_WIDGET (prompt->code));
}
