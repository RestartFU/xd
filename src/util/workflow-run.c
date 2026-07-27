#include "workflow-run.h"

#include <string.h>

#include "git-info.h"

#define WORKFLOW_RUN_PREFIX "workflow_run\n"

static gboolean
component_is_safe (const char *component)
{
  if (component == NULL || *component == '\0')
    return FALSE;

  for (const char *at = component; *at != '\0'; at++)
    if (!g_ascii_isalnum (*at) && *at != '-' && *at != '_' && *at != '.')
      return FALSE;

  return TRUE;
}

/* Returns the OWNER/REPOSITORY part of a GitHub URL or gh -R argument. */
static char *
repository_from_spec (const char *spec)
{
  g_autofree char *copy = NULL;
  char *path;
  char *slash;
  char *owner;
  char *repository;

  if (spec == NULL || *spec == '\0')
    return NULL;

  copy = g_strdup (spec);
  g_strstrip (copy);

  if (g_str_has_prefix (copy, "git@github.com:"))
    path = copy + strlen ("git@github.com:");
  else if (g_str_has_prefix (copy, "ssh://git@github.com/"))
    path = copy + strlen ("ssh://git@github.com/");
  else if (g_str_has_prefix (copy, "https://github.com/"))
    path = copy + strlen ("https://github.com/");
  else if (g_str_has_prefix (copy, "http://github.com/"))
    path = copy + strlen ("http://github.com/");
  else if (g_str_has_prefix (copy, "github.com/"))
    path = copy + strlen ("github.com/");
  else
    path = copy;

  if (g_str_has_suffix (path, ".git"))
    path[strlen (path) - strlen (".git")] = '\0';

  slash = strchr (path, '/');
  if (slash == NULL || strchr (slash + 1, '/') != NULL)
    return NULL;

  *slash = '\0';
  owner = path;
  repository = slash + 1;

  if (!component_is_safe (owner) || !component_is_safe (repository))
    return NULL;

  return g_strdup_printf ("%s/%s", owner, repository);
}

static gboolean
run_id_is_safe (const char *run_id)
{
  if (run_id == NULL || *run_id == '\0')
    return FALSE;

  for (const char *at = run_id; *at != '\0'; at++)
    if (!g_ascii_isdigit (*at))
      return FALSE;

  return TRUE;
}

static char *
repository_from_workdir (const char *workdir)
{
  g_autoptr (XdGitInfo) info = xd_git_info_for_path (workdir);

  return info != NULL ? repository_from_spec (info->remote_url) : NULL;
}

char *
xd_workflow_run_capture_tool (const char *message,
                              const char *workdir)
{
  g_auto (GStrv) argv = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *repository = NULL;
  const char *command;
  const char *run_id = NULL;
  int argc = 0;
  int watch_at = -1;

  if (message == NULL ||
      g_str_has_prefix (message, WORKFLOW_RUN_PREFIX) ||
      !g_str_has_prefix (message, "$ "))
    return g_strdup (message);

  command = message + 2;
  if (!g_shell_parse_argv (command, &argc, &argv, &error))
    return g_strdup (message);

  /* Commands may be part of `push && gh run watch ...`; find the useful
   * invocation rather than requiring it to be the whole shell line. */
  for (int i = 0; i + 3 < argc; i++)
    if (g_strcmp0 (argv[i], "gh") == 0 &&
        g_strcmp0 (argv[i + 1], "run") == 0 &&
        g_strcmp0 (argv[i + 2], "watch") == 0 &&
        run_id_is_safe (argv[i + 3]))
      {
        watch_at = i;
        run_id = argv[i + 3];
        break;
      }

  if (watch_at < 0)
    return g_strdup (message);

  for (int i = watch_at + 4; i < argc; i++)
    {
      const char *spec = NULL;

      if ((g_strcmp0 (argv[i], "--repo") == 0 ||
           g_strcmp0 (argv[i], "-R") == 0) &&
          i + 1 < argc)
        spec = argv[++i];
      else if (g_str_has_prefix (argv[i], "--repo="))
        spec = argv[i] + strlen ("--repo=");

      if (spec != NULL)
        {
          repository = repository_from_spec (spec);
          break;
        }
    }

  if (repository == NULL)
    repository = repository_from_workdir (workdir);
  if (repository == NULL)
    return g_strdup (message);

  {
    g_autofree char *url =
      g_strdup_printf ("https://github.com/%s/actions/runs/%s",
                       repository, run_id);

    return g_strdup_printf (WORKFLOW_RUN_PREFIX "%s\n%s", run_id, url);
  }
}

gboolean
xd_workflow_run_from_tool (const char *message,
                           char      **run_id,
                           char      **url)
{
  const char *id;
  const char *newline;
  const char *link;
  g_autofree char *id_copy = NULL;

  if (message == NULL || !g_str_has_prefix (message, WORKFLOW_RUN_PREFIX))
    return FALSE;

  id = message + strlen (WORKFLOW_RUN_PREFIX);
  newline = strchr (id, '\n');
  if (newline == NULL)
    return FALSE;

  id_copy = g_strndup (id, newline - id);
  link = newline + 1;

  if (!run_id_is_safe (id_copy) ||
      !g_str_has_prefix (link, "https://github.com/") ||
      strchr (link, '\n') != NULL)
    return FALSE;

  if (run_id != NULL)
    *run_id = g_steal_pointer (&id_copy);
  if (url != NULL)
    *url = g_strdup (link);

  return TRUE;
}
