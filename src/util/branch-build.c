#include "branch-build.h"

#include <string.h>

/*
 * A name git would take, and a shell cannot be talked into anything with.
 *
 * The ref reaches a command line, so the check is a whitelist rather than a
 * list of things to escape: letters, digits and the four punctuation marks
 * branch names actually use. Git's own refusals -- a leading dash, an empty
 * component, ".." anywhere, a trailing ".lock" -- are repeated here so a name
 * that cannot work fails while it is still a typo rather than a failed build.
 */
static gboolean
is_ref_name (const char *name)
{
  gsize length;

  if (name == NULL || *name == '\0')
    return FALSE;

  length = strlen (name);
  if (length > 200)
    return FALSE;

  for (gsize i = 0; i < length; i++)
    {
      char c = name[i];

      if (!g_ascii_isalnum (c) && c != '.' && c != '_' && c != '-' && c != '/')
        return FALSE;
    }

  if (name[0] == '-' || name[0] == '/' || name[0] == '.')
    return FALSE;

  if (name[length - 1] == '/' || name[length - 1] == '.')
    return FALSE;

  if (strstr (name, "..") != NULL || strstr (name, "//") != NULL)
    return FALSE;

  return !g_str_has_suffix (name, ".lock");
}

/* owner and repository, which are narrower than a ref name. */
static gboolean
is_repo_part (const char *part)
{
  if (part == NULL || *part == '\0' || part[0] == '-')
    return FALSE;

  for (const char *at = part; *at != '\0'; at++)
    if (!g_ascii_isalnum (*at) && *at != '.' && *at != '_' && *at != '-')
      return FALSE;

  return TRUE;
}

static gboolean
all_digits (const char *text)
{
  if (text == NULL || *text == '\0')
    return FALSE;

  for (const char *at = text; *at != '\0'; at++)
    if (!g_ascii_isdigit (*at))
      return FALSE;

  return TRUE;
}

static void
take_result (const char  *repo,
             const char  *label_body,
             const char  *ref_value,
             char       **url,
             char       **ref,
             char       **label)
{
  *url = g_strdup_printf ("https://github.com/%s.git", repo);
  *ref = g_strdup (ref_value);

  /* Named by where it is only when that is somewhere else: the repository this
   * build came from is the answer nobody needs repeated. */
  *label = g_strcmp0 (repo, XD_REPO) == 0
    ? g_strdup (label_body)
    : g_strdup_printf ("%s in %s", label_body, repo);
}

static gboolean
parse_pull_request (const char  *repo,
                    const char  *number,
                    char       **url,
                    char       **ref,
                    char       **label)
{
  g_autofree char *ref_value = NULL;
  g_autofree char *label_body = NULL;

  if (!all_digits (number) || strlen (number) > 9)
    return FALSE;

  ref_value = g_strdup_printf ("refs/pull/%s/head", number);
  label_body = g_strdup_printf ("pull request #%s", number);

  take_result (repo, label_body, ref_value, url, ref, label);
  return TRUE;
}

static gboolean
parse_branch (const char  *repo,
              const char  *branch,
              char       **url,
              char       **ref,
              char       **label)
{
  g_autofree char *ref_value = NULL;
  g_autofree char *label_body = NULL;

  if (!is_ref_name (branch))
    return FALSE;

  ref_value = g_strdup_printf ("refs/heads/%s", branch);
  label_body = g_strdup_printf ("branch %s", branch);

  take_result (repo, label_body, ref_value, url, ref, label);
  return TRUE;
}

/*
 * A github.com link, whichever of its pages was open when it was copied.
 *
 * The query and the fragment are cut first: a pull request is linked to with a
 * comment id on the end at least as often as it is linked to plain.
 */
static gboolean
parse_link (const char  *text,
            char       **url,
            char       **ref,
            char       **label)
{
  const char *host = strstr (text, "github.com/");
  g_autofree char *path = NULL;
  g_auto (GStrv) parts = NULL;
  g_autofree char *repo = NULL;
  char *cut;

  if (host == NULL)
    return FALSE;

  path = g_strdup (host + strlen ("github.com/"));

  cut = strpbrk (path, "?#");
  if (cut != NULL)
    *cut = '\0';

  parts = g_strsplit (path, "/", 0);
  if (g_strv_length (parts) < 2)
    return FALSE;

  /* Cloning by name, so a link that carries the .git suffix loses it here
   * rather than becoming a repository called "xd.git". */
  if (g_str_has_suffix (parts[1], ".git"))
    parts[1][strlen (parts[1]) - strlen (".git")] = '\0';

  if (!is_repo_part (parts[0]) || !is_repo_part (parts[1]))
    return FALSE;

  repo = g_strdup_printf ("%s/%s", parts[0], parts[1]);

  if (g_strv_length (parts) >= 4 && g_strcmp0 (parts[2], "pull") == 0)
    return parse_pull_request (repo, parts[3], url, ref, label);

  if (g_strv_length (parts) >= 4 && g_strcmp0 (parts[2], "tree") == 0)
    {
      /* A branch name may hold slashes, so everything past tree/ is the name
       * rather than the first component of it. */
      g_autofree char *branch = g_strjoinv ("/", parts + 3);

      return parse_branch (repo, branch, url, ref, label);
    }

  /* A repository on its own names no particular code to build. */
  return FALSE;
}

gboolean
xd_branch_build_parse (const char  *text,
                       char       **url,
                       char       **ref,
                       char       **label)
{
  g_autofree char *trimmed = NULL;

  g_return_val_if_fail (url != NULL && ref != NULL && label != NULL, FALSE);

  if (text == NULL)
    return FALSE;

  trimmed = g_strstrip (g_strdup (text));
  if (*trimmed == '\0')
    return FALSE;

  if (strstr (trimmed, "github.com/") != NULL)
    return parse_link (trimmed, url, ref, label);

  /* Written the way a pull request is referred to in prose, or dropped in as
   * the number alone. */
  if (trimmed[0] == '#')
    return parse_pull_request (XD_REPO, trimmed + 1, url, ref, label);

  if (all_digits (trimmed))
    return parse_pull_request (XD_REPO, trimmed, url, ref, label);

  return parse_branch (XD_REPO, trimmed, url, ref, label);
}

char *
xd_branch_build_checkout_dir (void)
{
  /* The cache, because it is exactly that: losing it costs one clone, and a
   * machine that clears caches should be free to clear this. */
  return g_build_filename (g_get_user_cache_dir (), XD_DATA_NAME, "source", NULL);
}

char *
xd_branch_build_command (const char *url,
                         const char *ref,
                         const char *checkout)
{
  g_autofree char *parent = NULL;
  g_autofree char *quoted_checkout = NULL;
  g_autofree char *quoted_parent = NULL;
  g_autofree char *quoted_url = NULL;
  g_autofree char *quoted_ref = NULL;

  g_return_val_if_fail (url != NULL && ref != NULL && checkout != NULL, NULL);

  parent = g_path_get_dirname (checkout);

  /* The ref and the URL were checked before they got here; the path was not --
   * it is wherever the home directory is, which is the one of the four that
   * can hold a quote. All four are quoted, because one of them must be. */
  quoted_checkout = g_shell_quote (checkout);
  quoted_parent = g_shell_quote (parent);
  quoted_url = g_shell_quote (url);
  quoted_ref = g_shell_quote (ref);

  /*
   * Shallow, and fetched into a repository that is kept: a rebuild after one
   * more commit is a fetch of one more commit, and docker has the layers of
   * the previous build to reuse.
   *
   * The tree is emptied of anything not in the ref before building, so what is
   * built is the branch and not the branch over the remains of the last one.
   */
  return g_strdup_printf (
    "set -eu\n"
    "checkout=%s\n"
    "mkdir -p %s\n"
    "[ -d \"$checkout/.git\" ] || git init -q \"$checkout\"\n"
    "git -C \"$checkout\" fetch --depth 1 --force %s %s\n"
    "git -C \"$checkout\" checkout -q --force --detach FETCH_HEAD\n"
    "git -C \"$checkout\" clean -qfdx\n"
    "cd \"$checkout\"\n"
    /*
     * The installer that comes with the branch is the one that runs, so where
     * things go is decided by the code being installed rather than by a copy
     * of its steps kept here. A branch from before it could install a bundle
     * from disk would quietly download the published nightly instead, and the
     * build would look like it had worked -- so that is a refusal, not a
     * fallback to installing the wrong thing.
     */
    "grep -q -- '--from)' scripts/install.sh ||"
    " { echo \"this branch's installer cannot install a local build;"
    " rebase it on master\" >&2; exit 1; }\n"
    "./scripts/build.sh --build-arg PROFILE=nightly\n"
    "sh scripts/install.sh --from dist\n",
    quoted_checkout, quoted_parent, quoted_url, quoted_ref);
}
