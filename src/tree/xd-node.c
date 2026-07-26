#include "xd-node.h"

struct _XdNode
{
  GObject parent_instance;

  XdNodeKind kind;
  char *name;
  char *path;
  char *folder_id;
  char *chat_id;
  char *icon_name;
  XdNodeState state;

  GListStore *children;   /* folders only */
  XdNode *parent;         /* weak */
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

G_DEFINE_FINAL_TYPE (XdNode, xd_node, G_TYPE_OBJECT)

static void
xd_node_get_property (GObject    *object,
                      guint       prop_id,
                      GValue     *value,
                      GParamSpec *pspec)
{
  XdNode *self = XD_NODE (object);

  switch (prop_id)
    {
    case PROP_NAME:
      g_value_set_string (value, self->name);
      break;
    case PROP_ICON_NAME:
      g_value_set_string (value, xd_node_get_icon_name (self));
      break;
    case PROP_STATE:
      g_value_set_int (value, self->state);
      break;
    default:
      G_OBJECT_WARN_INVALID_PROPERTY_ID (object, prop_id, pspec);
    }
}

static void
xd_node_finalize (GObject *object)
{
  XdNode *self = XD_NODE (object);

  if (self->parent != NULL)
    g_object_remove_weak_pointer (G_OBJECT (self->parent), (gpointer *) &self->parent);

  g_clear_pointer (&self->name, g_free);
  g_clear_pointer (&self->path, g_free);
  g_clear_pointer (&self->folder_id, g_free);
  g_clear_pointer (&self->chat_id, g_free);
  g_clear_pointer (&self->icon_name, g_free);
  g_clear_object (&self->children);

  G_OBJECT_CLASS (xd_node_parent_class)->finalize (object);
}

static void
xd_node_class_init (XdNodeClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->get_property = xd_node_get_property;
  object_class->finalize = xd_node_finalize;

  properties[PROP_NAME] =
    g_param_spec_string ("name", NULL, NULL, NULL,
                         G_PARAM_READABLE | G_PARAM_STATIC_STRINGS);

  properties[PROP_ICON_NAME] =
    g_param_spec_string ("icon-name", NULL, NULL, NULL,
                         G_PARAM_READABLE | G_PARAM_STATIC_STRINGS);

  properties[PROP_STATE] =
    g_param_spec_int ("state", NULL, NULL,
                      XD_NODE_IDLE, XD_NODE_WAITING, XD_NODE_IDLE,
                      G_PARAM_READABLE | G_PARAM_STATIC_STRINGS);

  g_object_class_install_properties (object_class, N_PROPS, properties);
}

static void
xd_node_init (XdNode *self)
{
}

XdNode *
xd_node_new_folder (const char *path,
                    const char *name,
                    const char *folder_id)
{
  XdNode *self = g_object_new (XD_TYPE_NODE, NULL);

  self->kind = XD_NODE_FOLDER;
  self->path = g_strdup (path);
  self->name = g_strdup (name);
  self->folder_id = g_strdup (folder_id);
  self->children = g_list_store_new (XD_TYPE_NODE);

  return self;
}

XdNode *
xd_node_new_chat (const char *chat_id,
                  const char *title,
                  XdNode     *parent)
{
  XdNode *self = g_object_new (XD_TYPE_NODE, NULL);

  self->kind = XD_NODE_CHAT;
  self->chat_id = g_strdup (chat_id);
  self->name = g_strdup (title);
  xd_node_set_parent (self, parent);

  return self;
}

XdNodeKind
xd_node_get_kind (XdNode *self)
{
  g_return_val_if_fail (XD_IS_NODE (self), XD_NODE_FOLDER);

  return self->kind;
}

const char *
xd_node_get_name (XdNode *self)
{
  g_return_val_if_fail (XD_IS_NODE (self), NULL);

  return self->name;
}

void
xd_node_set_name (XdNode     *self,
                  const char *name)
{
  g_return_if_fail (XD_IS_NODE (self));

  if (g_strcmp0 (self->name, name) == 0)
    return;

  g_free (self->name);
  self->name = g_strdup (name);

  g_object_notify_by_pspec (G_OBJECT (self), properties[PROP_NAME]);
}

const char *
xd_node_get_path (XdNode *self)
{
  g_return_val_if_fail (XD_IS_NODE (self), NULL);

  return self->path;
}

void
xd_node_set_path (XdNode     *self,
                  const char *path)
{
  g_return_if_fail (XD_IS_NODE (self));

  g_free (self->path);
  self->path = g_strdup (path);
}

const char *
xd_node_get_folder_id (XdNode *self)
{
  g_return_val_if_fail (XD_IS_NODE (self), NULL);

  return self->folder_id;
}

const char *
xd_node_get_chat_id (XdNode *self)
{
  g_return_val_if_fail (XD_IS_NODE (self), NULL);

  return self->chat_id;
}

const char *
xd_node_get_icon_name (XdNode *self)
{
  g_return_val_if_fail (XD_IS_NODE (self), NULL);

  if (self->kind == XD_NODE_FOLDER)
    return "folder-symbolic";

  return self->icon_name != NULL ? self->icon_name : "chat-bubble-text-symbolic";
}

void
xd_node_set_icon_name (XdNode     *self,
                       const char *icon_name)
{
  g_return_if_fail (XD_IS_NODE (self));

  if (g_strcmp0 (self->icon_name, icon_name) == 0)
    return;

  g_free (self->icon_name);
  self->icon_name = g_strdup (icon_name);

  g_object_notify_by_pspec (G_OBJECT (self), properties[PROP_ICON_NAME]);
}

XdNodeState
xd_node_get_state (XdNode *self)
{
  g_return_val_if_fail (XD_IS_NODE (self), XD_NODE_IDLE);

  return self->state;
}

void
xd_node_set_state (XdNode      *self,
                   XdNodeState  state)
{
  g_return_if_fail (XD_IS_NODE (self));

  if (self->state == state)
    return;

  self->state = state;

  g_object_notify_by_pspec (G_OBJECT (self), properties[PROP_STATE]);
}

GListStore *
xd_node_get_children (XdNode *self)
{
  g_return_val_if_fail (XD_IS_NODE (self), NULL);

  return self->children;
}

XdNode *
xd_node_get_parent (XdNode *self)
{
  g_return_val_if_fail (XD_IS_NODE (self), NULL);

  return self->parent;
}

void
xd_node_set_parent (XdNode *self,
                    XdNode *parent)
{
  g_return_if_fail (XD_IS_NODE (self));

  if (self->parent == parent)
    return;

  if (self->parent != NULL)
    g_object_remove_weak_pointer (G_OBJECT (self->parent), (gpointer *) &self->parent);

  self->parent = parent;

  if (parent != NULL)
    g_object_add_weak_pointer (G_OBJECT (parent), (gpointer *) &self->parent);
}
