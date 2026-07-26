#include "worktree.h"

#include "app-paths.h"
#include "git-info.h"

#include <errno.h>
#include <gio/gio.h>

static gboolean
run_git (const char  *cwd,
         const char *const *argv,
         gboolean    quiet_failure,
         char      **stderr_text,
         GError    **error)
{
  g_autoptr (GSubprocessLauncher) launcher = g_subprocess_launcher_new (
    G_SUBPROCESS_FLAGS_STDOUT_SILENCE | G_SUBPROCESS_FLAGS_STDERR_PIPE);
  g_autoptr (GSubprocess) process = NULL;
  g_autofree char *stderr_output = NULL;

  g_subprocess_launcher_set_cwd (launcher, cwd);
  process = g_subprocess_launcher_spawnv (launcher, argv, error);
  if (process == NULL)
    return FALSE;

  if (!g_subprocess_communicate_utf8 (process, NULL, NULL, NULL,
                                      &stderr_output, error))
    return FALSE;

  if (!g_subprocess_get_successful (process))
    {
      if (quiet_failure)
        return FALSE;

      g_strstrip (stderr_output);
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_FAILED,
                   "%s", *stderr_output != '\0'
                     ? stderr_output : "git worktree add failed");
      return FALSE;
    }

  if (stderr_text != NULL)
    *stderr_text = g_steal_pointer (&stderr_output);

  return TRUE;
}

char *
xd_worktree_create (const char  *workdir,
                    const char  *chat_id,
                    GError     **error)
{
  g_autoptr (XdGitInfo) git = NULL;
  g_autofree char *parent = NULL;
  g_autofree char *target = NULL;
  g_autofree char *branch = NULL;
  const char *probe_argv[] = { "git", "rev-parse", "--is-inside-work-tree", NULL };
  const char *branch_argv[] = {
    "git", "show-ref", "--verify", "--quiet", NULL, NULL
  };
  const char *add_argv[] = {
    "git", "worktree", "add", "-b", NULL, NULL, "HEAD", NULL
  };
  const char *reuse_argv[] = {
    "git", "worktree", "add", NULL, NULL, NULL
  };
  gboolean branch_exists;

  g_return_val_if_fail (chat_id != NULL && *chat_id != '\0', NULL);

  git = xd_git_info_for_path (workdir);
  if (git == NULL)
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                   "New worktree needs a Git working directory.");
      return NULL;
    }

  parent = g_build_filename (xd_app_data_dir (), "worktrees", git->name, NULL);
  target = g_build_filename (parent, chat_id, NULL);
  branch = g_strdup_printf ("xd/%s", chat_id);

  if (g_file_test (target, G_FILE_TEST_IS_DIR) &&
      run_git (target, probe_argv, TRUE, NULL, NULL))
    return g_steal_pointer (&target);

  if (g_file_test (target, G_FILE_TEST_EXISTS))
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_EXISTS,
                   "Cannot create worktree: %s already exists.", target);
      return NULL;
    }

  if (g_mkdir_with_parents (parent, 0700) != 0)
    {
      g_set_error (error, G_IO_ERROR, g_io_error_from_errno (errno),
                   "Cannot create %s", parent);
      return NULL;
    }

  branch_argv[4] = branch;
  branch_exists = run_git (git->root, branch_argv, TRUE, NULL, NULL);

  if (branch_exists)
    {
      reuse_argv[3] = target;
      reuse_argv[4] = branch;
      if (!run_git (git->root, reuse_argv, FALSE, NULL, error))
        return NULL;
    }
  else
    {
      add_argv[4] = branch;
      add_argv[5] = target;
      if (!run_git (git->root, add_argv, FALSE, NULL, error))
        return NULL;
    }

  return g_steal_pointer (&target);
}
