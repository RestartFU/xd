#pragma once

#include <gio/gio.h>

G_BEGIN_DECLS

typedef enum
{
  XD_NODE_FOLDER,
  XD_NODE_CHAT,
} XdNodeKind;

/*
 * What a row is doing, as far as the tree is concerned.
 *
 * The sidebar is often the only part of a chat on screen -- the user is
 * reading another one, or another window entirely -- so a chat says whether
 * it is still working, waiting to be answered, or done.
 *
 * A remote's own root uses the same states for the same reason: it is a row
 * that stands for a machine, and whether that machine is answering is
 * something the tree has to be able to show without being asked.
 */
typedef enum
{
  XD_NODE_IDLE,
  XD_NODE_WORKING,
  XD_NODE_WAITING,   /* it asked something and nobody has answered */
  XD_NODE_OFFLINE,   /* a remote that is not answering */
} XdNodeState;

#define XD_TYPE_NODE (xd_node_get_type ())
G_DECLARE_FINAL_TYPE (XdNode, xd_node, XD, NODE, GObject)

/*
 * One row of the workspace tree.
 *
 * Folders map to real directories and own their children; chats are leaves
 * identified by their row id in the database. Both live in the same model so
 * the sidebar reads like a file manager.
 */

XdNode      *xd_node_new_folder     (const char *path,
                                     const char *name,
                                     const char *folder_id);
XdNode      *xd_node_new_chat       (const char *chat_id,
                                     const char *title,
                                     XdNode     *parent);

XdNodeKind   xd_node_get_kind       (XdNode *self);
const char  *xd_node_get_name       (XdNode *self);
void         xd_node_set_name       (XdNode     *self,
                                     const char *name);
const char  *xd_node_get_path       (XdNode *self);
void         xd_node_set_path       (XdNode     *self,
                                     const char *path);
const char  *xd_node_get_folder_id  (XdNode *self);
const char  *xd_node_get_chat_id    (XdNode *self);
const char  *xd_node_get_icon_name  (XdNode *self);

/*
 * The icon a chat rests at, which is the assistant that last answered it.
 *
 * Chats made before the icon was recorded, and any backend that has since gone
 * away, fall back to a plain chat bubble. Folders rarely set one -- a folder
 * looks like a folder -- but a root that is not a directory at all, such as a
 * remote daemon, says so here.
 */
void         xd_node_set_icon_name  (XdNode     *self,
                                     const char *icon_name);

XdNodeState  xd_node_get_state      (XdNode *self);
void         xd_node_set_state      (XdNode      *self,
                                     XdNodeState  state);

/* Folders only; chats return NULL. Owned by the node. */
GListStore  *xd_node_get_children   (XdNode *self);

/* Weak: a child does not keep its parent alive. */
XdNode      *xd_node_get_parent     (XdNode *self);
void         xd_node_set_parent     (XdNode *self,
                                     XdNode *parent);

G_END_DECLS
