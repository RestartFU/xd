#include "git-diff.h"

#include <gio/gio.h>
#include <glib/gstdio.h>
#include <string.h>

/* A tool row must not turn one generated file into an unbounded transcript. */
#define DIFF_LIMIT (256 * 1024)
#define FILE_CHANGE_PREFIX "file_change\n"

struct _XdGitDiffTracker
{
  char *root;
  char *previous_tree;
};

static char *
native_git_path (char *path)
{
#ifdef G_OS_WIN32
  if (path != NULL &&
      path[0] == '/' &&
      g_ascii_isalpha (path[1]) &&
      path[2] == '/')
    {
      char drive[3] = { g_ascii_toupper (path[1]), ':', '\0' };
      char *native = g_strconcat (drive, path + 2, NULL);

      g_free (path);
      return native;
    }
#endif

  return path;
}

static char *
run_git (const char        *workdir,
         const char        *index_path,
         const char *const *argv,
         gboolean           accept_difference)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *stderr_text = NULL;
  char *output = NULL;

  if (workdir == NULL || *workdir == '\0')
    return NULL;

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                        G_SUBPROCESS_FLAGS_STDERR_PIPE);
  g_subprocess_launcher_set_cwd (launcher, workdir);
  if (index_path != NULL)
    {
      g_autofree char *git_index_path = g_strdup (index_path);

#ifdef G_OS_WIN32
      /*
       * MSYS2 Git accepts drive-letter paths, but its environment parser
       * expects forward slashes.  GLib's temporary-file APIs return native
       * backslashes.
       */
      g_strdelimit (git_index_path, "\\", '/');
#endif
      g_subprocess_launcher_setenv (launcher, "GIT_INDEX_FILE",
                                    git_index_path, TRUE);
    }

  process =
    g_subprocess_launcher_spawnv (launcher, argv, &error);
  if (process == NULL ||
      !g_subprocess_communicate_utf8 (
        process, NULL, NULL, &output, &stderr_text, &error))
    {
      if (g_getenv ("XD_GIT_DIFF_DEBUG") != NULL)
        g_printerr ("xd: cannot run git %s: %s\n",
                    argv[1] != NULL ? argv[1] : "",
                    error != NULL ? error->message : "git did not start");
      g_debug ("cannot run git %s: %s",
               argv[1] != NULL ? argv[1] : "",
               error != NULL ? error->message : "git did not start");
      g_free (output);
      return NULL;
    }

  if (!g_subprocess_get_successful (process) &&
      !(accept_difference && g_subprocess_get_exit_status (process) == 1))
    {
      if (g_getenv ("XD_GIT_DIFF_DEBUG") != NULL)
        g_printerr ("xd: git %s failed (%d): %s",
                    argv[1] != NULL ? argv[1] : "",
                    g_subprocess_get_exit_status (process),
                    stderr_text != NULL ? stderr_text : "no error output\n");
      g_debug ("git %s failed (%d): %s",
               argv[1] != NULL ? argv[1] : "",
               g_subprocess_get_exit_status (process),
               stderr_text != NULL ? stderr_text : "no error output");
      g_free (output);
      return NULL;
    }

  /*
   * Empty stdout may be reported as NULL on Windows.  Successful plumbing
   * commands such as `git add` still need a non-NULL success sentinel.
   */
  return output != NULL ? output : g_strdup ("");
}

static char *
repository_root (const char *workdir)
{
  const char *argv[] = {
    "git", "rev-parse", "--show-toplevel", NULL
  };
  g_autofree char *root = run_git (workdir, NULL, argv, FALSE);

  if (root == NULL)
    return NULL;

  g_strchomp (root);
  return *root != '\0'
    ? native_git_path (g_steal_pointer (&root)) : NULL;
}

static char *
user_index_path (const char *root)
{
  const char *argv[] = { "git", "rev-parse", "--git-path", "index", NULL };
  g_autofree char *reported = run_git (root, NULL, argv, FALSE);

  if (reported == NULL)
    return NULL;

  g_strchomp (reported);
  reported = native_git_path (g_steal_pointer (&reported));
  return g_path_is_absolute (reported)
    ? g_steal_pointer (&reported) : g_build_filename (root, reported, NULL);
}

static gboolean
seed_from_user_index (const char *index_path,
                      const char *temporary)
{
  g_autofree char *contents = NULL;
  gsize length = 0;

  return g_file_get_contents (index_path, &contents, &length, NULL) &&
         g_file_set_contents (temporary, contents, length, NULL);
}

/*
 * Materializes the worktree as a Git tree through a disposable index.
 *
 * HEAD seeds modes and tracked paths when it exists. `git add -A` then makes
 * that private index match disk, including ordinary untracked files, without
 * touching staging. write-tree stores only content-addressed objects.
 */
static char *
snapshot_tree (const char *root)
{
  const char *read_argv[] = { "git", "read-tree", "HEAD", NULL };
  const char *add_argv[] = { "git", "add", "-A", "--", ".", NULL };
  const char *write_argv[] = { "git", "write-tree", NULL };
  g_autofree char *user_index = NULL;
  g_autofree char *index_dir = NULL;
  g_autofree char *index_path = NULL;
  g_autofree char *ignored = NULL;
  g_autofree char *tree = NULL;
  gboolean seeded;
  int descriptor;

  /*
   * Keep the alternate index beside the real index.  Apart from avoiding a
   * cross-filesystem lock/rename, this gives Git a repository-native path on
   * Windows rather than a GLib system-temp path.
   */
  user_index = user_index_path (root);
  if (user_index == NULL)
    return NULL;

  index_dir = g_path_get_dirname (user_index);
  index_path = g_build_filename (index_dir, "xd-diff-index-XXXXXX", NULL);
  descriptor = g_mkstemp (index_path);
  if (descriptor < 0)
    {
      if (g_getenv ("XD_GIT_DIFF_DEBUG") != NULL)
        g_printerr ("xd: cannot make temporary Git index at %s\n", index_path);
      g_debug ("cannot make temporary Git index at %s", index_path);
      return NULL;
    }

  g_close (descriptor, NULL);
  /*
   * Cloning the user's index gives the disposable copy its stat cache, so Git
   * hashes changed files instead of every tracked file on every agent call.
   * The copy is immediately made to match disk and is never written back.
   */
  seeded = seed_from_user_index (user_index, index_path);
  if (!seeded)
    {
      /* Git expects a missing index or a valid one, not mkstemp's empty file.
       * read-tree failing here simply means an unborn HEAD. */
      g_remove (index_path);
      ignored = run_git (root, index_path, read_argv, FALSE);
      g_clear_pointer (&ignored, g_free);
    }

  ignored = run_git (root, index_path, add_argv, FALSE);
  if (ignored == NULL && seeded)
    {
      /* A corrupt or exotic index extension must not disable inline diffs.
       * Fall back to a clean HEAD index; this path is rare and slower. */
      g_remove (index_path);
      ignored = run_git (root, index_path, read_argv, FALSE);
      g_clear_pointer (&ignored, g_free);
      ignored = run_git (root, index_path, add_argv, FALSE);
    }
  if (ignored == NULL)
    {
      g_remove (index_path);
      return NULL;
    }

  tree = run_git (root, index_path, write_argv, FALSE);
  g_remove (index_path);
  if (tree == NULL)
    return NULL;

  g_strchomp (tree);
  return *tree != '\0' ? g_steal_pointer (&tree) : NULL;
}

gboolean
xd_tool_is_file_change (const char *message)
{
  return g_strcmp0 (message, "file_change") == 0 ||
         (message != NULL &&
          (g_str_has_prefix (message, "file_change  ") ||
           g_str_has_prefix (message, FILE_CHANGE_PREFIX)));
}

const char *
xd_git_diff_from_tool (const char *message)
{
  const char *patch;

  if (message == NULL || !g_str_has_prefix (message, FILE_CHANGE_PREFIX))
    return NULL;

  patch = message + strlen (FILE_CHANGE_PREFIX);
  return g_str_has_prefix (patch, "diff --git ") ? patch : NULL;
}

XdGitDiffTracker *
xd_git_diff_tracker_new (const char *workdir)
{
  XdGitDiffTracker *self = g_new0 (XdGitDiffTracker, 1);

  self->root = repository_root (workdir);
  if (self->root != NULL)
    self->previous_tree = snapshot_tree (self->root);

  if (self->previous_tree == NULL)
    {
      xd_git_diff_tracker_free (self);
      return NULL;
    }

  return self;
}

void
xd_git_diff_tracker_free (XdGitDiffTracker *self)
{
  if (self == NULL)
    return;

  g_free (self->root);
  g_free (self->previous_tree);
  g_free (self);
}

char *
xd_git_diff_tracker_capture (XdGitDiffTracker *self,
                             const char       *message)
{
  g_autofree char *current_tree = NULL;
  g_autofree char *patch = NULL;
  const char *argv[] = {
    "git", "--no-pager", "diff", "--no-ext-diff", "--no-color",
    NULL, NULL, "--", NULL
  };

  if (!xd_tool_is_file_change (message) ||
      xd_git_diff_from_tool (message) != NULL ||
      self == NULL)
    return g_strdup (message);

  current_tree = snapshot_tree (self->root);
  if (current_tree == NULL)
    return g_strdup (message);

  argv[5] = self->previous_tree;
  argv[6] = current_tree;
  patch = run_git (self->root, NULL, argv, FALSE);

  g_free (self->previous_tree);
  self->previous_tree = g_steal_pointer (&current_tree);

  if (patch == NULL || *patch == '\0')
    return g_strdup (message);

  if (strlen (patch) > DIFF_LIMIT)
    {
      patch[DIFF_LIMIT] = '\0';
      patch = g_realloc (patch, DIFF_LIMIT + strlen ("\n… diff truncated …\n") + 1);
      strcat (patch, "\n… diff truncated …\n");
    }

  g_strchomp (patch);
  return g_strconcat (FILE_CHANGE_PREFIX, patch, NULL);
}
