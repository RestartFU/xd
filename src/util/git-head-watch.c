#include "git-head-watch.h"

#include "git-info.h"

#define HEAD_SETTLE_MS 75

struct _XdGitHeadWatch
{
  GObject parent_instance;

  char *git_dir;
  GFileMonitor *monitor;
  guint settle_id;
};

enum
{
  SIGNAL_CHANGED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (XdGitHeadWatch, xd_git_head_watch, G_TYPE_OBJECT)

static gboolean
emit_settled_change (gpointer user_data)
{
  XdGitHeadWatch *self = user_data;

  self->settle_id = 0;
  g_signal_emit (self, signals[SIGNAL_CHANGED], 0);

  return G_SOURCE_REMOVE;
}

static gboolean
is_head (GFile *file)
{
  g_autofree char *name = file != NULL ? g_file_get_basename (file) : NULL;

  return g_strcmp0 (name, "HEAD") == 0;
}

static void
on_git_dir_changed (GFileMonitor      *monitor,
                    GFile             *file,
                    GFile             *other_file,
                    GFileMonitorEvent  event,
                    gpointer           user_data)
{
  XdGitHeadWatch *self = user_data;

  /*
   * Git normally writes HEAD.lock and renames it over HEAD. Depending on the
   * monitor backend, HEAD is either @file or @other_file.
   */
  if (!is_head (file) && !is_head (other_file))
    return;

  g_clear_handle_id (&self->settle_id, g_source_remove);
  self->settle_id =
    g_timeout_add (HEAD_SETTLE_MS, emit_settled_change, self);
}

void
xd_git_head_watch_set_workdir (XdGitHeadWatch *self,
                               const char     *workdir)
{
  g_autoptr (XdGitInfo) info = NULL;
  g_autoptr (GFile) directory = NULL;
  g_autoptr (GError) error = NULL;
  const char *git_dir;

  g_return_if_fail (XD_IS_GIT_HEAD_WATCH (self));

  info = xd_git_info_for_path (workdir);
  git_dir = info != NULL ? info->git_dir : NULL;
  if (g_strcmp0 (self->git_dir, git_dir) == 0)
    return;

  g_clear_handle_id (&self->settle_id, g_source_remove);
  g_clear_object (&self->monitor);
  g_clear_pointer (&self->git_dir, g_free);

  if (git_dir == NULL)
    return;

  self->git_dir = g_strdup (git_dir);
  directory = g_file_new_for_path (git_dir);
  self->monitor = g_file_monitor_directory (
    directory, G_FILE_MONITOR_WATCH_MOVES, NULL, &error);
  if (self->monitor == NULL)
    {
      g_debug ("cannot watch Git HEAD in %s: %s", git_dir, error->message);
      return;
    }

  g_signal_connect (self->monitor, "changed",
                    G_CALLBACK (on_git_dir_changed), self);
}

XdGitHeadWatch *
xd_git_head_watch_new (void)
{
  return g_object_new (XD_TYPE_GIT_HEAD_WATCH, NULL);
}

static void
xd_git_head_watch_dispose (GObject *object)
{
  XdGitHeadWatch *self = XD_GIT_HEAD_WATCH (object);

  g_clear_handle_id (&self->settle_id, g_source_remove);
  g_clear_object (&self->monitor);
  g_clear_pointer (&self->git_dir, g_free);

  G_OBJECT_CLASS (xd_git_head_watch_parent_class)->dispose (object);
}

static void
xd_git_head_watch_class_init (XdGitHeadWatchClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = xd_git_head_watch_dispose;

  signals[SIGNAL_CHANGED] =
    g_signal_new ("changed", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 0);
}

static void
xd_git_head_watch_init (XdGitHeadWatch *self)
{
}
