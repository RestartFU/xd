#pragma once

#include <adwaita.h>

#include "storage/storage.h"
#include "tree/fs-tree.h"

G_BEGIN_DECLS

typedef void (*XdSearchActivateFunc) (XdNode   *chat,
                                      gpointer  user_data);

/*
 * Full-text search across every message, so a chat can be found by something
 * said in it rather than by remembering where it was filed.
 */
void xd_search_dialog_present (GtkWidget            *parent,
                               XdStorage            *storage,
                               XdFsTree             *tree,
                               XdSearchActivateFunc  on_activate,
                               gpointer              user_data);

G_END_DECLS
