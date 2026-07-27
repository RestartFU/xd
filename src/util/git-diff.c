#include "git-diff.h"

#include <gio/gio.h>
#include <string.h>

/* A tool row must not be allowed to turn one generated file into an
 * unbounded transcript. Enough for a substantial source change. */
#define DIFF_LIMIT (256 * 1024)
#define FILE_CHANGE_PREFIX "file_change\n"

static char *
run_git (const char        *workdir,
         const char *const *argv,
         gboolean           accept_difference)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  char *output = NULL;

  if (workdir == NULL || *workdir == '\0')
    return NULL;

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                        G_SUBPROCESS_FLAGS_STDERR_SILENCE);
  g_subprocess_launcher_set_cwd (launcher, workdir);
  process = g_subprocess_launcher_spawnv (launcher, argv, &error);
  if (process == NULL ||
      !g_subprocess_communicate_utf8 (process, NULL, NULL, &output, NULL, &error))
    {
      g_debug ("cannot capture file diff: %s",
               error != NULL ? error->message : "git did not start");
      g_free (output);
      return NULL;
    }

  if (!g_subprocess_get_successful (process) &&
      !(accept_difference && g_subprocess_get_exit_status (process) == 1))
    {
      g_free (output);
      return NULL;
    }

  return output;
}

static void
append_limited (GString    *patch,
                const char *text)
{
  gsize available;
  gsize length;

  if (text == NULL || *text == '\0' || patch->len >= DIFF_LIMIT)
    return;

  available = DIFF_LIMIT - patch->len;
  length = MIN (strlen (text), available);
  g_string_append_len (patch, text, length);
}

/*
 * Adds files Git does not know about.
 *
 * A normal `git diff HEAD` intentionally omits them, although a newly written
 * file is exactly the kind of edit this feature needs to show. Porcelain -z
 * keeps spaces, quotes and newlines in paths unambiguous; each path is then a
 * separate argv element, never shell input.
 */
static void
append_untracked (GString    *patch,
                  const char *workdir)
{
  const char *status_argv[] = {
    "git", "status", "--porcelain", "-z", "--untracked-files=all", NULL
  };
  g_autofree char *status = run_git (workdir, status_argv, FALSE);
  const char *at;

  if (status == NULL)
    return;

  for (at = status; *at != '\0' && patch->len < DIFF_LIMIT; )
    {
      gsize length = strlen (at);

      if (length >= 4 && at[0] == '?' && at[1] == '?' && at[2] == ' ')
        {
          /* Git recognizes this sentinel itself on every platform. Passing
           * Windows' NUL device instead makes Git for Windows silently omit
           * the untracked file from the generated patch. */
          const char *empty = "/dev/null";
          const char *path = at + 3;
          const char *diff_argv[] = {
            "git", "--no-pager", "diff", "--no-index", "--", empty, path, NULL
          };
          g_autofree char *diff = run_git (workdir, diff_argv, TRUE);

          append_limited (patch, diff);
        }

      at += length + 1;
    }
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

  /* A captured Git patch always starts this way. This boundary keeps a future
   * multi-line summary from accidentally being rendered as source changes. */
  return g_str_has_prefix (patch, "diff --git ") ? patch : NULL;
}

char *
xd_git_diff_capture_tool (const char *message,
                          const char *workdir)
{
  const char *head_argv[] = {
    "git", "--no-pager", "diff", "--no-ext-diff", "--no-color", "HEAD", "--",
    NULL
  };
  const char *staged_argv[] = {
    "git", "--no-pager", "diff", "--no-ext-diff", "--no-color", "--cached",
    NULL
  };
  g_autoptr (GString) patch = NULL;
  g_autofree char *tracked = NULL;

  if (!xd_tool_is_file_change (message) ||
      xd_git_diff_from_tool (message) != NULL)
    return g_strdup (message);

  patch = g_string_new (NULL);

  /* HEAD gives one coherent view of staged and unstaged edits. An unborn
   * repository has no HEAD; --cached is its equivalent for already added
   * files, and the status pass below supplies everything not yet added. */
  tracked = run_git (workdir, head_argv, FALSE);
  if (tracked == NULL)
    tracked = run_git (workdir, staged_argv, FALSE);
  append_limited (patch, tracked);
  append_untracked (patch, workdir);

  if (patch->len == 0)
    return g_strdup (message);

  if (patch->len >= DIFF_LIMIT)
    g_string_append (patch, "\n… diff truncated …\n");

  g_strchomp (patch->str);
  patch->len = strlen (patch->str);

  return g_strconcat (FILE_CHANGE_PREFIX, patch->str, NULL);
}
