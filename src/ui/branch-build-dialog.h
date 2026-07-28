#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

/*
 * Where a branch is named, built and installed.
 *
 * The pull request or branch is kept in settings as it was typed, so the
 * dialog opens on the one being worked on: after another commit, the whole
 * gesture is opening this and pressing the one button.
 */

/*
 * A build finished and was installed over this copy.
 *
 * What is running is still the old one, which is the same situation an update
 * leaves behind -- so the restart is offered where an update's is, rather than
 * from a dialog that has nothing else left to say.
 */
typedef void (*XdBranchBuildDoneFunc) (gpointer user_data);

void xd_branch_build_dialog_present (GtkWidget             *parent,
                                     XdBranchBuildDoneFunc  on_installed,
                                     gpointer               user_data);

G_END_DECLS
