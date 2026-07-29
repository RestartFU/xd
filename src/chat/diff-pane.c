#include "diff-pane.h"

#include <string.h>

#include "diff-view.h"

/*
 * What the agent changed, without leaving the chat.
 *
 * Read-only on purpose: staging and committing are decisions worth making
 * where the whole repository is in view, and a pane that can commit from
 * inside a chat invites doing it without looking. This answers "what did it
 * just do to my files".
 */

struct _XdDiffPane
{
  AdwBin parent_instance;

  char *workdir;
  char *base;               /* what the branch is measured against */
  XdRemoteClient *remote;
  char *chat_id;
  gboolean branch_mode;
  GCancellable *cancellable;

  GtkListView *diff_lines;
  GtkLabel *summary;
  GtkWidget *stack;
};

G_DEFINE_FINAL_TYPE (XdDiffPane, xd_diff_pane, ADW_TYPE_BIN)

typedef enum
{
  DIFF_READ_BASE,
  DIFF_READ_WORKING_ALL,
  DIFF_READ_BRANCH_ALL,
} DiffRead;

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

/*
 * Git excludes untracked files from ordinary diffs. Append a no-index patch
 * for each one so "Working" really means every visible working-tree change.
 * NUL separation preserves spaces and newlines in paths.
 */
static const char *WORKING_DIFF_SCRIPT =
  "git --no-pager diff HEAD; "
  "git ls-files --others --exclude-standard -z | "
  "xargs -0 -n 1 sh -c "
  "'[ \"$#\" -eq 0 ] || "
  "git --no-pager diff --no-index -- /dev/null \"$1\"' sh";

/*
 * Runs git in the working directory and hands back its output.
 *
 * git is spawned rather than a library being linked: xd already depends on
 * the user's git for everything else it reports, and the plumbing commands
 * used here have output formats git keeps stable on purpose.
 */
static void
run_local_git (XdDiffPane          *self,
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

static const char *
diff_read_name (DiffRead read)
{
  switch (read)
    {
    case DIFF_READ_BASE:        return "base";
    case DIFF_READ_WORKING_ALL: return "working-all";
    case DIFF_READ_BRANCH_ALL:  return "branch-all";
    default:                    return NULL;
    }
}

/*
 * The pane consumes command output the same way locally and remotely.
 *
 * Local reads spawn git here. Remote reads name the exact read-only view and
 * let the daemon resolve the chat's own working directory; paths on the
 * client machine are never used to reach into a remote repository.
 */
static void
run_git (XdDiffPane          *self,
         DiffRead             read,
         const char          *path,
         GAsyncReadyCallback  callback,
         gpointer             user_data)
{
  if (self->remote != NULL)
    {
      g_autoptr (JsonBuilder) builder = json_builder_new ();
      g_autoptr (JsonNode) request = NULL;

      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "op");
      json_builder_add_string_value (builder, "diff-read");
      json_builder_set_member_name (builder, "chat");
      json_builder_add_string_value (builder, self->chat_id);
      json_builder_set_member_name (builder, "read");
      json_builder_add_string_value (builder, diff_read_name (read));
      if (path != NULL)
        {
          json_builder_set_member_name (builder, "path");
          json_builder_add_string_value (builder, path);
        }
      if (self->base != NULL && read == DIFF_READ_BRANCH_ALL)
        {
          json_builder_set_member_name (builder, "base");
          json_builder_add_string_value (builder, self->base);
        }
      json_builder_end_object (builder);

      request = json_builder_get_root (builder);
      xd_remote_client_call_async (self->remote, request, self->cancellable,
                                   callback, user_data);
      return;
    }

  switch (read)
    {
    case DIFF_READ_BASE:
      {
        const char *argv[] = { "sh", "-c", BASE_SCRIPT, NULL };

        run_local_git (self, argv, callback, user_data);
        break;
      }
    case DIFF_READ_WORKING_ALL:
      {
        const char *argv[] = { "sh", "-c", WORKING_DIFF_SCRIPT, NULL };

        run_local_git (self, argv, callback, user_data);
        break;
      }
    case DIFF_READ_BRANCH_ALL:
      {
        g_autofree char *range = g_strdup_printf ("%s...HEAD", self->base);
        const char *argv[] = { "git", "--no-pager", "diff",
                               range, NULL };

        run_local_git (self, argv, callback, user_data);
        break;
      }
    }
}

static gboolean
finish_git_read (GObject       *source,
                 GAsyncResult  *result,
                 char         **output,
                 GError       **error)
{
  if (G_IS_SUBPROCESS (source))
    return g_subprocess_communicate_utf8_finish (
      G_SUBPROCESS (source), result, output, NULL, error);

  if (XD_IS_REMOTE_CLIENT (source))
    {
      g_autoptr (JsonObject) reply =
        xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, error);

      if (reply == NULL)
        return FALSE;

      *output = g_strdup (
        json_object_get_string_member_with_default (reply, "output", ""));
      return TRUE;
    }

  g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_FAILED,
                       "Unknown diff source.");
  return FALSE;
}

static void
show_read_error (XdDiffPane *self,
                 GError     *error)
{
  const char *summary;

  if (g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    return;

  summary = self->remote != NULL &&
            g_strcmp0 (error != NULL ? error->message : NULL, "Unknown op") == 0
    ? "Update xd on the remote machine"
    : "Could not read changes";

  gtk_label_set_label (self->summary, summary);
  gtk_widget_set_tooltip_text (GTK_WIDGET (self->summary),
                               error != NULL ? error->message : NULL);
  gtk_stack_set_visible_child_name (GTK_STACK (self->stack), "empty");
}

/* --- the complete diff ----------------------------------------------------- */

static void
clear_diff (XdDiffPane *self)
{
  xd_diff_view_fill_file_sections (
    self->diff_lines, "", NULL, NULL);
}

static void
on_diff_read (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  XdDiffPane *self = user_data;
  g_autofree char *output = NULL;
  g_autoptr (GError) error = NULL;
  guint additions = 0;
  guint deletions = 0;
  guint changed = 0;

  if (!finish_git_read (source, result, &output, &error))
    {
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        {
          g_debug ("cannot read the diff: %s", error->message);
          show_read_error (self, error);
        }

      return;
    }

  if (output == NULL || *output == '\0')
    {
      clear_diff (self);
      gtk_label_set_label (self->summary, "No changes");
      gtk_stack_set_visible_child_name (GTK_STACK (self->stack), "empty");
      return;
    }

  for (const char *at = output; at != NULL && *at != '\0';)
    {
      if ((at == output || at[-1] == '\n') &&
          g_str_has_prefix (at, "diff --git "))
        changed++;

      at = strchr (at, '\n');
      if (at != NULL)
        at++;
    }

  xd_diff_view_fill_file_sections (
    self->diff_lines, output, &additions, &deletions);

  {
    g_autofree char *summary = g_strdup_printf (
      changed == 1 ? "1 file changed  ·  +%u  −%u"
                   : "%u files changed  ·  +%u  −%u",
      changed, additions, deletions);

    gtk_label_set_label (self->summary, summary);
  }
  gtk_stack_set_visible_child_name (GTK_STACK (self->stack), "changes");
}

static void
read_diff (XdDiffPane *self)
{
  if (self->branch_mode)
    run_git (self, DIFF_READ_BRANCH_ALL, NULL, on_diff_read, self);
  else
    run_git (self, DIFF_READ_WORKING_ALL, NULL, on_diff_read, self);
}

static void
on_base_read (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  XdDiffPane *self = user_data;
  g_autofree char *output = NULL;
  g_autoptr (GError) error = NULL;

  if (!finish_git_read (source, result, &output, &error))
    {
      show_read_error (self, error);
      return;
    }

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

  read_diff (self);
}

void
xd_diff_pane_refresh (XdDiffPane *self)
{
  g_return_if_fail (XD_IS_DIFF_PANE (self));

  if (!gtk_widget_is_visible (GTK_WIDGET (self)))
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
      run_git (self, DIFF_READ_BASE, NULL, on_base_read, self);
      return;
    }

  read_diff (self);
}

void
xd_diff_pane_set_workdir (XdDiffPane *self,
                          const char *workdir)
{
  g_return_if_fail (XD_IS_DIFF_PANE (self));

  if (g_strcmp0 (self->workdir, workdir) == 0)
    return;

  g_free (self->workdir);
  self->workdir = g_strdup (workdir);

  xd_diff_pane_refresh (self);
}

void
xd_diff_pane_set_remote (XdDiffPane     *self,
                         XdRemoteClient *client,
                         const char     *chat_id)
{
  g_return_if_fail (XD_IS_DIFF_PANE (self));
  g_return_if_fail (client == NULL || XD_IS_REMOTE_CLIENT (client));
  g_return_if_fail ((client == NULL) == (chat_id == NULL));

  if (self->remote == client && g_strcmp0 (self->chat_id, chat_id) == 0)
    return;

  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  self->cancellable = g_cancellable_new ();

  g_set_object (&self->remote, client);
  g_free (self->chat_id);
  self->chat_id = g_strdup (chat_id);

  xd_diff_pane_refresh (self);
}

XdDiffPane *
xd_diff_pane_new (void)
{
  return g_object_new (XD_TYPE_DIFF_PANE, NULL);
}

static void
on_scope_changed (GtkToggleButton *button,
                  gpointer         user_data)
{
  XdDiffPane *self = user_data;

  self->branch_mode = gtk_toggle_button_get_active (button);

  xd_diff_pane_refresh (self);
}

static void
xd_diff_pane_dispose (GObject *object)
{
  XdDiffPane *self = XD_DIFF_PANE (object);

  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  g_clear_object (&self->remote);
  g_clear_pointer (&self->workdir, g_free);
  g_clear_pointer (&self->base, g_free);
  g_clear_pointer (&self->chat_id, g_free);

  G_OBJECT_CLASS (xd_diff_pane_parent_class)->dispose (object);
}

static void
xd_diff_pane_class_init (XdDiffPaneClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = xd_diff_pane_dispose;
}

static void
xd_diff_pane_init (XdDiffPane *self)
{
  GtkWidget *box = gtk_box_new (GTK_ORIENTATION_VERTICAL, 0);
  GtkWidget *header = gtk_box_new (GTK_ORIENTATION_HORIZONTAL, 8);
  GtkWidget *refresh = gtk_button_new_from_icon_name ("view-refresh-symbolic");
  GtkWidget *diff_window = gtk_scrolled_window_new ();
  GtkWidget *empty = adw_status_page_new ();

  self->summary = GTK_LABEL (gtk_label_new ("No changes"));
  gtk_label_set_xalign (self->summary, 0.0f);
  gtk_widget_set_hexpand (GTK_WIDGET (self->summary), TRUE);
  gtk_widget_add_css_class (GTK_WIDGET (self->summary), "heading");

  gtk_widget_add_css_class (refresh, "flat");
  gtk_widget_set_tooltip_text (refresh, "Read again");
  g_signal_connect_swapped (refresh, "clicked",
                            G_CALLBACK (xd_diff_pane_refresh), self);

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

  self->diff_lines = GTK_LIST_VIEW (
    xd_diff_view_new_file_sections ());
  gtk_widget_set_hexpand (GTK_WIDGET (self->diff_lines), TRUE);

  gtk_scrolled_window_set_policy (GTK_SCROLLED_WINDOW (diff_window),
                                  GTK_POLICY_EXTERNAL, GTK_POLICY_EXTERNAL);
  gtk_scrolled_window_set_child (GTK_SCROLLED_WINDOW (diff_window),
                                 GTK_WIDGET (self->diff_lines));
  gtk_widget_set_vexpand (diff_window, TRUE);

  adw_status_page_set_icon_name (ADW_STATUS_PAGE (empty), "object-select-symbolic");
  adw_status_page_set_title (ADW_STATUS_PAGE (empty), "Nothing Changed");

  self->stack = gtk_stack_new ();
  gtk_stack_add_named (GTK_STACK (self->stack), empty, "empty");
  gtk_stack_add_named (GTK_STACK (self->stack), diff_window, "changes");
  gtk_widget_set_vexpand (self->stack, TRUE);

  gtk_box_append (GTK_BOX (box), header);
  gtk_box_append (GTK_BOX (box), gtk_separator_new (GTK_ORIENTATION_HORIZONTAL));
  gtk_box_append (GTK_BOX (box), self->stack);

  adw_bin_set_child (ADW_BIN (self), box);
}
