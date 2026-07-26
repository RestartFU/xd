#pragma once

#include <gio/gio.h>

#include "remote/client.h"
#include "tree/xd-node.h"

G_BEGIN_DECLS

#define XD_TYPE_REMOTE_TREE (xd_remote_tree_get_type ())
G_DECLARE_FINAL_TYPE (XdRemoteTree, xd_remote_tree, XD, REMOTE_TREE, GObject)

/*
 * A daemon's workspaces, as the same XdNode tree the sidebar already draws.
 *
 * This is the filesystem tree's counterpart: where that one enumerates
 * directories and watches them, this one asks the daemon for its tree and
 * refetches when the connection comes up. The nodes are the same, so the
 * sidebar does not know which kind of tree a row came from.
 *
 * Remote folders have no directory on this machine, so a node's handle is a
 * URI naming the daemon rather than a path -- which also keeps the local
 * tree's own lookups from ever matching one.
 */

XdRemoteTree   *xd_remote_tree_new         (XdRemoteClient *client);

XdRemoteClient *xd_remote_tree_get_client  (XdRemoteTree *self);

/* The remote itself, which is a root of its own beside the local workspaces. */
XdNode         *xd_remote_tree_get_root    (XdRemoteTree *self);

/* That root, as a one-row model the sidebar can hold beside the others. */
GListModel     *xd_remote_tree_get_model   (XdRemoteTree *self);

/* Asks for the tree again. Happens by itself whenever the line comes up. */
void            xd_remote_tree_refresh     (XdRemoteTree *self);

XdNode         *xd_remote_tree_lookup_chat (XdRemoteTree *self,
                                            const char   *chat_id);

/* A folder by the URI it is drawn with; the remote's own root included. */
XdNode         *xd_remote_tree_lookup      (XdRemoteTree *self,
                                            const char   *path);

/*
 * Things to be done to the daemon's tree.
 *
 * None of them touch the nodes here. A client never edits the tree it is
 * showing: it says what it wants done, the daemon does it and the tree is read
 * again -- so what is on screen is always what the daemon has, rather than a
 * guess that has to be taken back when it turns out to be wrong.
 *
 * They therefore return nothing. A daemon that refuses raises ::failed with
 * something to show the user, and a new chat arrives as ::chat-created once
 * the tree it is in has caught up.
 *
 * @parent may be NULL to mean the top level of the remote.
 */
void            xd_remote_tree_create_folder (XdRemoteTree *self,
                                              XdNode       *parent,
                                              const char   *name);
void            xd_remote_tree_rename_folder (XdRemoteTree *self,
                                              XdNode       *folder,
                                              const char   *name);
void            xd_remote_tree_move_folder   (XdRemoteTree *self,
                                              XdNode       *folder,
                                              XdNode       *new_parent);
void            xd_remote_tree_trash_folder  (XdRemoteTree *self,
                                              XdNode       *folder);

/*
 * The backend, model and effort are the daemon's to decide -- they come from
 * the folder chain, which lives over there. @workdir may be NULL to inherit
 * the folder's; anything else must be a directory on the daemon, which is why
 * there is an op for listing them.
 */
void            xd_remote_tree_create_chat   (XdRemoteTree *self,
                                              XdNode       *folder,
                                              const char   *title,
                                              const char   *workdir);

/*
 * The directories inside @path on the daemon, for choosing where a chat runs.
 * NULL asks for its home. The callback gets the path listed and a NULL
 * terminated array of names, or NULL when it could not be read.
 */
typedef void (*XdRemoteDirFunc) (const char         *path,
                                 const char *const  *entries,
                                 gpointer            user_data);

void            xd_remote_tree_list_dir      (XdRemoteTree    *self,
                                              const char      *path,
                                              XdRemoteDirFunc  callback,
                                              gpointer         user_data);
void            xd_remote_tree_rename_chat   (XdRemoteTree *self,
                                              XdNode       *chat,
                                              const char   *title);
void            xd_remote_tree_delete_chat   (XdRemoteTree *self,
                                              XdNode       *chat);

/* True when @node is this remote's own row or lives under it. */
gboolean        xd_remote_tree_owns        (XdRemoteTree *self,
                                            XdNode       *node);

/* True for a path a remote tree minted, which is how a row is told apart from
 * a local one -- a remote's chats are read from the daemon, and the folder
 * operations that edit a directory do not apply to them. */
gboolean        xd_remote_tree_is_remote_path (const char *path);

G_END_DECLS
