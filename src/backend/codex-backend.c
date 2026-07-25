#include "backend.h"

/*
 * Codex, driven non-interactively.
 *
 *   codex exec --json [-m MODEL] [-C DIR] -s read-only <prompt>
 *   codex exec resume <THREAD> --json [...] <prompt>
 *
 * Output is one JSON object per line: "thread.started" carries the id used to
 * resume, "item.completed" delivers finished items -- the agent's reply among
 * them -- and "turn.completed" ends the turn.
 *
 * Unlike claude, this mode reports no token-level deltas, so a reply arrives
 * in one piece. The event names are also less settled than claude's, which is
 * why everything unrecognised is skipped rather than treated as an error.
 */

/* Codex has no plan mode of its own, so planning runs read-only and the
 * instruction to plan rather than act rides in the prompt. */
static const char *
codex_sandbox (AiAccess access)
{
  switch (access)
    {
    case AI_ACCESS_EDIT: return "workspace-write";
    case AI_ACCESS_FULL: return "danger-full-access";
    case AI_ACCESS_PLAN:
    case AI_ACCESS_READ_ONLY:
    default:             return "read-only";
    }
}

static GPtrArray *
codex_build_argv (const AiBackend *self,
                  const AiRunSpec *spec)
{
  GPtrArray *argv = g_ptr_array_new_with_free_func (g_free);

  g_ptr_array_add (argv, g_strdup (self->program));
  g_ptr_array_add (argv, g_strdup ("exec"));

  if (spec->resume_session_id != NULL)
    {
      g_ptr_array_add (argv, g_strdup ("resume"));
      g_ptr_array_add (argv, g_strdup (spec->resume_session_id));
    }

  g_ptr_array_add (argv, g_strdup ("--json"));

  g_ptr_array_add (argv, g_strdup ("-s"));
  g_ptr_array_add (argv, g_strdup (codex_sandbox (spec->access)));

  /* Codex takes effort as a config override rather than a flag. */
  if (spec->effort != AI_EFFORT_DEFAULT)
    {
      g_ptr_array_add (argv, g_strdup ("-c"));
      g_ptr_array_add (argv, g_strdup_printf ("model_reasoning_effort=\"%s\"",
                                              ai_effort_to_string (spec->effort)));
    }

  if (spec->model != NULL)
    {
      g_ptr_array_add (argv, g_strdup ("-m"));
      g_ptr_array_add (argv, g_strdup (spec->model));
    }

  if (spec->workdir != NULL)
    {
      g_ptr_array_add (argv, g_strdup ("-C"));
      g_ptr_array_add (argv, g_strdup (spec->workdir));
    }

  /* Codex has no --append-system-prompt, so folder instructions ride along in
   * front of the prompt itself -- as does the plan-only instruction, which it
   * has no flag for either. */
  {
    g_autoptr (GString) prompt = g_string_new (NULL);

    if (spec->access == AI_ACCESS_PLAN)
      g_string_append (prompt, "Plan only: describe what you would do and why. "
                               "Do not modify anything.\n\n");

    if (spec->system_prompt != NULL)
      g_string_append_printf (prompt, "%s\n\n", spec->system_prompt);

    g_string_append (prompt, spec->prompt);
    g_ptr_array_add (argv, g_string_free (g_steal_pointer (&prompt), FALSE));
  }

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
parse_item (AiParser    *parser,
            JsonObject  *root,
            AiEventFunc  callback,
            gpointer     user_data)
{
  JsonObject *item = ai_json_get_object (root, "item");
  const char *type = ai_json_get_string (item, "type");
  const char *text;

  if (g_strcmp0 (type, "agent_message") != 0)
    {
      /* Reasoning, command executions and the like: worth a line in the
       * transcript, but not part of the reply. */
      emit (callback, user_data, AI_EVENT_TOOL_USE, type, NULL);
      return;
    }

  text = ai_json_get_string (item, "text");
  if (text == NULL)
    return;

  parser->streamed_text = TRUE;
  emit (callback, user_data, AI_EVENT_TEXT_DELTA, text, NULL);
}

static void
codex_parse_object (AiParser    *parser,
                    JsonObject  *root,
                    AiEventFunc  callback,
                    gpointer     user_data)
{
  const char *type = ai_json_get_string (root, "type");

  if (g_strcmp0 (type, "thread.started") == 0)
    {
      const char *thread_id = ai_json_get_string (root, "thread_id");

      if (thread_id != NULL)
        emit (callback, user_data, AI_EVENT_SESSION_STARTED, NULL, thread_id);

      return;
    }

  if (g_strcmp0 (type, "item.completed") == 0)
    {
      parse_item (parser, root, callback, user_data);
      return;
    }

  if (g_strcmp0 (type, "turn.completed") == 0)
    {
      emit (callback, user_data, AI_EVENT_RESULT, NULL, NULL);
      return;
    }

  if (g_strcmp0 (type, "turn.failed") == 0 || g_strcmp0 (type, "error") == 0)
    {
      JsonObject *error = ai_json_get_object (root, "error");
      const char *message = ai_json_get_string (error, "message");

      emit (callback, user_data, AI_EVENT_ERROR,
            message != NULL ? message : "The turn failed", NULL);
      return;
    }

  g_debug ("codex: ignoring event %s", type != NULL ? type : "(none)");
}

/*
 * Codex exposes no way to list its models, so these are the ones seen in the
 * local Codex configuration and session history. The first entry is what new
 * chats get, so the newest model leads.
 */
static const AiModel codex_models[] = {
  { "gpt-5.6-sol",         "GPT-5.6 Sol" },
  { "gpt-5.6-luna",        "GPT-5.6 Luna" },
  { "gpt-5.6-terra",       "GPT-5.6 Terra" },
  { "gpt-5.5",             "GPT-5.5" },
  { "gpt-5.3-codex-spark", "GPT-5.3 Codex Spark" },
};

const AiBackend hy_codex_backend = {
  .id = "codex",
  .display_name = "Codex",
  .program = "codex",
  .icon_name = "hy-backend-codex-symbolic",
  .default_model = "gpt-5.6-sol",
  .models = codex_models,
  .n_models = G_N_ELEMENTS (codex_models),
  .build_argv = codex_build_argv,
  .parse_object = codex_parse_object,
};
