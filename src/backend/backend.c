#include "backend.h"

extern const AiBackend hy_claude_backend;
extern const AiBackend hy_codex_backend;

static const AiBackend *const backends[] = {
  &hy_claude_backend,
  &hy_codex_backend,
};

const AiBackend *
ai_backend_lookup (const char *id)
{
  if (id == NULL)
    return NULL;

  for (gsize i = 0; i < G_N_ELEMENTS (backends); i++)
    {
      if (g_strcmp0 (backends[i]->id, id) == 0)
        return backends[i];
    }

  return NULL;
}

const AiBackend *const *
ai_backend_all (guint *n_backends)
{
  if (n_backends != NULL)
    *n_backends = G_N_ELEMENTS (backends);

  return backends;
}

const char *
ai_backend_model_label (const AiBackend *self,
                        const char      *model_id)
{
  g_return_val_if_fail (self != NULL, NULL);

  if (model_id == NULL || *model_id == '\0')
    return self->n_models > 0 ? self->models[0].display_name : "Default";

  for (gsize i = 0; i < self->n_models; i++)
    {
      if (g_strcmp0 (self->models[i].id, model_id) == 0)
        return self->models[i].display_name;
    }

  /* A model set by hand, or one that shipped after this build. */
  return model_id;
}

const char *
ai_json_get_string (JsonObject *object,
                    const char *member)
{
  JsonNode *node;

  if (object == NULL || !json_object_has_member (object, member))
    return NULL;

  node = json_object_get_member (object, member);
  if (!JSON_NODE_HOLDS_VALUE (node))
    return NULL;

  return json_node_get_string (node);
}

JsonObject *
ai_json_get_object (JsonObject *object,
                    const char *member)
{
  JsonNode *node;

  if (object == NULL || !json_object_has_member (object, member))
    return NULL;

  node = json_object_get_member (object, member);
  if (!JSON_NODE_HOLDS_OBJECT (node))
    return NULL;

  return json_node_get_object (node);
}

AiParser *
ai_parser_new (const AiBackend *backend)
{
  AiParser *self;

  g_return_val_if_fail (backend != NULL, NULL);

  self = g_new0 (AiParser, 1);
  self->backend = backend;
  self->json = json_parser_new ();

  return self;
}

void
ai_parser_free (AiParser *self)
{
  if (self == NULL)
    return;

  g_clear_object (&self->json);
  g_free (self);
}

void
ai_parser_feed_line (AiParser    *self,
                     const char  *line,
                     AiEventFunc  callback,
                     gpointer     user_data)
{
  g_autoptr (GError) error = NULL;
  JsonNode *root;

  g_return_if_fail (self != NULL);
  g_return_if_fail (callback != NULL);

  if (line == NULL || *line == '\0')
    return;

  if (!json_parser_load_from_data (self->json, line, -1, &error))
    {
      /* The CLIs occasionally print something that is not an event. Log it
       * and carry on rather than ending the turn. */
      g_debug ("%s: unparsable output: %s", self->backend->id, line);
      return;
    }

  root = json_parser_get_root (self->json);
  if (root == NULL || !JSON_NODE_HOLDS_OBJECT (root))
    return;

  self->backend->parse_object (self, json_node_get_object (root),
                               callback, user_data);
}
