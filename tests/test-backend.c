#include "backend/backend.h"
#include "backend/codex-backend.h"
#include "util/subagent-tool.h"

#include <glib/gstdio.h>

/* Collects everything a fixture produces so a test can assert on the whole
 * sequence rather than one event at a time. */
typedef struct
{
  GString *text;
  char *session_id;
  char *last_tool;
  GStrv commands;
  guint n_command_events;
  guint n_deltas;
  guint n_tools;
  guint n_usage;
  guint n_results;
  guint n_errors;
  guint64 context_used;
  guint64 context_window;
} Collected;

static void
collect (const AiEvent *event,
         gpointer       user_data)
{
  Collected *collected = user_data;

  switch (event->type)
    {
    case AI_EVENT_SESSION_STARTED:
      g_free (collected->session_id);
      collected->session_id = g_strdup (event->session_id);
      break;

    case AI_EVENT_COMMANDS:
      collected->n_command_events++;
      g_strfreev (collected->commands);
      collected->commands = g_strdupv ((char **) event->commands);
      break;

    case AI_EVENT_TEXT_DELTA:
      collected->n_deltas++;
      g_string_append (collected->text, event->text);
      break;

    case AI_EVENT_TOOL_USE:
      collected->n_tools++;
      g_free (collected->last_tool);
      collected->last_tool = g_strdup (event->text);
      break;

    case AI_EVENT_USAGE:
      collected->n_usage++;
      collected->context_used = event->context_used;
      collected->context_window = event->context_window;
      break;

    case AI_EVENT_RESULT:
      collected->n_results++;
      if (event->session_id != NULL)
        {
          g_free (collected->session_id);
          collected->session_id = g_strdup (event->session_id);
        }
      break;

    case AI_EVENT_ERROR:
      collected->n_errors++;
      break;

    default:
      break;
    }
}

static void
collected_clear (Collected *collected)
{
  g_string_free (collected->text, TRUE);
  g_free (collected->session_id);
  g_free (collected->last_tool);
  g_strfreev (collected->commands);
}

/* Replays a captured CLI transcript through the backend's parser. */
static void
replay_fixture (const char *backend_id,
                const char *fixture,
                Collected  *collected)
{
  g_autoptr (AiParser) parser = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *path = NULL;
  g_autofree char *contents = NULL;
  g_auto (GStrv) lines = NULL;
  const AiBackend *backend;

  backend = ai_backend_lookup (backend_id);
  g_assert_nonnull (backend);

  path = g_build_filename (g_getenv ("G_TEST_SRCDIR"), "fixtures", fixture, NULL);
  g_assert_true (g_file_get_contents (path, &contents, NULL, &error));
  g_assert_no_error (error);

  collected->text = g_string_new (NULL);

  parser = ai_parser_new (backend);
  lines = g_strsplit (contents, "\n", -1);
  for (gsize i = 0; lines[i] != NULL; i++)
    ai_parser_feed_line (parser, lines[i], collect, collected);
}

/*
 * With --include-partial-messages, claude reports the reply twice: once as
 * token deltas and again as the finished assistant message. Emitting both
 * would show the answer twice over.
 */
static void
test_claude_stream (void)
{
  Collected collected = { 0 };

  replay_fixture ("claude", "claude-stream.jsonl", &collected);

  g_assert_cmpstr (collected.session_id, ==, "653dbf2a-6521-4412-9ac9-81b4d94160e7");
  g_assert_cmpstr (collected.text->str, ==, "hello from hy");
  g_assert_cmpuint (collected.n_usage, ==, 1);
  g_assert_cmpuint (collected.context_used, ==, 21335);
  g_assert_cmpuint (collected.context_window, ==, 1000000);
  g_assert_cmpuint (collected.n_results, ==, 1);
  g_assert_cmpuint (collected.n_errors, ==, 0);
  g_assert_cmpuint (collected.n_command_events, ==, 1);
  g_assert_true (g_strv_contains (
    (const char *const *) collected.commands, "simplify"));
  g_assert_true (g_strv_contains (
    (const char *const *) collected.commands, "review"));

  collected_clear (&collected);
}

/*
 * A real turn that used a tool, captured from the CLI.
 *
 * Tool calls are announced before their arguments exist and only described
 * once the block closes, so they are reported from a different event than the
 * one that names them. That is easy to break while changing how replies are
 * assembled, and nothing else in the transcript would show it.
 */
static void
test_claude_reports_tool_use (void)
{
  Collected collected = { 0 };

  replay_fixture ("claude", "claude-tool-use.jsonl", &collected);

  g_assert_cmpuint (collected.n_tools, >=, 1);
  g_assert_cmpuint (collected.n_results, ==, 1);
  g_assert_cmpuint (collected.n_errors, ==, 0);

  collected_clear (&collected);
}

static void
test_codex_stream (void)
{
  Collected collected = { 0 };
  g_autofree char *old_home = g_strdup (g_getenv ("CODEX_HOME"));
  g_autofree char *home = g_dir_make_tmp ("xd-codex-home-XXXXXX", NULL);
  g_autofree char *sessions = g_build_filename (home, "sessions", NULL);
  g_autofree char *rollout = g_build_filename (
    sessions,
    "rollout-019f9b16-df5f-7182-bdc6-1cce26148979.jsonl", NULL);

  g_assert_cmpint (g_mkdir_with_parents (sessions, 0700), ==, 0);
  g_assert_true (g_file_set_contents (
    rollout,
    "{\"payload\":{\"type\":\"token_count\",\"info\":{"
    "\"last_token_usage\":{\"total_tokens\":15555},"
    "\"model_context_window\":258400}}}\n",
    -1, NULL));
  g_setenv ("CODEX_HOME", home, TRUE);

  replay_fixture ("codex", "codex-exec.jsonl", &collected);

  g_assert_cmpstr (collected.session_id, ==, "019f9b16-df5f-7182-bdc6-1cce26148979");
  g_assert_cmpstr (collected.text->str, ==, "hello from hy");
  g_assert_cmpuint (collected.n_usage, ==, 1);
  g_assert_cmpuint (collected.context_used, ==, 15555);
  g_assert_cmpuint (collected.context_window, ==, 258400);
  g_assert_cmpuint (collected.n_results, ==, 1);
  g_assert_cmpuint (collected.n_errors, ==, 0);
  g_assert_cmpuint (collected.n_tools, ==, 1);

  collected_clear (&collected);
  if (old_home != NULL)
    g_setenv ("CODEX_HOME", old_home, TRUE);
  else
    g_unsetenv ("CODEX_HOME");
  g_assert_cmpint (g_remove (rollout), ==, 0);
  g_assert_cmpint (g_rmdir (sessions), ==, 0);
  g_assert_cmpint (g_rmdir (home), ==, 0);
}

/* Output the parser does not recognise must be skipped, not fatal: the CLIs
 * add event types over time and print the occasional stray line. */
static void
test_garbage_is_survivable (void)
{
  Collected collected = { 0 };
  g_autoptr (AiParser) parser = ai_parser_new (ai_backend_lookup ("claude"));
  const char *lines[] = {
    "",
    "not json at all",
    "{\"type\":\"something_new\",\"payload\":42}",
    "[1,2,3]",
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\","
      "\"delta\":{\"type\":\"text_delta\",\"text\":\"still here\"}}}",
  };

  collected.text = g_string_new (NULL);

  for (gsize i = 0; i < G_N_ELEMENTS (lines); i++)
    ai_parser_feed_line (parser, lines[i], collect, &collected);

  g_assert_cmpstr (collected.text->str, ==, "still here");

  collected_clear (&collected);
}

static char *
argv_to_string (const AiBackend *backend,
                const AiRunSpec *spec)
{
  g_autoptr (GPtrArray) argv = backend->build_argv (backend, spec);

  /* pdata ends with the NULL terminator, which is what g_strjoinv wants. */
  return g_strjoinv (" ", (char **) argv->pdata);
}

static void
test_claude_argv (void)
{
  const AiBackend *backend = ai_backend_lookup ("claude");
  AiRunSpec spec = { .prompt = "hello" };
  g_autofree char *plain = NULL;
  g_autofree char *resumed = NULL;

  plain = argv_to_string (backend, &spec);
  g_assert_nonnull (strstr (plain, "--output-format stream-json"));
  g_assert_nonnull (strstr (plain, "--verbose"));
  g_assert_null (strstr (plain, "--resume"));

  spec.resume_session_id = "sess-1";
  spec.model = "claude-opus-5";
  spec.system_prompt = "always answer in French";
  resumed = argv_to_string (backend, &spec);

  g_assert_nonnull (strstr (resumed, "--resume sess-1"));
  g_assert_nonnull (strstr (resumed, "--model claude-opus-5"));
  g_assert_nonnull (strstr (resumed, "--append-system-prompt always answer in French"));
}

static void
test_codex_argv (void)
{
  const AiBackend *backend = ai_backend_lookup ("codex");
  AiRunSpec spec = { .prompt = "hello" };
  g_autofree char *plain = NULL;
  g_autofree char *instructions = NULL;

  plain = argv_to_string (backend, &spec);
  g_assert_nonnull (strstr (plain, "app-server --listen stdio://"));
  g_assert_null (strstr (plain, "exec"));

  /* Instructions now travel as developerInstructions, separate from user
   * input, on thread/start and thread/resume. */
  spec.system_prompt = "be brief";
  instructions = xd_codex_developer_instructions (&spec);
  g_assert_nonnull (strstr (instructions, "be brief"));
  g_assert_nonnull (
    strstr (instructions, "Co-authored-by: Codex <codex@openai.com>"));
  g_assert_nonnull (
    strstr (instructions, "unless the user specifically asks you not to"));
}

/*
 * Access maps onto whatever each CLI calls it. The default has to be the
 * least permissive rung: an unrecognised value must never open the sandbox.
 */
static void
test_access_maps_to_each_cli (void)
{
  const AiBackend *claude = ai_backend_lookup ("claude");
  AiRunSpec spec = { .prompt = "hello" };

  struct { AiAccess access; const char *claude_flag; const char *codex_policy; } cases[] = {
    { AI_ACCESS_PLAN,      "--permission-mode plan",              "readOnly" },
    { AI_ACCESS_READ_ONLY, "--permission-mode manual",            "readOnly" },
    { AI_ACCESS_EDIT,      "--permission-mode acceptEdits",       "workspaceWrite" },
    { AI_ACCESS_FULL,      "--permission-mode bypassPermissions", "dangerFullAccess" },
  };

  for (gsize i = 0; i < G_N_ELEMENTS (cases); i++)
    {
      g_autofree char *claude_argv = NULL;

      spec.access = cases[i].access;
      claude_argv = argv_to_string (claude, &spec);

      g_assert_nonnull (strstr (claude_argv, cases[i].claude_flag));
      g_assert_cmpstr (xd_codex_sandbox_policy_type (cases[i].access), ==,
                       cases[i].codex_policy);
    }

  /* Codex has no plan mode, so planning has to be asked for in words. */
  spec.access = AI_ACCESS_PLAN;
  {
    g_autofree char *instructions =
      xd_codex_developer_instructions (&spec);

    g_assert_nonnull (strstr (instructions, "<plan_mode>"));
  }

  g_assert_cmpint (ai_access_from_string ("something-new"), ==, AI_ACCESS_READ_ONLY);
  g_assert_cmpint (ai_access_from_string (NULL), ==, AI_ACCESS_READ_ONLY);
}

static void
test_effort_maps_to_each_cli (void)
{
  const AiBackend *claude = ai_backend_lookup ("claude");
  AiRunSpec spec = { .prompt = "hello", .effort = AI_EFFORT_XHIGH };
  g_autofree char *claude_argv = argv_to_string (claude, &spec);
  g_autofree char *unset = NULL;

  g_assert_nonnull (strstr (claude_argv, "--effort xhigh"));

  /* Every chat names an effort, so the flag is always passed. */
  spec.effort = AI_EFFORT_LOW;
  unset = argv_to_string (claude, &spec);
  g_assert_nonnull (strstr (unset, "--effort low"));
}

/*
 * Tool calls have to be reported where they happened, not collected at the
 * end: the arguments only arrive after the block opens, so the call is
 * described when the block closes rather than from the finished message.
 */
static void
test_tool_calls_are_reported_in_order (void)
{
  g_autoptr (AiParser) parser = ai_parser_new (ai_backend_lookup ("claude"));
  Collected collected = { 0 };
  g_autoptr (GString) order = g_string_new (NULL);
  const char *lines[] = {
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_start\",\"index\":0,"
      "\"content_block\":{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{}}}}",
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"index\":0,"
      "\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file_path\\\":\"}}}",
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"index\":0,"
      "\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"src/main.c\\\"}\"}}}",
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\",\"index\":0}}",
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"index\":1,"
      "\"delta\":{\"type\":\"text_delta\",\"text\":\"done\"}}}",
  };

  collected.text = g_string_new (NULL);

  for (gsize i = 0; i < G_N_ELEMENTS (lines); i++)
    ai_parser_feed_line (parser, lines[i], collect, &collected);

  /* The argument reached the summary, so the row says what was read. */
  g_assert_cmpuint (collected.n_tools, ==, 1);
  g_assert_cmpstr (collected.text->str, ==, "done");

  collected_clear (&collected);
}

/*
 * Tool-only assistant messages still have a finished-message event even when
 * their streamed block already emitted the call. No text delta exists to
 * trigger the older duplicate guard.
 */
static void
test_tool_only_message_is_not_repeated (void)
{
  g_autoptr (AiParser) parser = ai_parser_new (ai_backend_lookup ("claude"));
  Collected collected = { .text = g_string_new (NULL) };
  const char *lines[] = {
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_start\","
      "\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"name\":\"Agent\","
      "\"input\":{}}}}",
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\","
      "\"index\":0,\"delta\":{\"type\":\"input_json_delta\","
      "\"partial_json\":\"{\\\"subagent_type\\\":\\\"Explore\\\","
      "\\\"description\\\":\\\"Explore module patterns\\\"}\"}}}",
    "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\","
      "\"name\":\"Agent\",\"input\":{\"subagent_type\":\"Explore\","
      "\"description\":\"Explore module patterns\"}}]}}",
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\","
      "\"index\":0}}",
  };

  for (gsize i = 0; i < G_N_ELEMENTS (lines); i++)
    ai_parser_feed_line (parser, lines[i], collect, &collected);

  g_assert_cmpuint (collected.n_tools, ==, 1);
  collected_clear (&collected);
}

/*
 * Claude asks to edit first and executes afterwards. The file-change event
 * must land after tool_result, when the diff tracker can see the new file.
 */
static void
test_claude_defers_file_changes_until_tool_result (void)
{
  g_autoptr (AiParser) parser = ai_parser_new (ai_backend_lookup ("claude"));
  Collected collected = { .text = g_string_new (NULL) };
  const char *request[] = {
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_start\","
      "\"index\":0,\"content_block\":{\"type\":\"tool_use\","
      "\"id\":\"toolu_edit\",\"name\":\"Edit\",\"input\":{}}}}",
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\","
      "\"index\":0,\"delta\":{\"type\":\"input_json_delta\","
      "\"partial_json\":\"{\\\"file_path\\\":\\\"src/main.c\\\"}\"}}}",
    "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\","
      "\"id\":\"toolu_edit\",\"name\":\"Edit\","
      "\"input\":{\"file_path\":\"src/main.c\"}}]}}",
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\","
      "\"index\":0}}",
  };
  const char *result =
    "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\","
    "\"tool_use_id\":\"toolu_edit\",\"content\":\"updated\"}]}}";

  for (gsize i = 0; i < G_N_ELEMENTS (request); i++)
    ai_parser_feed_line (parser, request[i], collect, &collected);

  g_assert_cmpuint (collected.n_tools, ==, 0);

  ai_parser_feed_line (parser, result, collect, &collected);

  g_assert_cmpuint (collected.n_tools, ==, 1);
  g_assert_cmpstr (collected.last_tool, ==, "file_change  src/main.c");

  collected_clear (&collected);
}

static void
test_tool_summary_names_the_work (void)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  g_autofree char *bash = NULL;
  g_autofree char *bare = NULL;
  g_autofree char *file_change = NULL;
  g_autofree char *edit = NULL;
  g_autofree char *write = NULL;
  g_autofree char *subagent = NULL;
  g_autofree char *subagent_identity = NULL;
  g_autofree char *subagent_task = NULL;

  g_assert_true (json_parser_load_from_data (
    parser, "{\"command\":\"git status\",\"file_path\":\"ignored\"}", -1, NULL));

  /* A command is shown as one: the name of the tool that ran it says nothing
   * the command does not. */
  bash = ai_tool_summary ("Bash", json_node_get_object (json_parser_get_root (parser)));
  g_assert_cmpstr (bash, ==, "$ git status");

  /* And the shell it was run through is the wrapper, not the work. */
  {
    g_autoptr (JsonParser) shell = json_parser_new ();
    g_autofree char *summary = NULL;

    g_assert_true (json_parser_load_from_data (
      shell,
      "{\"command\":\"/run/current-system/sw/bin/bash -lc \\\"rg -n 'x|y' src/\\\"\"}",
      -1, NULL));

    summary = ai_tool_summary ("command_execution",
                               json_node_get_object (json_parser_get_root (shell)));
    g_assert_cmpstr (summary, ==, "$ rg -n 'x|y' src/");
  }

  /* Nothing identifying: the name alone, never a dangling separator. */
  bare = ai_tool_summary ("Think", NULL);
  g_assert_cmpstr (bare, ==, "Think");

  /* file_change has structured changes rather than one path. Its stable name
   * is what the chat uses to replace the dead tool line with the diff pane. */
  file_change = ai_tool_summary ("file_change", NULL);
  g_assert_cmpstr (file_change, ==, "file_change");

  {
    g_autoptr (JsonParser) file = json_parser_new ();

    g_assert_true (json_parser_load_from_data (
      file, "{\"file_path\":\"src/main.c\"}", -1, NULL));
    edit = ai_tool_summary (
      "Edit", json_node_get_object (json_parser_get_root (file)));
    write = ai_tool_summary (
      "write", json_node_get_object (json_parser_get_root (file)));
    g_assert_cmpstr (edit, ==, "file_change  src/main.c");
    g_assert_cmpstr (write, ==, "file_change  src/main.c");
  }

  {
    g_autoptr (JsonParser) agent = json_parser_new ();

    g_assert_true (json_parser_load_from_data (
      agent,
      "{\"subagent_type\":\"Explore\","
      "\"description\":\"Trace the storage layer\"}",
      -1, NULL));

    subagent = ai_tool_summary (
      "Agent", json_node_get_object (json_parser_get_root (agent)));
    g_assert_true (xd_subagent_tool_from_tool (
      subagent, &subagent_identity, &subagent_task));
    g_assert_cmpstr (subagent_identity, ==, "Explore");
    g_assert_cmpstr (subagent_task, ==, "Trace the storage layer");
  }

  {
    g_autoptr (JsonParser) collab = json_parser_new ();

    g_clear_pointer (&subagent, g_free);
    g_clear_pointer (&subagent_identity, g_free);
    g_clear_pointer (&subagent_task, g_free);

    g_assert_true (json_parser_load_from_data (
      collab,
      "{\"tool\":\"spawnAgent\",\"model\":\"gpt-5\","
      "\"prompt\":\"Review the diff\"}",
      -1, NULL));

    subagent = ai_tool_summary (
      "collab_tool_call",
      json_node_get_object (json_parser_get_root (collab)));
    g_assert_true (xd_subagent_tool_from_tool (
      subagent, &subagent_identity, &subagent_task));
    g_assert_cmpstr (subagent_identity, ==, "gpt-5");
    g_assert_cmpstr (subagent_task, ==, "Review the diff");
  }
}

static void
test_unknown_backend (void)
{
  g_assert_null (ai_backend_lookup ("gpt-9"));
  g_assert_null (ai_backend_lookup (NULL));
}

/*
 * Every model is named and every backend nominates one for new chats, so a
 * chat can always say which model answered it. The nominated model has to be
 * one the backend actually lists, or the picker would open on nothing.
 */
static void
test_every_backend_names_its_models (void)
{
  const AiBackend *const *backends;
  guint n_backends;

  backends = ai_backend_all (&n_backends);
  g_assert_cmpuint (n_backends, >, 0);

  for (guint i = 0; i < n_backends; i++)
    {
      const AiBackend *backend = backends[i];
      gboolean default_is_listed = FALSE;

      g_assert_cmpuint (backend->n_models, >, 0);
      g_assert_nonnull (backend->icon_name);
      g_assert_nonnull (backend->default_model);

      for (gsize m = 0; m < backend->n_models; m++)
        {
          g_assert_nonnull (backend->models[m].id);
          g_assert_nonnull (backend->models[m].display_name);

          if (g_strcmp0 (backend->models[m].id, backend->default_model) == 0)
            default_is_listed = TRUE;
        }

      g_assert_true (default_is_listed);
    }
}

static void
test_model_labels (void)
{
  const AiBackend *claude = ai_backend_lookup ("claude");

  g_assert_cmpstr (ai_backend_model_label (claude, "claude-opus-5"), ==,
                   "Claude Opus 5");
  /* Chats created before models were pinned have none stored, and read back
   * as the backend's own default rather than as a blank. */
  g_assert_cmpstr (ai_backend_model_label (claude, NULL), ==, "Claude Opus 5");

  /* A model set by hand, or one released after this build, still reads back
   * as something rather than blank. */
  g_assert_cmpstr (ai_backend_model_label (claude, "claude-from-the-future"), ==,
                   "claude-from-the-future");
}

/* A model id must reach the CLI as the flag it actually understands. */
static void
test_model_reaches_argv (void)
{
  const AiBackend *claude = ai_backend_lookup ("claude");
  AiRunSpec spec = { .prompt = "hello", .model = "claude-opus-5" };
  g_autofree char *claude_argv = argv_to_string (claude, &spec);

  g_assert_nonnull (strstr (claude_argv, "--model claude-opus-5"));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/backend/claude/stream", test_claude_stream);
  g_test_add_func ("/backend/claude/argv", test_claude_argv);
  g_test_add_func ("/backend/codex/stream", test_codex_stream);
  g_test_add_func ("/backend/codex/argv", test_codex_argv);
  g_test_add_func ("/backend/garbage", test_garbage_is_survivable);
  g_test_add_func ("/backend/unknown", test_unknown_backend);
  g_test_add_func ("/backend/models/named", test_every_backend_names_its_models);
  g_test_add_func ("/backend/models/labels", test_model_labels);
  g_test_add_func ("/backend/models/argv", test_model_reaches_argv);
  g_test_add_func ("/backend/access", test_access_maps_to_each_cli);
  g_test_add_func ("/backend/effort", test_effort_maps_to_each_cli);
  g_test_add_func ("/backend/tools/order", test_tool_calls_are_reported_in_order);
  g_test_add_func ("/backend/tools/tool-only-once",
                   test_tool_only_message_is_not_repeated);
  g_test_add_func ("/backend/tools/file-change-after-result",
                   test_claude_defers_file_changes_until_tool_result);
  g_test_add_func ("/backend/tools/live", test_claude_reports_tool_use);
  g_test_add_func ("/backend/tools/summary", test_tool_summary_names_the_work);

  return g_test_run ();
}
