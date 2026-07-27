#pragma once

#include <gtk/gtk.h>

#include "remote/remote-tree.h"
#include "tree/xd-node.h"

G_BEGIN_DECLS

/*
 * Edits the context attached directly to one folder.
 *
 * @remote is NULL for a local folder. A remote folder is read and written by
 * its daemon, because its settings file does not exist on this machine.
 */
void xd_folder_context_dialog_present (GtkWidget    *parent,
                                       XdNode       *folder,
                                       XdRemoteTree *remote);

G_END_DECLS
