#include "util/worktree.h"

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

static char *
read_command (const char        *cwd,
              const char *const *argv)
{
  g_autoptr (GSubprocessLauncher) launcher =
    g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                               G_SUBPROCESS_FLAGS_STDERR_PIPE);
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *stderr_text = NULL;
  char *stdout_text = NULL;

  g_subprocess_launcher_set_cwd (launcher, cwd);
  process = g_subprocess_launcher_spawnv (launcher, argv, &error);
  g_assert_no_error (error);
  g_assert_true (g_subprocess_communicate_utf8 (
    process, NULL, NULL, &stdout_text, &stderr_text, &error));
  g_assert_no_error (error);
  g_assert_true (g_subprocess_get_successful (process));
  g_strchomp (stdout_text);

  return stdout_text;
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
test_create_and_reuse (void)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *dir = g_dir_make_tmp ("xd-worktree-XXXXXX", &error);
  g_autofree char *repo = NULL;
  g_autofree char *file = NULL;
  g_autofree char *worktree = NULL;
  g_autofree char *worktree_name_dir = NULL;
  g_autofree char *worktree_repo_dir = NULL;
  g_autofree char *worktrees_dir = NULL;
  g_autofree char *checkout_name = NULL;
  g_autofree char *worktree_name = NULL;
  g_autofree char *worktree_repo_name = NULL;
  g_autofree char *worktrees_name = NULL;
  g_autofree char *again = NULL;
  g_autofree char *branch = NULL;
  g_autofree char *expected_branch = NULL;
  g_autofree char *second = NULL;
  g_autofree char *second_parent = NULL;
  g_autofree char *second_name = NULL;
  g_autoptr (GPtrArray) listed = NULL;
  const char *init[] = { "git", "init", "-q", "-b", "main", NULL };
  const char *add[] = { "git", "add", "hello.txt", NULL };
  const char *commit[] = {
    "git", "-c", "user.name=xd tests", "-c", "user.email=xd@example.com",
    "commit", "-q", "-m", "initial", NULL
  };
  const char *show_branch[] = { "git", "branch", "--show-current", NULL };

  g_assert_no_error (error);

  repo = g_build_filename (dir, "repo", NULL);
  file = g_build_filename (repo, "hello.txt", NULL);
  g_assert_cmpint (g_mkdir (repo, 0700), ==, 0);

  run (repo, init);
  g_assert_true (g_file_set_contents (file, "hello\n", -1, &error));
  g_assert_no_error (error);
  run (repo, add);
  run (repo, commit);

  worktree = xd_worktree_create (
    repo, "12345678-1234-1234-1234-123456789abc",
    "Fix parser crashes!", &error);
  g_assert_no_error (error);
  g_assert_nonnull (worktree);
  g_assert_true (g_file_test (worktree, G_FILE_TEST_IS_DIR));
  worktree_name_dir = g_path_get_dirname (worktree);
  worktree_repo_dir = g_path_get_dirname (worktree_name_dir);
  worktrees_dir = g_path_get_dirname (worktree_repo_dir);
  checkout_name = g_path_get_basename (worktree);
  worktree_name = g_path_get_basename (worktree_name_dir);
  worktree_repo_name = g_path_get_basename (worktree_repo_dir);
  worktrees_name = g_path_get_basename (worktrees_dir);
  g_assert_cmpstr (checkout_name, ==, "repo");
  g_assert_cmpstr (worktree_name, ==, "fix-parser-crashes");
  g_assert_cmpstr (worktree_repo_name, ==, "repo");
  g_assert_cmpstr (worktrees_name, ==, "worktrees");

  branch = read_command (worktree, show_branch);
  expected_branch = g_strdup_printf (
    "xd/fix-parser-crashes-%08x",
    g_str_hash ("12345678-1234-1234-1234-123456789abc"));
  g_assert_cmpstr (branch, ==, expected_branch);

  /* Retrying after creation must reuse it, not fail on its branch. */
  again = xd_worktree_create (
    repo, "12345678-1234-1234-1234-123456789abc",
    "Fix parser crashes!", &error);
  g_assert_no_error (error);
  g_assert_cmpstr (again, ==, worktree);

  listed = xd_worktree_list (repo, &error);
  g_assert_no_error (error);
  g_assert_cmpuint (listed->len, ==, 2);
  g_assert_true (((XdWorktreeInfo *) g_ptr_array_index (listed, 0))->main);
  g_assert_true (((XdWorktreeInfo *) g_ptr_array_index (listed, 0))->current);
  g_assert_false (((XdWorktreeInfo *) g_ptr_array_index (listed, 1))->current);
  g_assert_true (xd_worktree_path_equal (
    ((XdWorktreeInfo *) g_ptr_array_index (listed, 1))->path, worktree));
  g_assert_cmpstr (
    ((XdWorktreeInfo *) g_ptr_array_index (listed, 1))->branch,
    ==, expected_branch);

  /* Similar requests stay readable and receive a numeric directory suffix,
   * while their branches remain independently addressable. */
  second = xd_worktree_create (
    repo, "abcdefab-1234-1234-1234-123456789abc",
    "Fix parser crashes!", &error);
  g_assert_no_error (error);
  g_assert_nonnull (second);
  g_assert_cmpstr (second, !=, worktree);
  second_parent = g_path_get_dirname (second);
  second_name = g_path_get_basename (second_parent);
  g_assert_cmpstr (second_name, ==, "fix-parser-crashes-2");

  remove_tree (dir);
}

static void
test_requires_a_repository (void)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *dir = g_dir_make_tmp ("xd-not-repo-XXXXXX", &error);
  g_autofree char *worktree = NULL;

  g_assert_no_error (error);
  worktree = xd_worktree_create (dir, "chat", "Fix tests", &error);
  g_assert_null (worktree);
  g_assert_error (error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED);
  g_clear_error (&error);

  g_assert_null (xd_worktree_list (dir, &error));
  g_assert_error (error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED);

  remove_tree (dir);
}

static void
test_registered_path_stays_in_repository (void)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *dir = g_dir_make_tmp ("xd-worktree-path-XXXXXX", &error);
  g_autofree char *repo = g_build_filename (dir, "repo", NULL);
  g_autofree char *other = g_build_filename (dir, "other", NULL);
  g_autofree char *linked = g_build_filename (dir, "linked", NULL);
  g_autofree char *resolved = NULL;
  const char *init[] = { "git", "init", "-q", NULL };
  const char *identity[] = {
    "git", "-c", "user.name=xd", "-c", "user.email=xd@example.com",
    "commit", "--allow-empty", "-qm", "initial", NULL
  };
  const char *add[] = {
    "git", "worktree", "add", "-q", "-b", "linked", NULL, "HEAD", NULL
  };

  g_assert_no_error (error);
  g_assert_cmpint (g_mkdir (repo, 0700), ==, 0);
  g_assert_cmpint (g_mkdir (other, 0700), ==, 0);
  run (repo, init);
  run (repo, identity);
  add[6] = linked;
  run (repo, add);

  resolved = xd_worktree_registered_path (repo, linked);
  g_assert_nonnull (resolved);
  g_assert_true (xd_worktree_path_equal (resolved, linked));

  g_clear_pointer (&resolved, g_free);
  resolved = xd_worktree_registered_path (repo, other);
  g_assert_null (resolved);

  remove_tree (dir);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/worktree/create-and-reuse", test_create_and_reuse);
  g_test_add_func ("/worktree/requires-repository", test_requires_a_repository);
  g_test_add_func ("/worktree/registered-path",
                   test_registered_path_stays_in_repository);

  return g_test_run ();
}
