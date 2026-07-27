#include "backend.h"

/*
 * Cerebras is driven through OpenCode rather than the raw inference API.
 *
 *   opencode run --format json --model cerebras/MODEL [OPTIONS] <prompt>
 *
 * That keeps Cerebras on the same footing as the other backends: it remains a
 * coding agent with tools and resumable sessions, not a text-only completion
 * endpoint. OpenCode reads CEREBRAS_API_KEY or its own credential store.
 */

static const char *PLAN_INSTRUCTIONS =
  "<plan_mode>\n"
  "Plan the requested work without changing files. Inspect the project and run "
  "read-only checks as needed, then return a concrete implementation plan.\n"
  "</plan_mode>";

static const char *READ_ONLY_INSTRUCTIONS =
  "<read_only_mode>\n"
  "Answer the request normally, but do not create, edit, move, or delete files "
  "and do not run commands that change project or external state.\n"
  "</read_only_mode>";

static char *
build_prompt (const AiRunSpec *spec)
{
  g_autoptr (GString) prompt = g_string_new (NULL);

  if (spec->access == AI_ACCESS_PLAN)
    g_string_append_printf (prompt, "%s\n\n", PLAN_INSTRUCTIONS);
  else if (spec->access == AI_ACCESS_READ_ONLY)
    g_string_append_printf (prompt, "%s\n\n", READ_ONLY_INSTRUCTIONS);

  if (spec->system_prompt != NULL && *spec->system_prompt != '\0')
    g_string_append_printf (prompt, "%s\n\n", spec->system_prompt);

  g_string_append (prompt, spec->prompt != NULL ? spec->prompt : "");

  return g_string_free (g_steal_pointer (&prompt), FALSE);
}

static const char *
variant_for (const AiRunSpec *spec,
             const char      *model)
{
  /* GLM currently exposes reasoning, but no selectable effort variants. */
  if (g_str_has_suffix (model, "/zai-glm-4.7"))
    return NULL;

  switch (spec->effort)
    {
    case AI_EFFORT_LOW:    return "low";
    case AI_EFFORT_MEDIUM: return "medium";
    case AI_EFFORT_HIGH:
    case AI_EFFORT_XHIGH:
    case AI_EFFORT_MAX:
    default:               return "high";
    }
}

static GPtrArray *
cerebras_build_argv (const AiBackend *self,
                     const AiRunSpec *spec)
{
  GPtrArray *argv = g_ptr_array_new_with_free_func (g_free);
  const char *model = spec->model != NULL ? spec->model : self->default_model;
  const char *variant = variant_for (spec, model);

  g_ptr_array_add (argv, g_strdup (self->program));
  g_ptr_array_add (argv, g_strdup ("run"));
  g_ptr_array_add (argv, g_strdup ("--format"));
  g_ptr_array_add (argv, g_strdup ("json"));
  g_ptr_array_add (argv, g_strdup ("--model"));
  g_ptr_array_add (argv, g_strdup (model));

  if (variant != NULL)
    {
      g_ptr_array_add (argv, g_strdup ("--variant"));
      g_ptr_array_add (argv, g_strdup (variant));
    }

  /*
   * OpenCode's built-in plan agent denies edits. Non-interactive build-agent
   * permission requests are auto-rejected unless --auto is present.
   */
  if (spec->access == AI_ACCESS_PLAN ||
      spec->access == AI_ACCESS_READ_ONLY)
    {
      g_ptr_array_add (argv, g_strdup ("--agent"));
      g_ptr_array_add (argv, g_strdup ("plan"));
    }
  else
    {
      g_ptr_array_add (argv, g_strdup ("--auto"));
    }

  if (spec->resume_session_id != NULL)
    {
      g_ptr_array_add (argv, g_strdup ("--session"));
      g_ptr_array_add (argv, g_strdup (spec->resume_session_id));
    }

  g_ptr_array_add (argv, build_prompt (spec));
  g_ptr_array_add (argv, NULL);

  return argv;
}

static void
emit (AiEventFunc  callback,
      gpointer     user_data,
      AiEventType  type,
      const char  *text,
      const char  *session_id)
{
  AiEvent event = { .type = type, .text = text, .session_id = session_id };

  callback (&event, user_data);
}

static void
emit_session_once (AiParser    *parser,
                   JsonObject  *root,
                   AiEventFunc  callback,
                   gpointer     user_data)
{
  const char *session_id = ai_json_get_string (root, "sessionID");

  if (session_id == NULL || parser->session_id != NULL)
    return;

  parser->session_id = g_strdup (session_id);
  emit (callback, user_data, AI_EVENT_SESSION_STARTED, NULL, session_id);
}

static const char *
error_message (JsonObject *root)
{
  JsonObject *error = ai_json_get_object (root, "error");
  JsonObject *data = ai_json_get_object (error, "data");
  const char *message = ai_json_get_string (data, "message");

  if (message != NULL)
    return message;

  message = ai_json_get_string (error, "message");
  if (message != NULL)
    return message;

  message = ai_json_get_string (error, "name");
  return message != NULL ? message : "OpenCode reported a failed turn";
}

static void
cerebras_parse_object (AiParser    *parser,
                       JsonObject  *root,
                       AiEventFunc  callback,
                       gpointer     user_data)
{
  const char *type = ai_json_get_string (root, "type");
  JsonObject *part;

  emit_session_once (parser, root, callback, user_data);

  if (g_strcmp0 (type, "text") == 0)
    {
      part = ai_json_get_object (root, "part");
      const char *text = ai_json_get_string (part, "text");

      if (text != NULL)
        emit (callback, user_data, AI_EVENT_TEXT_DELTA, text, NULL);
      return;
    }

  if (g_strcmp0 (type, "tool_use") == 0)
    {
      JsonObject *state;
      JsonObject *input;
      g_autofree char *summary = NULL;

      part = ai_json_get_object (root, "part");
      state = ai_json_get_object (part, "state");
      input = ai_json_get_object (state, "input");
      summary = ai_tool_summary (ai_json_get_string (part, "tool"), input);
      emit (callback, user_data, AI_EVENT_TOOL_USE, summary, NULL);
      return;
    }

  if (g_strcmp0 (type, "error") == 0)
    {
      emit (callback, user_data, AI_EVENT_ERROR, error_message (root), NULL);
      return;
    }

  /* step_start, step_finish and reasoning do not belong in the transcript. */
  g_debug ("cerebras: ignoring event %s", type != NULL ? type : "(none)");
}

static const AiModel cerebras_models[] = {
  { "cerebras/zai-glm-4.7",   "Cerebras GLM 4.7" },
  { "cerebras/gemma-4-31b",   "Cerebras Gemma 4 31B" },
  { "cerebras/gpt-oss-120b",  "Cerebras GPT OSS 120B" },
};

const AiBackend xd_cerebras_backend = {
  .id = "cerebras",
  .display_name = "Cerebras",
  .program = "opencode",
  .icon_name = "xd-backend-cerebras",
  .default_model = "cerebras/zai-glm-4.7",
  .models = cerebras_models,
  .n_models = G_N_ELEMENTS (cerebras_models),
  .build_argv = cerebras_build_argv,
  .parse_object = cerebras_parse_object,
};
