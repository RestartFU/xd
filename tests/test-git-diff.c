#include "util/git-diff.h"

#include <gio/gio.h>
#include <glib/gstdio.h>

static void
run (const char        *cwd,
     const char *const *argv)
{
  g_autoptr (GSubprocessLauncher) launcher =
    g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_SILENCE |
                               G_SUBPROCESS_FLAGS_STDERR_PIPE);
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *stderr_text = NULL;

  g_subprocess_launcher_set_cwd (launcher, cwd);
  process = g_subprocess_launcher_spawnv (launcher, argv, &error);
  g_assert_no_error (error);
  g_assert_true (g_subprocess_communicate_utf8 (
    process, NULL, NULL, NULL, &stderr_text, &error));
  g_assert_no_error (error);
  if (!g_subprocess_get_successful (process))
    g_test_message ("%s", stderr_text);
  g_assert_true (g_subprocess_get_successful (process));
}

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
test_capture_file_change (void)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *dir = g_dir_make_tmp ("xd-git-diff-XXXXXX", &error);
  g_autofree char *tracked = NULL;
  g_autofree char *untracked = NULL;
  g_autofree char *message = NULL;
  const char *patch;
  const char *init[] = { "git", "init", "-q", NULL };
  const char *add[] = { "git", "add", "tracked.txt", NULL };
  const char *commit[] = {
    "git", "-c", "user.name=xd tests", "-c", "user.email=xd@example.com",
    "commit", "-q", "-m", "initial", NULL
  };

  g_assert_no_error (error);
  tracked = g_build_filename (dir, "tracked.txt", NULL);
  untracked = g_build_filename (dir, "new file.txt", NULL);

  run (dir, init);
  g_assert_true (g_file_set_contents (tracked, "before\n", -1, &error));
  g_assert_no_error (error);
  run (dir, add);
  run (dir, commit);

  g_assert_true (g_file_set_contents (tracked, "after\n", -1, &error));
  g_assert_no_error (error);
  g_assert_true (g_file_set_contents (untracked, "new\n", -1, &error));
  g_assert_no_error (error);

  message = xd_git_diff_capture_tool ("file_change", dir);
  patch = xd_git_diff_from_tool (message);

  g_assert_nonnull (patch);
  g_assert_nonnull (strstr (patch, "-before"));
  g_assert_nonnull (strstr (patch, "+after"));
  g_assert_nonnull (strstr (patch, "new file.txt"));
  g_assert_nonnull (strstr (patch, "+new"));

  remove_tree (dir);
}

static void
test_ignores_other_tools (void)
{
  g_autofree char *message =
    xd_git_diff_capture_tool ("$ git status", "/does/not/matter");

  g_assert_cmpstr (message, ==, "$ git status");
  g_assert_false (xd_tool_is_file_change (message));
  g_assert_null (xd_git_diff_from_tool (message));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/git-diff/captures-file-change",
                   test_capture_file_change);
  g_test_add_func ("/git-diff/ignores-other-tools",
                   test_ignores_other_tools);

  return g_test_run ();
}
