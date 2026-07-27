#pragma once

#include <glib.h>

G_BEGIN_DECLS

typedef struct
{
  char *path;
  char *branch;
  gboolean detached;
  gboolean main;
  gboolean current;
  gboolean prunable;
} XdWorktreeInfo;

void       xd_worktree_info_free (XdWorktreeInfo *self);
gboolean   xd_worktree_path_equal (const char     *a,
                                   const char     *b);

/*
 * Every checkout registered with the repository containing @workdir.
 *
 * The main checkout is first, matching `git worktree list`. Returns NULL when
 * @workdir is not a repository or Git cannot read its worktree metadata.
 */
GPtrArray *xd_worktree_list      (const char      *workdir,
                                  GError         **error);

/*
 * Creates the private checkout used by a new chat.
 *
 * The checkout starts at the current HEAD and gets its own xd/<chat-id>
 * branch under ../worktrees/<repository>/<chat-id>. Its path is returned; an
 * existing checkout from an interrupted first-send attempt is reused.
 */
char      *xd_worktree_create    (const char      *workdir,
                                  const char      *chat_id,
                                  GError         **error);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (XdWorktreeInfo, xd_worktree_info_free)

G_END_DECLS
