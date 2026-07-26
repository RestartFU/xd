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

/* True when @node is this remote's own row or lives under it. */
gboolean        xd_remote_tree_owns        (XdRemoteTree *self,
                                            XdNode       *node);

/* True for a path a remote tree minted, which is how a row is told apart from
 * a local one -- a remote's chats are read from the daemon, and the folder
 * operations that edit a directory do not apply to them. */
gboolean        xd_remote_tree_is_remote_path (const char *path);

G_END_DECLS
