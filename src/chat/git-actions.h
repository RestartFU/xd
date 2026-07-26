#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define XD_TYPE_GIT_ACTIONS (xd_git_actions_get_type ())

G_DECLARE_FINAL_TYPE (XdGitActions, xd_git_actions, XD, GIT_ACTIONS, AdwBin)

XdGitActions *xd_git_actions_new         (void);

void          xd_git_actions_set_workdir (XdGitActions *self,
                                          const char   *workdir);

/*
 * Re-reads the repository and picks the action to offer.
 *
 * Called when a turn ends and when the pane it sits in changes, since those
 * are the moments the answer can have changed without the user doing it.
 */
void          xd_git_actions_refresh     (XdGitActions *self);

G_END_DECLS
