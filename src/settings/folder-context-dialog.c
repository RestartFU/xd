#include "folder-context-dialog.h"

#include "folder-settings.h"
#include "ui/panel-style.h"

typedef struct
{
  grefcount refs;
  GtkWindow *window;            /* weak */
  XdNode *folder;
  XdRemoteTree *remote;         /* NULL for a local folder */
  XdFolderSettings *settings;   /* local folder's file */
  GCancellable *cancellable;
  GtkTextView *context;
  GtkLabel *status;
  GtkButton *save;
} ContextPrompt;

static ContextPrompt *
context_prompt_ref (ContextPrompt *prompt)
{
  g_ref_count_inc (&prompt->refs);
  return prompt;
}

static void
context_prompt_unref (ContextPrompt *prompt)
{
  if (!g_ref_count_dec (&prompt->refs))
    return;

  g_clear_object (&prompt->cancellable);
  g_clear_object (&prompt->remote);
  g_clear_object (&prompt->folder);
  g_clear_pointer (&prompt->settings, xd_folder_settings_free);
  g_free (prompt);
}

static void
on_window_gone (gpointer user_data,
                GObject *where_window_was)
{
  ContextPrompt *prompt = user_data;

  prompt->window = NULL;
  if (prompt->cancellable != NULL)
    g_cancellable_cancel (prompt->cancellable);

  context_prompt_unref (prompt);
}

static void
close_prompt (ContextPrompt *prompt)
{
  GtkWindow *window = prompt->window;

  if (window == NULL)
    return;

  prompt->window = NULL;
  if (prompt->cancellable != NULL)
    g_cancellable_cancel (prompt->cancellable);

  g_object_weak_unref (G_OBJECT (window), on_window_gone, prompt);
  context_prompt_unref (prompt);
  gtk_window_destroy (window);
}

static void
show_status (ContextPrompt *prompt,
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
set_busy (ContextPrompt *prompt,
          gboolean       busy)
{
  if (prompt->window == NULL)
    return;

  gtk_widget_set_sensitive (GTK_WIDGET (prompt->context), !busy);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->save), !busy);
  gtk_button_set_label (prompt->save, busy ? "Saving…" : "Save");
}

static char *
context_text (GtkTextView *view)
{
  GtkTextBuffer *buffer = gtk_text_view_get_buffer (view);
  GtkTextIter start, end;
  char *text;

  gtk_text_buffer_get_bounds (buffer, &start, &end);
  text = gtk_text_buffer_get_text (buffer, &start, &end, FALSE);
  g_strstrip (text);

  if (*text == '\0')
    g_clear_pointer (&text, g_free);

  return text;
}

static void
on_remote_saved (GObject      *source,
                 GAsyncResult *result,
                 gpointer      user_data)
{
  ContextPrompt *prompt = user_data;
  g_autoptr (GError) error = NULL;

  g_clear_object (&prompt->cancellable);

  if (prompt->window == NULL)
    {
      context_prompt_unref (prompt);
      return;
    }

  if (!xd_remote_tree_set_folder_context_finish (prompt->remote, result, &error))
    {
      show_status (prompt, error->message, TRUE);
      set_busy (prompt, FALSE);
      context_prompt_unref (prompt);
      return;
    }

  close_prompt (prompt);
  context_prompt_unref (prompt);
}

static void
save_context (ContextPrompt *prompt)
{
  g_autofree char *text = NULL;

  if (prompt->window == NULL || prompt->cancellable != NULL ||
      !gtk_widget_get_sensitive (GTK_WIDGET (prompt->save)))
    return;

  text = context_text (prompt->context);

  if (prompt->remote != NULL)
    {
      show_status (prompt, NULL, FALSE);
      set_busy (prompt, TRUE);
      prompt->cancellable = g_cancellable_new ();

      xd_remote_tree_set_folder_context_async (
        prompt->remote, prompt->folder, text, prompt->cancellable,
        on_remote_saved, context_prompt_ref (prompt));
      return;
    }

  {
    g_autoptr (GError) error = NULL;

    g_free (prompt->settings->instructions);
    prompt->settings->instructions = g_steal_pointer (&text);

    if (!xd_folder_settings_save (prompt->settings,
                                  xd_node_get_path (prompt->folder), &error))
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
  save_context (user_data);
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
  ContextPrompt *prompt = user_data;

  if (keyval == GDK_KEY_Escape)
    {
      close_prompt (prompt);
      return GDK_EVENT_STOP;
    }

  if ((keyval == GDK_KEY_Return || keyval == GDK_KEY_KP_Enter) &&
      (state & GDK_CONTROL_MASK) != 0)
    {
      save_context (prompt);
      return GDK_EVENT_STOP;
    }

  return GDK_EVENT_PROPAGATE;
}

static void
show_context (ContextPrompt *prompt,
              const char    *context)
{
  GtkTextBuffer *buffer = gtk_text_view_get_buffer (prompt->context);

  gtk_text_buffer_set_text (buffer, context != NULL ? context : "", -1);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->context), TRUE);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->save), TRUE);
  show_status (prompt, NULL, FALSE);
  gtk_widget_grab_focus (GTK_WIDGET (prompt->context));
}

static void
on_remote_loaded (GObject      *source,
                  GAsyncResult *result,
                  gpointer      user_data)
{
  ContextPrompt *prompt = user_data;
  g_autoptr (GError) error = NULL;
  g_autofree char *context = NULL;

  g_clear_object (&prompt->cancellable);

  if (prompt->window == NULL)
    {
      context_prompt_unref (prompt);
      return;
    }

  if (!xd_remote_tree_get_folder_context_finish (
        prompt->remote, result, &context, &error))
    {
      show_status (prompt, error->message, TRUE);
      context_prompt_unref (prompt);
      return;
    }

  show_context (prompt, context);
  context_prompt_unref (prompt);
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
xd_folder_context_dialog_present (GtkWidget    *parent,
                                  XdNode       *folder,
                                  XdRemoteTree *remote)
{
  g_autofree char *title_text = NULL;
  GtkWindow *parent_window;
  ContextPrompt *prompt;
  GtkWidget *window;
  GtkWidget *column;
  GtkWidget *header;
  GtkWidget *title;
  GtkWidget *description;
  GtkWidget *body;
  GtkWidget *field_label;
  GtkWidget *frame;
  GtkWidget *scroller;
  GtkWidget *footer;
  GtkWidget *spacer;
  GtkWidget *cancel;
  GtkEventController *keys;

  g_return_if_fail (GTK_IS_WIDGET (parent));
  g_return_if_fail (XD_IS_NODE (folder));
  g_return_if_fail (xd_node_get_kind (folder) == XD_NODE_FOLDER);

  xd_panel_style_ensure ();

  parent_window = GTK_WINDOW (gtk_widget_get_root (parent));
  prompt = g_new0 (ContextPrompt, 1);
  g_ref_count_init (&prompt->refs);
  prompt->folder = g_object_ref (folder);
  prompt->remote = remote != NULL ? g_object_ref (remote) : NULL;

  window = gtk_window_new ();
  prompt->window = GTK_WINDOW (window);
  gtk_window_set_transient_for (GTK_WINDOW (window), parent_window);
  gtk_window_set_application (
    GTK_WINDOW (window), gtk_window_get_application (parent_window));
  gtk_window_set_destroy_with_parent (GTK_WINDOW (window), TRUE);
  gtk_window_set_modal (GTK_WINDOW (window), TRUE);
  gtk_window_set_decorated (GTK_WINDOW (window), FALSE);
  gtk_window_set_default_size (GTK_WINDOW (window), 620, 500);
  gtk_widget_add_css_class (window, "xd-panel");

  title_text = g_strdup_printf ("Agent Context · %s",
                                xd_node_get_name (folder));
  title = gtk_label_new (title_text);
  gtk_label_set_xalign (GTK_LABEL (title), 0.0f);
  gtk_widget_add_css_class (title, "title-3");

  description = gtk_label_new (
    "These instructions are added to every agent turn in this folder. "
    "Context from parent folders is applied before this text.");
  gtk_label_set_xalign (GTK_LABEL (description), 0.0f);
  gtk_label_set_wrap (GTK_LABEL (description), TRUE);
  gtk_widget_add_css_class (description, "dim-label");

  header = gtk_box_new (GTK_ORIENTATION_VERTICAL, 5);
  gtk_box_append (GTK_BOX (header), title);
  gtk_box_append (GTK_BOX (header), description);
  gtk_widget_add_css_class (header, "xd-panel-bar");
  gtk_widget_add_css_class (header, "xd-panel-head");

  body = gtk_box_new (GTK_ORIENTATION_VERTICAL, 8);
  gtk_widget_set_margin_top (body, 22);
  gtk_widget_set_margin_bottom (body, 22);
  gtk_widget_set_margin_start (body, 22);
  gtk_widget_set_margin_end (body, 22);
  gtk_widget_set_vexpand (body, TRUE);

  field_label = gtk_label_new ("Context for this folder");
  gtk_label_set_xalign (GTK_LABEL (field_label), 0.0f);
  gtk_widget_add_css_class (field_label, "caption");
  gtk_widget_add_css_class (field_label, "dim-label");
  gtk_box_append (GTK_BOX (body), field_label);

  prompt->context = GTK_TEXT_VIEW (gtk_text_view_new ());
  gtk_text_view_set_wrap_mode (prompt->context, GTK_WRAP_WORD_CHAR);
  gtk_text_view_set_top_margin (prompt->context, 10);
  gtk_text_view_set_bottom_margin (prompt->context, 10);
  gtk_text_view_set_left_margin (prompt->context, 10);
  gtk_text_view_set_right_margin (prompt->context, 10);
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->context), remote == NULL);

  scroller = gtk_scrolled_window_new ();
  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (scroller),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_widget_set_vexpand (scroller, TRUE);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (scroller),
                                 GTK_WIDGET (prompt->context));

  frame = gtk_frame_new (NULL);
  gtk_widget_set_vexpand (frame, TRUE);
  gtk_frame_set_child (GTK_FRAME (frame), scroller);
  gtk_box_append (GTK_BOX (body), frame);

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
  gtk_widget_set_sensitive (GTK_WIDGET (prompt->save), remote == NULL);
  gtk_box_append (GTK_BOX (footer), GTK_WIDGET (prompt->save));
  gtk_widget_add_css_class (footer, "xd-panel-bar");
  gtk_widget_add_css_class (footer, "xd-panel-foot");

  column = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  gtk_box_append (GTK_BOX (column), header);
  gtk_box_append (GTK_BOX (column), body);
  gtk_box_append (GTK_BOX (column), footer);
  gtk_window_set_child (GTK_WINDOW (window), column);

  g_signal_connect (prompt->save, "clicked",
                    G_CALLBACK (on_save_clicked), prompt);
  g_signal_connect (cancel, "clicked",
                    G_CALLBACK (on_cancel_clicked), prompt);

  keys = gtk_event_controller_key_new ();
  gtk_event_controller_set_propagation_phase (keys, GTK_PHASE_CAPTURE);
  g_signal_connect (keys, "key-pressed", G_CALLBACK (on_key), prompt);
  gtk_widget_add_controller (window, keys);

  g_object_set_data_full (G_OBJECT (window), "context-prompt", prompt,
                          (GDestroyNotify) context_prompt_unref);
  g_object_weak_ref (G_OBJECT (window), on_window_gone,
                     context_prompt_ref (prompt));

  gtk_window_present (GTK_WINDOW (window));

  if (remote != NULL)
    {
      show_status (prompt, "Loading context…", FALSE);
      prompt->cancellable = g_cancellable_new ();
      xd_remote_tree_get_folder_context_async (
        remote, folder, prompt->cancellable, on_remote_loaded,
        context_prompt_ref (prompt));
    }
  else
    {
      g_autoptr (GError) error = NULL;

      prompt->settings =
        xd_folder_settings_ensure (xd_node_get_path (folder), &error);
      if (prompt->settings == NULL)
        {
          gtk_widget_set_sensitive (GTK_WIDGET (prompt->context), FALSE);
          gtk_widget_set_sensitive (GTK_WIDGET (prompt->save), FALSE);
          show_status (prompt, error->message, TRUE);
        }
      else
        {
          show_context (prompt, prompt->settings->instructions);
        }
    }
}
