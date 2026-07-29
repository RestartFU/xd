#include "codex-backend.h"

/*
 * Codex's rich-client transport is app-server.
 *
 * One persistent process multiplexes threads and turns over JSON-RPC. The
 * legacy exec parser remains below so old captured transcripts still exercise
 * the shared event vocabulary and upgrades can tolerate stored fixtures.
 */

/*
 * Codex has no plan mode of its own, so it has to be asked for in words.
 *
 * The distinction that matters is exploring versus doing: a good plan needs
 * the model to read, search and check things first, so forbidding it from
 * touching anything at all would make the plan worse. This mirrors the split
 * t3code draws in its own Codex plan instructions (MIT).
 */
static const char *CODEX_PLAN_INSTRUCTIONS =
  "<plan_mode>\n"
  "You are planning, not implementing. Produce a plan detailed enough that "
  "someone else could carry it out without having to make decisions.\n\n"
  "Explore freely: read and search files, inspect configuration and types, "
  "run tests, builds and dry runs. Anything that tells you how things "
  "actually are makes the plan better.\n\n"
  "Do not carry the work out: no editing or writing files, no patches, "
  "migrations or codegen, no formatters that rewrite, and no commands whose "
  "purpose is to do the work rather than to understand it.\n\n"
  "If asked to go ahead and do something, plan that instead. Plan mode ends "
  "when the user leaves it, not because a message sounds like an "
  "instruction.\n\n"
  "Write the plan as Markdown: \"##\" headings for the parts of the work, "
  "\"-\" for lists, \"1.\" for steps that happen in order, and fenced code "
  "blocks for commands, paths and snippets. It is read in a window that "
  "renders it, so a wall of prose is harder to follow than the same plan with "
  "its structure showing.\n"
  "</plan_mode>";

static const char *CODEX_COMMIT_INSTRUCTIONS =
  "<commit_attribution>\n"
  "When you create a Git commit, add this trailer to the commit message unless "
  "the user specifically asks you not to:\n\n"
  "Co-authored-by: Codex <codex@openai.com>\n"
  "</commit_attribution>";

const char *
xd_codex_sandbox_policy_type (AiAccess access)
{
  switch (access)
    {
    case AI_ACCESS_EDIT: return "workspaceWrite";
    case AI_ACCESS_FULL: return "dangerFullAccess";
    case AI_ACCESS_PLAN:
    case AI_ACCESS_READ_ONLY:
    default:             return "readOnly";
    }
}

static GPtrArray *
codex_build_argv (const AiBackend *self,
                  const AiRunSpec *spec)
{
  GPtrArray *argv = g_ptr_array_new_with_free_func (g_free);

  g_ptr_array_add (argv, g_strdup (self->program));
  g_ptr_array_add (argv, g_strdup ("app-server"));
  g_ptr_array_add (argv, g_strdup ("--listen"));
  g_ptr_array_add (argv, g_strdup ("stdio://"));
  g_ptr_array_add (argv, NULL);

  return argv;
}

char *
xd_codex_developer_instructions (const AiRunSpec *spec)
{
  g_autoptr (GString) text = g_string_new (NULL);

  g_return_val_if_fail (spec != NULL, NULL);

  if (spec->access == AI_ACCESS_PLAN)
    g_string_append_printf (text, "%s\n\n", CODEX_PLAN_INSTRUCTIONS);

  g_string_append (text, CODEX_COMMIT_INSTRUCTIONS);

  if (spec->system_prompt != NULL && *spec->system_prompt != '\0')
    g_string_append_printf (text, "\n\n%s", spec->system_prompt);

  return g_string_free (g_steal_pointer (&text), FALSE);
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

/* Finds Codex's rollout for a thread under sessions/YYYY/MM/DD. */
static char *
find_rollout (const char *directory,
              const char *thread_id,
              guint       depth)
{
  g_autoptr (GDir) dir = NULL;
  g_autofree char *suffix = NULL;
  const char *name;

  if (depth > 4)
    return NULL;

  dir = g_dir_open (directory, 0, NULL);
  if (dir == NULL)
    return NULL;

  suffix = g_strdup_printf ("%s.jsonl", thread_id);
  while ((name = g_dir_read_name (dir)) != NULL)
    {
      g_autofree char *path = g_build_filename (directory, name, NULL);

      if (g_file_test (path, G_FILE_TEST_IS_DIR))
        {
          char *found = find_rollout (path, thread_id, depth + 1);

          if (found != NULL)
            return found;
        }
      else if (g_str_has_suffix (name, suffix))
        {
          return g_steal_pointer (&path);
        }
    }

  return NULL;
}

/*
 * --json exposes accumulated turn spend, not current context. Codex writes
 * exact live context beside the session, which is the value its own TUI uses.
 */
static gboolean
rollout_context (AiParser *parser,
                 guint64  *used,
                 guint64  *window)
{
  const char *configured = g_getenv ("CODEX_HOME");
  g_autofree char *home = configured != NULL && *configured != '\0'
    ? g_strdup (configured)
    : g_build_filename (g_get_home_dir (), ".codex", NULL);
  g_autofree char *sessions = g_build_filename (home, "sessions", NULL);
  g_autofree char *path = NULL;
  g_autoptr (GFile) file = NULL;
  g_autoptr (GFileInputStream) input = NULL;
  g_autoptr (GFileInfo) info = NULL;
  g_autoptr (GDataInputStream) lines = NULL;
  g_autoptr (JsonParser) json = json_parser_new ();
  goffset offset = 0;
  guint64 latest_used = 0;
  guint64 latest_window = 0;

  if (parser->session_id == NULL)
    return FALSE;

  path = find_rollout (sessions, parser->session_id, 0);
  if (path == NULL)
    return FALSE;

  file = g_file_new_for_path (path);
  input = g_file_read (file, NULL, NULL);
  info = g_file_query_info (file, G_FILE_ATTRIBUTE_STANDARD_SIZE,
                            G_FILE_QUERY_INFO_NONE, NULL, NULL);
  if (input == NULL || info == NULL)
    return FALSE;

  /* Rollouts can be huge. Token counts sit near the end; one MiB leaves room
   * for a large final response without reading the whole conversation. */
  if (g_file_info_get_size (info) > 1024 * 1024)
    offset = g_file_info_get_size (info) - 1024 * 1024;

  if (offset > 0 &&
      !g_seekable_seek (G_SEEKABLE (input), offset, G_SEEK_SET, NULL, NULL))
    return FALSE;

  lines = g_data_input_stream_new (G_INPUT_STREAM (input));

  /* A tail seek can land mid-record. Discard that fragment. */
  if (offset > 0)
    {
      g_autofree char *fragment =
        g_data_input_stream_read_line_utf8 (lines, NULL, NULL, NULL);
    }

  while (TRUE)
    {
      g_autofree char *line =
        g_data_input_stream_read_line_utf8 (lines, NULL, NULL, NULL);
      JsonObject *root;
      JsonObject *payload;
      JsonObject *usage;
      JsonObject *context;

      if (line == NULL)
        break;
      if (strstr (line, "\"token_count\"") == NULL ||
          !json_parser_load_from_data (json, line, -1, NULL))
        continue;

      root = json_node_get_object (json_parser_get_root (json));
      payload = ai_json_get_object (root, "payload");
      if (g_strcmp0 (ai_json_get_string (payload, "type"), "token_count") != 0)
        continue;

      context = ai_json_get_object (payload, "info");
      usage = ai_json_get_object (context, "last_token_usage");
      if (context == NULL || usage == NULL)
        continue;

      latest_used = MAX (json_object_get_int_member_with_default (
                           usage, "total_tokens", 0), 0);
      latest_window = MAX (json_object_get_int_member_with_default (
                             context, "model_context_window", 0), 0);
    }

  if (latest_used == 0 || latest_window == 0)
    return FALSE;

  *used = latest_used;
  *window = latest_window;
  return TRUE;
}

static void
emit_usage (AiParser    *parser,
            JsonObject  *root,
            AiEventFunc  callback,
            gpointer     user_data)
{
  JsonObject *usage = ai_json_get_object (root, "usage");
  gint64 input;
  gint64 output;
  guint64 used;
  guint64 window;
  AiEvent event;

  if (usage == NULL)
    return;

  /* cached_input_tokens is a subset of input_tokens in Codex output. */
  input = MAX (json_object_get_int_member_with_default (
                 usage, "input_tokens", 0), 0);
  output = MAX (json_object_get_int_member_with_default (
                  usage, "output_tokens", 0), 0);
  used = input + output;
  window = ai_backend_context_window (parser->backend, parser->model);
  rollout_context (parser, &used, &window);

  event = (AiEvent) {
    .type = AI_EVENT_USAGE,
    .context_used = used,
    .context_window = window,
  };
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
      g_autofree char *summary = ai_tool_summary (type, item);

      emit (callback, user_data, AI_EVENT_TOOL_USE, summary, NULL);
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
        {
          g_free (parser->session_id);
          parser->session_id = g_strdup (thread_id);
          emit (callback, user_data, AI_EVENT_SESSION_STARTED, NULL, thread_id);
        }

      return;
    }

  if (g_strcmp0 (type, "item.started") == 0)
    {
      JsonObject *item = ai_json_get_object (root, "item");
      const char *item_type = ai_json_get_string (item, "type");
      const char *item_id = ai_json_get_string (item, "id");

      /*
       * Commands can take minutes. Reporting them only after completion hides
       * exactly the workflow watch the user needs to follow alongside the
       * agent. File changes stay completion-only: capturing their diff before
       * the edit lands would preserve an empty or stale patch.
       */
      if (g_strcmp0 (item_type, "command_execution") == 0)
        {
          g_autofree char *summary = ai_tool_summary (item_type, item);

          emit (callback, user_data, AI_EVENT_TOOL_USE, summary, NULL);
          if (item_id != NULL)
            g_hash_table_add (parser->started_commands, g_strdup (item_id));
        }

      return;
    }

  if (g_strcmp0 (type, "item.completed") == 0)
    {
      JsonObject *item = ai_json_get_object (root, "item");
      const char *item_id = ai_json_get_string (item, "id");

      if (item_id != NULL &&
          g_hash_table_remove (parser->started_commands, item_id))
        return;

      parse_item (parser, root, callback, user_data);
      return;
    }

  if (g_strcmp0 (type, "turn.completed") == 0)
    {
      emit_usage (parser, root, callback, user_data);
      emit (callback, user_data, AI_EVENT_RESULT, NULL, NULL);
      return;
    }

  if (g_strcmp0 (type, "turn.failed") == 0 || g_strcmp0 (type, "error") == 0)
    {
      JsonObject *error = ai_json_get_object (root, "error");
      const char *message = ai_json_get_string (root, "message");

      if (message == NULL)
        message = ai_json_get_string (error, "message");

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
  { "gpt-5.6-sol",         "GPT-5.6 Sol",         272000 },
  { "gpt-5.6-luna",        "GPT-5.6 Luna",        272000 },
  { "gpt-5.6-terra",       "GPT-5.6 Terra",       272000 },
  { "gpt-5.5",             "GPT-5.5",             272000 },
  { "gpt-5.3-codex-spark", "GPT-5.3 Codex Spark", 128000 },
};

const AiBackend xd_codex_backend = {
  .id = "codex",
  .display_name = "Codex",
  .program = "codex",
  .icon_name = "xd-backend-codex-symbolic",
  .transport = AI_TRANSPORT_CODEX_APP_SERVER,
  .default_model = "gpt-5.6-sol",
  .models = codex_models,
  .n_models = G_N_ELEMENTS (codex_models),
  .build_argv = codex_build_argv,
  .parse_object = codex_parse_object,
};
