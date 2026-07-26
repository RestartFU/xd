#pragma once

#include <gio/gio.h>

#include "xd-node.h"
#include "storage/storage.h"

G_BEGIN_DECLS

#define XD_TYPE_FS_TREE (xd_fs_tree_get_type ())
G_DECLARE_FINAL_TYPE (XdFsTree, xd_fs_tree, XD, FS_TREE, GObject)

/*
 * Mirrors a directory tree into XdNode folders and keeps it in sync.
 *
 * Directories are scanned lazily-but-eagerly: each folder is enumerated
 * asynchronously as soon as it is discovered, and watched afterwards, so
 * changes made outside the app show up on their own. Hidden entries and
 * anything that is not a directory are ignored.
 */

XdFsTree    *xd_fs_tree_new            (const char *root_path,
                                        XdStorage  *storage);

const char  *xd_fs_tree_get_root_path  (XdFsTree *self);
XdNode      *xd_fs_tree_get_root       (XdFsTree *self);

/* Top-level workspaces. Owned by the tree. */
GListModel  *xd_fs_tree_get_model      (XdFsTree *self);

/* @parent may be NULL to create a workspace at the root. */
XdNode      *xd_fs_tree_create_folder  (XdFsTree    *self,
                                        XdNode      *parent,
                                        const char  *name,
                                        GError     **error);

gboolean     xd_fs_tree_rename_folder  (XdFsTree    *self,
                                        XdNode      *node,
                                        const char  *new_name,
                                        GError     **error);

/*
 * Moves a folder under @new_parent, or to the root when it is NULL.
 *
 * The folder's id lives in a dotfile inside it, so it travels with the
 * directory and the chats written against it stay attached.
 */
gboolean     xd_fs_tree_move_folder    (XdFsTree    *self,
                                        XdNode      *node,
                                        XdNode      *new_parent,
                                        GError     **error);

/* Moves the folder to the trash rather than deleting it outright. */
gboolean     xd_fs_tree_trash_folder   (XdFsTree    *self,
                                        XdNode      *node,
                                        GError     **error);

/* Looks a folder up by absolute path; NULL when it is not in the tree. */
XdNode      *xd_fs_tree_lookup         (XdFsTree    *self,
                                        const char  *path);

/* Chats live in the database but appear as leaves of their folder. */
XdNode      *xd_fs_tree_create_chat    (XdFsTree    *self,
                                        XdNode      *folder,
                                        const char  *title,
                                        const char  *backend,
                                        const char  *model,
                                        const char  *effort,
                                        const char  *workdir,
                                        GError     **error);

gboolean     xd_fs_tree_rename_chat    (XdFsTree    *self,
                                        XdNode      *chat,
                                        const char  *title,
                                        GError     **error);

gboolean     xd_fs_tree_delete_chat    (XdFsTree    *self,
                                        XdNode      *chat,
                                        GError     **error);

/* Moves a chat to the top of its folder, matching the most-recent-first order
 * the sidebar shows. */
void         xd_fs_tree_bump_chat      (XdFsTree    *self,
                                        XdNode      *chat);

XdNode      *xd_fs_tree_lookup_chat    (XdFsTree    *self,
                                        const char  *chat_id);

G_END_DECLS
