#include "diff-pane.h"

#include <string.h>

/*
 * What the agent changed, without leaving the chat.
 *
 * Read-only on purpose: staging and committing are decisions worth making
 * where the whole repository is in view, and a pane that can commit from
 * inside a chat invites doing it without looking. This answers "what did it
 * just do to my files".
 */

struct _HyDiffPane
{
  AdwBin parent_instance;

  char *workdir;
  char *base;               /* what the branch is measured against */
  gboolean branch_mode;
  GCancellable *cancellable;

  GtkListBox *files;
  GtkTextView *diff;
  GtkLabel *summary;
  GtkWidget *stack;
  GtkWidget *changes;
};

G_DEFINE_FINAL_TYPE (HyDiffPane, hy_diff_pane, ADW_TYPE_BIN)

typedef struct
{
  HyDiffPane *pane;
  char *path;
  gboolean untracked;
} DiffRequest;

/*
 * Where this branch left the one it came from.
 *
 * Tried in order: what the remote calls its default, then the usual names.
 * A shell runs the chain because each candidate only matters if the ones
 * before it are absent, and spawning git five times to find that out is
 * five round trips for one answer.
 */
static const char *BASE_SCRIPT =
  "for ref in \"$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD)\" "
  "origin/main origin/master main master; do "
  "  [ -n \"$ref\" ] || continue; "
  "  git rev-parse --verify --quiet \"$ref\" >/dev/null && { echo \"$ref\"; exit 0; }; "
  "done";

static void load_diff (HyDiffPane *self, const char *path, gboolean untracked);

static void
diff_request_free (DiffRequest *request)
{
  g_free (request->path);
  g_free (request);
}

/*
 * Runs git in the working directory and hands back its output.
 *
 * git is spawned rather than a library being linked: hy already depends on
 * the user's git for everything else it reports, and the plumbing commands
 * used here have output formats git keeps stable on purpose.
 */
static void
run_git (HyDiffPane          *self,
         const char *const   *argv,
         GAsyncReadyCallback  callback,
         gpointer             user_data)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                        G_SUBPROCESS_FLAGS_STDERR_SILENCE);
  g_subprocess_launcher_set_cwd (launcher, self->workdir);

  process = g_subprocess_launcher_spawnv (launcher, (const char * const *) argv, &error);
  if (process == NULL)
    {
      g_debug ("cannot run git: %s", error->message);
      return;
    }

  g_subprocess_communicate_utf8_async (process, NULL, self->cancellable,
                                       callback, user_data);
}

/* --- the diff of one file -------------------------------------------------- */

static void
on_diff_read (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  DiffRequest *request = user_data;
  g_autofree char *output = NULL;
  g_autoptr (GError) error = NULL;
  GtkTextBuffer *buffer;
  GtkTextIter at;
  g_auto (GStrv) lines = NULL;

  if (!g_subprocess_communicate_utf8_finish (G_SUBPROCESS (source), result,
                                             &output, NULL, &error))
    {
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        g_debug ("cannot read the diff: %s", error->message);

      diff_request_free (request);
      return;
    }

  buffer = gtk_text_view_get_buffer (request->pane->diff);
  gtk_text_buffer_set_text (buffer, "", 0);
  gtk_text_buffer_get_start_iter (buffer, &at);

  lines = g_strsplit (output != NULL ? output : "", "\n", -1);

  for (gsize i = 0; lines[i] != NULL; i++)
    {
      const char *line = lines[i];
      const char *tag = NULL;

      /* "+++"/"---" name the file and are not changes, so they read as
       * headers rather than as a whole file added and removed. */
      if (g_str_has_prefix (line, "diff ") || g_str_has_prefix (line, "index ") ||
          g_str_has_prefix (line, "+++") || g_str_has_prefix (line, "---") ||
          g_str_has_prefix (line, "new file") || g_str_has_prefix (line, "deleted file"))
        tag = "header";
      else if (g_str_has_prefix (line, "@@"))
        tag = "hunk";
      else if (line[0] == '+')
        tag = "added";
      else if (line[0] == '-')
        tag = "removed";

      if (tag != NULL)
        gtk_text_buffer_insert_with_tags_by_name (buffer, &at, line, -1, tag, NULL);
      else
        gtk_text_buffer_insert (buffer, &at, line, -1);

      gtk_text_buffer_insert (buffer, &at, "\n", 1);
    }

  diff_request_free (request);
}

static void
load_diff (HyDiffPane *self,
           const char *path,
           gboolean    untracked)
{
  DiffRequest *request;

  if (self->workdir == NULL || path == NULL)
    return;

  request = g_new0 (DiffRequest, 1);
  request->pane = self;
  request->path = g_strdup (path);
  request->untracked = untracked;

  if (self->branch_mode)
    {
      /* Three dots: what this branch added since it diverged, rather than
       * every difference between the two tips. That is the change under
       * review -- what a pull request shows. */
      g_autofree char *range = g_strdup_printf ("%s...HEAD", self->base);
      const char *argv[] = { "git", "--no-pager", "diff", range, "--", path, NULL };

      run_git (self, argv, on_diff_read, request);
    }
  else if (untracked)
    {
      /* A file git does not know about has nothing to be compared against,
       * so it is diffed against nothing and reads as all additions. */
      const char *argv[] = { "git", "--no-pager", "diff", "--no-index",
                             "--", "/dev/null", path, NULL };

      run_git (self, argv, on_diff_read, request);
    }
  else
    {
      /* Against HEAD rather than the index, so staged and unstaged changes
       * appear together -- an agent's work is not usually split between
       * them, and a half-shown diff would be misleading. */
      const char *argv[] = { "git", "--no-pager", "diff", "HEAD", "--", path, NULL };

      run_git (self, argv, on_diff_read, request);
    }
}

/* --- the list of changed files --------------------------------------------- */

static void
on_file_selected (GtkListBox    *box,
                  GtkListBoxRow *row,
                  gpointer       user_data)
{
  HyDiffPane *self = user_data;

  if (row == NULL)
    return;

  load_diff (self, g_object_get_data (G_OBJECT (row), "path"),
             GPOINTER_TO_INT (g_object_get_data (G_OBJECT (row), "untracked")));
}

/* The two status letters, as words. */
static const char *
status_label (const char *code)
{
  if (code[0] == '?')            return "New";
  if (code[0] == 'A' || code[1] == 'A') return "Added";
  if (code[0] == 'D' || code[1] == 'D') return "Deleted";
  if (code[0] == 'R')            return "Renamed";

  return "Modified";
}

static void
add_file_row (HyDiffPane *self,
              const char *code,
              const char *path)
{
  GtkWidget *row = gtk_list_box_row_new ();
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *name = gtk_label_new (path);
  GtkWidget *state = gtk_label_new (status_label (code));

  gtk_label_set_xalign (GTK_LABEL (name), 0.0f);
  gtk_label_set_ellipsize (GTK_LABEL (name), PANGO_ELLIPSIZE_START);
  gtk_widget_set_hexpand (name, TRUE);
  gtk_widget_set_tooltip_text (name, path);

  gtk_widget_add_css_class (state, "dim-label");
  gtk_widget_add_css_class (state, "caption");

  gtk_box_append (GTK_BOX (box), name);
  gtk_box_append (GTK_BOX (box), state);
  gtk_widget_set_margin_start (box, 8);
  gtk_widget_set_margin_end (box, 8);
  gtk_widget_set_margin_top (box, 4);
  gtk_widget_set_margin_bottom (box, 4);

  gtk_list_box_row_set_child (GTK_LIST_BOX_ROW (row), box);
  g_object_set_data_full (G_OBJECT (row), "path", g_strdup (path), g_free);
  g_object_set_data (G_OBJECT (row), "untracked", GINT_TO_POINTER (code[0] == '?'));

  gtk_list_box_append (self->files, row);
}

static void
on_status_read (GObject      *source,
                GAsyncResult *result,
                gpointer      user_data)
{
  HyDiffPane *self = user_data;
  g_autofree char *output = NULL;
  g_autoptr (GError) error = NULL;
  g_auto (GStrv) lines = NULL;
  guint changed = 0;

  if (!g_subprocess_communicate_utf8_finish (G_SUBPROCESS (source), result,
                                             &output, NULL, &error))
    {
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        g_debug ("cannot read the status: %s", error->message);
      return;
    }

  gtk_list_box_remove_all (self->files);
  gtk_text_buffer_set_text (gtk_text_view_get_buffer (self->diff), "", 0);

  lines = g_strsplit (output != NULL ? output : "", "\n", -1);

  for (gsize i = 0; lines[i] != NULL; i++)
    {
      if (strlen (lines[i]) < 4)
        continue;

      /* status --porcelain gives "XY path"; diff --name-status gives the
       * letter, a tab, then the path. */
      if (self->branch_mode)
        {
          const char *tab = strchr (lines[i], '\t');

          if (tab == NULL)
            continue;

          add_file_row (self, lines[i], tab + 1);
        }
      else
        {
          add_file_row (self, lines[i], lines[i] + 3);
        }

      changed++;
    }

  if (changed == 0)
    {
      gtk_label_set_label (self->summary, "No changes");
      gtk_stack_set_visible_child_name (GTK_STACK (self->stack), "empty");
      return;
    }

  {
    g_autofree char *summary = changed == 1
      ? g_strdup ("1 file changed")
      : g_strdup_printf ("%u files changed", changed);

    gtk_label_set_label (self->summary, summary);
  }
  gtk_stack_set_visible_child_name (GTK_STACK (self->stack), "changes");

  /* Half the pane at most: enough to see the shape of the change list without
   * pushing the diff itself off the bottom. */
  {
    int available = gtk_widget_get_height (self->changes);

    if (available > 0)
      gtk_paned_set_position (GTK_PANED (self->changes),
                              MIN (available / 2, (int) changed * 34 + 8));
  }

  /* Showing the first file beats showing an empty pane next to a list. */
  gtk_list_box_select_row (self->files,
                           gtk_list_box_get_row_at_index (self->files, 0));
}

static void
read_changed_files (HyDiffPane *self)
{
  if (self->branch_mode)
    {
      g_autofree char *range = g_strdup_printf ("%s...HEAD", self->base);
      const char *argv[] = { "git", "--no-pager", "diff", "--name-status",
                             range, NULL };

      run_git (self, argv, on_status_read, self);
    }
  else
    {
      const char *argv[] = { "git", "status", "--porcelain", NULL };

      run_git (self, argv, on_status_read, self);
    }
}

static void
on_base_read (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  HyDiffPane *self = user_data;
  g_autofree char *output = NULL;
  g_autoptr (GError) error = NULL;

  if (!g_subprocess_communicate_utf8_finish (G_SUBPROCESS (source), result,
                                             &output, NULL, &error))
    return;

  if (output != NULL)
    g_strstrip (output);

  if (output == NULL || *output == '\0')
    {
      /* No branch to compare against: a repository with no remote and no
       * main, or a checkout of the default branch itself. */
      gtk_label_set_label (self->summary, "No branch to compare against");
      gtk_stack_set_visible_child_name (GTK_STACK (self->stack), "empty");
      return;
    }

  g_free (self->base);
  self->base = g_steal_pointer (&output);

  read_changed_files (self);
}

void
hy_diff_pane_refresh (HyDiffPane *self)
{
  g_return_if_fail (HY_IS_DIFF_PANE (self));

  if (!gtk_widget_get_visible (GTK_WIDGET (self)))
    return;

  /* An earlier read is no longer the answer to anything. */
  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  self->cancellable = g_cancellable_new ();

  if (self->workdir == NULL)
    {
      gtk_label_set_label (self->summary, "No working directory");
      gtk_stack_set_visible_child_name (GTK_STACK (self->stack), "empty");
      return;
    }

  /* Resolved every time rather than remembered: branches are switched and
   * remotes are added while the pane sits there. */
  if (self->branch_mode)
    {
      const char *argv[] = { "sh", "-c", BASE_SCRIPT, NULL };

      run_git (self, argv, on_base_read, self);
      return;
    }

  read_changed_files (self);
}

void
hy_diff_pane_set_workdir (HyDiffPane *self,
                          const char *workdir)
{
  g_return_if_fail (HY_IS_DIFF_PANE (self));

  if (g_strcmp0 (self->workdir, workdir) == 0)
    return;

  g_free (self->workdir);
  self->workdir = g_strdup (workdir);

  hy_diff_pane_refresh (self);
}

HyDiffPane *
hy_diff_pane_new (void)
{
  return g_object_new (HY_TYPE_DIFF_PANE, NULL);
}

static void
on_scope_changed (GtkToggleButton *button,
                  gpointer         user_data)
{
  HyDiffPane *self = user_data;

  self->branch_mode = gtk_toggle_button_get_active (button);

  hy_diff_pane_refresh (self);
}

static void
hy_diff_pane_dispose (GObject *object)
{
  HyDiffPane *self = HY_DIFF_PANE (object);

  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  g_clear_pointer (&self->workdir, g_free);
  g_clear_pointer (&self->base, g_free);

  G_OBJECT_CLASS (hy_diff_pane_parent_class)->dispose (object);
}

static void
hy_diff_pane_class_init (HyDiffPaneClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = hy_diff_pane_dispose;
}

static void
hy_diff_pane_init (HyDiffPane *self)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *header = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *refresh = gtk_button_new_from_icon_name ("view-refresh-symbolic");
  GtkWidget *files_window = gtk_scrolled_window_new ();
  GtkWidget *diff_window = gtk_scrolled_window_new ();
  GtkWidget *empty = adw_status_page_new ();
  GtkWidget *changes = gtk_paned_new (GTK_ORIENTATION_VERTICAL);
  GtkTextBuffer *buffer;

  self->summary = GTK_LABEL (gtk_label_new ("No changes"));
  gtk_label_set_xalign (self->summary, 0.0f);
  gtk_widget_set_hexpand (GTK_WIDGET (self->summary), TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (self->summary), "heading");

  gtk_widget_add_css_class (refresh, "flat");
  gtk_widget_set_tooltip_text (refresh, "Read again");
  g_signal_connect_swapped (refresh, "clicked",
                            G_CALLBACK (hy_diff_pane_refresh), self);

  /*
   * Two questions, both worth asking here: what has changed since the last
   * commit, and what this branch changes as a whole. The second is the one a
   * pull request shows, and is not visible from the working tree at all once
   * the work has been committed.
   */
  {
    GtkWidget *modes = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 0);
    GtkWidget *working = gtk_toggle_button_new_with_label ("Working");
    GtkWidget *branch = gtk_toggle_button_new_with_label ("Branch");

    gtk_widget_set_tooltip_text (working, "Changes not yet committed");
    gtk_widget_set_tooltip_text (branch, "Everything this branch changes");
    gtk_toggle_button_set_group (GTK_TOGGLE_BUTTON (branch),
                                 GTK_TOGGLE_BUTTON (working));
    gtk_toggle_button_set_active (GTK_TOGGLE_BUTTON (working), TRUE);
    g_signal_connect (branch, "toggled", G_CALLBACK (on_scope_changed), self);

    gtk_widget_add_css_class (modes, "linked");
    gtk_box_append (GTK_BOX (modes), working);
    gtk_box_append (GTK_BOX (modes), branch);

    gtk_box_append (GTK_BOX (header), GTK_WIDGET (self->summary));
    gtk_box_append (GTK_BOX (header), modes);
    gtk_box_append (GTK_BOX (header), refresh);
  }
  gtk_widget_set_margin_start (header, 12);
  gtk_widget_set_margin_end (header, 6);
  gtk_widget_set_margin_top (header, 6);
  gtk_widget_set_margin_bottom (header, 6);

  self->files = GTK_LIST_BOX (gtk_list_box_new ());
  gtk_list_box_set_selection_mode (self->files, GTK_SELECTION_SINGLE);
  gtk_widget_add_css_class (GTK_WIDGET (self->files), "navigation-sidebar");
  g_signal_connect (self->files, "row-selected", G_CALLBACK (on_file_selected), self);

  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (files_window),
                                 GTK_WIDGET (self->files));
  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (files_window),
                                  GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);

  self->diff = GTK_TEXT_VIEW (gtk_text_view_new ());
  gtk_text_view_set_editable (self->diff, FALSE);
  gtk_text_view_set_monospace (self->diff, TRUE);
  gtk_text_view_set_left_margin (self->diff, 8);
  gtk_text_view_set_top_margin (self->diff, 4);

  buffer = gtk_text_view_get_buffer (self->diff);
  gtk_text_buffer_create_tag (buffer, "added", "foreground", "#57e389", NULL);
  gtk_text_buffer_create_tag (buffer, "removed", "foreground", "#f66151", NULL);
  gtk_text_buffer_create_tag (buffer, "hunk", "foreground", "#78aeed", NULL);
  gtk_text_buffer_create_tag (buffer, "header", "weight", PANGO_WEIGHT_BOLD, NULL);

  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (diff_window),
                                 GTK_WIDGET (self->diff));

  /*
   * The list takes what it needs before it starts scrolling.
   *
   * A fixed height wasted the pane both ways: five rows visible with a
   * hundred files left to scroll through, and the same five rows floating
   * above empty space when only one file changed. It grows to its contents
   * and stops at half the pane, past which the diff needs the room more.
   */
  gtk_scrolled_window_set_propagate_natural_height (GTK_SCROLLED_WINDOW (files_window), TRUE);
  gtk_widget_set_vexpand (files_window, FALSE);

  gtk_paned_set_start_child (GTK_PANED (changes), files_window);
  gtk_paned_set_end_child (GTK_PANED (changes), diff_window);
  gtk_paned_set_resize_start_child (GTK_PANED (changes), TRUE);
  gtk_paned_set_shrink_start_child (GTK_PANED (changes), FALSE);
  gtk_paned_set_resize_end_child (GTK_PANED (changes), TRUE);

  adw_status_page_set_icon_name (ADW_STATUS_PAGE (empty), "object-select-symbolic");
  adw_status_page_set_title (ADW_STATUS_PAGE (empty), "Nothing Changed");

  self->stack = gtk_stack_new ();
  gtk_stack_add_named (GTK_STACK (self->stack), empty, "empty");
  gtk_stack_add_named (GTK_STACK (self->stack), changes, "changes");
  self->changes = changes;
  gtk_widget_set_vexpand (self->stack, TRUE);

  gtk_box_append (GTK_BOX (box), header);
  gtk_box_append (GTK_BOX (box), gtk_separator_new (GTK_ORIENTATION_HORIZONTAL));
  gtk_box_append (GTK_BOX (box), self->stack);

  adw_bin_set_child (ADW_BIN (self), box);
}
