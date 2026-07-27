#include "util/git-head-watch.h"
#include "util/git-info.h"

#include <glib/gstdio.h>

typedef struct
{
  GMainLoop *loop;
  guint changes;
} Observed;

static void
remove_tree (const char *path)
{
  g_autoptr (GDir) dir = g_dir_open (path, 0, NULL);
  const char *name;

  while (dir != NULL && (name = g_dir_read_name (dir)) != NULL)
    {
      g_autofree char *child = g_build_filename (path, name, NULL);

      if (g_file_test (child, G_FILE_TEST_IS_DIR) &&
          !g_file_test (child, G_FILE_TEST_IS_SYMLINK))
        remove_tree (child);
      else
        g_remove (child);
    }

  g_rmdir (path);
}

static void
on_changed (XdGitHeadWatch *watch,
            gpointer        user_data)
{
  Observed *observed = user_data;

  observed->changes++;
  g_main_loop_quit (observed->loop);
}

static gboolean
on_timeout (gpointer user_data)
{
  Observed *observed = user_data;

  g_main_loop_quit (observed->loop);
  return G_SOURCE_REMOVE;
}

static void
test_atomic_head_rewrite_is_seen (void)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *dir = g_dir_make_tmp ("xd-git-head-watch-XXXXXX", &error);
  g_autofree char *git_dir = NULL;
  g_autofree char *head = NULL;
  g_autofree char *lock = NULL;
  g_autoptr (XdGitHeadWatch) watch = xd_git_head_watch_new ();
  g_autoptr (XdGitInfo) info = NULL;
  Observed observed = { .loop = g_main_loop_new (NULL, FALSE) };
  guint timeout;

  g_assert_no_error (error);
  git_dir = g_build_filename (dir, ".git", NULL);
  head = g_build_filename (git_dir, "HEAD", NULL);
  lock = g_build_filename (git_dir, "HEAD.lock", NULL);

  g_assert_cmpint (g_mkdir (git_dir, 0700), ==, 0);
  g_assert_true (g_file_set_contents (
    head, "ref: refs/heads/master\n", -1, &error));
  g_assert_no_error (error);

  g_signal_connect (watch, "changed", G_CALLBACK (on_changed), &observed);
  xd_git_head_watch_set_workdir (watch, dir);

  /* The lock-and-rename sequence used by checkout/switch. */
  g_assert_true (g_file_set_contents (
    lock, "ref: refs/heads/feature\n", -1, &error));
  g_assert_no_error (error);
  g_assert_cmpint (g_rename (lock, head), ==, 0);

  timeout = g_timeout_add_seconds (5, on_timeout, &observed);
  g_main_loop_run (observed.loop);
  if (observed.changes > 0)
    g_source_remove (timeout);

  g_assert_cmpuint (observed.changes, ==, 1);
  info = xd_git_info_for_path (dir);
  g_assert_nonnull (info);
  g_assert_cmpstr (info->branch, ==, "feature");

  g_object_unref (watch);
  watch = NULL;
  g_main_loop_unref (observed.loop);
  remove_tree (dir);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/git-head-watch/atomic-rewrite",
                   test_atomic_head_rewrite_is_seen);

  return g_test_run ();
}
