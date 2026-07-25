#pragma once

#include <adwaita.h>

#include "fs-tree.h"

G_BEGIN_DECLS

#define HY_TYPE_SIDEBAR (hy_sidebar_get_type ())
G_DECLARE_FINAL_TYPE (HySidebar, hy_sidebar, HY, SIDEBAR, AdwBin)

/*
 * The workspace tree, presented as a file-manager-style list.
 *
 * Emits ::node-selected whenever the selection lands on a different node, and
 * ::node-activated on double-click or Enter.
 */
HySidebar *hy_sidebar_new (HyFsTree *tree);

G_END_DECLS
