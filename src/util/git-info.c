#include "git-info.h"

#include <gio/gio.h>
#include <string.h>

void
hy_git_info_free (HyGitInfo *self)
{
  if (self == NULL)
    return;

  g_free (self->root);
  g_free (self->name);
  g_free (self->branch);
  g_free (self->remote_url);
  g_free (self);
}

/* Walks up looking for .git, which may be a directory or, inside a linked
 * worktree, a file pointing at one. */
static char *
find_repository_root (const char *path)
{
  g_autofree char *current = g_strdup (path);

  while (current != NULL && *current != '\0')
    {
      g_autofree char *dot_git = g_build_filename (current, ".git", NULL);
      char *parent;

      if (g_file_test (dot_git, G_FILE_TEST_EXISTS))
        return g_steal_pointer (&current);

      parent = g_path_get_dirname (current);
      if (g_strcmp0 (parent, current) == 0)
        {
          g_free (parent);
          return NULL;
        }

      g_free (current);
      current = parent;
    }

  return NULL;
}

/* Resolves .git to the directory actually holding HEAD and config. For a
 * linked worktree that is <main>/.git/worktrees/<name>. */
static char *
resolve_git_dir (const char *root,
                 gboolean   *linked)
{
  g_autofree char *dot_git = g_build_filename (root, ".git", NULL);
  g_autofree char *contents = NULL;
  const char *pointer;

  *linked = FALSE;

  if (g_file_test (dot_git, G_FILE_TEST_IS_DIR))
    return g_steal_pointer (&dot_git);

  if (!g_file_get_contents (dot_git, &contents, NULL, NULL))
    return NULL;

  pointer = contents;
  if (!g_str_has_prefix (pointer, "gitdir:"))
    return NULL;

  pointer += strlen ("gitdir:");
  while (*pointer == ' ')
    pointer++;

  *linked = TRUE;

  {
    g_autofree char *target = g_strdup (pointer);

    g_strchomp (target);

    if (g_path_is_absolute (target))
      return g_steal_pointer (&target);

    return g_build_filename (root, target, NULL);
  }
}

static void
read_head (HyGitInfo  *self,
           const char *git_dir)
{
  g_autofree char *head_path = g_build_filename (git_dir, "HEAD", NULL);
  g_autofree char *contents = NULL;

  if (!g_file_get_contents (head_path, &contents, NULL, NULL))
    return;

  g_strchomp (contents);

  if (g_str_has_prefix (contents, "ref: refs/heads/"))
    {
      self->branch = g_strdup (contents + strlen ("ref: refs/heads/"));
      return;
    }

  /* Detached: HEAD holds the commit itself. */
  self->detached = TRUE;
  self->branch = g_strndup (contents, 8);
}

/*
 * Pulls origin's URL out of the config.
 *
 * Parsed by hand rather than with GKeyFile: git indents its keys with tabs and
 * writes sections as [remote "origin"], neither of which GKeyFile handles.
 */
static void
read_origin_url (HyGitInfo  *self,
                 const char *git_dir)
{
  g_autofree char *config_path = g_build_filename (git_dir, "config", NULL);
  g_autofree char *contents = NULL;
  g_auto (GStrv) lines = NULL;
  gboolean in_origin = FALSE;

  if (!g_file_get_contents (config_path, &contents, NULL, NULL))
    return;

  lines = g_strsplit (contents, "\n", -1);

  for (gsize i = 0; lines[i] != NULL; i++)
    {
      char *line = g_strstrip (lines[i]);

      if (*line == '[')
        {
          in_origin = g_str_has_prefix (line, "[remote \"origin\"]");
          continue;
        }

      if (!in_origin || !g_str_has_prefix (line, "url"))
        continue;

      {
        char *equals = strchr (line, '=');

        if (equals != NULL)
          {
            self->remote_url = g_strdup (g_strstrip (equals + 1));
            return;
          }
      }
    }
}

HyGitInfo *
hy_git_info_for_path (const char *path)
{
  g_autofree char *root = NULL;
  g_autofree char *git_dir = NULL;
  HyGitInfo *self;
  gboolean linked = FALSE;

  if (path == NULL)
    return NULL;

  root = find_repository_root (path);
  if (root == NULL)
    return NULL;

  git_dir = resolve_git_dir (root, &linked);
  if (git_dir == NULL)
    return NULL;

  self = g_new0 (HyGitInfo, 1);
  self->root = g_strdup (root);
  self->name = g_path_get_basename (root);
  self->linked_worktree = linked;

  read_head (self, git_dir);

  /* A linked worktree keeps its own HEAD but shares the main repository's
   * config, which is one level up from .git/worktrees/<name>. */
  if (linked)
    {
      g_autofree char *worktrees = g_path_get_dirname (git_dir);
      g_autofree char *common = g_path_get_dirname (worktrees);

      read_origin_url (self, common);
    }
  else
    {
      read_origin_url (self, git_dir);
    }

  return self;
}
