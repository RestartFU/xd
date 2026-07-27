#include "agent-secrets-dialog.h"

#include "agent-secrets.h"
#include "ui/panel-style.h"

typedef struct _SecretsPrompt SecretsPrompt;

typedef struct
{
  SecretsPrompt *prompt;       /* unowned */
  GtkWidget *box;
  GtkEditable *name;
  GtkEditable *value;
  gboolean existing;
} SecretRow;

struct _SecretsPrompt
{
  grefcount refs;
  GtkWindow *window;           /* weak */
  XdRemoteTree *remote;
  XdAgentSecrets *local;
  GCancellable *cancellable;
  GPtrArray *rows;             /* SecretRow* */
  GtkBox *rows_box;
  GtkLabel *status;
  GtkButton *add;
  GtkButton *save;
};

static SecretsPrompt *
secrets_prompt_ref (SecretsPrompt *prompt)
{
  g_ref_count_inc (&prompt->refs);
  return prompt;
}

static void
secret_row_free (SecretRow *row)
{
  g_free (row);
}

static void
secrets_prompt_unref (SecretsPrompt *prompt)
{
  if (!g_ref_count_dec (&prompt->refs))
    return;

  g_clear_object (&prompt->cancellable);
  g_clear_object (&prompt->remote);
  g_clear_pointer (&prompt->local, xd_agent_secrets_free);
  g_clear_pointer (&prompt->rows, g_ptr_array_unref);
  g_free (prompt);
}

static void
on_window_gone (gpointer user_data,
                GObject *where_window_was)
{
  SecretsPrompt *prompt = user_data;

  prompt->window = NULL;
  if (prompt->cancellable != NULL)
    g_cancellable_cancel (prompt->cancellable);
  secrets_prompt_unref (prompt);
}

static void
close_prompt (SecretsPrompt *prompt)
{
  GtkWindow *window = prompt->window;

  if (window == NULL)
    return;

  prompt->window = NULL;
  if (prompt->cancellable != NULL)
    g_cancellable_cancel (prompt->cancellable);
  g_object_weak_unref (G_OBJECT (window), on_window_gone, prompt);
  secrets_prompt_unref (prompt);
  gtk_window_destroy (window);
}

static void
show_status (SecretsPrompt *prompt,
             const char    *message,
             gboolean       error)
{
  if (prompt->window == NULL)
    return;

  gtk_label_set_label (prompt->status, message != NULL ? message : "");
  gtk_widget_set_visible (GTK_WIDGET (prompt->status), message != NULL);
  if (error)
    gtk_widget_add_css_class (GTK_WIDGET (prompt->status), "error");
  else
    gtk_widget_remove_css_class (GTK_WIDGET (prompt->status), "error");
}

static void
set_busy (SecretsPrompt *prompt,
          gboolean       busy)
{
  if (prompt->window == NULL)
    return;

  gtk_widget_set_sensitive (GTK_WIDGET (prompt->rows_box), !busy);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->add), !busy);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->save), !busy);
  gtk_button_set_label (prompt->save, busy ? "Saving…" : "Save");
}

static void
on_remove_clicked (GtkButton *button,
                   gpointer   user_data)
{
  SecretRow *row = user_data;
  SecretsPrompt *prompt = row->prompt;

  gtk_box_remove (prompt->rows_box, row->box);
  g_ptr_array_remove (prompt->rows, row);
  show_status (prompt, NULL, FALSE);
}

static SecretRow *
append_row (SecretsPrompt *prompt,
            const char    *name,
            gboolean       existing)
{
  SecretRow *row = g_new0 (SecretRow, 1);
  GtkWidget *name_entry = gtk_entry_new ();
  GtkWidget *value_entry = gtk_password_entry_new ();
  GtkWidget *remove = gtk_button_new_from_icon_name ("user-trash-symbolic");

  row->prompt = prompt;
  row->existing = existing;
  row->box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  row->name = GTK_EDITABLE (name_entry);
  row->value = GTK_EDITABLE (value_entry);

  gtk_widget_set_hexpand (name_entry, TRUE);
  gtk_widget_set_hexpand (value_entry, TRUE);
  gtk_entry_set_placeholder_text (GTK_ENTRY (name_entry),
                                  "ENVIRONMENT_VARIABLE");
  gtk_editable_set_text (row->name, name != NULL ? name : "");
  gtk_editable_set_editable (row->name, !existing);
  gtk_password_entry_set_show_peek_icon (
    GTK_PASSWORD_ENTRY (value_entry), TRUE);
  g_object_set (value_entry, "placeholder-text",
                existing ? "Unchanged" : "Secret value", NULL);

  gtk_widget_add_css_class (remove, "flat");
  gtk_widget_set_tooltip_text (remove, "Remove secret");
  g_signal_connect (remove, "clicked",
                    G_CALLBACK (on_remove_clicked), row);

  gtk_box_append (GTK_BOX (row->box), name_entry);
  gtk_box_append (GTK_BOX (row->box), value_entry);
  gtk_box_append (GTK_BOX (row->box), remove);
  gtk_box_append (prompt->rows_box, row->box);
  g_ptr_array_add (prompt->rows, row);

  return row;
}

static void
on_add_clicked (GtkButton *button,
                gpointer   user_data)
{
  SecretRow *row = append_row (user_data, NULL, FALSE);

  gtk_widget_grab_focus (GTK_WIDGET (row->name));
}

typedef struct
{
  char *name;
  char *value;                 /* NULL means keep */
} PreparedSecret;

static void
prepared_secret_free (PreparedSecret *entry)
{
  g_free (entry->name);
  g_free (entry->value);
  g_free (entry);
}

static GPtrArray *
prepare_entries (SecretsPrompt *prompt)
{
  g_autoptr (GHashTable) seen =
    g_hash_table_new_full (g_str_hash, g_str_equal, g_free, NULL);
  GPtrArray *entries =
    g_ptr_array_new_with_free_func ((GDestroyNotify) prepared_secret_free);

  for (guint i = 0; i < prompt->rows->len; i++)
    {
      SecretRow *row = g_ptr_array_index (prompt->rows, i);
      PreparedSecret *entry = g_new0 (PreparedSecret, 1);
      const char *value = gtk_editable_get_text (row->value);

      entry->name = g_strdup (gtk_editable_get_text (row->name));
      g_strstrip (entry->name);

      if (!xd_agent_secret_name_is_valid (entry->name))
        {
          show_status (
            prompt,
            "Names must use letters, numbers and underscores, and cannot "
            "start with a number.", TRUE);
          gtk_widget_grab_focus (GTK_WIDGET (row->name));
          prepared_secret_free (entry);
          g_ptr_array_unref (entries);
          return NULL;
        }

      if (g_hash_table_contains (seen, entry->name))
        {
          show_status (prompt, "Secret names must be unique.", TRUE);
          gtk_widget_grab_focus (GTK_WIDGET (row->name));
          prepared_secret_free (entry);
          g_ptr_array_unref (entries);
          return NULL;
        }

      if (*value != '\0')
        entry->value = g_strdup (value);
      else if (!row->existing)
        {
          show_status (prompt, "A new secret needs a value.", TRUE);
          gtk_widget_grab_focus (GTK_WIDGET (row->value));
          prepared_secret_free (entry);
          g_ptr_array_unref (entries);
          return NULL;
        }

      g_hash_table_add (seen, g_strdup (entry->name));
      g_ptr_array_add (entries, entry);
    }

  return entries;
}

static gboolean
save_local (SecretsPrompt *prompt,
            GPtrArray     *entries,
            GError       **error)
{
  g_autoptr (GHashTable) desired =
    g_hash_table_new (g_str_hash, g_str_equal);
  g_auto (GStrv) old_names = xd_agent_secrets_names (prompt->local);

  for (guint i = 0; i < entries->len; i++)
    {
      PreparedSecret *entry = g_ptr_array_index (entries, i);

      g_hash_table_add (desired, entry->name);
      if (entry->value != NULL &&
          !xd_agent_secrets_set (
            prompt->local, entry->name, entry->value, error))
        return FALSE;
    }

  for (gsize i = 0; old_names[i] != NULL; i++)
    if (!g_hash_table_contains (desired, old_names[i]))
      xd_agent_secrets_remove (prompt->local, old_names[i]);

  return xd_agent_secrets_save (prompt->local, error);
}

static void
on_remote_saved (GObject      *source,
                 GAsyncResult *result,
                 gpointer      user_data)
{
  SecretsPrompt *prompt = user_data;
  g_autoptr (GError) error = NULL;

  g_clear_object (&prompt->cancellable);
  if (prompt->window == NULL)
    {
      secrets_prompt_unref (prompt);
      return;
    }

  if (!xd_remote_tree_set_agent_secrets_finish (
        prompt->remote, result, &error))
    {
      show_status (prompt, error->message, TRUE);
      set_busy (prompt, FALSE);
      secrets_prompt_unref (prompt);
      return;
    }

  close_prompt (prompt);
  secrets_prompt_unref (prompt);
}

static void
save_secrets (SecretsPrompt *prompt)
{
  g_autoptr (GPtrArray) entries = NULL;

  if (prompt->window == NULL || prompt->cancellable != NULL ||
      !gtk_widget_get_sensitive (GTK_WIDGET (prompt->save)))
    return;

  entries = prepare_entries (prompt);
  if (entries == NULL)
    return;

  show_status (prompt, NULL, FALSE);

  if (prompt->remote != NULL)
    {
      g_autofree XdAgentSecretUpdate *updates =
        g_new0 (XdAgentSecretUpdate, entries->len);

      for (guint i = 0; i < entries->len; i++)
        {
          PreparedSecret *entry = g_ptr_array_index (entries, i);

          updates[i].name = entry->name;
          updates[i].value = entry->value;
        }

      set_busy (prompt, TRUE);
      prompt->cancellable = g_cancellable_new ();
      xd_remote_tree_set_agent_secrets_async (
        prompt->remote, updates, entries->len, prompt->cancellable,
        on_remote_saved, secrets_prompt_ref (prompt));
      return;
    }

  {
    g_autoptr (GError) error = NULL;

    if (!save_local (prompt, entries, &error))
      {
        show_status (prompt, error->message, TRUE);
        return;
      }
  }

  close_prompt (prompt);
}

static void
on_save_clicked (GtkButton *button,
                 gpointer   user_data)
{
  save_secrets (user_data);
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
  if (keyval == GDK_KEY_Escape)
    {
      close_prompt (user_data);
      return GDK_EVENT_STOP;
    }

  if ((keyval == GDK_KEY_Return || keyval == GDK_KEY_KP_Enter) &&
      (state & GDK_CONTROL_MASK) != 0)
    {
      save_secrets (user_data);
      return GDK_EVENT_STOP;
    }

  return GDK_EVENT_PROPAGATE;
}

static void
show_names (SecretsPrompt    *prompt,
            const char *const *names)
{
  for (gsize i = 0; names != NULL && names[i] != NULL; i++)
    append_row (prompt, names[i], TRUE);

  if (prompt->rows->len == 0)
    {
      SecretRow *row = append_row (prompt, NULL, FALSE);

      gtk_widget_grab_focus (GTK_WIDGET (row->name));
    }

  gtk_widget_set_sensitive (GTK_WIDGET (prompt->rows_box), TRUE);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->add), TRUE);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->save), TRUE);
  show_status (prompt, NULL, FALSE);
}

static void
on_remote_loaded (GObject      *source,
                  GAsyncResult *result,
                  gpointer      user_data)
{
  SecretsPrompt *prompt = user_data;
  g_autoptr (GError) error = NULL;
  g_auto (GStrv) names = NULL;

  g_clear_object (&prompt->cancellable);
  if (prompt->window == NULL)
    {
      secrets_prompt_unref (prompt);
      return;
    }

  names = xd_remote_tree_get_agent_secrets_finish (
    prompt->remote, result, &error);
  if (names == NULL)
    {
      show_status (prompt, error->message, TRUE);
      secrets_prompt_unref (prompt);
      return;
    }

  show_names (prompt, (const char *const *) names);
  secrets_prompt_unref (prompt);
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
xd_agent_secrets_dialog_present (GtkWidget    *parent,
                                 XdRemoteTree *remote)
{
  SecretsPrompt *prompt;
  GtkWindow *parent_window;
  GtkWidget *window;
  GtkWidget *column;
  GtkWidget *header;
  GtkWidget *title;
  GtkWidget *description;
  GtkWidget *body;
  GtkWidget *label_row;
  GtkWidget *field_label;
  GtkWidget *scroller;
  GtkWidget *footer;
  GtkWidget *spacer;
  GtkWidget *cancel;
  GtkEventController *keys;

  g_return_if_fail (GTK_IS_WIDGET (parent));
  g_return_if_fail (remote == NULL || XD_IS_REMOTE_TREE (remote));

  xd_panel_style_ensure ();

  parent_window = GTK_WINDOW (gtk_widget_get_root (parent));
  prompt = g_new0 (SecretsPrompt, 1);
  g_ref_count_init (&prompt->refs);
  prompt->remote = remote != NULL ? g_object_ref (remote) : NULL;
  prompt->rows =
    g_ptr_array_new_with_free_func ((GDestroyNotify) secret_row_free);

  window = gtk_window_new ();
  prompt->window = GTK_WINDOW (window);
  gtk_window_set_transient_for (GTK_WINDOW (window), parent_window);
  gtk_window_set_application (
    GTK_WINDOW (window), gtk_window_get_application (parent_window));
  gtk_window_set_destroy_with_parent (GTK_WINDOW (window), TRUE);
  gtk_window_set_modal (GTK_WINDOW (window), TRUE);
  gtk_window_set_decorated (GTK_WINDOW (window), FALSE);
  gtk_window_set_default_size (GTK_WINDOW (window), 700, 500);
  gtk_widget_add_css_class (window, "xd-panel");

  title = gtk_label_new (remote != NULL
                         ? "Agent Secrets · Remote Machine"
                         : "Agent Secrets · This Machine");
  gtk_label_set_xalign (GTK_LABEL (title), 0.0f);
  gtk_widget_add_css_class (title, "title-3");

  description = gtk_label_new (
    remote != NULL
      ? "Stored in a private per-user file on the remote machine. Values never "
        "enter the prompt; remote agent processes receive them as environment "
        "variables."
      : "Stored in a private per-user file on this machine. Values never enter "
        "the prompt; agent processes receive them as environment variables.");
  gtk_label_set_xalign (GTK_LABEL (description), 0.0f);
  gtk_label_set_wrap (GTK_LABEL (description), TRUE);
  gtk_widget_add_css_class (description, "dim-label");

  header = gtk_box_new (GTK_ORIENTATION_VERTICAL, 5);
  gtk_box_append (GTK_BOX (header), title);
  gtk_box_append (GTK_BOX (header), description);
  gtk_widget_add_css_class (header, "xd-panel-bar");
  gtk_widget_add_css_class (header, "xd-panel-head");

  body = gtk_box_new (GTK_ORIENTATION_VERTICAL, 10);
  gtk_widget_set_margin_top (body, 22);
  gtk_widget_set_margin_bottom (body, 22);
  gtk_widget_set_margin_start (body, 22);
  gtk_widget_set_margin_end (body, 22);
  gtk_widget_set_vexpand (body, TRUE);

  label_row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  field_label = gtk_label_new ("Environment variables");
  gtk_label_set_xalign (GTK_LABEL (field_label), 0.0f);
  gtk_widget_set_hexpand (field_label, TRUE);
  gtk_widget_add_css_class (field_label, "caption");
  gtk_widget_add_css_class (field_label, "dim-label");
  gtk_box_append (GTK_BOX (label_row), field_label);

  prompt->add = GTK_BUTTON (gtk_button_new_with_label ("Add Secret"));
  gtk_widget_add_css_class (GTK_WIDGET (prompt->add), "flat");
  gtk_box_append (GTK_BOX (label_row), GTK_WIDGET (prompt->add));
  gtk_box_append (GTK_BOX (body), label_row);

  prompt->rows_box = GTK_BOX (gtk_box_new (GTK_ORIENTATION_VERTICAL, 8));
  scroller = gtk_scrolled_window_new ();
  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scroller),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_widget_set_vexpand (scroller, TRUE);
  gtk_scrolled_window_set_child (
    GTK_SCROLLED_WINDOW (scroller), GTK_WIDGET (prompt->rows_box));
  gtk_box_append (GTK_BOX (body), scroller);

  prompt->status = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_xalign (prompt->status, 0.0f);
  gtk_label_set_wrap (prompt->status, TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (prompt->status), "dim-label");
  gtk_widget_set_visible (GTK_WIDGET (prompt->status), FALSE);
  gtk_box_append (GTK_BOX (body), GTK_WIDGET (prompt->status));

  footer = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 12);
  gtk_box_append (GTK_BOX (footer), hint ("Esc", "Cancel"));
  gtk_box_append (GTK_BOX (footer), hint ("Ctrl Enter", "Save"));
  spacer = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
  gtk_widget_set_hexpand (spacer, TRUE);
  gtk_box_append (GTK_BOX (footer), spacer);

  cancel = gtk_button_new_with_label ("Cancel");
  gtk_widget_add_css_class (cancel, "flat");
  gtk_box_append (GTK_BOX (footer), cancel);

  prompt->save = GTK_BUTTON (gtk_button_new_with_label ("Save"));
  gtk_widget_add_css_class (GTK_WIDGET (prompt->save), "xd-panel-action");
  gtk_box_append (GTK_BOX (footer), GTK_WIDGET (prompt->save));
  gtk_widget_add_css_class (footer, "xd-panel-bar");
  gtk_widget_add_css_class (footer, "xd-panel-foot");

  column = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  gtk_box_append (GTK_BOX (column), header);
  gtk_box_append (GTK_BOX (column), body);
  gtk_box_append (GTK_BOX (column), footer);
  gtk_window_set_child (GTK_WINDOW (window), column);

  g_signal_connect (prompt->add, "clicked",
                    G_CALLBACK (on_add_clicked), prompt);
  g_signal_connect (prompt->save, "clicked",
                    G_CALLBACK (on_save_clicked), prompt);
  g_signal_connect (cancel, "clicked",
                    G_CALLBACK (on_cancel_clicked), prompt);

  keys = gtk_event_controller_key_new ();
  gtk_event_controller_set_propagation_phase (keys, GTK_PHASE_CAPTURE);
  g_signal_connect (keys, "key-pressed", G_CALLBACK (on_key), prompt);
  gtk_widget_add_controller (window, keys);

  g_object_set_data_full (G_OBJECT (window), "secrets-prompt", prompt,
                          (GDestroyNotify) secrets_prompt_unref);
  g_object_weak_ref (G_OBJECT (window), on_window_gone,
                     secrets_prompt_ref (prompt));

  gtk_widget_set_sensitive (GTK_WIDGET (prompt->rows_box), remote == NULL);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->add), remote == NULL);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->save), remote == NULL);
  gtk_window_present (GTK_WINDOW (window));

  if (remote != NULL)
    {
      show_status (prompt, "Loading secret names…", FALSE);
      prompt->cancellable = g_cancellable_new ();
      xd_remote_tree_get_agent_secrets_async (
        remote, prompt->cancellable, on_remote_loaded,
        secrets_prompt_ref (prompt));
    }
  else
    {
      g_autoptr (GError) error = NULL;
      g_auto (GStrv) names = NULL;

      prompt->local = xd_agent_secrets_load (NULL, &error);
      if (prompt->local == NULL)
        {
          gtk_widget_set_sensitive (GTK_WIDGET (prompt->rows_box), FALSE);
          gtk_widget_set_sensitive (GTK_WIDGET (prompt->add), FALSE);
          gtk_widget_set_sensitive (GTK_WIDGET (prompt->save), FALSE);
          show_status (prompt, error->message, TRUE);
        }
      else
        {
          names = xd_agent_secrets_names (prompt->local);
          show_names (prompt, (const char *const *) names);
        }
    }
}
