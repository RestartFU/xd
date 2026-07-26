#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * What repository a chat is pointed at.
 *
 * Read straight out of .git rather than by running git: it is a handful of
 * small files, the answer is wanted on every chat switch, and it keeps the
 * app working on machines without git installed.
 */
typedef struct
{
  char *root;         /* top of the working tree */
  char *name;         /* its directory name */
  char *branch;       /* branch, or a short commit id when detached */
  char *remote_url;   /* origin's URL, when there is one */
  gboolean detached;
  gboolean linked_worktree;   /* a `git worktree`, not the main checkout */
} XdGitInfo;

void       xd_git_info_free     (XdGitInfo *self);

/* NULL when @path is not inside a repository. */
XdGitInfo *xd_git_info_for_path (const char *path);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (XdGitInfo, xd_git_info_free)

G_END_DECLS
