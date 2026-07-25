#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define HY_TYPE_GIT_ACTIONS (hy_git_actions_get_type ())

G_DECLARE_FINAL_TYPE (HyGitActions, hy_git_actions, HY, GIT_ACTIONS, AdwBin)

HyGitActions *hy_git_actions_new         (void);

void          hy_git_actions_set_workdir (HyGitActions *self,
                                          const char   *workdir);

/*
 * Re-reads the repository and picks the action to offer.
 *
 * Called when a turn ends and when the pane it sits in changes, since those
 * are the moments the answer can have changed without the user doing it.
 */
void          hy_git_actions_refresh     (HyGitActions *self);

G_END_DECLS
