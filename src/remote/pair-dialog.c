#include "pair-dialog.h"

/*
 * Pairing, which is the one moment a client trusts a daemon it has never seen.
 *
 * The code is short-lived and single-use, so this is asked for and answered in
 * one go rather than kept anywhere: what is worth keeping is the token the
 * daemon hands back, and the certificate it offered while doing so.
 */

typedef struct
{
  GtkWidget *parent;
  GtkEditable *host;
  GtkEditable *port;
  GtkEditable *code;
  XdRemoteClient *client;
  XdRemotePairedCallback callback;
  gpointer user_data;
} PairPrompt;

static void
pair_prompt_free (PairPrompt *prompt)
{
  g_clear_object (&prompt->client);
  g_clear_object (&prompt->parent);
  g_free (prompt);
}

static void
show_error (PairPrompt *prompt,
            const char *message)
{
  AdwAlertDialog *dialog;

  dialog = ADW_ALERT_DIALOG (adw_alert_dialog_new ("Could Not Pair", message));
  adw_alert_dialog_add_response (dialog, "close", "Close");
  adw_alert_dialog_set_default_response (dialog, "close");
  adw_dialog_present (ADW_DIALOG (dialog), prompt->parent);
}

static void
on_paired (GObject      *source,
           GAsyncResult *result,
           gpointer      user_data)
{
  PairPrompt *prompt = user_data;
  g_autoptr (GError) error = NULL;

  if (!xd_remote_client_pair_finish (XD_REMOTE_CLIENT (source), result, &error))
    {
      show_error (prompt, error->message);
      pair_prompt_free (prompt);
      return;
    }

  prompt->callback (prompt->client, prompt->user_data);
  pair_prompt_free (prompt);
}

/*
 * The code as the daemon printed it.
 *
 * It is compared exactly, and it was read off a terminal and typed back in, so
 * the spaces and the lower case that come with doing that are taken out here
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

static void
on_response (GObject      *source,
             GAsyncResult *result,
             gpointer      user_data)
{
  PairPrompt *prompt = user_data;
  const char *response;
  const char *host;
  g_autofree char *code = NULL;
  guint16 port;

  response = adw_alert_dialog_choose_finish (ADW_ALERT_DIALOG (source), result);

  if (g_strcmp0 (response, "connect") != 0)
    {
      pair_prompt_free (prompt);
      return;
    }

  host = gtk_editable_get_text (prompt->host);
  code = normalise_code (gtk_editable_get_text (prompt->code));
  port = (guint16) g_ascii_strtoull (gtk_editable_get_text (prompt->port), NULL, 10);

  if (*host == '\0' || *code == '\0')
    {
      show_error (prompt, "A remote needs an address and the code the daemon "
                          "printed for “xd serve --pair”.");
      pair_prompt_free (prompt);
      return;
    }

  prompt->client = xd_remote_client_new (host, port);

  xd_remote_client_pair_async (prompt->client, code, NULL, NULL,
                               on_paired, prompt);
}

static GtkWidget *
entry_row (const char *title,
           const char *text)
{
  GtkWidget *row = adw_entry_row_new ();

  adw_preferences_row_set_title (ADW_PREFERENCES_ROW (row), title);
  gtk_editable_set_text (GTK_EDITABLE (row), text != NULL ? text : "");
  g_object_set (row, "activates-default", TRUE, NULL);

  return row;
}

void
xd_remote_pair_dialog_present (GtkWidget              *parent,
                               GSettings              *settings,
                               XdRemotePairedCallback  callback,
                               gpointer                user_data)
{
  g_autofree char *known_host = NULL;
  g_autofree char *known_port = NULL;
  AdwAlertDialog *dialog;
  PairPrompt *prompt;
  GtkWidget *group;
  GtkWidget *host_row;
  GtkWidget *port_row;
  GtkWidget *code_row;

  g_return_if_fail (GTK_IS_WIDGET (parent));
  g_return_if_fail (callback != NULL);

  known_host = g_settings_get_string (settings, "remote-host");
  known_port = g_strdup_printf ("%d", g_settings_get_int (settings, "remote-port"));

  dialog = ADW_ALERT_DIALOG (
    adw_alert_dialog_new ("Connect to a Remote",
                          "Run “xd serve --pair” on the other machine "
                          "and enter the code it prints. It is good for a few "
                          "minutes and for one device."));
  adw_alert_dialog_add_responses (dialog,
                                  "cancel", "Cancel",
                                  "connect", "Connect",
                                  NULL);
  adw_alert_dialog_set_response_appearance (dialog, "connect",
                                            ADW_RESPONSE_SUGGESTED);
  adw_alert_dialog_set_default_response (dialog, "connect");
  adw_alert_dialog_set_close_response (dialog, "cancel");

  host_row = entry_row ("Host", known_host);
  port_row = entry_row ("Port", known_port);
  code_row = entry_row ("Pairing Code", NULL);

  group = adw_preferences_group_new ();
  adw_preferences_group_add (ADW_PREFERENCES_GROUP (group), host_row);
  adw_preferences_group_add (ADW_PREFERENCES_GROUP (group), port_row);
  adw_preferences_group_add (ADW_PREFERENCES_GROUP (group), code_row);
  adw_alert_dialog_set_extra_child (dialog, group);

  prompt = g_new0 (PairPrompt, 1);
  prompt->parent = g_object_ref (parent);
  prompt->host = GTK_EDITABLE (host_row);
  prompt->port = GTK_EDITABLE (port_row);
  prompt->code = GTK_EDITABLE (code_row);
  prompt->callback = callback;
  prompt->user_data = user_data;

  adw_alert_dialog_choose (dialog, parent, NULL, on_response, prompt);
}
