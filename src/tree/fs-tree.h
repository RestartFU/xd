#pragma once

#include <gio/gio.h>

#include "hy-node.h"
#include "storage/storage.h"

G_BEGIN_DECLS

#define HY_TYPE_FS_TREE (hy_fs_tree_get_type ())
G_DECLARE_FINAL_TYPE (HyFsTree, hy_fs_tree, HY, FS_TREE, GObject)

/*
 * Mirrors a directory tree into HyNode folders and keeps it in sync.
 *
 * Directories are scanned lazily-but-eagerly: each folder is enumerated
 * asynchronously as soon as it is discovered, and watched afterwards, so
 * changes made outside the app show up on their own. Hidden entries and
 * anything that is not a directory are ignored.
 */

HyFsTree    *hy_fs_tree_new            (const char *root_path,
                                        HyStorage  *storage);

const char  *hy_fs_tree_get_root_path  (HyFsTree *self);
HyNode      *hy_fs_tree_get_root       (HyFsTree *self);

/* Top-level workspaces. Owned by the tree. */
GListModel  *hy_fs_tree_get_model      (HyFsTree *self);

/* @parent may be NULL to create a workspace at the root. */
HyNode      *hy_fs_tree_create_folder  (HyFsTree    *self,
                                        HyNode      *parent,
                                        const char  *name,
                                        GError     **error);

gboolean     hy_fs_tree_rename_folder  (HyFsTree    *self,
                                        HyNode      *node,
                                        const char  *new_name,
                                        GError     **error);

/*
 * Moves a folder under @new_parent, or to the root when it is NULL.
 *
 * The folder's id lives in a dotfile inside it, so it travels with the
 * directory and the chats written against it stay attached.
 */
gboolean     hy_fs_tree_move_folder    (HyFsTree    *self,
                                        HyNode      *node,
                                        HyNode      *new_parent,
                                        GError     **error);

/* Moves the folder to the trash rather than deleting it outright. */
gboolean     hy_fs_tree_trash_folder   (HyFsTree    *self,
                                        HyNode      *node,
                                        GError     **error);

/* Looks a folder up by absolute path; NULL when it is not in the tree. */
HyNode      *hy_fs_tree_lookup         (HyFsTree    *self,
                                        const char  *path);

/* Chats live in the database but appear as leaves of their folder. */
HyNode      *hy_fs_tree_create_chat    (HyFsTree    *self,
                                        HyNode      *folder,
                                        const char  *title,
                                        const char  *backend,
                                        const char  *model,
                                        const char  *effort,
                                        const char  *workdir,
                                        GError     **error);

gboolean     hy_fs_tree_rename_chat    (HyFsTree    *self,
                                        HyNode      *chat,
                                        const char  *title,
                                        GError     **error);

gboolean     hy_fs_tree_delete_chat    (HyFsTree    *self,
                                        HyNode      *chat,
                                        GError     **error);

/* Moves a chat to the top of its folder, matching the most-recent-first order
 * the sidebar shows. */
void         hy_fs_tree_bump_chat      (HyFsTree    *self,
                                        HyNode      *chat);

HyNode      *hy_fs_tree_lookup_chat    (HyFsTree    *self,
                                        const char  *chat_id);

G_END_DECLS
