#include "worktree.h"

#include "git-info.h"

#include <errno.h>
#include <gio/gio.h>
#include <glib/gstdio.h>
#include <string.h>

static char *
normalize_worktree_path (const char *path)
{
  g_autofree char *native = g_strdup (path);

#ifdef G_OS_WIN32
  if (native[0] == '/' &&
      g_ascii_isalpha (native[1]) &&
      native[2] == '/')
    {
      char drive[3] = { g_ascii_toupper (native[1]), ':', '\0' };
      char *converted = g_strconcat (drive, native + 2, NULL);

      g_free (g_steal_pointer (&native));
      native = converted;
    }
#endif

  return g_canonicalize_filename (native, NULL);
}

static char *
current_worktree_path (const char *cwd)
{
  g_autoptr (GSubprocessLauncher) launcher =
    g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                               G_SUBPROCESS_FLAGS_STDERR_SILENCE);
  g_autoptr (GSubprocess) process = NULL;
  g_autofree char *stdout_text = NULL;
  const char *argv[] = {
    "git", "rev-parse", "--show-toplevel", NULL
  };

  g_subprocess_launcher_set_cwd (launcher, cwd);
  process = g_subprocess_launcher_spawnv (launcher, argv, NULL);
  if (process == NULL ||
      !g_subprocess_communicate_utf8 (
        process, NULL, NULL, &stdout_text, NULL, NULL) ||
      !g_subprocess_get_successful (process))
    return NULL;

  g_strchomp (stdout_text);
  return normalize_worktree_path (stdout_text);
}

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

void
xd_worktree_info_free (XdWorktreeInfo *self)
{
  if (self == NULL)
    return;

  g_free (self->path);
  g_free (self->branch);
  g_free (self);
}

gboolean
xd_worktree_path_equal (const char *a,
                        const char *b)
{
  g_autoptr (GFile) a_file = NULL;
  g_autoptr (GFile) b_file = NULL;
  g_autoptr (GFileInfo) a_info = NULL;
  g_autoptr (GFileInfo) b_info = NULL;
  GStatBuf a_stat;
  GStatBuf b_stat;

  if (a == NULL || b == NULL)
    return a == b;

  a_file = g_file_new_for_path (a);
  b_file = g_file_new_for_path (b);
  a_info = g_file_query_info (
    a_file,
    G_FILE_ATTRIBUTE_ID_FILE "," G_FILE_ATTRIBUTE_ID_FILESYSTEM,
    G_FILE_QUERY_INFO_NONE, NULL, NULL);
  b_info = g_file_query_info (
    b_file,
    G_FILE_ATTRIBUTE_ID_FILE "," G_FILE_ATTRIBUTE_ID_FILESYSTEM,
    G_FILE_QUERY_INFO_NONE, NULL, NULL);

  if (a_info != NULL && b_info != NULL)
    {
      const char *a_id =
        g_file_info_get_attribute_string (a_info, G_FILE_ATTRIBUTE_ID_FILE);
      const char *b_id =
        g_file_info_get_attribute_string (b_info, G_FILE_ATTRIBUTE_ID_FILE);
      const char *a_filesystem = g_file_info_get_attribute_string (
        a_info, G_FILE_ATTRIBUTE_ID_FILESYSTEM);
      const char *b_filesystem = g_file_info_get_attribute_string (
        b_info, G_FILE_ATTRIBUTE_ID_FILESYSTEM);

      if (a_id != NULL && b_id != NULL &&
          a_filesystem != NULL && b_filesystem != NULL)
        return g_str_equal (a_id, b_id) &&
               g_str_equal (a_filesystem, b_filesystem);
    }

  if (g_stat (a, &a_stat) == 0 && g_stat (b, &b_stat) == 0)
    return a_stat.st_dev == b_stat.st_dev &&
           a_stat.st_ino == b_stat.st_ino;

  {
    g_autofree char *canonical_a = g_canonicalize_filename (a, NULL);
    g_autofree char *canonical_b = g_canonicalize_filename (b, NULL);

    return g_strcmp0 (canonical_a, canonical_b) == 0;
  }
}

static XdWorktreeInfo *
finish_worktree (XdWorktreeInfo *item,
                 guint           position,
                 const char     *current_path)
{
  if (item == NULL || item->path == NULL || item->prunable)
    {
      xd_worktree_info_free (item);
      return NULL;
    }

  item->main = position == 0;
  item->current = xd_worktree_path_equal (item->path, current_path);

  return item;
}

GPtrArray *
xd_worktree_list (const char  *workdir,
                  GError     **error)
{
  g_autoptr (XdGitInfo) git = NULL;
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GBytes) stdout_bytes = NULL;
  g_autoptr (GBytes) stderr_bytes = NULL;
  g_autoptr (GPtrArray) result = NULL;
  g_autofree char *current_path = NULL;
  XdWorktreeInfo *item = NULL;
  const char *argv[] = {
    "git", "worktree", "list", "--porcelain", "-z", NULL
  };
  const guint8 *data;
  gsize length;
  gsize offset = 0;

  git = xd_git_info_for_path (workdir);
  if (git == NULL)
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                   "Worktree selection needs a Git working directory.");
      return NULL;
    }

  launcher = g_subprocess_launcher_new (
    G_SUBPROCESS_FLAGS_STDOUT_PIPE | G_SUBPROCESS_FLAGS_STDERR_PIPE);
  g_subprocess_launcher_set_cwd (launcher, git->root);
  process = g_subprocess_launcher_spawnv (launcher, argv, error);
  if (process == NULL)
    return NULL;

  if (!g_subprocess_communicate (process, NULL, NULL, &stdout_bytes,
                                 &stderr_bytes, error))
    return NULL;

  if (!g_subprocess_get_successful (process))
    {
      gsize stderr_length = 0;
      const char *stderr_data = stderr_bytes != NULL
        ? g_bytes_get_data (stderr_bytes, &stderr_length) : NULL;
      g_autofree char *message =
        g_strndup (stderr_data != NULL ? stderr_data : "", stderr_length);

      g_strstrip (message);
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_FAILED, "%s",
                   *message != '\0' ? message : "git worktree list failed");
      return NULL;
    }

  result = g_ptr_array_new_with_free_func (
    (GDestroyNotify) xd_worktree_info_free);
  current_path = current_worktree_path (git->root);
  data = g_bytes_get_data (stdout_bytes, &length);

  while (offset < length)
    {
      const guint8 *end = memchr (data + offset, '\0', length - offset);
      gsize token_length =
        end != NULL ? (gsize) (end - (data + offset)) : length - offset;
      g_autofree char *token =
        g_strndup ((const char *) data + offset, token_length);

      offset += token_length + (end != NULL ? 1 : 0);

      if (token_length == 0)
        {
          XdWorktreeInfo *finished =
            finish_worktree (
              item, result->len,
              current_path != NULL ? current_path : git->root);

          if (finished != NULL)
            g_ptr_array_add (result, finished);
          item = NULL;
          continue;
        }

      if (item == NULL)
        item = g_new0 (XdWorktreeInfo, 1);

      if (g_str_has_prefix (token, "worktree "))
        item->path =
          normalize_worktree_path (token + strlen ("worktree "));
      else if (g_str_has_prefix (token, "branch refs/heads/"))
        {
          g_free (item->branch);
          item->branch = g_strdup (token + strlen ("branch refs/heads/"));
        }
      else if (g_str_has_prefix (token, "HEAD ") && item->branch == NULL)
        item->branch = g_strndup (token + strlen ("HEAD "), 8);
      else if (g_strcmp0 (token, "detached") == 0)
        item->detached = TRUE;
      else if (g_str_has_prefix (token, "prunable"))
        item->prunable = TRUE;
    }

  if (item != NULL)
    {
      XdWorktreeInfo *finished =
        finish_worktree (
          item, result->len,
          current_path != NULL ? current_path : git->root);

      if (finished != NULL)
        g_ptr_array_add (result, finished);
    }

  if (result->len == 0)
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_FAILED,
                   "Git returned no worktrees.");
      return NULL;
    }

  return g_steal_pointer (&result);
}

char *
xd_worktree_create (const char  *workdir,
                    const char  *chat_id,
                    GError     **error)
{
  g_autoptr (XdGitInfo) git = NULL;
  g_autoptr (GPtrArray) worktrees = NULL;
  XdWorktreeInfo *main;
  g_autofree char *repository_parent = NULL;
  g_autofree char *repository_name = NULL;
  g_autofree char *parent = NULL;
  g_autofree char *target = NULL;
  g_autofree char *branch = NULL;
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

  worktrees = xd_worktree_list (workdir, error);
  if (worktrees == NULL)
    return NULL;

  branch = g_strdup_printf ("xd/%s", chat_id);
  for (guint i = 0; i < worktrees->len; i++)
    {
      XdWorktreeInfo *item = g_ptr_array_index (worktrees, i);

      /* Reuse retries and worktrees made by older xd versions in their old
       * app-data location. Moving a checked-out branch would make Git reject
       * the retry even though the desired checkout is already ready. */
      if (!item->detached && g_strcmp0 (item->branch, branch) == 0)
        return g_strdup (item->path);
    }

  main = g_ptr_array_index (worktrees, 0);
  repository_parent = g_path_get_dirname (main->path);
  repository_name = g_path_get_basename (main->path);
  parent = g_build_filename (repository_parent, "worktrees",
                             repository_name, NULL);
  target = g_build_filename (parent, chat_id, NULL);

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
