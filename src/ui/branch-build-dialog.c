#include "branch-build-dialog.h"

#include <string.h>

#include "panel-style.h"
#include "util/branch-build.h"
#include "util/host-launch.h"

/*
 * One build at a time, and it outlives the window it was started from.
 *
 * A build takes minutes, and the popup it was started from is a thing anyone
 * would click away from in the meantime -- so the run is kept here and the
 * popup is a view of it. Opening the button again while one is running shows
 * the same build rather than offering to start a second.
 */

/* Enough of the output to say what went wrong, and not the whole of a build
 * that printed a hundred thousand lines of compiler noise. */
#define TAIL_LIMIT (8 * 1024)
#define TAIL_LINES 8

/*
 * What is on screen is redrawn on a timer, not per line of output.
 *
 * docker prints faster than anything can be read, and a window rewritten on
 * every line spends its time on text nobody sees -- which is what made the
 * client crawl while a build ran. Twice a second is as often as a build
 * changes what it is doing.
 */
#define FLUSH_MILLISECONDS 500

typedef struct
{
  grefcount refs;

  GSettings *settings;
  GSubprocess *process;
  GCancellable *cancellable;
  GString *output;          /* the tail, kept for when it fails */
  char *label;              /* what is being built, in words */
  char *trouble;            /* why the last run stopped, or NULL */
  gboolean running;
  gboolean stopped;         /* this run was stopped from the popup */
  gboolean focused;         /* the popup has had the focus at least once */
  guint flush_id;

  XdBranchBuildDoneFunc on_installed;
  gpointer user_data;

  /* The popup on it, while one is open. Unowned: cleared when it closes. */
  GtkWindow *window;
  GtkEditable *entry;
  GtkLabel *status;
  GtkLabel *activity;
  GtkButton *action;
  GtkWidget *spinner;
} Build;

static Build *current;

static void start_build (Build *build);
static void show_state (Build *build);

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

  g_clear_handle_id (&build->flush_id, g_source_remove);
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

/* The last line, which is what a build is doing now. */
static char *
last_line (Build *build)
{
  const char *at = build->output->str + build->output->len;

  if (build->output->len == 0)
    return g_strdup ("");

  /* The output ends in a newline, so the line wanted is the one before it. */
  while (at > build->output->str && (at[-1] == '\n' || at[-1] == '\r'))
    at--;
  while (at > build->output->str && at[-1] != '\n')
    at--;

  return g_strstrip (g_strdup (at));
}

/* The last few lines, which is where the reason a build failed usually is. */
static char *
tail (Build *build)
{
  const char *at = build->output->str + build->output->len;
  int lines = 0;

  while (at > build->output->str)
    {
      at--;
      if (*at == '\n' && ++lines > TAIL_LINES)
        return g_strstrip (g_strdup (at + 1));
    }

  return g_strstrip (g_strdup (build->output->str));
}

static void
show_state (Build *build)
{
  g_autofree char *url = NULL;
  g_autofree char *ref = NULL;
  g_autofree char *label = NULL;
  g_autofree char *said = NULL;
  gboolean understood;
  const char *text;

  if (build->window == NULL)
    return;

  text = gtk_editable_get_text (build->entry);
  understood = xd_branch_build_parse (text, &url, &ref, &label);

  gtk_widget_set_sensitive (GTK_WIDGET (build->entry), !build->running);
  gtk_widget_set_visible (build->spinner, build->running);
  gtk_spinner_set_spinning (GTK_SPINNER (build->spinner), build->running);

  if (build->running)
    {
      g_autofree char *doing = last_line (build);

      said = g_strdup_printf ("Building %s…", build->label);
      gtk_label_set_label (build->status, said);
      gtk_button_set_label (build->action, "Stop");
      gtk_widget_remove_css_class (GTK_WIDGET (build->action), "suggested-action");
      gtk_widget_add_css_class (GTK_WIDGET (build->action), "destructive-action");
      gtk_widget_set_sensitive (GTK_WIDGET (build->action), TRUE);

      /* One line, replaced in place: a build's output is worth watching, not
       * worth scrolling. */
      gtk_label_set_label (build->activity, doing);
      gtk_widget_set_visible (GTK_WIDGET (build->activity), TRUE);
      return;
    }

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

  /* What it printed before it stopped stays up, since that is the answer to
   * why it stopped. */
  if (build->trouble != NULL && build->output->len > 0)
    {
      g_autofree char *lines = tail (build);

      gtk_label_set_label (build->activity, lines);
      gtk_widget_set_visible (GTK_WIDGET (build->activity), TRUE);
      return;
    }

  gtk_widget_set_visible (GTK_WIDGET (build->activity), FALSE);
}

static gboolean
flush_output (gpointer user_data)
{
  Build *build = user_data;

  build->flush_id = 0;

  if (build->window != NULL && build->running)
    {
      g_autofree char *doing = last_line (build);

      gtk_label_set_label (build->activity, doing);
    }

  return G_SOURCE_REMOVE;
}

static void
append_output (Build      *build,
               const char *line)
{
  g_string_append (build->output, line);
  g_string_append_c (build->output, '\n');

  /* Trimmed from the front at a line boundary, so what is kept is whole
   * lines. */
  if (build->output->len > TAIL_LIMIT)
    {
      const char *keep =
        strchr (build->output->str + (build->output->len - TAIL_LIMIT), '\n');

      g_string_erase (build->output, 0,
                      keep != NULL
                        ? (gssize) ((keep + 1) - build->output->str)
                        : (gssize) (build->output->len - TAIL_LIMIT));
    }

  if (build->flush_id == 0 && build->window != NULL)
    build->flush_id = g_timeout_add (FLUSH_MILLISECONDS, flush_output, build);
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
  g_clear_handle_id (&build->flush_id, g_source_remove);
  g_clear_pointer (&build->trouble, g_free);

  if (!ok)
    {
      build->trouble = build->stopped
        ? g_strdup ("Stopped.")
        : g_strdup_printf ("%s did not build.", build->label);

      show_state (build);
      build_unref (build);
      return;
    }

  build->trouble = g_strdup_printf ("Installed %s. Restart to run it.",
                                    build->label);
  g_string_truncate (build->output, 0);
  show_state (build);

  if (build->on_installed != NULL)
    build->on_installed (build->user_data);

  if (build->window != NULL)
    gtk_window_close (build->window);

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

  append_output (build, "Fetching…");
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

/* --- the popup -------------------------------------------------------------- */

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

static gboolean
on_key (GtkEventControllerKey *keys,
        guint                  keyval,
        guint                  keycode,
        GdkModifierType        state,
        gpointer               user_data)
{
  Build *build = user_data;

  if (keyval != GDK_KEY_Escape || build->window == NULL)
    return GDK_EVENT_PROPAGATE;

  gtk_window_close (build->window);
  return GDK_EVENT_STOP;
}

/*
 * Clicking anything else puts it away.
 *
 * Which is what a popup is: it sits over the window until the window is what
 * is wanted again. The build is not the popup, so a run started here carries
 * on -- pressing the button again comes back to it.
 */
static void
on_active_changed (GtkWindow  *window,
                   GParamSpec *pspec,
                   gpointer    user_data)
{
  Build *build = user_data;

  if (gtk_window_is_active (window))
    {
      build->focused = TRUE;
      return;
    }

  /* Not before it has had the focus once: a compositor that does not hand it
   * to a new window would otherwise close this on the way up. */
  if (build->focused)
    gtk_window_close (window);
}

static void
on_window_gone (gpointer  user_data,
                GObject  *where_window_was)
{
  Build *build = user_data;

  build->window = NULL;
  build->focused = FALSE;
  build->entry = NULL;
  build->status = NULL;
  build->activity = NULL;
  build->action = NULL;
  build->spinner = NULL;

  build_unref (build);
}

static void
on_close_clicked (GtkButton *button,
                  gpointer   user_data)
{
  Build *build = user_data;

  if (build->window != NULL)
    gtk_window_close (build->window);
}

static gboolean
save_source (GtkWindow *window,
             gpointer   user_data)
{
  Build *build = user_data;

  /* Typed and left there without building: still the answer to "which branch"
   * the next time this opens. */
  if (build->entry != NULL)
    g_settings_set_string (build->settings, "build-source",
                           gtk_editable_get_text (build->entry));

  return GDK_EVENT_PROPAGATE;
}

/* The keys a panel says it takes, in the row along its foot. */
static GtkWidget *
hint (const char *key,
      const char *what)
{
  GtkWidget *row = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 6);
  GtkWidget *name = gtk_label_new (key);
  GtkWidget *label = gtk_label_new (what);

  gtk_widget_add_css_class (name, "xd-key");
  gtk_widget_add_css_class (label, "dim-label");
  gtk_box_append (GTK_BOX (row), name);
  gtk_box_append (GTK_BOX (row), label);

  return row;
}

void
xd_branch_build_dialog_present (GtkWidget             *parent,
                                XdBranchBuildDoneFunc  on_installed,
                                gpointer               user_data)
{
  Build *build = build_get ();
  g_autofree char *saved = NULL;
  GtkWindow *parent_window;
  GtkWidget *window;
  GtkWidget *column;
  GtkWidget *header;
  GtkWidget *title;
  GtkWidget *description;
  GtkWidget *body;
  GtkWidget *entry;
  GtkWidget *footer;
  GtkWidget *spacer;
  GtkWidget *close;
  GtkEventController *keys;

  g_return_if_fail (GTK_IS_WIDGET (parent));

  /* Already open: that window is the one to look at. */
  if (build->window != NULL)
    {
      gtk_window_present (build->window);
      return;
    }

  xd_panel_style_ensure ();

  saved = g_settings_get_string (build->settings, "build-source");
  parent_window = GTK_WINDOW (gtk_widget_get_root (parent));

  build->on_installed = on_installed;
  build->user_data = user_data;

  /*
   * A window of its own, the way the pairing panel is one.
   *
   * It was a dialog, which is a widget inside the window carrying its own
   * idea of what a panel looks like and of what should happen to the window
   * underneath it -- and getting that idea to agree with this app took three
   * attempts across two libadwaita versions. A panel xd draws itself has
   * nothing to disagree with, and it is the shape the rest of xd's own
   * windows already are.
   */
  window = gtk_window_new ();
  build->window = GTK_WINDOW (window);
  gtk_window_set_transient_for (GTK_WINDOW (window), parent_window);
  gtk_window_set_application (
    GTK_WINDOW (window), gtk_window_get_application (parent_window));
  gtk_window_set_destroy_with_parent (GTK_WINDOW (window), TRUE);
  gtk_window_set_decorated (GTK_WINDOW (window), FALSE);
  gtk_window_set_default_size (GTK_WINDOW (window), 620, -1);
  gtk_widget_add_css_class (window, "xd-panel");

  title = gtk_label_new ("Build a Branch");
  gtk_label_set_xalign (GTK_LABEL (title), 0.0f);
  gtk_widget_add_css_class (title, "title-3");

  description = gtk_label_new (
    "The branch is fetched, built the way the nightly is built, and installed "
    "over this copy. It needs Docker; Git is bundled. The update button puts the "
    "nightly back.");
  gtk_label_set_xalign (GTK_LABEL (description), 0.0f);
  gtk_label_set_wrap (GTK_LABEL (description), TRUE);
  gtk_widget_add_css_class (description, "dim-label");

  header = gtk_box_new (GTK_ORIENTATION_VERTICAL, 5);
  gtk_box_append (GTK_BOX (header), title);
  gtk_box_append (GTK_BOX (header), description);
  gtk_widget_add_css_class (header, "xd-panel-bar");
  gtk_widget_add_css_class (header, "xd-panel-head");

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

  /*
   * What the build is printing, one line at a time.
   *
   * It was a scrolling view of the whole log, rewritten on every line docker
   * printed -- which flickered, jumped under the scrollbar and left the client
   * with no time for anything else. A build is worth watching, not worth
   * reading: the line it is on now while it runs, and the last few of them if
   * it stops.
   */
  build->activity = GTK_LABEL (gtk_label_new (NULL));
  gtk_label_set_xalign (build->activity, 0.0f);
  gtk_label_set_ellipsize (build->activity, PANGO_ELLIPSIZE_END);
  gtk_label_set_selectable (build->activity, TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (build->activity), "xd-workflow-log");
  gtk_widget_set_visible (GTK_WIDGET (build->activity), FALSE);

  body = gtk_box_new (GTK_ORIENTATION_VERTICAL, 12);
  gtk_widget_set_margin_top (body, 20);
  gtk_widget_set_margin_bottom (body, 20);
  gtk_widget_set_margin_start (body, 22);
  gtk_widget_set_margin_end (body, 22);
  gtk_box_append (GTK_BOX (body), entry);
  gtk_box_append (GTK_BOX (body), GTK_WIDGET (build->status));
  gtk_box_append (GTK_BOX (body), GTK_WIDGET (build->activity));

  footer = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 12);
  gtk_box_append (GTK_BOX (footer), hint ("Esc", "Close"));
  gtk_box_append (GTK_BOX (footer), hint ("Enter", "Build"));
  spacer = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
  gtk_widget_set_hexpand (spacer, TRUE);
  gtk_box_append (GTK_BOX (footer), spacer);

  build->spinner = gtk_spinner_new ();
  gtk_widget_set_visible (build->spinner, FALSE);
  gtk_box_append (GTK_BOX (footer), build->spinner);

  close = gtk_button_new_with_label ("Close");
  gtk_widget_add_css_class (close, "flat");
  g_signal_connect (close, "clicked", G_CALLBACK (on_close_clicked), build);
  gtk_box_append (GTK_BOX (footer), close);

  build->action = GTK_BUTTON (gtk_button_new_with_label ("Build and Install"));
  gtk_widget_add_css_class (GTK_WIDGET (build->action), "xd-panel-action");
  g_signal_connect (build->action, "clicked",
                    G_CALLBACK (on_action_clicked), build);
  gtk_box_append (GTK_BOX (footer), GTK_WIDGET (build->action));
  gtk_widget_add_css_class (footer, "xd-panel-bar");
  gtk_widget_add_css_class (footer, "xd-panel-foot");

  column = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  gtk_box_append (GTK_BOX (column), header);
  gtk_box_append (GTK_BOX (column), body);
  gtk_box_append (GTK_BOX (column), footer);
  gtk_window_set_child (GTK_WINDOW (window), column);
  gtk_window_set_default_widget (GTK_WINDOW (window),
                                 GTK_WIDGET (build->action));

  keys = gtk_event_controller_key_new ();
  gtk_event_controller_set_propagation_phase (keys, GTK_PHASE_CAPTURE);
  g_signal_connect (keys, "key-pressed", G_CALLBACK (on_key), build);
  gtk_widget_add_controller (window, keys);

  g_signal_connect (window, "notify::is-active",
                    G_CALLBACK (on_active_changed), build);
  g_signal_connect (window, "close-request", G_CALLBACK (save_source), build);
  g_object_weak_ref (G_OBJECT (window), on_window_gone, build_ref (build));

  show_state (build);

  gtk_window_present (GTK_WINDOW (window));
  gtk_widget_grab_focus (entry);
}
