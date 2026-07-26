#include "fs-tree.h"

#include "backend/backend.h"

#include <string.h>

#include "settings/folder-settings.h"

/* Directory changes arrive in bursts (git checkouts, editors); coalesce them. */
#define RESCAN_DEBOUNCE_MS 200

#define ENUMERATE_ATTRS \
  G_FILE_ATTRIBUTE_STANDARD_NAME "," \
  G_FILE_ATTRIBUTE_STANDARD_TYPE "," \
  G_FILE_ATTRIBUTE_STANDARD_IS_HIDDEN

typedef struct
{
  HyFsTree *tree;         /* unowned; the tree outlives its watches */
  HyNode *node;           /* unowned; removed from the table before the node dies */
  GFileMonitor *monitor;
  guint debounce_id;
} Watch;

struct _HyFsTree
{
  GObject parent_instance;

  char *root_path;
  HyNode *root;
  HyStorage *storage;
  GHashTable *watches;    /* HyNode* -> Watch* */
  GCancellable *cancellable;
};

enum
{
  SIGNAL_CHAT_REMOVED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (HyFsTree, hy_fs_tree, G_TYPE_OBJECT)

/* The icon of the assistant a chat is set to, so the tree says at a glance
 * which one has been answering. */
static const char *
backend_icon (const char *backend_id)
{
  const AiBackend *backend = ai_backend_lookup (backend_id);

  return backend != NULL ? backend->icon_name : NULL;
}

static void scan_node (HyFsTree *self, HyNode *node);
static void watch_node (HyFsTree *self, HyNode *node);
static void forget_subtree (HyFsTree *self, HyNode *node);

/* --- watches -------------------------------------------------------------- */

static void
watch_free (gpointer data)
{
  Watch *watch = data;

  g_clear_handle_id (&watch->debounce_id, g_source_remove);

  if (watch->monitor != NULL)
    {
      g_file_monitor_cancel (watch->monitor);
      g_clear_object (&watch->monitor);
    }

  g_free (watch);
}

static gboolean
on_debounce_elapsed (gpointer data)
{
  Watch *watch = data;

  watch->debounce_id = 0;
  scan_node (watch->tree, watch->node);

  return G_SOURCE_REMOVE;
}

static void
on_directory_changed (GFileMonitor      *monitor,
                      GFile             *file,
                      GFile             *other_file,
                      GFileMonitorEvent  event,
                      gpointer           data)
{
  Watch *watch = data;

  switch (event)
    {
    case G_FILE_MONITOR_EVENT_CREATED:
    case G_FILE_MONITOR_EVENT_DELETED:
    case G_FILE_MONITOR_EVENT_MOVED_IN:
    case G_FILE_MONITOR_EVENT_MOVED_OUT:
    case G_FILE_MONITOR_EVENT_RENAMED:
      break;
    default:
      return;
    }

  g_clear_handle_id (&watch->debounce_id, g_source_remove);
  watch->debounce_id = g_timeout_add (RESCAN_DEBOUNCE_MS, on_debounce_elapsed, watch);
}

static void
watch_node (HyFsTree *self,
            HyNode   *node)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GFile) file = NULL;
  GFileMonitor *monitor;
  Watch *watch;

  if (g_hash_table_contains (self->watches, node))
    return;

  file = g_file_new_for_path (hy_node_get_path (node));
  monitor = g_file_monitor_directory (file, G_FILE_MONITOR_WATCH_MOVES,
                                      self->cancellable, &error);
  if (monitor == NULL)
    {
      /* Not fatal: the folder simply will not update by itself. */
      g_debug ("cannot watch %s: %s", hy_node_get_path (node), error->message);
      return;
    }

  watch = g_new0 (Watch, 1);
  watch->tree = self;
  watch->node = node;
  watch->monitor = monitor;

  g_signal_connect (monitor, "changed", G_CALLBACK (on_directory_changed), watch);
  g_hash_table_insert (self->watches, node, watch);
}

static void
forget_subtree (HyFsTree *self,
                HyNode   *node)
{
  GListStore *children = hy_node_get_children (node);

  g_hash_table_remove (self->watches, node);

  if (children == NULL)
    return;

  for (guint i = 0; i < g_list_model_get_n_items (G_LIST_MODEL (children)); i++)
    {
      g_autoptr (HyNode) child = g_list_model_get_item (G_LIST_MODEL (children), i);

      if (hy_node_get_kind (child) == HY_NODE_FOLDER)
        forget_subtree (self, child);
    }
}

/* --- scanning ------------------------------------------------------------- */

typedef struct
{
  HyFsTree *self;
  HyNode *node;
  GFileEnumerator *enumerator;
  GPtrArray *names;      /* owned char* of subdirectory names found on disk */
} ScanOp;

static void
scan_op_free (ScanOp *op)
{
  g_clear_object (&op->enumerator);
  g_clear_object (&op->node);
  g_clear_object (&op->self);
  g_clear_pointer (&op->names, g_ptr_array_unref);
  g_free (op);
}

/* Folders sort alphabetically ahead of chats, so a name search can stop early. */
static int
find_child_folder (HyNode     *parent,
                   const char *name)
{
  GListModel *model = G_LIST_MODEL (hy_node_get_children (parent));

  for (guint i = 0; i < g_list_model_get_n_items (model); i++)
    {
      g_autoptr (HyNode) child = g_list_model_get_item (model, i);

      if (hy_node_get_kind (child) != HY_NODE_FOLDER)
        break;

      if (g_strcmp0 (hy_node_get_name (child), name) == 0)
        return (int) i;
    }

  return -1;
}

static guint
folder_insert_position (HyNode     *parent,
                        const char *name)
{
  GListModel *model = G_LIST_MODEL (hy_node_get_children (parent));
  guint n_items = g_list_model_get_n_items (model);
  guint i;

  for (i = 0; i < n_items; i++)
    {
      g_autoptr (HyNode) child = g_list_model_get_item (model, i);

      if (hy_node_get_kind (child) != HY_NODE_FOLDER)
        break;

      if (g_utf8_collate (hy_node_get_name (child), name) > 0)
        break;
    }

  return i;
}

/* Chats sort after every folder, so this is where the chat section starts. */
static guint
chat_section_start (HyNode *folder)
{
  GListModel *model = G_LIST_MODEL (hy_node_get_children (folder));
  guint n_items = g_list_model_get_n_items (model);
  guint i;

  for (i = 0; i < n_items; i++)
    {
      g_autoptr (HyNode) child = g_list_model_get_item (model, i);

      if (hy_node_get_kind (child) != HY_NODE_FOLDER)
        break;
    }

  return i;
}

static void
load_chats (HyFsTree *self,
            HyNode   *folder)
{
  g_autoptr (GPtrArray) chats = NULL;
  g_autoptr (GError) error = NULL;
  const char *folder_id = hy_node_get_folder_id (folder);
  guint position;

  if (self->storage == NULL || folder_id == NULL)
    return;

  chats = hy_storage_list_chats (self->storage, folder_id, &error);
  if (chats == NULL)
    {
      g_warning ("cannot list chats of %s: %s", hy_node_get_name (folder),
                 error->message);
      return;
    }

  /* list_chats already returns most-recent-first, so appending in order keeps
   * that ordering in the sidebar. */
  position = chat_section_start (folder);
  for (guint i = 0; i < chats->len; i++)
    {
      const HyChat *chat = g_ptr_array_index (chats, i);
      HyNode *node = hy_node_new_chat (chat->id, chat->title, folder);

      hy_node_set_icon_name (node, backend_icon (chat->backend));

      g_list_store_insert (hy_node_get_children (folder), position + i, node);
      g_object_unref (node);
    }
}

static HyNode *
add_folder_child (HyFsTree   *self,
                  HyNode     *parent,
                  const char *name)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (HyFolderSettings) settings = NULL;
  g_autofree char *path = NULL;
  HyNode *child;

  path = g_build_filename (hy_node_get_path (parent), name, NULL);

  /* Minting the folder id touches disk, but only the first time a folder is
   * seen, and the file is a few hundred bytes. */
  settings = hy_folder_settings_ensure (path, &error);
  if (settings == NULL)
    {
      g_warning ("cannot initialise %s: %s", path, error->message);
      return NULL;
    }

  child = hy_node_new_folder (path, name, settings->id);
  hy_node_set_parent (child, parent);

  g_list_store_insert (hy_node_get_children (parent),
                       folder_insert_position (parent, name), child);
  g_object_unref (child);

  load_chats (self, child);

  return child;
}

static void
reconcile (HyFsTree *self,
           HyNode   *node,
           GPtrArray *names)
{
  GListStore *children = hy_node_get_children (node);
  GListModel *model = G_LIST_MODEL (children);
  g_autoptr (GHashTable) on_disk = g_hash_table_new (g_str_hash, g_str_equal);
  guint i = 0;

  for (guint n = 0; n < names->len; n++)
    g_hash_table_add (on_disk, g_ptr_array_index (names, n));

  /* Drop folders that disappeared. Chat leaves are not ours to touch. */
  while (i < g_list_model_get_n_items (model))
    {
      g_autoptr (HyNode) child = g_list_model_get_item (model, i);

      if (hy_node_get_kind (child) != HY_NODE_FOLDER)
        break;

      if (!g_hash_table_contains (on_disk, hy_node_get_name (child)))
        {
          forget_subtree (self, child);
          g_list_store_remove (children, i);
          continue;
        }

      i++;
    }

  /* Add the ones we have not seen before, and scan only those: the rest have
   * their own monitors and reconcile themselves. */
  for (guint n = 0; n < names->len; n++)
    {
      const char *name = g_ptr_array_index (names, n);
      HyNode *child;

      if (find_child_folder (node, name) >= 0)
        continue;

      child = add_folder_child (self, node, name);
      if (child != NULL)
        scan_node (self, child);
    }
}

static void on_next_files (GObject *source, GAsyncResult *result, gpointer data);

static void
request_next_files (ScanOp *op)
{
  g_file_enumerator_next_files_async (op->enumerator, 64, G_PRIORITY_DEFAULT,
                                      op->self->cancellable, on_next_files, op);
}

static void
on_next_files (GObject      *source,
               GAsyncResult *result,
               gpointer      data)
{
  ScanOp *op = data;
  g_autoptr (GError) error = NULL;
  GList *infos;

  infos = g_file_enumerator_next_files_finish (op->enumerator, result, &error);

  if (error != NULL)
    {
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        g_warning ("cannot read %s: %s", hy_node_get_path (op->node), error->message);
      scan_op_free (op);
      return;
    }

  if (infos == NULL)
    {
      reconcile (op->self, op->node, op->names);
      watch_node (op->self, op->node);
      scan_op_free (op);
      return;
    }

  for (GList *l = infos; l != NULL; l = l->next)
    {
      GFileInfo *info = l->data;

      if (g_file_info_get_file_type (info) != G_FILE_TYPE_DIRECTORY)
        continue;
      if (g_file_info_get_is_hidden (info))
        continue;

      g_ptr_array_add (op->names, g_strdup (g_file_info_get_name (info)));
    }

  g_list_free_full (infos, g_object_unref);
  request_next_files (op);
}

static void
on_enumerate_ready (GObject      *source,
                    GAsyncResult *result,
                    gpointer      data)
{
  ScanOp *op = data;
  g_autoptr (GError) error = NULL;

  op->enumerator = g_file_enumerate_children_finish (G_FILE (source), result, &error);

  if (op->enumerator == NULL)
    {
      /* A folder vanishing mid-scan is ordinary: it was renamed or removed
       * while the rescan was in flight, and the parent's own scan will drop
       * it. Only unexpected failures are worth a warning. */
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED) &&
          !g_error_matches (error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND))
        g_warning ("cannot open %s: %s", hy_node_get_path (op->node), error->message);
      else
        g_debug ("scan of %s abandoned: %s", hy_node_get_path (op->node), error->message);

      scan_op_free (op);
      return;
    }

  request_next_files (op);
}

static void
scan_node (HyFsTree *self,
           HyNode   *node)
{
  g_autoptr (GFile) file = NULL;
  ScanOp *op;

  op = g_new0 (ScanOp, 1);
  op->self = g_object_ref (self);
  op->node = g_object_ref (node);
  op->names = g_ptr_array_new_with_free_func (g_free);

  file = g_file_new_for_path (hy_node_get_path (node));
  g_file_enumerate_children_async (file, ENUMERATE_ATTRS,
                                   G_FILE_QUERY_INFO_NONE, G_PRIORITY_DEFAULT,
                                   self->cancellable, on_enumerate_ready, op);
}

/* --- public API ----------------------------------------------------------- */

HyFsTree *
hy_fs_tree_new (const char *root_path,
                HyStorage  *storage)
{
  g_autoptr (GError) error = NULL;
  g_autoptr (GFile) root_file = NULL;
  HyFsTree *self;

  g_return_val_if_fail (root_path != NULL, NULL);

  root_file = g_file_new_for_path (root_path);
  if (!g_file_make_directory_with_parents (root_file, NULL, &error) &&
      !g_error_matches (error, G_IO_ERROR, G_IO_ERROR_EXISTS))
    g_warning ("cannot create %s: %s", root_path, error->message);

  self = g_object_new (HY_TYPE_FS_TREE, NULL);
  self->root_path = g_strdup (root_path);
  self->storage = storage != NULL ? g_object_ref (storage) : NULL;
  self->root = hy_node_new_folder (root_path, "Workspaces", NULL);

  scan_node (self, self->root);

  return self;
}

const char *
hy_fs_tree_get_root_path (HyFsTree *self)
{
  g_return_val_if_fail (HY_IS_FS_TREE (self), NULL);

  return self->root_path;
}

HyNode *
hy_fs_tree_get_root (HyFsTree *self)
{
  g_return_val_if_fail (HY_IS_FS_TREE (self), NULL);

  return self->root;
}

GListModel *
hy_fs_tree_get_model (HyFsTree *self)
{
  g_return_val_if_fail (HY_IS_FS_TREE (self), NULL);

  return G_LIST_MODEL (hy_node_get_children (self->root));
}

HyNode *
hy_fs_tree_create_folder (HyFsTree    *self,
                          HyNode      *parent,
                          const char  *name,
                          GError     **error)
{
  g_autoptr (GFile) file = NULL;
  g_autofree char *path = NULL;

  g_return_val_if_fail (HY_IS_FS_TREE (self), NULL);
  g_return_val_if_fail (name != NULL && *name != '\0', NULL);

  if (parent == NULL)
    parent = self->root;

  if (strchr (name, G_DIR_SEPARATOR) != NULL)
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_INVALID_FILENAME,
                   "A folder name cannot contain \"%c\"", G_DIR_SEPARATOR);
      return NULL;
    }

  path = g_build_filename (hy_node_get_path (parent), name, NULL);
  file = g_file_new_for_path (path);

  if (!g_file_make_directory (file, NULL, error))
    return NULL;

  /* Insert eagerly so the caller can select the new folder; the monitor's
   * later reconcile finds it already present and does nothing. */
  return add_folder_child (self, parent, name);
}

/* A rename moves the whole subtree, so every descendant path needs rewriting. */
static void
rebase_subtree (HyNode     *node,
                const char *new_path)
{
  GListStore *children;

  hy_node_set_path (node, new_path);

  children = hy_node_get_children (node);
  if (children == NULL)
    return;

  for (guint i = 0; i < g_list_model_get_n_items (G_LIST_MODEL (children)); i++)
    {
      g_autoptr (HyNode) child = g_list_model_get_item (G_LIST_MODEL (children), i);
      g_autofree char *child_path = NULL;

      if (hy_node_get_kind (child) != HY_NODE_FOLDER)
        continue;

      child_path = g_build_filename (new_path, hy_node_get_name (child), NULL);
      rebase_subtree (child, child_path);
    }
}

/* True when @folder is @node or lives somewhere inside it. */
static gboolean
is_within (HyNode *folder,
           HyNode *node)
{
  for (HyNode *at = folder; at != NULL; at = hy_node_get_parent (at))
    {
      if (at == node)
        return TRUE;
    }

  return FALSE;
}

gboolean
hy_fs_tree_move_folder (HyFsTree    *self,
                        HyNode      *node,
                        HyNode      *new_parent,
                        GError     **error)
{
  g_autoptr (GFile) source = NULL;
  g_autoptr (GFile) destination = NULL;
  g_autofree char *new_path = NULL;
  const char *name;
  HyNode *old_parent;

  g_return_val_if_fail (HY_IS_FS_TREE (self), FALSE);
  g_return_val_if_fail (HY_IS_NODE (node), FALSE);
  g_return_val_if_fail (hy_node_get_kind (node) == HY_NODE_FOLDER, FALSE);

  /* NULL means the top level, which is the root node's own children. */
  if (new_parent == NULL)
    new_parent = self->root;

  old_parent = hy_node_get_parent (node);
  if (new_parent == old_parent)
    return TRUE;

  /* A folder cannot hold itself, and moving one into its own subtree would
   * take the destination along with it. */
  if (new_parent != NULL && is_within (new_parent, node))
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                   "A folder cannot be moved inside itself.");
      return FALSE;
    }

  name = hy_node_get_name (node);
  new_path = g_build_filename (hy_node_get_path (new_parent), name, NULL);

  source = g_file_new_for_path (hy_node_get_path (node));
  destination = g_file_new_for_path (new_path);

  if (g_file_query_exists (destination, NULL))
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_EXISTS,
                   "\u201c%s\u201d already has a folder called \u201c%s\u201d.",
                   new_parent == self->root ? "Workspaces" : hy_node_get_name (new_parent),
                   name);
      return FALSE;
    }

  /* A plain rename, since everything is under one root. Across filesystems
   * this fails rather than copying, which is the honest outcome: the folder
   * would no longer be the same directory the chats were written against. */
  if (!g_file_move (source, destination, G_FILE_COPY_NONE, NULL, NULL, NULL, error))
    return FALSE;

  rebase_subtree (node, new_path);

  g_object_ref (node);

  if (old_parent != NULL)
    {
      guint position;

      if (g_list_store_find (hy_node_get_children (old_parent), node, &position))
        g_list_store_remove (hy_node_get_children (old_parent), position);
    }

  hy_node_set_parent (node, new_parent);
  g_list_store_insert (hy_node_get_children (new_parent),
                       folder_insert_position (new_parent, name), node);

  g_object_unref (node);

  return TRUE;
}

gboolean
hy_fs_tree_rename_folder (HyFsTree    *self,
                          HyNode      *node,
                          const char  *new_name,
                          GError     **error)
{
  g_autoptr (GFile) file = NULL;
  g_autoptr (GFile) renamed = NULL;
  g_autofree char *new_path = NULL;
  HyNode *parent;

  g_return_val_if_fail (HY_IS_FS_TREE (self), FALSE);
  g_return_val_if_fail (HY_IS_NODE (node), FALSE);
  g_return_val_if_fail (new_name != NULL && *new_name != '\0', FALSE);

  file = g_file_new_for_path (hy_node_get_path (node));
  renamed = g_file_set_display_name (file, new_name, NULL, error);
  if (renamed == NULL)
    return FALSE;

  /* The id lives in .hy.json inside the folder, so it travels with it and the
   * folder's chats stay attached. */
  new_path = g_file_get_path (renamed);
  rebase_subtree (node, new_path);
  hy_node_set_name (node, new_name);

  /* Re-sort by lifting the node out and letting the scan put it back. */
  parent = hy_node_get_parent (node);
  if (parent != NULL)
    {
      guint position;

      if (g_list_store_find (hy_node_get_children (parent), node, &position))
        {
          g_object_ref (node);
          g_list_store_remove (hy_node_get_children (parent), position);
          g_list_store_insert (hy_node_get_children (parent),
                               folder_insert_position (parent, new_name), node);
          g_object_unref (node);
        }
    }

  return TRUE;
}

gboolean
hy_fs_tree_trash_folder (HyFsTree    *self,
                         HyNode      *node,
                         GError     **error)
{
  g_autoptr (GFile) file = NULL;
  HyNode *parent;
  guint position;

  g_return_val_if_fail (HY_IS_FS_TREE (self), FALSE);
  g_return_val_if_fail (HY_IS_NODE (node), FALSE);

  file = g_file_new_for_path (hy_node_get_path (node));
  if (!g_file_trash (file, NULL, error))
    return FALSE;

  parent = hy_node_get_parent (node);
  if (parent == NULL)
    parent = self->root;

  forget_subtree (self, node);

  if (g_list_store_find (hy_node_get_children (parent), node, &position))
    g_list_store_remove (hy_node_get_children (parent), position);

  return TRUE;
}

static HyNode *
lookup_in (HyNode     *node,
           const char *path)
{
  GListStore *children = hy_node_get_children (node);

  if (g_strcmp0 (hy_node_get_path (node), path) == 0)
    return node;

  if (children == NULL)
    return NULL;

  for (guint i = 0; i < g_list_model_get_n_items (G_LIST_MODEL (children)); i++)
    {
      g_autoptr (HyNode) child = g_list_model_get_item (G_LIST_MODEL (children), i);
      HyNode *found;

      if (hy_node_get_kind (child) != HY_NODE_FOLDER)
        continue;

      /* Only descend where the path can actually be. */
      if (!g_str_has_prefix (path, hy_node_get_path (child)))
        continue;

      found = lookup_in (child, path);
      if (found != NULL)
        return found;
    }

  return NULL;
}

HyNode *
hy_fs_tree_lookup (HyFsTree   *self,
                   const char *path)
{
  g_return_val_if_fail (HY_IS_FS_TREE (self), NULL);
  g_return_val_if_fail (path != NULL, NULL);

  return lookup_in (self->root, path);
}

/* --- chats ---------------------------------------------------------------- */

HyNode *
hy_fs_tree_create_chat (HyFsTree    *self,
                        HyNode      *folder,
                        const char  *title,
                        const char  *backend,
                        const char  *model,
                        const char  *effort,
                        const char  *workdir,
                        GError     **error)
{
  g_autofree char *chat_id = NULL;
  HyNode *node;

  g_return_val_if_fail (HY_IS_FS_TREE (self), NULL);
  g_return_val_if_fail (HY_IS_NODE (folder), NULL);
  g_return_val_if_fail (self->storage != NULL, NULL);

  if (hy_node_get_folder_id (folder) == NULL)
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                   "Chats live in a folder; pick one first");
      return NULL;
    }

  chat_id = hy_storage_create_chat (self->storage, hy_node_get_folder_id (folder),
                                    title, backend, model, effort, workdir,
                                    error);
  if (chat_id == NULL)
    return NULL;

  node = hy_node_new_chat (chat_id, title, folder);
  hy_node_set_icon_name (node, backend_icon (backend));
  g_list_store_insert (hy_node_get_children (folder),
                       chat_section_start (folder), node);
  g_object_unref (node);

  return node;
}

gboolean
hy_fs_tree_rename_chat (HyFsTree    *self,
                        HyNode      *chat,
                        const char  *title,
                        GError     **error)
{
  g_return_val_if_fail (HY_IS_FS_TREE (self), FALSE);
  g_return_val_if_fail (HY_IS_NODE (chat), FALSE);

  if (!hy_storage_set_chat_title (self->storage, hy_node_get_chat_id (chat),
                                  title, error))
    return FALSE;

  hy_node_set_name (chat, title);

  return TRUE;
}

gboolean
hy_fs_tree_delete_chat (HyFsTree    *self,
                        HyNode      *chat,
                        GError     **error)
{
  HyNode *parent;
  guint position;

  g_return_val_if_fail (HY_IS_FS_TREE (self), FALSE);
  g_return_val_if_fail (HY_IS_NODE (chat), FALSE);

  if (!hy_storage_delete_chat (self->storage, hy_node_get_chat_id (chat), error))
    return FALSE;

  parent = hy_node_get_parent (chat);

  /* Announced before the store drops it, so whoever is showing the chat is
   * still holding a node rather than being handed a dead one. */
  g_signal_emit (self, signals[SIGNAL_CHAT_REMOVED], 0, chat);

  if (parent != NULL &&
      g_list_store_find (hy_node_get_children (parent), chat, &position))
    g_list_store_remove (hy_node_get_children (parent), position);

  return TRUE;
}

void
hy_fs_tree_bump_chat (HyFsTree *self,
                      HyNode   *chat)
{
  HyNode *parent;
  guint position, target;

  g_return_if_fail (HY_IS_FS_TREE (self));
  g_return_if_fail (HY_IS_NODE (chat));

  parent = hy_node_get_parent (chat);
  if (parent == NULL)
    return;

  if (!g_list_store_find (hy_node_get_children (parent), chat, &position))
    return;

  target = chat_section_start (parent);
  if (position == target)
    return;

  g_object_ref (chat);
  g_list_store_remove (hy_node_get_children (parent), position);
  g_list_store_insert (hy_node_get_children (parent), target, chat);
  g_object_unref (chat);
}

static HyNode *
lookup_chat_in (HyNode     *node,
                const char *chat_id)
{
  GListStore *children = hy_node_get_children (node);

  if (children == NULL)
    return NULL;

  for (guint i = 0; i < g_list_model_get_n_items (G_LIST_MODEL (children)); i++)
    {
      g_autoptr (HyNode) child = g_list_model_get_item (G_LIST_MODEL (children), i);
      HyNode *found;

      if (hy_node_get_kind (child) == HY_NODE_CHAT)
        {
          if (g_strcmp0 (hy_node_get_chat_id (child), chat_id) == 0)
            return child;
          continue;
        }

      found = lookup_chat_in (child, chat_id);
      if (found != NULL)
        return found;
    }

  return NULL;
}

HyNode *
hy_fs_tree_lookup_chat (HyFsTree   *self,
                        const char *chat_id)
{
  g_return_val_if_fail (HY_IS_FS_TREE (self), NULL);
  g_return_val_if_fail (chat_id != NULL, NULL);

  return lookup_chat_in (self->root, chat_id);
}

/* --- GObject -------------------------------------------------------------- */

static void
hy_fs_tree_dispose (GObject *object)
{
  HyFsTree *self = HY_FS_TREE (object);

  g_cancellable_cancel (self->cancellable);
  g_clear_pointer (&self->watches, g_hash_table_unref);
  g_clear_object (&self->root);
  g_clear_object (&self->storage);
  g_clear_object (&self->cancellable);

  G_OBJECT_CLASS (hy_fs_tree_parent_class)->dispose (object);
}

static void
hy_fs_tree_finalize (GObject *object)
{
  HyFsTree *self = HY_FS_TREE (object);

  g_clear_pointer (&self->root_path, g_free);

  G_OBJECT_CLASS (hy_fs_tree_parent_class)->finalize (object);
}

static void
hy_fs_tree_class_init (HyFsTreeClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = hy_fs_tree_dispose;
  object_class->finalize = hy_fs_tree_finalize;

  /* Whoever is showing a chat has to hear that it is gone; there is nothing
   * left to show and anything typed into it would be written to a row that
   * no longer exists. */
  signals[SIGNAL_CHAT_REMOVED] =
    g_signal_new ("chat-removed", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, HY_TYPE_NODE);
}

static void
hy_fs_tree_init (HyFsTree *self)
{
  self->watches = g_hash_table_new_full (NULL, NULL, NULL, watch_free);
  self->cancellable = g_cancellable_new ();
}
