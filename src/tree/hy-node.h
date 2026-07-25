#pragma once

#include <gio/gio.h>

G_BEGIN_DECLS

typedef enum
{
  HY_NODE_FOLDER,
  HY_NODE_CHAT,
} HyNodeKind;

#define HY_TYPE_NODE (hy_node_get_type ())
G_DECLARE_FINAL_TYPE (HyNode, hy_node, HY, NODE, GObject)

/*
 * One row of the workspace tree.
 *
 * Folders map to real directories and own their children; chats are leaves
 * identified by their row id in the database. Both live in the same model so
 * the sidebar reads like a file manager.
 */

HyNode      *hy_node_new_folder     (const char *path,
                                     const char *name,
                                     const char *folder_id);
HyNode      *hy_node_new_chat       (const char *chat_id,
                                     const char *title,
                                     HyNode     *parent);

HyNodeKind   hy_node_get_kind       (HyNode *self);
const char  *hy_node_get_name       (HyNode *self);
void         hy_node_set_name       (HyNode     *self,
                                     const char *name);
const char  *hy_node_get_path       (HyNode *self);
void         hy_node_set_path       (HyNode     *self,
                                     const char *path);
const char  *hy_node_get_folder_id  (HyNode *self);
const char  *hy_node_get_chat_id    (HyNode *self);
const char  *hy_node_get_icon_name  (HyNode *self);

/* Folders only; chats return NULL. Owned by the node. */
GListStore  *hy_node_get_children   (HyNode *self);

/* Weak: a child does not keep its parent alive. */
HyNode      *hy_node_get_parent     (HyNode *self);
void         hy_node_set_parent     (HyNode *self,
                                     HyNode *parent);

G_END_DECLS
