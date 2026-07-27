#pragma once

#include <gio/gio.h>

G_BEGIN_DECLS

#define XD_TYPE_GIT_HEAD_WATCH (xd_git_head_watch_get_type ())

G_DECLARE_FINAL_TYPE (XdGitHeadWatch, xd_git_head_watch,
                      XD, GIT_HEAD_WATCH, GObject)

/*
 * Watches the repository containing @workdir for branch/detached-HEAD
 * changes. Emits "changed" after Git has finished its atomic HEAD rewrite.
 */
XdGitHeadWatch *xd_git_head_watch_new         (void);
void            xd_git_head_watch_set_workdir (XdGitHeadWatch *self,
                                                const char     *workdir);

G_END_DECLS
