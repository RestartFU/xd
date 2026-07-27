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
test_capture_only_change_since_previous_event (void)
{
  g_autoptr (GError) error = NULL;
#ifdef G_OS_WIN32
  g_autofree char *cwd = g_get_current_dir ();
  g_autofree char *dir =
    g_build_filename (cwd, "xd-git-diff-XXXXXX", NULL);
#else
  g_autofree char *dir = g_dir_make_tmp ("xd-git-diff-XXXXXX", &error);
#endif
  g_autofree char *tracked = NULL;
  g_autofree char *preexisting = NULL;
  g_autofree char *created = NULL;
  g_autofree char *first_message = NULL;
  g_autofree char *second_message = NULL;
  g_autoptr (XdGitDiffTracker) tracker = NULL;
  const char *first_patch;
  const char *second_patch;
  const char *init[] = { "git", "init", "-q", NULL };
  const char *add[] = { "git", "add", "tracked.txt", NULL };
  const char *commit[] = {
    "git", "-c", "user.name=xd tests", "-c", "user.email=xd@example.com",
    "commit", "-q", "-m", "initial", NULL
  };

#ifdef G_OS_WIN32
  /*
   * MSYS exposes /tmp, but native GLib subprocesses cannot reliably chdir to
   * that virtual path.  Meson's native working directory is writable and
   * gives the test the same path form the application receives.
   */
  g_assert_nonnull (g_mkdtemp (dir));
#else
  g_assert_no_error (error);
#endif
  tracked = g_build_filename (dir, "tracked.txt", NULL);
  preexisting = g_build_filename (dir, "preexisting.txt", NULL);
  created = g_build_filename (dir, "new file.txt", NULL);

  run (dir, init);
  g_assert_true (g_file_set_contents (tracked, "before\n", -1, &error));
  g_assert_no_error (error);
  run (dir, add);
  run (dir, commit);

  /* Dirty before the turn: neither change belongs to a later tool call. */
  g_assert_true (g_file_set_contents (tracked, "before turn\n", -1, &error));
  g_assert_no_error (error);
  g_assert_true (g_file_set_contents (preexisting, "already here\n", -1, &error));
  g_assert_no_error (error);

  tracker = xd_git_diff_tracker_new (dir);
  g_assert_nonnull (tracker);

  g_assert_true (g_file_set_contents (tracked, "first call\n", -1, &error));
  first_message = xd_git_diff_tracker_capture (tracker, "file_change");
  first_patch = xd_git_diff_from_tool (first_message);
  g_assert_nonnull (first_patch);
  g_assert_nonnull (strstr (first_patch, "-before turn"));
  g_assert_nonnull (strstr (first_patch, "+first call"));
  g_assert_null (strstr (first_patch, "-before\n"));
  g_assert_null (strstr (first_patch, "preexisting.txt"));

  g_assert_true (g_file_set_contents (created, "second call\n", -1, &error));
  second_message = xd_git_diff_tracker_capture (tracker, "file_change");
  second_patch = xd_git_diff_from_tool (second_message);
  g_assert_nonnull (second_patch);
  g_assert_nonnull (strstr (second_patch, "new file.txt"));
  g_assert_nonnull (strstr (second_patch, "+second call"));
  g_assert_null (strstr (second_patch, "tracked.txt"));
  g_assert_null (strstr (second_patch, "preexisting.txt"));

  {
    const char *index_unchanged[] = {
      "git", "diff", "--cached", "--quiet", NULL
    };

    run (dir, index_unchanged);
  }

  remove_tree (dir);
}

static void
test_ignores_other_tools (void)
{
  g_autofree char *message =
    xd_git_diff_tracker_capture (NULL, "$ git status");

  g_assert_cmpstr (message, ==, "$ git status");
  g_assert_false (xd_tool_is_file_change (message));
  g_assert_null (xd_git_diff_from_tool (message));
}

int
main (int   argc,
      char *argv[])
{
  g_setenv ("XD_GIT_DIFF_DEBUG", "1", TRUE);
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/git-diff/captures-only-current-call",
                   test_capture_only_change_since_previous_event);
  g_test_add_func ("/git-diff/ignores-other-tools",
                   test_ignores_other_tools);

  return g_test_run ();
}
