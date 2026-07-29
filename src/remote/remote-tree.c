#include "remote-tree.h"

#include "backend/backend.h"

#include <stdlib.h>

/*
 * The daemon's answer to "tree", turned into nodes and kept that way.
 *
 * Folders and chats arrive flat, each naming its parent, and are reconciled
 * against what is already on screen rather than rebuilt: a row that did not
 * change keeps its node, so the folder the user had open stays open and the
 * chat they are reading stays selected.
 */

#define REMOTE_URI_SCHEME "xd://"

struct _XdRemoteTree
{
  GObject parent_instance;

  XdRemoteClient *client;
  XdNode *root;
  GListStore *roots;        /* the root, as one row */

  GHashTable *folders;      /* folder id -> XdNode*, owning a reference */
  GHashTable *chats;        /* chat id   -> XdNode*, owning a reference */
  gboolean loaded;          /* at least one complete tree snapshot arrived */

  /* A chat just made on the daemon, to be handed over once the tree it lives
   * in has been read back and there is a node to hand over. */
  char *opening;

  GCancellable *cancellable;
};

enum
{
  SIGNAL_LOADED,
  SIGNAL_FAILED,
  SIGNAL_CHAT_CREATED,
  SIGNAL_CHAT_REMOVED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (XdRemoteTree, xd_remote_tree, G_TYPE_OBJECT)

static void set_root_state (XdRemoteTree *self, XdNodeState state);

/* --- reconciling ---------------------------------------------------------- */

/*
 * A row with no daemon id is an inline editor owned by the client.
 *
 * The sidebar inserts one while a new folder or chat is being named. A tree
 * reply knows nothing about that placeholder, so treating the reply as an
 * exhaustive list would remove the entry while the user is typing.
 */
static gboolean
is_client_placeholder (XdNode *node)
{
  return xd_node_get_kind (node) == XD_NODE_FOLDER
    ? xd_node_get_folder_id (node) == NULL
    : xd_node_get_chat_id (node) == NULL;
}

/*
 * Brings @store to exactly @desired, moving as little as possible.
 *
 * Positions before the one being looked at already match, so a node that is
 * already where it belongs is never removed -- and a row that is not removed
 * is a row that keeps its expansion and its place in the selection.
 */
static void
reconcile_children (GListStore *store,
                    GPtrArray  *desired)
{
  GListModel *model = G_LIST_MODEL (store);
  g_autoptr (GPtrArray) target =
    g_ptr_array_new_with_free_func (g_object_unref);

  for (guint i = 0; i < desired->len; i++)
    g_ptr_array_add (target, g_object_ref (g_ptr_array_index (desired, i)));

  /*
   * Keep client placeholders at their current positions while reconciling
   * every daemon-owned row around them. Their row, entry text and focus then
   * survive a refresh that finishes while the user is naming something.
   */
  for (guint i = 0; i < g_list_model_get_n_items (model); i++)
    {
      g_autoptr (XdNode) node = g_list_model_get_item (model, i);

      if (is_client_placeholder (node))
        g_ptr_array_insert (target, MIN (i, target->len),
                            g_steal_pointer (&node));
    }

  for (guint i = 0; i < target->len; i++)
    {
      XdNode *wanted = g_ptr_array_index (target, i);
      guint at;

      if (i < g_list_model_get_n_items (model))
        {
          g_autoptr (XdNode) here = g_list_model_get_item (model, i);

          if (here == wanted)
            continue;
        }

      if (g_list_store_find (store, wanted, &at))
        g_list_store_remove (store, at);

      g_list_store_insert (store, i, wanted);
    }

  while (g_list_model_get_n_items (model) > target->len)
    g_list_store_remove (store, target->len);
}

static int
by_name (gconstpointer a,
         gconstpointer b)
{
  XdNode *first = *(XdNode * const *) a;
  XdNode *second = *(XdNode * const *) b;

  return g_utf8_collate (xd_node_get_name (first), xd_node_get_name (second));
}

typedef struct
{
  XdRemoteTree *tree;
  GHashTable *folders;      /* folder id -> XdNode*, this reply's folders */
  GHashTable *children;     /* XdNode* parent -> GPtrArray* of children */

  /* Chats the daemon no longer has, held alive long enough to say so: whoever
   * is reading one has to hear that it is gone. */
  GPtrArray *removed;
} Reload;

static GPtrArray *
children_of (Reload *reload,
             XdNode *parent)
{
  GPtrArray *children = g_hash_table_lookup (reload->children, parent);

  if (children == NULL)
    {
      children = g_ptr_array_new ();
      g_hash_table_insert (reload->children, parent, children);
    }

  return children;
}

static char *
folder_uri (XdRemoteTree *self,
            const char   *folder_id)
{
  return g_strdup_printf ("%s%s:%u/%s", REMOTE_URI_SCHEME,
                          xd_remote_client_get_host (self->client),
                          xd_remote_client_get_port (self->client),
                          folder_id);
}

/* The icon of the assistant a chat is set to, as the local tree shows it. */
static const char *
backend_icon (const char *backend_id)
{
  const AiBackend *backend = ai_backend_lookup (backend_id);

  return backend != NULL ? backend->icon_name : NULL;
}

static const char *
member_string (JsonObject *row,
               const char *name)
{
  return json_object_get_string_member_with_default (row, name, NULL);
}

static void
read_folders (Reload    *reload,
              JsonArray *rows)
{
  XdRemoteTree *self = reload->tree;

  /* Two passes over the same array: every folder has to exist as a node before
   * the parents naming them can be resolved, and the daemon is under no
   * obligation to have listed them in that order. */
  for (guint i = 0; rows != NULL && i < json_array_get_length (rows); i++)
    {
      JsonObject *row = json_array_get_object_element (rows, i);
      const char *id = member_string (row, "id");
      const char *name = member_string (row, "name");
      XdNode *node;

      if (id == NULL)
        continue;

      node = g_hash_table_lookup (self->folders, id);
      if (node == NULL)
        {
          g_autofree char *uri = folder_uri (self, id);

          node = xd_node_new_folder (uri, name, id);
          g_hash_table_insert (self->folders, g_strdup (id), node);
        }
      else
        {
          xd_node_set_name (node, name);
        }

      g_hash_table_insert (reload->folders, (gpointer) id, node);
    }

  for (guint i = 0; rows != NULL && i < json_array_get_length (rows); i++)
    {
      JsonObject *row = json_array_get_object_element (rows, i);
      const char *id = member_string (row, "id");
      const char *parent_id = member_string (row, "parent");
      XdNode *node = id != NULL ? g_hash_table_lookup (reload->folders, id) : NULL;
      XdNode *parent = self->root;

      if (node == NULL)
        continue;

      if (parent_id != NULL)
        {
          parent = g_hash_table_lookup (reload->folders, parent_id);

          /* A folder whose parent the daemon did not send would otherwise
           * disappear; showing it at the top is better than losing it. */
          if (parent == NULL)
            parent = self->root;
        }

      xd_node_set_parent (node, parent);
      g_ptr_array_add (children_of (reload, parent), node);
    }
}

static void
read_chats (Reload    *reload,
            JsonArray *rows)
{
  XdRemoteTree *self = reload->tree;
  g_autoptr (GHashTable) seen = g_hash_table_new (g_str_hash, g_str_equal);
  GHashTableIter iter;
  gpointer id, node;

  for (guint i = 0; rows != NULL && i < json_array_get_length (rows); i++)
    {
      JsonObject *row = json_array_get_object_element (rows, i);
      const char *chat_id = member_string (row, "id");
      const char *folder_id = member_string (row, "folder");
      const char *title = member_string (row, "title");
      const char *backend = member_string (row, "backend");
      gboolean working =
        json_object_get_boolean_member_with_default (row, "working", FALSE);
      XdNode *folder = folder_id != NULL
        ? g_hash_table_lookup (reload->folders, folder_id) : NULL;
      XdNode *chat;

      /* A chat whose folder is not in the tree has nowhere to be drawn. */
      if (chat_id == NULL || folder == NULL)
        continue;

      chat = g_hash_table_lookup (self->chats, chat_id);
      if (chat == NULL)
        {
          chat = xd_node_new_chat (chat_id, title, folder);
          g_hash_table_insert (self->chats, g_strdup (chat_id), chat);
        }
      else
        {
          xd_node_set_name (chat, title);
          xd_node_set_parent (chat, folder);
        }

      xd_node_set_icon_name (chat, backend_icon (backend));
      if (working)
        xd_node_set_state (chat, XD_NODE_WORKING);
      else if (xd_node_get_state (chat) == XD_NODE_WORKING)
        xd_node_set_state (chat, XD_NODE_IDLE);

      /* The daemon lists a folder's chats most-recent-first, which is the
       * order the sidebar shows them in, and they sort after its folders. */
      g_ptr_array_add (children_of (reload, folder), chat);
      g_hash_table_add (seen, (gpointer) chat_id);
    }

  g_hash_table_iter_init (&iter, self->chats);
  while (g_hash_table_iter_next (&iter, &id, &node))
    {
      if (g_hash_table_contains (seen, id))
        continue;

      g_ptr_array_add (reload->removed, g_object_ref (node));
      g_hash_table_iter_remove (&iter);
    }
}

/* Folders the daemon no longer has, and everything remembered about them. */
static void
forget_missing_folders (Reload *reload)
{
  GHashTableIter iter;
  gpointer id, node;

  g_hash_table_iter_init (&iter, reload->tree->folders);
  while (g_hash_table_iter_next (&iter, &id, &node))
    {
      if (!g_hash_table_contains (reload->folders, id))
        g_hash_table_iter_remove (&iter);
    }
}

static void
apply_tree (XdRemoteTree *self,
            JsonObject   *reply)
{
  Reload reload = {
    .tree = self,
    .folders = g_hash_table_new (g_str_hash, g_str_equal),
    .children = g_hash_table_new_full (NULL, NULL, NULL,
                                       (GDestroyNotify) g_ptr_array_unref),
    .removed = g_ptr_array_new_with_free_func (g_object_unref),
  };
  GHashTableIter iter;
  gpointer folder_id, folder;

  read_folders (&reload, json_object_has_member (reply, "folders")
                         ? json_object_get_array_member (reply, "folders") : NULL);
  read_chats (&reload, json_object_has_member (reply, "chats")
                       ? json_object_get_array_member (reply, "chats") : NULL);
  forget_missing_folders (&reload);

  /*
   * Folders first and alphabetically, then the chats in the order they came.
   *
   * The chats keep that order rather than being sorted: the daemon lists them
   * most-recently-used first, which is what the sidebar shows for local ones,
   * and folders were added to each group before any chat was, so sorting the
   * leading run of folders is enough.
   */
  {
    GHashTableIter groups;
    gpointer parent, children;

    g_hash_table_iter_init (&groups, reload.children);
    while (g_hash_table_iter_next (&groups, &parent, &children))
      {
        GPtrArray *rows = children;
        guint folders = 0;

        while (folders < rows->len &&
               xd_node_get_kind (g_ptr_array_index (rows, folders)) == XD_NODE_FOLDER)
          folders++;

        qsort (rows->pdata, folders, sizeof (gpointer), by_name);
      }
  }

  reconcile_children (xd_node_get_children (self->root),
                      children_of (&reload, self->root));

  g_hash_table_iter_init (&iter, self->folders);
  while (g_hash_table_iter_next (&iter, &folder_id, &folder))
    reconcile_children (xd_node_get_children (folder),
                        children_of (&reload, folder));

  /* Out of the tree by now, and still alive because this array holds them:
   * whoever is showing one is holding a node, not being handed a dead one. */
  for (guint i = 0; i < reload.removed->len; i++)
    g_signal_emit (self, signals[SIGNAL_CHAT_REMOVED], 0,
                   g_ptr_array_index (reload.removed, i));

  g_hash_table_unref (reload.folders);
  g_hash_table_unref (reload.children);
  g_ptr_array_unref (reload.removed);

  self->loaded = TRUE;
  set_root_state (self, XD_NODE_IDLE);

  /* The chat that was just made now has a row, which is the first moment it
   * can be opened. */
  if (self->opening != NULL)
    {
      g_autofree char *chat_id = g_steal_pointer (&self->opening);
      XdNode *chat = g_hash_table_lookup (self->chats, chat_id);

      if (chat != NULL)
        g_signal_emit (self, signals[SIGNAL_CHAT_CREATED], 0, chat);
    }

  g_signal_emit (self, signals[SIGNAL_LOADED], 0);
}

/* --- fetching ------------------------------------------------------------- */

/*
 * How the remote's own row shows what the connection is doing.
 *
 * The icon changes with it rather than only the colour: a row that has gone
 * red is easy to miss on a tree the user is not looking at, and a plug pulled
 * out of a socket says the same thing at a glance that "offline" says in
 * words.
 */
static void
set_root_state (XdRemoteTree *self,
                XdNodeState   state)
{
  gboolean offline = state == XD_NODE_OFFLINE;

  xd_node_set_icon_name (self->root, offline ? "network-offline-symbolic"
                                             : "network-server-symbolic");
  xd_node_set_state (self->root, state);
}

static void
on_tree_received (GObject      *source,
                  GAsyncResult *result,
                  gpointer      user_data)
{
  g_autoptr (XdRemoteTree) self = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (reply == NULL)
    {
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        g_warning ("cannot read the tree of %s: %s",
                   xd_node_get_name (self->root), error->message);

      /* The connection will come back on its own and ask again; until then the
       * remote shows whatever it last had rather than pretending to work. */
      set_root_state (self, xd_remote_client_is_connected (self->client)
                            ? XD_NODE_IDLE : XD_NODE_OFFLINE);
      return;
    }

  apply_tree (self, reply);
}

void
xd_remote_tree_refresh (XdRemoteTree *self)
{
  g_return_if_fail (XD_IS_REMOTE_TREE (self));

  if (!xd_remote_client_is_connected (self->client))
    {
      set_root_state (self, XD_NODE_OFFLINE);
      return;
    }

  /* The remote's own row says it is working only before its first snapshot.
   * Later refreshes are background reconciliation: swapping the stable server
   * icon for animated dots on every tree event makes the connection row flash.
   * After a disconnect it also stays offline until a reply confirms recovery. */
  if (!self->loaded)
    set_root_state (self, XD_NODE_WORKING);

  xd_remote_client_call_op_async (self->client, "tree", NULL, NULL,
                                  self->cancellable, on_tree_received,
                                  g_object_ref (self));
}

/* --- asking the daemon to change something --------------------------------- */

typedef struct
{
  XdRemoteTree *tree;
  char *heading;        /* what to say if the daemon says no */
  gboolean opens_chat;  /* the answer names a chat that should be opened */
  XdNode *renamed_node; /* optimistic rename to undo if the daemon says no */
  char *old_name;
} Intent;

static void
intent_free (Intent *intent)
{
  g_clear_object (&intent->tree);
  g_clear_object (&intent->renamed_node);
  g_free (intent->heading);
  g_free (intent->old_name);
  g_free (intent);
}

static void
on_intent_answered (GObject      *source,
                    GAsyncResult *result,
                    gpointer      user_data)
{
  Intent *intent = user_data;
  XdRemoteTree *self = intent->tree;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);

  if (reply == NULL)
    {
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        {
          if (intent->renamed_node != NULL)
            xd_node_set_name (intent->renamed_node, intent->old_name);

          g_signal_emit (self, signals[SIGNAL_FAILED], 0, intent->heading,
                         error->message);
        }

      intent_free (intent);
      return;
    }

  if (intent->opens_chat)
    {
      g_free (self->opening);
      self->opening = g_strdup (member_string (reply, "id"));
    }

  /* Nothing is read back here: the daemon broadcasts what changed, and this
   * client hears it on the same connection like every other device. Doing it
   * both ways would be two answers to the same question. */
  intent_free (intent);
}

static gboolean
send_intent (XdRemoteTree *self,
             JsonBuilder  *builder,
             const char   *heading,
             gboolean      opens_chat,
             XdNode       *renamed_node)
{
  g_autoptr (JsonNode) request = json_builder_get_root (builder);
  Intent *intent;

  if (!xd_remote_client_is_connected (self->client))
    {
      g_signal_emit (self, signals[SIGNAL_FAILED], 0, heading,
                     "The daemon is not connected.");
      return FALSE;
    }

  intent = g_new0 (Intent, 1);
  intent->tree = g_object_ref (self);
  intent->heading = g_strdup (heading);
  intent->opens_chat = opens_chat;
  if (renamed_node != NULL)
    {
      intent->renamed_node = g_object_ref (renamed_node);
      intent->old_name = g_strdup (xd_node_get_name (renamed_node));
    }

  xd_remote_client_call_async (self->client, request, self->cancellable,
                               on_intent_answered, intent);
  return TRUE;
}

/* An op naming the folder or chat it acts on, which is most of them. A NULL
 * @subject means the top level of the remote. */
static JsonBuilder *
intent_for (const char *op,
            const char *subject_name,
            XdNode     *subject)
{
  JsonBuilder *builder = json_builder_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, op);

  if (subject != NULL && xd_node_get_kind (subject) == XD_NODE_FOLDER &&
      xd_node_get_folder_id (subject) != NULL)
    {
      json_builder_set_member_name (builder, subject_name);
      json_builder_add_string_value (builder, xd_node_get_folder_id (subject));
    }
  else if (subject != NULL && xd_node_get_kind (subject) == XD_NODE_CHAT)
    {
      json_builder_set_member_name (builder, subject_name);
      json_builder_add_string_value (builder, xd_node_get_chat_id (subject));
    }

  return builder;
}

/*
 * The remote's own root stands for the top level, not for a folder.
 *
 * Its id is the URI it is drawn with rather than anything the daemon knows, so
 * passing it on would name a folder that does not exist. Left out instead,
 * which is how the daemon spells "the workspace root".
 */
static XdNode *
folder_argument (XdRemoteTree *self,
                 XdNode       *folder)
{
  return folder == self->root ? NULL : folder;
}

void
xd_remote_tree_create_folder (XdRemoteTree *self,
                              XdNode       *parent,
                              const char   *name)
{
  g_autoptr (JsonBuilder) builder = NULL;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (name != NULL && *name != '\0');

  builder = intent_for ("new-folder", "parent", folder_argument (self, parent));
  json_builder_set_member_name (builder, "name");
  json_builder_add_string_value (builder, name);
  json_builder_end_object (builder);

  send_intent (self, builder, "Could not create the folder", FALSE, NULL);
}

void
xd_remote_tree_rename_folder (XdRemoteTree *self,
                              XdNode       *folder,
                              const char   *name)
{
  g_autoptr (JsonBuilder) builder = NULL;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (XD_IS_NODE (folder));
  g_return_if_fail (xd_node_get_folder_id (folder) != NULL);
  g_return_if_fail (name != NULL && *name != '\0');

  builder = intent_for ("rename-folder", "folder", folder);
  json_builder_set_member_name (builder, "name");
  json_builder_add_string_value (builder, name);
  json_builder_end_object (builder);

  if (send_intent (
        self, builder, "Could not rename the folder", FALSE, folder))
    xd_node_set_name (folder, name);
}

void
xd_remote_tree_move_folder (XdRemoteTree *self,
                            XdNode       *folder,
                            XdNode       *new_parent)
{
  g_autoptr (JsonBuilder) builder = NULL;
  XdNode *parent = folder_argument (self, new_parent);

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (XD_IS_NODE (folder));

  builder = intent_for ("move-folder", "folder", folder);

  if (parent != NULL)
    {
      json_builder_set_member_name (builder, "parent");
      json_builder_add_string_value (builder, xd_node_get_folder_id (parent));
    }

  json_builder_end_object (builder);

  send_intent (self, builder, "Cannot Move the Folder", FALSE, NULL);
}

void
xd_remote_tree_trash_folder (XdRemoteTree *self,
                             XdNode       *folder)
{
  g_autoptr (JsonBuilder) builder = NULL;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (XD_IS_NODE (folder));

  builder = intent_for ("trash-folder", "folder", folder);
  json_builder_end_object (builder);

  send_intent (
    self, builder, "Could not move the folder to the trash", FALSE, NULL);
}

void
xd_remote_tree_get_folder_context_async (XdRemoteTree        *self,
                                         XdNode              *folder,
                                         GCancellable        *cancellable,
                                         GAsyncReadyCallback  callback,
                                         gpointer             user_data)
{
  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (XD_IS_NODE (folder));

  xd_remote_client_call_op_async (
    self->client, "folder-context", "folder",
    xd_node_get_folder_id (folder), cancellable, callback, user_data);
}

gboolean
xd_remote_tree_get_folder_context_finish (XdRemoteTree  *self,
                                          GAsyncResult  *result,
                                          char         **context,
                                          GError       **error)
{
  g_autoptr (JsonObject) reply = NULL;
  JsonNode *node;

  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), FALSE);
  g_return_val_if_fail (context != NULL, FALSE);

  *context = NULL;
  reply = xd_remote_client_call_finish (self->client, result, error);
  if (reply == NULL)
    return FALSE;

  node = json_object_get_member (reply, "context");
  if (node == NULL)
    {
      g_set_error_literal (error, XD_REMOTE_ERROR,
                           XD_REMOTE_ERROR_PROTOCOL,
                           "The daemon omitted folder context.");
      return FALSE;
    }

  if (!JSON_NODE_HOLDS_NULL (node))
    {
      if (!JSON_NODE_HOLDS_VALUE (node) ||
          json_node_get_value_type (node) != G_TYPE_STRING)
        {
          g_set_error_literal (error, XD_REMOTE_ERROR,
                               XD_REMOTE_ERROR_PROTOCOL,
                               "The daemon sent invalid folder context.");
          return FALSE;
        }

      *context = json_node_dup_string (node);
    }

  return TRUE;
}

void
xd_remote_tree_set_folder_context_async (XdRemoteTree        *self,
                                         XdNode              *folder,
                                         const char          *context,
                                         GCancellable        *cancellable,
                                         GAsyncReadyCallback  callback,
                                         gpointer             user_data)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (XD_IS_NODE (folder));
  g_return_if_fail (xd_node_get_folder_id (folder) != NULL);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "set-folder-context");
  json_builder_set_member_name (builder, "folder");
  json_builder_add_string_value (builder, xd_node_get_folder_id (folder));
  json_builder_set_member_name (builder, "context");
  if (context != NULL)
    json_builder_add_string_value (builder, context);
  else
    json_builder_add_null_value (builder);
  json_builder_end_object (builder);
  request = json_builder_get_root (builder);

  xd_remote_client_call_async (self->client, request, cancellable,
                               callback, user_data);
}

gboolean
xd_remote_tree_set_folder_context_finish (XdRemoteTree *self,
                                          GAsyncResult *result,
                                          GError      **error)
{
  g_autoptr (JsonObject) reply = NULL;

  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), FALSE);

  reply = xd_remote_client_call_finish (self->client, result, error);
  return reply != NULL;
}

void
xd_remote_tree_get_agent_secrets_async (XdRemoteTree        *self,
                                        XdNode              *folder,
                                        GCancellable        *cancellable,
                                        GAsyncReadyCallback  callback,
                                        gpointer             user_data)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (folder == NULL || XD_IS_NODE (folder));

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "agent-secrets");
  if (folder != NULL)
    {
      json_builder_set_member_name (builder, "folder");
      json_builder_add_string_value (builder, xd_node_get_folder_id (folder));
    }
  json_builder_end_object (builder);
  request = json_builder_get_root (builder);

  xd_remote_client_call_async (self->client, request, cancellable,
                               callback, user_data);
}

GStrv
xd_remote_tree_get_agent_secrets_finish (XdRemoteTree  *self,
                                         GAsyncResult  *result,
                                         GError       **error)
{
  g_autoptr (JsonObject) reply = NULL;
  JsonNode *names_node;
  JsonArray *names;
  GStrv values;

  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), NULL);

  reply = xd_remote_client_call_finish (self->client, result, error);
  if (reply == NULL)
    return NULL;

  names_node = json_object_get_member (reply, "names");
  if (names_node == NULL || !JSON_NODE_HOLDS_ARRAY (names_node))
    {
      g_set_error_literal (error, XD_REMOTE_ERROR,
                           XD_REMOTE_ERROR_PROTOCOL,
                           "The daemon omitted agent secret names.");
      return NULL;
    }

  names = json_node_get_array (names_node);
  values = g_new0 (char *, json_array_get_length (names) + 1);
  for (guint i = 0; i < json_array_get_length (names); i++)
    {
      JsonNode *name = json_array_get_element (names, i);

      if (!JSON_NODE_HOLDS_VALUE (name) ||
          json_node_get_value_type (name) != G_TYPE_STRING)
        {
          g_strfreev (values);
          g_set_error_literal (error, XD_REMOTE_ERROR,
                               XD_REMOTE_ERROR_PROTOCOL,
                               "The daemon sent an invalid secret name.");
          return NULL;
        }

      values[i] = json_node_dup_string (name);
    }

  return values;
}

void
xd_remote_tree_set_agent_secrets_async (
                                       XdRemoteTree              *self,
                                       XdNode                    *folder,
                                       const XdAgentSecretUpdate *entries,
                                       gsize                      n_entries,
                                       GCancellable              *cancellable,
                                       GAsyncReadyCallback        callback,
                                       gpointer                   user_data)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (folder == NULL || XD_IS_NODE (folder));
  g_return_if_fail (entries != NULL || n_entries == 0);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "set-agent-secrets");
  if (folder != NULL)
    {
      json_builder_set_member_name (builder, "folder");
      json_builder_add_string_value (builder, xd_node_get_folder_id (folder));
    }
  json_builder_set_member_name (builder, "entries");
  json_builder_begin_array (builder);
  for (gsize i = 0; i < n_entries; i++)
    {
      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "name");
      json_builder_add_string_value (builder, entries[i].name);
      if (entries[i].value != NULL)
        {
          json_builder_set_member_name (builder, "value");
          json_builder_add_string_value (builder, entries[i].value);
        }
      json_builder_end_object (builder);
    }
  json_builder_end_array (builder);
  json_builder_end_object (builder);
  request = json_builder_get_root (builder);

  xd_remote_client_call_async (self->client, request, cancellable,
                               callback, user_data);
}

gboolean
xd_remote_tree_set_agent_secrets_finish (XdRemoteTree  *self,
                                         GAsyncResult  *result,
                                         GError       **error)
{
  g_autoptr (JsonObject) reply = NULL;

  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), FALSE);

  reply = xd_remote_client_call_finish (self->client, result, error);
  return reply != NULL;
}

void
xd_remote_tree_create_chat (XdRemoteTree *self,
                            XdNode       *folder,
                            const char   *title,
                            const char   *workdir)
{
  g_autoptr (JsonBuilder) builder = NULL;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (XD_IS_NODE (folder));

  builder = intent_for ("new-chat", "folder", folder);
  json_builder_set_member_name (builder, "title");
  json_builder_add_string_value (builder, title != NULL ? title : "New Chat");

  if (workdir != NULL && *workdir != '\0')
    {
      json_builder_set_member_name (builder, "workdir");
      json_builder_add_string_value (builder, workdir);
    }

  json_builder_end_object (builder);

  send_intent (self, builder, "Could not start the chat", TRUE, NULL);
}

/* --- browsing the daemon's directories -------------------------------------- */

typedef struct
{
  XdRemoteDirFunc callback;
  gpointer user_data;
} Listing;

static void
on_dir_listed (GObject      *source,
               GAsyncResult *result,
               gpointer      user_data)
{
  Listing *listing = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;
  g_autoptr (GPtrArray) entries = g_ptr_array_new_with_free_func (g_free);
  JsonArray *rows;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (reply == NULL)
    {
      listing->callback (NULL, NULL, error->message, listing->user_data);
      g_free (listing);
      return;
    }

  rows = json_object_has_member (reply, "entries")
    ? json_object_get_array_member (reply, "entries") : NULL;

  for (guint i = 0; rows != NULL && i < json_array_get_length (rows); i++)
    g_ptr_array_add (entries, g_strdup (json_array_get_string_element (rows, i)));

  g_ptr_array_add (entries, NULL);

  listing->callback (member_string (reply, "path"),
                     (const char *const *) entries->pdata, NULL,
                     listing->user_data);

  g_free (listing);
}

void
xd_remote_tree_list_dir (XdRemoteTree    *self,
                         const char      *path,
                         GCancellable    *cancellable,
                         XdRemoteDirFunc  callback,
                         gpointer         user_data)
{
  Listing *listing;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (callback != NULL);

  listing = g_new0 (Listing, 1);
  listing->callback = callback;
  listing->user_data = user_data;

  xd_remote_client_call_op_async (self->client, "list-dir", "path", path,
                                  cancellable != NULL ? cancellable
                                                      : self->cancellable,
                                  on_dir_listed, listing);
}

void
xd_remote_tree_rename_chat (XdRemoteTree *self,
                            XdNode       *chat,
                            const char   *title)
{
  g_autoptr (JsonBuilder) builder = NULL;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (XD_IS_NODE (chat));
  g_return_if_fail (title != NULL && *title != '\0');

  builder = intent_for ("rename-chat", "chat", chat);
  json_builder_set_member_name (builder, "title");
  json_builder_add_string_value (builder, title);
  json_builder_end_object (builder);

  if (send_intent (
        self, builder, "Could not rename the chat", FALSE, chat))
    xd_node_set_name (chat, title);
}

void
xd_remote_tree_delete_chat (XdRemoteTree *self,
                            XdNode       *chat)
{
  g_autoptr (JsonBuilder) builder = NULL;

  g_return_if_fail (XD_IS_REMOTE_TREE (self));
  g_return_if_fail (XD_IS_NODE (chat));

  builder = intent_for ("delete-chat", "chat", chat);
  json_builder_end_object (builder);

  send_intent (self, builder, "Could not delete the chat", FALSE, NULL);
}

/*
 * What the daemon says while nobody asked.
 *
 * The tree is read again when it says it changed -- by another device, or by
 * the window open on the daemon's own screen, which writes to the same
 * database and is no different from here. A chat that is working says so on
 * its row, the way a local one does, because from the sidebar there is no
 * difference worth drawing.
 */
static void
on_client_event (XdRemoteClient *client,
                 JsonObject     *event,
                 gpointer        user_data)
{
  XdRemoteTree *self = user_data;
  const char *name = member_string (event, "event");
  const char *chat_id = member_string (event, "chat");
  XdNode *chat = chat_id != NULL
    ? g_hash_table_lookup (self->chats, chat_id) : NULL;

  if (g_strcmp0 (name, "tree") == 0)
    {
      xd_remote_tree_refresh (self);
      return;
    }

  if (chat == NULL)
    return;

  if (g_strcmp0 (name, "turn-started") == 0)
    xd_node_set_state (chat, XD_NODE_WORKING);
  else if (g_strcmp0 (name, "turn-finished") == 0)
    {
      gboolean waiting =
        json_object_get_boolean_member_with_default (event, "waiting", FALSE);

      xd_node_set_state (
        chat,
        waiting ? XD_NODE_WAITING
        : xd_node_is_active (chat) ? XD_NODE_IDLE
                                   : XD_NODE_DONE);
    }
}

static void
on_client_opened (XdRemoteClient *client,
                  gpointer        user_data)
{
  XdRemoteTree *self = user_data;

  xd_remote_tree_refresh (self);
}

static void
on_client_closed (XdRemoteClient *client,
                  const char     *reason,
                  gpointer        user_data)
{
  XdRemoteTree *self = user_data;

  /* The rows stay as they were -- they are what the daemon last said, and it
   * is worth being able to read them while it is away. What changes is the
   * remote's own row, which stops claiming to be a live view. */
  set_root_state (self, XD_NODE_OFFLINE);
}

/* --- public API ----------------------------------------------------------- */

XdRemoteTree *
xd_remote_tree_new (XdRemoteClient *client)
{
  XdRemoteTree *self;
  g_autofree char *uri = NULL;

  g_return_val_if_fail (XD_IS_REMOTE_CLIENT (client), NULL);

  self = g_object_new (XD_TYPE_REMOTE_TREE, NULL);
  self->client = g_object_ref (client);

  uri = g_strdup_printf ("%s%s:%u/", REMOTE_URI_SCHEME,
                         xd_remote_client_get_host (client),
                         xd_remote_client_get_port (client));

  /* The URI stands in for the folder id as well, so the sidebar can remember
   * whether the remote was left open without it ever colliding with a real
   * folder's id. */
  self->root = xd_node_new_folder (uri, xd_remote_client_get_host (client), uri);

  g_list_store_append (self->roots, self->root);

  g_signal_connect (client, "opened", G_CALLBACK (on_client_opened), self);
  g_signal_connect (client, "closed", G_CALLBACK (on_client_closed), self);
  g_signal_connect (client, "event", G_CALLBACK (on_client_event), self);

  /* Already up, when the tree is made for a client that has been paired this
   * moment rather than one about to connect. Otherwise the remote starts the
   * way it will look until the first greeting lands: not answering. */
  if (xd_remote_client_is_connected (client))
    xd_remote_tree_refresh (self);
  else
    set_root_state (self, XD_NODE_OFFLINE);

  return self;
}

XdRemoteClient *
xd_remote_tree_get_client (XdRemoteTree *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), NULL);

  return self->client;
}

XdNode *
xd_remote_tree_get_root (XdRemoteTree *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), NULL);

  return self->root;
}

GListModel *
xd_remote_tree_get_model (XdRemoteTree *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), NULL);

  return G_LIST_MODEL (self->roots);
}

XdNode *
xd_remote_tree_lookup_chat (XdRemoteTree *self,
                            const char   *chat_id)
{
  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), NULL);
  g_return_val_if_fail (chat_id != NULL, NULL);

  return g_hash_table_lookup (self->chats, chat_id);
}

XdNode *
xd_remote_tree_lookup (XdRemoteTree *self,
                       const char   *path)
{
  GHashTableIter iter;
  gpointer id, node;

  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), NULL);
  g_return_val_if_fail (path != NULL, NULL);

  if (g_strcmp0 (xd_node_get_path (self->root), path) == 0)
    return self->root;

  g_hash_table_iter_init (&iter, self->folders);
  while (g_hash_table_iter_next (&iter, &id, &node))
    {
      if (g_strcmp0 (xd_node_get_path (node), path) == 0)
        return node;
    }

  return NULL;
}

gboolean
xd_remote_tree_owns (XdRemoteTree *self,
                     XdNode       *node)
{
  g_return_val_if_fail (XD_IS_REMOTE_TREE (self), FALSE);

  for (XdNode *at = node; at != NULL; at = xd_node_get_parent (at))
    {
      if (at == self->root)
        return TRUE;
    }

  return FALSE;
}

gboolean
xd_remote_tree_is_remote_path (const char *path)
{
  return path != NULL && g_str_has_prefix (path, REMOTE_URI_SCHEME);
}

/* --- GObject -------------------------------------------------------------- */

static void
xd_remote_tree_dispose (GObject *object)
{
  XdRemoteTree *self = XD_REMOTE_TREE (object);

  g_cancellable_cancel (self->cancellable);

  if (self->client != NULL)
    g_signal_handlers_disconnect_by_data (self->client, self);

  g_clear_pointer (&self->opening, g_free);
  g_clear_pointer (&self->folders, g_hash_table_unref);
  g_clear_pointer (&self->chats, g_hash_table_unref);
  g_clear_object (&self->roots);
  g_clear_object (&self->root);
  g_clear_object (&self->client);
  g_clear_object (&self->cancellable);

  G_OBJECT_CLASS (xd_remote_tree_parent_class)->dispose (object);
}

static void
xd_remote_tree_class_init (XdRemoteTreeClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = xd_remote_tree_dispose;

  /* The tree now matches what the daemon last said. */
  signals[SIGNAL_LOADED] =
    g_signal_new ("loaded", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 0);

  /* The daemon would not do something that was asked of it, with a heading and
   * what it said. Nothing here changed, so this is the whole outcome. */
  signals[SIGNAL_FAILED] =
    g_signal_new ("failed", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 2,
                  G_TYPE_STRING, G_TYPE_STRING);

  /* A chat made here now exists in the tree, and is the one to open: making a
   * chat is only ever the first half of starting one. */
  signals[SIGNAL_CHAT_CREATED] =
    g_signal_new ("chat-created", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, XD_TYPE_NODE);

  /* A chat the daemon no longer has -- deleted from here, or from another
   * device. Whoever is showing it is showing something that is gone. */
  signals[SIGNAL_CHAT_REMOVED] =
    g_signal_new ("chat-removed", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, XD_TYPE_NODE);
}

static void
xd_remote_tree_init (XdRemoteTree *self)
{
  self->roots = g_list_store_new (XD_TYPE_NODE);
  self->folders = g_hash_table_new_full (g_str_hash, g_str_equal,
                                         g_free, g_object_unref);
  self->chats = g_hash_table_new_full (g_str_hash, g_str_equal,
                                       g_free, g_object_unref);
  self->cancellable = g_cancellable_new ();
}
