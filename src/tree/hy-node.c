#include "hy-node.h"

struct _HyNode
{
  GObject parent_instance;

  HyNodeKind kind;
  char *name;
  char *path;
  char *folder_id;
  char *chat_id;
  char *icon_name;
  HyNodeState state;

  GListStore *children;   /* folders only */
  HyNode *parent;         /* weak */
};

enum
{
  PROP_0,
  PROP_NAME,
  PROP_ICON_NAME,
  PROP_STATE,
  N_PROPS,
};

static GParamSpec *properties[N_PROPS];

G_DEFINE_FINAL_TYPE (HyNode, hy_node, G_TYPE_OBJECT)

static void
hy_node_get_property (GObject    *object,
                      guint       prop_id,
                      GValue     *value,
                      GParamSpec *pspec)
{
  HyNode *self = HY_NODE (object);

  switch (prop_id)
    {
    case PROP_NAME:
      g_value_set_string (value, self->name);
      break;
    case PROP_ICON_NAME:
      g_value_set_string (value, hy_node_get_icon_name (self));
      break;
    case PROP_STATE:
      g_value_set_int (value, self->state);
      break;
    default:
      G_OBJECT_WARN_INVALID_PROPERTY_ID (object, prop_id, pspec);
    }
}

static void
hy_node_finalize (GObject *object)
{
  HyNode *self = HY_NODE (object);

  if (self->parent != NULL)
    g_object_remove_weak_pointer (G_OBJECT (self->parent), (gpointer *) &self->parent);

  g_clear_pointer (&self->name, g_free);
  g_clear_pointer (&self->path, g_free);
  g_clear_pointer (&self->folder_id, g_free);
  g_clear_pointer (&self->chat_id, g_free);
  g_clear_pointer (&self->icon_name, g_free);
  g_clear_object (&self->children);

  G_OBJECT_CLASS (hy_node_parent_class)->finalize (object);
}

static void
hy_node_class_init (HyNodeClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->get_property = hy_node_get_property;
  object_class->finalize = hy_node_finalize;

  properties[PROP_NAME] =
    g_param_spec_string ("name", NULL, NULL, NULL,
                         G_PARAM_READABLE | G_PARAM_STATIC_STRINGS);

  properties[PROP_ICON_NAME] =
    g_param_spec_string ("icon-name", NULL, NULL, NULL,
                         G_PARAM_READABLE | G_PARAM_STATIC_STRINGS);

  properties[PROP_STATE] =
    g_param_spec_int ("state", NULL, NULL,
                      HY_NODE_IDLE, HY_NODE_WAITING, HY_NODE_IDLE,
                      G_PARAM_READABLE | G_PARAM_STATIC_STRINGS);

  g_object_class_install_properties (object_class, N_PROPS, properties);
}

static void
hy_node_init (HyNode *self)
{
}

HyNode *
hy_node_new_folder (const char *path,
                    const char *name,
                    const char *folder_id)
{
  HyNode *self = g_object_new (HY_TYPE_NODE, NULL);

  self->kind = HY_NODE_FOLDER;
  self->path = g_strdup (path);
  self->name = g_strdup (name);
  self->folder_id = g_strdup (folder_id);
  self->children = g_list_store_new (HY_TYPE_NODE);

  return self;
}

HyNode *
hy_node_new_chat (const char *chat_id,
                  const char *title,
                  HyNode     *parent)
{
  HyNode *self = g_object_new (HY_TYPE_NODE, NULL);

  self->kind = HY_NODE_CHAT;
  self->chat_id = g_strdup (chat_id);
  self->name = g_strdup (title);
  hy_node_set_parent (self, parent);

  return self;
}

HyNodeKind
hy_node_get_kind (HyNode *self)
{
  g_return_val_if_fail (HY_IS_NODE (self), HY_NODE_FOLDER);

  return self->kind;
}

const char *
hy_node_get_name (HyNode *self)
{
  g_return_val_if_fail (HY_IS_NODE (self), NULL);

  return self->name;
}

void
hy_node_set_name (HyNode     *self,
                  const char *name)
{
  g_return_if_fail (HY_IS_NODE (self));

  if (g_strcmp0 (self->name, name) == 0)
    return;

  g_free (self->name);
  self->name = g_strdup (name);

  g_object_notify_by_pspec (G_OBJECT (self), properties[PROP_NAME]);
}

const char *
hy_node_get_path (HyNode *self)
{
  g_return_val_if_fail (HY_IS_NODE (self), NULL);

  return self->path;
}

void
hy_node_set_path (HyNode     *self,
                  const char *path)
{
  g_return_if_fail (HY_IS_NODE (self));

  g_free (self->path);
  self->path = g_strdup (path);
}

const char *
hy_node_get_folder_id (HyNode *self)
{
  g_return_val_if_fail (HY_IS_NODE (self), NULL);

  return self->folder_id;
}

const char *
hy_node_get_chat_id (HyNode *self)
{
  g_return_val_if_fail (HY_IS_NODE (self), NULL);

  return self->chat_id;
}

const char *
hy_node_get_icon_name (HyNode *self)
{
  g_return_val_if_fail (HY_IS_NODE (self), NULL);

  if (self->kind == HY_NODE_FOLDER)
    return "folder-symbolic";

  return self->icon_name != NULL ? self->icon_name : "chat-bubble-text-symbolic";
}

void
hy_node_set_icon_name (HyNode     *self,
                       const char *icon_name)
{
  g_return_if_fail (HY_IS_NODE (self));

  if (g_strcmp0 (self->icon_name, icon_name) == 0)
    return;

  g_free (self->icon_name);
  self->icon_name = g_strdup (icon_name);

  g_object_notify_by_pspec (G_OBJECT (self), properties[PROP_ICON_NAME]);
}

HyNodeState
hy_node_get_state (HyNode *self)
{
  g_return_val_if_fail (HY_IS_NODE (self), HY_NODE_IDLE);

  return self->state;
}

void
hy_node_set_state (HyNode      *self,
                   HyNodeState  state)
{
  g_return_if_fail (HY_IS_NODE (self));

  if (self->state == state)
    return;

  self->state = state;

  g_object_notify_by_pspec (G_OBJECT (self), properties[PROP_STATE]);
}

GListStore *
hy_node_get_children (HyNode *self)
{
  g_return_val_if_fail (HY_IS_NODE (self), NULL);

  return self->children;
}

HyNode *
hy_node_get_parent (HyNode *self)
{
  g_return_val_if_fail (HY_IS_NODE (self), NULL);

  return self->parent;
}

void
hy_node_set_parent (HyNode *self,
                    HyNode *parent)
{
  g_return_if_fail (HY_IS_NODE (self));

  if (self->parent == parent)
    return;

  if (self->parent != NULL)
    g_object_remove_weak_pointer (G_OBJECT (self->parent), (gpointer *) &self->parent);

  self->parent = parent;

  if (parent != NULL)
    g_object_add_weak_pointer (G_OBJECT (parent), (gpointer *) &self->parent);
}
