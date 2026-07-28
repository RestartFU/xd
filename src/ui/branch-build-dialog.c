#include "branch-build-dialog.h"

#include <string.h>

#include "util/branch-build.h"
#include "util/host-launch.h"

/*
 * One build at a time, and it outlives the dialog.
 *
 * A build takes minutes, and the dialog it was started from is a thing anyone
 * would close in the meantime -- so the run is kept here and the dialog is a
 * view of it. Opening the button again while one is running shows the same
 * output rather than offering to start a second.
 */

/* Enough of the output to see what went wrong, and not the whole of a build
 * that printed a hundred thousand lines of compiler output. */
#define LOG_LIMIT (64 * 1024)

typedef struct
{
  grefcount refs;

  GSettings *settings;
  GSubprocess *process;
  GCancellable *cancellable;
  GString *output;
  char *label;              /* what is being built, in words */
  char *trouble;            /* why the last run stopped, or NULL */
  gboolean running;
  gboolean stopped;         /* this run was stopped from the dialog */

  XdBranchBuildDoneFunc on_installed;
  gpointer user_data;

  /* The dialog on it, while one is open. Unowned: cleared when it closes. */
  AdwDialog *dialog;
  GtkEditable *entry;
  GtkLabel *status;
  GtkWidget *log_window;
  GtkTextView *log;
  GtkButton *action;
  GtkWidget *spinner;
} Build;

static Build *current;

static void start_build (Build *build);

static Build *
build_ref (Build *build)
{
  g_ref_count_inc (&build->refs);
  return build;
}

static void
build_unref (Build *build)
{
  if (!g_ref_count_dec (&build->refs))
    return;

  g_clear_object (&build->settings);
  g_clear_object (&build->process);
  g_clear_object (&build->cancellable);
  g_string_free (build->output, TRUE);
  g_free (build->label);
  g_free (build->trouble);
  g_free (build);
}

static Build *
build_get (void)
{
  if (current == NULL)
    {
      current = g_new0 (Build, 1);
      g_ref_count_init (&current->refs);
      current->output = g_string_new (NULL);
      current->settings = g_settings_new (XD_APP_ID);
    }

  return current;
}

/* --- what it is saying ------------------------------------------------------ */

static void
show_output (Build *build)
{
  GtkTextBuffer *buffer;
  GtkTextIter end;

  if (build->log == NULL)
    return;

  buffer = gtk_text_view_get_buffer (build->log);
  gtk_text_buffer_set_text (buffer, build->output->str, -1);

  gtk_widget_set_visible (build->log_window, build->output->len > 0);

  /* The end is where a build says what it is doing now. */
  gtk_text_buffer_get_end_iter (buffer, &end);
  gtk_text_view_scroll_to_iter (build->log, &end, 0.0, TRUE, 0.0, 1.0);
}

static void
show_state (Build *build)
{
  g_autofree char *url = NULL;
  g_autofree char *ref = NULL;
  g_autofree char *label = NULL;
  gboolean understood;
  const char *text;

  if (build->dialog == NULL)
    return;

  text = gtk_editable_get_text (build->entry);
  understood = xd_branch_build_parse (text, &url, &ref, &label);

  gtk_widget_set_sensitive (GTK_WIDGET (build->entry), !build->running);
  gtk_widget_set_visible (build->spinner, build->running);
  gtk_spinner_set_spinning (GTK_SPINNER (build->spinner), build->running);

  if (build->running)
    {
      g_autofree char *says = g_strdup_printf ("Building %s…", build->label);

      gtk_label_set_label (build->status, says);
      gtk_button_set_label (build->action, "Stop");
      gtk_widget_remove_css_class (GTK_WIDGET (build->action), "suggested-action");
      gtk_widget_add_css_class (GTK_WIDGET (build->action), "destructive-action");
      gtk_widget_set_sensitive (GTK_WIDGET (build->action), TRUE);
    }
  else
    {
      gtk_label_set_label (
        build->status,
        build->trouble != NULL ? build->trouble
        : understood            ? label
        : *text == '\0'         ? "A pull request link, a branch link, or a branch name."
                                : "Not a pull request or a branch.");
      gtk_button_set_label (build->action, "Build and Install");
      gtk_widget_remove_css_class (GTK_WIDGET (build->action), "destructive-action");
      gtk_widget_add_css_class (GTK_WIDGET (build->action), "suggested-action");
      gtk_widget_set_sensitive (GTK_WIDGET (build->action), understood);
    }

  show_output (build);
}

static void
append_output (Build      *build,
               const char *line)
{
  g_string_append (build->output, line);
  g_string_append_c (build->output, '\n');

  /* Trimmed from the front at a line boundary, so what is shown is still
   * whole lines. */
  if (build->output->len > LOG_LIMIT)
    {
      const char *keep = strchr (build->output->str + (build->output->len - LOG_LIMIT),
                                 '\n');

      g_string_erase (build->output, 0,
                      keep != NULL
                        ? (keep + 1) - build->output->str
                        : (gssize) (build->output->len - LOG_LIMIT));
    }

  show_output (build);
}

/* --- running it ------------------------------------------------------------- */

static void read_line (Build            *build,
                       GDataInputStream *stream);

static void
on_line_read (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  Build *build = user_data;
  GDataInputStream *stream = G_DATA_INPUT_STREAM (source);
  g_autoptr (GError) error = NULL;
  g_autofree char *line =
    g_data_input_stream_read_line_finish_utf8 (stream, result, NULL, &error);

  /* End of the output, or a run that was stopped: either way there is nothing
   * more to read, and the exit status is what says which. */
  if (line == NULL)
    {
      build_unref (build);
      return;
    }

  append_output (build, line);
  read_line (build, stream);
  build_unref (build);
}

static void
read_line (Build            *build,
           GDataInputStream *stream)
{
  g_data_input_stream_read_line_async (stream, G_PRIORITY_DEFAULT_IDLE,
                                       build->cancellable, on_line_read,
                                       build_ref (build));
}

static void
on_finished (GObject      *source,
             GAsyncResult *result,
             gpointer      user_data)
{
  Build *build = user_data;
  g_autoptr (GError) error = NULL;
  gboolean ok = g_subprocess_wait_check_finish (G_SUBPROCESS (source), result,
                                                &error);

  build->running = FALSE;
  g_clear_object (&build->process);

  g_clear_pointer (&build->trouble, g_free);

  if (!ok)
    {
      /* The output above already says what failed, at whatever length the
       * tool that failed says it in; this is the line over it. */
      build->trouble = build->stopped
        ? g_strdup ("Stopped.")
        : g_strdup_printf ("%s did not build. What it printed is below.",
                           build->label);

      show_state (build);
      build_unref (build);
      return;
    }

  {
    g_autofree char *says =
      g_strdup_printf ("Installed %s. Restart to run it.", build->label);

    append_output (build, says);
  }

  show_state (build);

  if (build->on_installed != NULL)
    build->on_installed (build->user_data);

  if (build->dialog != NULL)
    adw_dialog_close (build->dialog);

  build_unref (build);
}

static void
start_build (Build *build)
{
  g_autofree char *url = NULL;
  g_autofree char *ref = NULL;
  g_autofree char *label = NULL;
  g_autofree char *checkout = NULL;
  g_autofree char *command = NULL;
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GError) error = NULL;
  g_auto (GStrv) environment = xd_host_environ ();

  if (build->running || build->entry == NULL ||
      !xd_branch_build_parse (gtk_editable_get_text (build->entry),
                              &url, &ref, &label))
    return;

  checkout = xd_branch_build_checkout_dir ();
  command = xd_branch_build_command (url, ref, checkout);

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                        G_SUBPROCESS_FLAGS_STDERR_MERGE);
  if (environment != NULL)
    g_subprocess_launcher_set_environ (launcher, environment);

  g_clear_object (&build->cancellable);
  build->cancellable = g_cancellable_new ();

  {
    const char *argv[] = { "sh", "-c", command, NULL };

    build->process = g_subprocess_launcher_spawnv (launcher, argv, &error);
  }

  g_clear_pointer (&build->trouble, g_free);
  g_string_truncate (build->output, 0);
  build->stopped = FALSE;

  /* Kept as it was typed: after another commit, opening this and pressing the
   * button is the whole gesture. */
  g_settings_set_string (build->settings, "build-source",
                         gtk_editable_get_text (build->entry));

  if (build->process == NULL)
    {
      build->trouble = g_strdup (error->message);
      show_state (build);
      return;
    }

  g_free (build->label);
  build->label = g_steal_pointer (&label);
  build->running = TRUE;

  append_output (build, "Fetching, building and installing. This takes a few minutes.");
  show_state (build);

  {
    g_autoptr (GDataInputStream) lines = g_data_input_stream_new (
      g_subprocess_get_stdout_pipe (build->process));

    read_line (build, lines);
  }

  g_subprocess_wait_check_async (build->process, NULL, on_finished,
                                 build_ref (build));
}

static void
stop_build (Build *build)
{
  if (!build->running || build->process == NULL)
    return;

  /* The docker client is what is killed; what it was told to build stops with
   * it, and the next run picks up whatever layers it did finish. */
  build->stopped = TRUE;
  g_cancellable_cancel (build->cancellable);
  g_subprocess_force_exit (build->process);
}

/* --- the dialog ------------------------------------------------------------- */

static void
on_action_clicked (GtkButton *button,
                   gpointer   user_data)
{
  Build *build = user_data;

  if (build->running)
    stop_build (build);
  else
    start_build (build);
}

static void
on_entry_changed (GtkEditable *entry,
                  gpointer     user_data)
{
  Build *build = user_data;

  /* Whatever went wrong last time was about the old text. */
  g_clear_pointer (&build->trouble, g_free);
  show_state (build);
}

static void
on_entry_activated (GtkEditable *entry,
                    gpointer     user_data)
{
  start_build (user_data);
}

static void
on_dialog_closed (AdwDialog *dialog,
                  gpointer   user_data)
{
  Build *build = user_data;

  /* Typed and left there without building: still the answer to "which branch"
   * the next time this opens. */
  if (build->entry != NULL)
    g_settings_set_string (build->settings, "build-source",
                           gtk_editable_get_text (build->entry));

  /* The run carries on without a window on it; only the view goes. */
  build->dialog = NULL;
  build->entry = NULL;
  build->status = NULL;
  build->log = NULL;
  build->log_window = NULL;
  build->action = NULL;
  build->spinner = NULL;

  build_unref (build);
}

void
xd_branch_build_dialog_present (GtkWidget             *parent,
                                XdBranchBuildDoneFunc  on_installed,
                                gpointer               user_data)
{
  Build *build = build_get ();
  g_autofree char *saved = g_settings_get_string (build->settings, "build-source");
  GtkWidget *toolbar;
  GtkWidget *content;
  GtkWidget *entry;
  GtkWidget *hint;
  GtkWidget *row;

  /* Already open: that window is the one to look at. */
  if (build->dialog != NULL)
    {
      adw_dialog_present (build->dialog, parent);
      return;
    }

  build->on_installed = on_installed;
  build->user_data = user_data;

  build->dialog = ADW_DIALOG (adw_dialog_new ());
  adw_dialog_set_title (build->dialog, "Build a Branch");
  adw_dialog_set_content_width (build->dialog, 620);

  entry = gtk_entry_new ();
  build->entry = GTK_EDITABLE (entry);
  gtk_editable_set_text (build->entry, saved);
  gtk_entry_set_placeholder_text (
    GTK_ENTRY (entry),
    "https://github.com/" XD_REPO "/pull/128, or a branch name");
  g_signal_connect (entry, "changed", G_CALLBACK (on_entry_changed), build);
  g_signal_connect (entry, "activate", G_CALLBACK (on_entry_activated), build);

  build->status = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_xalign (build->status, 0.0f);
  gtk_label_set_wrap (build->status, TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (build->status), "dim-label");

  hint = gtk_label_new (
    "The branch is fetched, built the way the nightly is built, and installed"
    " over this copy. It needs git and docker. The update button puts the"
    " nightly back.");
  gtk_label_set_xalign (GTK_LABEL (hint), 0.0f);
  gtk_label_set_wrap (GTK_LABEL (hint), TRUE);
  gtk_widget_add_css_class (hint, "dim-label");

  build->log = GTK_TEXT_VIEW (gtk_text_view_new ());
  gtk_text_view_set_editable (build->log, FALSE);
  gtk_text_view_set_cursor_visible (build->log, FALSE);
  gtk_text_view_set_monospace (build->log, TRUE);
  gtk_text_view_set_wrap_mode (build->log, GTK_WRAP_WORD_CHAR);
  gtk_widget_add_css_class (GTK_WIDGET (build->log), "xd-workflow-log");

  build->log_window = gtk_scrolled_window_new ();
  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (build->log_window),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (build->log_window),
                                 GTK_WIDGET (build->log));
  gtk_widget_set_size_request (build->log_window, -1, 220);
  gtk_widget_set_visible (build->log_window, FALSE);

  build->spinner = gtk_spinner_new ();
  build->action = GTK_BUTTON (gtk_button_new ());
  g_signal_connect (build->action, "clicked",
                    G_CALLBACK (on_action_clicked), build);

  row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  gtk_widget_set_halign (row, GTK_ALIGN_END);
  gtk_box_append (GTK_BOX (row), build->spinner);
  gtk_box_append (GTK_BOX (row), GTK_WIDGET (build->action));

  content = gtk_box_new (GTK_ORIENTATION_VERTICAL, 10);
  gtk_widget_set_margin_start (content, 16);
  gtk_widget_set_margin_end (content, 16);
  gtk_widget_set_margin_top (content, 8);
  gtk_widget_set_margin_bottom (content, 16);
  gtk_box_append (GTK_BOX (content), entry);
  gtk_box_append (GTK_BOX (content), GTK_WIDGET (build->status));
  gtk_box_append (GTK_BOX (content), build->log_window);
  gtk_box_append (GTK_BOX (content), hint);
  gtk_box_append (GTK_BOX (content), row);

  toolbar = adw_toolbar_view_new ();
  adw_toolbar_view_add_top_bar (ADW_TOOLBAR_VIEW (toolbar), adw_header_bar_new ());
  adw_toolbar_view_set_content (ADW_TOOLBAR_VIEW (toolbar), content);

  adw_dialog_set_child (build->dialog, toolbar);
  g_signal_connect (build->dialog, "closed",
                    G_CALLBACK (on_dialog_closed), build_ref (build));

  show_state (build);

  adw_dialog_present (build->dialog, parent);
  gtk_widget_grab_focus (entry);
}
