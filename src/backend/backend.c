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

/* Stored in the database, so these strings are part of the file format. */
static const struct { AiEffort effort; const char *id; const char *label; } efforts[] = {
  { AI_EFFORT_DEFAULT, "default", "Default effort" },
  { AI_EFFORT_LOW,     "low",     "Low" },
  { AI_EFFORT_MEDIUM,  "medium",  "Medium" },
  { AI_EFFORT_HIGH,    "high",    "High" },
  { AI_EFFORT_XHIGH,   "xhigh",   "Extra high" },
  { AI_EFFORT_MAX,     "max",     "Max" },
};

static const struct { AiAccess access; const char *id; const char *label; const char *icon; } accesses[] = {
  { AI_ACCESS_PLAN,      "plan",      "Plan only",   "view-list-bullet-symbolic" },
  { AI_ACCESS_READ_ONLY, "read-only", "Read only",   "changes-prevent-symbolic" },
  { AI_ACCESS_EDIT,      "edit",      "Edit files",  "document-edit-symbolic" },
  { AI_ACCESS_FULL,      "full",      "Full access", "changes-allow-symbolic" },
};

const char *
ai_effort_to_string (AiEffort effort)
{
  for (gsize i = 0; i < G_N_ELEMENTS (efforts); i++)
    if (efforts[i].effort == effort)
      return efforts[i].id;

  return "default";
}

AiEffort
ai_effort_from_string (const char *name)
{
  for (gsize i = 0; i < G_N_ELEMENTS (efforts); i++)
    if (g_strcmp0 (efforts[i].id, name) == 0)
      return efforts[i].effort;

  return AI_EFFORT_DEFAULT;
}

const char *
ai_effort_label (AiEffort effort)
{
  for (gsize i = 0; i < G_N_ELEMENTS (efforts); i++)
    if (efforts[i].effort == effort)
      return efforts[i].label;

  return "Default effort";
}

const char *
ai_access_to_string (AiAccess access)
{
  for (gsize i = 0; i < G_N_ELEMENTS (accesses); i++)
    if (accesses[i].access == access)
      return accesses[i].id;

  return "read-only";
}

AiAccess
ai_access_from_string (const char *name)
{
  for (gsize i = 0; i < G_N_ELEMENTS (accesses); i++)
    if (g_strcmp0 (accesses[i].id, name) == 0)
      return accesses[i].access;

  /* Anything unrecognised errs on the safe side. */
  return AI_ACCESS_READ_ONLY;
}

const char *
ai_access_label (AiAccess access)
{
  for (gsize i = 0; i < G_N_ELEMENTS (accesses); i++)
    if (accesses[i].access == access)
      return accesses[i].label;

  return "Read only";
}

const char *
ai_access_icon_name (AiAccess access)
{
  for (gsize i = 0; i < G_N_ELEMENTS (accesses); i++)
    if (accesses[i].access == access)
      return accesses[i].icon;

  return "changes-prevent-symbolic";
}

const char *
ai_backend_model_label (const AiBackend *self,
                        const char      *model_id)
{
  g_return_val_if_fail (self != NULL, NULL);

  /* Chats created before models were pinned have none stored. */
  if (model_id == NULL || *model_id == '\0')
    model_id = self->default_model;

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
