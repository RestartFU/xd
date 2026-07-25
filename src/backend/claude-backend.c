#include "backend.h"

/*
 * Claude Code, driven non-interactively.
 *
 *   claude -p <prompt> --output-format stream-json --verbose
 *          --include-partial-messages [--model M] [--append-system-prompt S]
 *          [--resume SESSION]
 *
 * The output is one JSON object per line. A "system"/"init" line opens the
 * session and carries the id needed to resume it, "stream_event" lines carry
 * token deltas, and a final "result" line repeats the whole reply.
 *
 * No permission flags are passed: in print mode the CLI already refuses tool
 * use by default, which is what we want for a chat window.
 */

/*
 * "manual" asks before every tool use, and in print mode there is nobody to
 * ask, so it comes out read-only -- which is exactly the intent.
 */
static const char *
claude_permission_mode (AiAccess access)
{
  switch (access)
    {
    case AI_ACCESS_PLAN:  return "plan";
    case AI_ACCESS_EDIT:  return "acceptEdits";
    case AI_ACCESS_FULL:  return "bypassPermissions";
    case AI_ACCESS_READ_ONLY:
    default:              return "manual";
    }
}

static GPtrArray *
claude_build_argv (const AiBackend *self,
                   const AiRunSpec *spec)
{
  GPtrArray *argv = g_ptr_array_new_with_free_func (g_free);

  g_ptr_array_add (argv, g_strdup (self->program));

  if (spec->resume_session_id != NULL)
    {
      g_ptr_array_add (argv, g_strdup ("--resume"));
      g_ptr_array_add (argv, g_strdup (spec->resume_session_id));
    }

  g_ptr_array_add (argv, g_strdup ("-p"));
  g_ptr_array_add (argv, g_strdup (spec->prompt));

  g_ptr_array_add (argv, g_strdup ("--output-format"));
  g_ptr_array_add (argv, g_strdup ("stream-json"));

  /* stream-json is only accepted together with --verbose. */
  g_ptr_array_add (argv, g_strdup ("--verbose"));
  g_ptr_array_add (argv, g_strdup ("--include-partial-messages"));

  if (spec->model != NULL)
    {
      g_ptr_array_add (argv, g_strdup ("--model"));
      g_ptr_array_add (argv, g_strdup (spec->model));
    }

  if (spec->system_prompt != NULL)
    {
      g_ptr_array_add (argv, g_strdup ("--append-system-prompt"));
      g_ptr_array_add (argv, g_strdup (spec->system_prompt));
    }

  if (spec->effort != AI_EFFORT_DEFAULT)
    {
      g_ptr_array_add (argv, g_strdup ("--effort"));
      g_ptr_array_add (argv, g_strdup (ai_effort_to_string (spec->effort)));
    }

  g_ptr_array_add (argv, g_strdup ("--permission-mode"));
  g_ptr_array_add (argv, g_strdup (claude_permission_mode (spec->access)));

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

/* A "stream_event" wraps the raw Anthropic streaming event. */
static void
parse_stream_event (AiParser    *parser,
                    JsonObject  *root,
                    AiEventFunc  callback,
                    gpointer     user_data)
{
  JsonObject *event = ai_json_get_object (root, "event");
  const char *type = ai_json_get_string (event, "type");

  if (g_strcmp0 (type, "content_block_delta") == 0)
    {
      JsonObject *delta = ai_json_get_object (event, "delta");
      const char *text = ai_json_get_string (delta, "text");

      if (g_strcmp0 (ai_json_get_string (delta, "type"), "text_delta") == 0 &&
          text != NULL)
        {
          parser->streamed_text = TRUE;
          emit (callback, user_data, AI_EVENT_TEXT_DELTA, text, NULL);
        }
      return;
    }

  if (g_strcmp0 (type, "content_block_start") == 0)
    {
      JsonObject *block = ai_json_get_object (event, "content_block");

      if (g_strcmp0 (ai_json_get_string (block, "type"), "tool_use") == 0)
        emit (callback, user_data, AI_EVENT_TOOL_USE,
              ai_json_get_string (block, "name"), NULL);
    }
}

/* The complete assistant message, sent after the deltas that built it. */
static void
parse_assistant (AiParser    *parser,
                 JsonObject  *root,
                 AiEventFunc  callback,
                 gpointer     user_data)
{
  JsonObject *message = ai_json_get_object (root, "message");
  JsonArray *content;

  if (message == NULL || !json_object_has_member (message, "content"))
    return;

  content = json_object_get_array_member (message, "content");
  if (content == NULL)
    return;

  for (guint i = 0; i < json_array_get_length (content); i++)
    {
      JsonObject *block = json_array_get_object_element (content, i);
      const char *type = ai_json_get_string (block, "type");

      if (g_strcmp0 (type, "tool_use") == 0)
        {
          emit (callback, user_data, AI_EVENT_TOOL_USE,
                ai_json_get_string (block, "name"), NULL);
          continue;
        }

      /* Without partial messages this is the only place the text appears, so
       * it is the fallback path; with them it would be a duplicate. */
      if (g_strcmp0 (type, "text") == 0 && !parser->streamed_text)
        emit (callback, user_data, AI_EVENT_TEXT_DELTA,
              ai_json_get_string (block, "text"), NULL);
    }
}

static void
claude_parse_object (AiParser    *parser,
                     JsonObject  *root,
                     AiEventFunc  callback,
                     gpointer     user_data)
{
  const char *type = ai_json_get_string (root, "type");

  if (g_strcmp0 (type, "system") == 0)
    {
      const char *session_id = ai_json_get_string (root, "session_id");

      if (g_strcmp0 (ai_json_get_string (root, "subtype"), "init") == 0 &&
          session_id != NULL)
        emit (callback, user_data, AI_EVENT_SESSION_STARTED, NULL, session_id);

      return;
    }

  if (g_strcmp0 (type, "stream_event") == 0)
    {
      parse_stream_event (parser, root, callback, user_data);
      return;
    }

  if (g_strcmp0 (type, "assistant") == 0)
    {
      parse_assistant (parser, root, callback, user_data);
      return;
    }

  if (g_strcmp0 (type, "result") == 0)
    {
      const char *text = ai_json_get_string (root, "result");
      gboolean failed = json_object_has_member (root, "is_error") &&
                        json_object_get_boolean_member (root, "is_error");

      emit (callback, user_data,
            failed ? AI_EVENT_ERROR : AI_EVENT_RESULT,
            text, ai_json_get_string (root, "session_id"));
      return;
    }

  /* Anything else -- rate_limit_event, system/status, future additions -- is
   * not something the transcript needs. */
  g_debug ("claude: ignoring event %s", type != NULL ? type : "(none)");
}

/*
 * The CLI takes either a bare alias ("opus", "sonnet") or a full model name.
 * Full names are used here so a chat keeps answering with the model it was
 * started on rather than following whatever the alias points at later.
 *
 * The first entry is what new chats get, so the newest model leads.
 */
static const AiModel claude_models[] = {
  { "claude-opus-5",    "Claude Opus 5" },
  { "claude-fable-5",   "Claude Fable 5" },
  { "claude-sonnet-5",  "Claude Sonnet 5" },
  { "claude-haiku-4-5", "Claude Haiku 4.5" },
  { "claude-opus-4-8",  "Claude Opus 4.8" },
};

const AiBackend hy_claude_backend = {
  .id = "claude",
  .display_name = "Claude Code",
  .program = "claude",
  .icon_name = "hy-backend-claude",
  .default_model = "claude-opus-5",
  .models = claude_models,
  .n_models = G_N_ELEMENTS (claude_models),
  .build_argv = claude_build_argv,
  .parse_object = claude_parse_object,
};
