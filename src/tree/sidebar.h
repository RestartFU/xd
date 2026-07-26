#pragma once

#include <adwaita.h>

#include "fs-tree.h"
#include "remote/remote-tree.h"

G_BEGIN_DECLS

#define XD_TYPE_SIDEBAR (xd_sidebar_get_type ())
G_DECLARE_FINAL_TYPE (XdSidebar, xd_sidebar, XD, SIDEBAR, AdwBin)

/*
 * The workspace tree, presented as a file-manager-style list.
 *
 * Emits ::node-selected whenever the selection lands on a different node, and
 * ::node-activated on double-click or Enter.
 */
XdSidebar *xd_sidebar_new        (XdFsTree *tree);

/*
 * The remote whose root sits beside the local workspaces; NULL takes it away.
 *
 * Its rows are the same nodes as any other, drawn the same way, and read-only:
 * the daemon owns that tree, and the folder operations here edit directories on
 * this machine.
 */
void       xd_sidebar_set_remote (XdSidebar    *self,
                                  XdRemoteTree *remote);

/*
 * Selects a saved chat once its asynchronous local or remote tree arrives.
 * Any selection made before then cancels the restore.
 */
void       xd_sidebar_restore_chat (XdSidebar *self,
                                    const char *chat_id,
                                    gboolean    remote);

G_END_DECLS
