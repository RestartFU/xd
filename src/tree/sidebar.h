#pragma once

#include <adwaita.h>

#include "fs-tree.h"

G_BEGIN_DECLS

#define XD_TYPE_SIDEBAR (xd_sidebar_get_type ())
G_DECLARE_FINAL_TYPE (XdSidebar, xd_sidebar, XD, SIDEBAR, AdwBin)

/*
 * The workspace tree, presented as a file-manager-style list.
 *
 * Emits ::node-selected whenever the selection lands on a different node, and
 * ::node-activated on double-click or Enter.
 */
XdSidebar *xd_sidebar_new (XdFsTree *tree);

G_END_DECLS
