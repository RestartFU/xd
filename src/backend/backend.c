#include "backend.h"

extern const AiBackend xd_claude_backend;
extern const AiBackend xd_codex_backend;
extern const AiBackend xd_cerebras_backend;

static const AiBackend *const backends[] = {
  &xd_claude_backend,
  &xd_codex_backend,
  &xd_cerebras_backend,
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

  return "high";
}

AiEffort
ai_effort_from_string (const char *name)
{
  for (gsize i = 0; i < G_N_ELEMENTS (efforts); i++)
    if (g_strcmp0 (efforts[i].id, name) == 0)
      return efforts[i].effort;

  return AI_EFFORT_HIGH;
}

/* Pulls "key": "value" out of JSON, or key = "value" out of TOML. Both files
 * are the CLI's own, so a hand-rolled scan beats pulling in a parser. */
static AiEffort
effort_from_config (const char *path,
                    const char *key)
{
  g_autofree char *contents = NULL;
  g_autoptr (GRegex) regex = NULL;
  g_autoptr (GMatchInfo) match = NULL;
  g_autofree char *pattern = NULL;

  if (!g_file_get_contents (path, &contents, NULL, NULL))
    return AI_EFFORT_HIGH;

  pattern = g_strdup_printf ("\"?%s\"?\\s*[:=]\\s*\"([a-zA-Z]+)\"", key);
  regex = g_regex_new (pattern, 0, 0, NULL);

  if (regex != NULL && g_regex_match (regex, contents, 0, &match))
    {
      g_autofree char *value = g_match_info_fetch (match, 1);
      g_autofree char *lowered = g_ascii_strdown (value, -1);

      for (gsize i = 0; i < G_N_ELEMENTS (efforts); i++)
        if (g_strcmp0 (efforts[i].id, lowered) == 0)
          return efforts[i].effort;
    }

  return AI_EFFORT_HIGH;
}

AiEffort
ai_backend_default_effort (const AiBackend *self)
{
  g_autofree char *path = NULL;

  g_return_val_if_fail (self != NULL, AI_EFFORT_HIGH);

  if (g_strcmp0 (self->id, "codex") == 0)
    {
      path = g_build_filename (g_get_home_dir (), ".codex", "config.toml", NULL);
      return effort_from_config (path, "model_reasoning_effort");
    }

  if (g_strcmp0 (self->id, "claude") != 0)
    return AI_EFFORT_HIGH;

  path = g_build_filename (g_get_home_dir (), ".claude", "settings.json", NULL);

  return effort_from_config (path, "effortLevel");
}

const char *
ai_effort_label (AiEffort effort)
{
  for (gsize i = 0; i < G_N_ELEMENTS (efforts); i++)
    if (efforts[i].effort == effort)
      return efforts[i].label;

  return "High";
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

guint64
ai_backend_context_window (const AiBackend *self,
                           const char      *model_id)
{
  g_return_val_if_fail (self != NULL, 0);

  if (model_id == NULL || *model_id == '\0')
    model_id = self->default_model;

  /*
   * Codex ships the effective window in its own refreshed model catalog.
   * Prefer that to compiled metadata, since account/runtime caps can change
   * without an xd release.
   */
  if (g_strcmp0 (self->id, "codex") == 0)
    {
      const char *configured = g_getenv ("CODEX_HOME");
      g_autofree char *home = configured != NULL && *configured != '\0'
        ? g_strdup (configured)
        : g_build_filename (g_get_home_dir (), ".codex", NULL);
      g_autofree char *path =
        g_build_filename (home, "models_cache.json", NULL);
      g_autoptr (JsonParser) parser = json_parser_new ();

      if (json_parser_load_from_file (parser, path, NULL))
        {
          JsonObject *root = json_node_get_object (json_parser_get_root (parser));
          JsonArray *models = root != NULL &&
                              json_object_has_member (root, "models")
                                ? json_object_get_array_member (root, "models")
                                : NULL;

          for (guint i = 0; models != NULL && i < json_array_get_length (models); i++)
            {
              JsonObject *model = json_array_get_object_element (models, i);

              if (g_strcmp0 (ai_json_get_string (model, "slug"), model_id) == 0)
                return json_object_get_int_member_with_default (
                  model, "context_window", 0);
            }
        }
    }

  for (gsize i = 0; i < self->n_models; i++)
    if (g_strcmp0 (self->models[i].id, model_id) == 0)
      return self->models[i].context_window;

  return 0;
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

/* Enough of a command or path to recognise it, not enough to wrap the line. */
#define TOOL_DETAIL_LIMIT 110

static void
pending_tool_free (gpointer data)
{
  AiPendingTool *pending = data;

  g_free (pending->name);
  g_string_free (pending->json, TRUE);
  g_free (pending);
}

/*
 * The command as it would have been typed.
 *
 * Both CLIs run commands through a shell, so what arrives is the whole
 * invocation -- /run/current-system/sw/bin/bash -lc "the actual command" --
 * and the part worth reading is inside the quotes. Everything in front of it
 * is the same on every line, which makes it the least useful thing occupying
 * the most room.
 */
static char *
unwrap_shell (const char *command)
{
  static const char *const flags[] = { " -lic ", " -lc ", " -ic ", " -c " };
  g_autofree char *program = NULL;
  const char *space = strchr (command, ' ');
  const char *inner = NULL;
  gsize length;

  if (space == NULL)
    return g_strdup (command);

  /* Only when the thing being run really is a shell: anything else is a
   * command whose first word matters. */
  program = g_path_get_basename (g_strndup (command, space - command));
  if (!g_str_has_suffix (program, "sh"))
    return g_strdup (command);

  for (gsize i = 0; i < G_N_ELEMENTS (flags) && inner == NULL; i++)
    {
      const char *at = strstr (command, flags[i]);

      if (at != NULL)
        inner = at + strlen (flags[i]);
    }

  if (inner == NULL)
    return g_strdup (command);

  /* The shell's argument is one quoted string; the quotes are the wrapper's,
   * not the command's. */
  length = strlen (inner);
  if (length >= 2 && (inner[0] == '"' || inner[0] == '\'') &&
      inner[length - 1] == inner[0])
    return g_strndup (inner + 1, length - 2);

  return g_strdup (inner);
}

char *
ai_tool_summary (const char *tool_name,
                 JsonObject *input)
{
  /* In the order that identifies the work best: what is being run beats
   * where, which beats how. */
  static const char *keys[] = {
    "command", "file_path", "filePath", "path", "pattern", "url", "query",
    "description", "notebook_path", "notebookPath", "prompt",
  };
  const char *detail = NULL;
  g_autofree char *trimmed = NULL;
  g_autofree char *heading = NULL;
  gboolean is_command = FALSE;

  if (tool_name == NULL)
    tool_name = "tool";

  /*
   * A subagent is not a tool call like the others.
   *
   * It is another agent doing its own work, and a row reading "Task" hides
   * that entirely. The kind of agent is what distinguishes one from the next,
   * so it goes in the heading rather than the detail.
   */
  if (g_strcmp0 (tool_name, "Task") == 0)
    {
      const char *kind = ai_json_get_string (input, "subagent_type");

      heading = kind != NULL ? g_strdup_printf ("Subagent: %s", kind)
                             : g_strdup ("Subagent");
      tool_name = heading;
    }

  for (gsize i = 0; i < G_N_ELEMENTS (keys) && detail == NULL; i++)
    {
      detail = ai_json_get_string (input, keys[i]);
      is_command = detail != NULL && i == 0;
    }

  if (detail == NULL || *detail == '\0')
    return g_strdup (tool_name);

  /*
   * A command is shown as a command.
   *
   * The name of the tool that ran it -- "Bash", "command_execution" -- says
   * nothing the command does not, and repeating it down the left of every row
   * pushes the part being read off the end of the line.
   */
  if (is_command)
    {
      g_autofree char *command = unwrap_shell (detail);

      trimmed = g_strdup (command);
      g_strdelimit (trimmed, "\n\r\t", ' ');
      g_strstrip (trimmed);

      if (g_utf8_strlen (trimmed, -1) > TOOL_DETAIL_LIMIT)
        {
          g_autofree char *shortened =
            g_utf8_substring (trimmed, 0, TOOL_DETAIL_LIMIT);

          return g_strdup_printf ("$ %s…", shortened);
        }

      return g_strdup_printf ("$ %s", trimmed);
    }

  trimmed = g_strdup (detail);
  g_strdelimit (trimmed, "\n\r\t", ' ');
  g_strstrip (trimmed);

  if (g_utf8_strlen (trimmed, -1) > TOOL_DETAIL_LIMIT)
    {
      g_autofree char *shortened = g_utf8_substring (trimmed, 0, TOOL_DETAIL_LIMIT);

      return g_strdup_printf ("%s  %s…", tool_name, shortened);
    }

  return g_strdup_printf ("%s  %s", tool_name, trimmed);
}

AiParser *
ai_parser_new (const AiBackend *backend)
{
  AiParser *self;

  g_return_val_if_fail (backend != NULL, NULL);

  self = g_new0 (AiParser, 1);
  self->backend = backend;
  self->json = json_parser_new ();
  self->model = g_strdup (backend->default_model);
  self->pending_tools = g_hash_table_new_full (g_direct_hash, g_direct_equal,
                                               NULL, pending_tool_free);

  return self;
}

void
ai_parser_free (AiParser *self)
{
  if (self == NULL)
    return;

  g_clear_pointer (&self->pending_tools, g_hash_table_unref);
  g_clear_object (&self->json);
  g_free (self->session_id);
  g_free (self->model);
  g_free (self);
}

void
ai_parser_set_model (AiParser   *self,
                     const char *model)
{
  g_return_if_fail (self != NULL);

  g_free (self->model);
  self->model = g_strdup (model != NULL ? model : self->backend->default_model);
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
