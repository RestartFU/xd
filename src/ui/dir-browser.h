#pragma once

#include <adwaita.h>

#include "remote/remote-tree.h"

G_BEGIN_DECLS

/*
 * Called with the directory that was picked, or NULL when the browser was
 * dismissed without picking one -- which means "wherever the folder runs",
 * not "do nothing".
 */
typedef void (*XdDirChosenFunc) (const char *path,
                                 gpointer    user_data);

/*
 * Picks a directory to work in.
 *
 * Browsing rather than typing, because the answer is a path on a machine and
 * the person choosing it is looking at a tree of folders, not thinking in
 * paths. It reads the directories of whichever machine the work will happen
 * on: @remote lists the daemon's, and NULL lists this one's -- a chat that
 * runs over there cannot be pointed at a directory over here.
 *
 * Enter goes into the highlighted directory, Backspace comes back out, and the
 * button (or Ctrl+Enter) takes the directory currently being shown. Escape
 * leaves it to the folder.
 */
void xd_dir_browser_present (GtkWidget       *parent,
                             XdRemoteTree    *remote,
                             const char      *start,
                             XdDirChosenFunc  chosen,
                             gpointer         user_data);

G_END_DECLS
