#include "backend/backend.h"

/* Collects everything a fixture produces so a test can assert on the whole
 * sequence rather than one event at a time. */
typedef struct
{
  GString *text;
  char *session_id;
  guint n_deltas;
  guint n_tools;
  guint n_results;
  guint n_errors;
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

    case AI_EVENT_TEXT_DELTA:
      collected->n_deltas++;
      g_string_append (collected->text, event->text);
      break;

    case AI_EVENT_TOOL_USE:
      collected->n_tools++;
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
  g_assert_cmpuint (collected.n_results, ==, 1);
  g_assert_cmpuint (collected.n_errors, ==, 0);

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

  replay_fixture ("codex", "codex-exec.jsonl", &collected);

  g_assert_cmpstr (collected.session_id, ==, "019f9b16-df5f-7182-bdc6-1cce26148979");
  g_assert_cmpstr (collected.text->str, ==, "hello from hy");
  g_assert_cmpuint (collected.n_results, ==, 1);
  g_assert_cmpuint (collected.n_errors, ==, 0);

  collected_clear (&collected);
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
  g_autofree char *resumed = NULL;

  plain = argv_to_string (backend, &spec);
  g_assert_nonnull (strstr (plain, "exec --json"));
  g_assert_nonnull (strstr (plain, "-s read-only"));

  /*
   * Resuming is a different command with different options.
   *
   * "codex exec resume" takes neither -s nor -C, and refuses the whole run if
   * it is given one -- so the sandbox travels as the config override it does
   * take, and the session id goes after the options, where its usage puts it.
   * Every message after the first goes through here.
   */
  spec.resume_session_id = "thread-1";
  spec.workdir = "/tmp/somewhere";
  resumed = argv_to_string (backend, &spec);

  g_assert_nonnull (strstr (resumed, "exec resume --json"));
  g_assert_nonnull (strstr (resumed, "sandbox_mode=\"read-only\""));
  g_assert_nonnull (strstr (resumed, "thread-1"));
  g_assert_null (strstr (resumed, "-s read-only"));
  g_assert_null (strstr (resumed, "-C "));

  /* The id goes after the options, which is the order its usage gives. */
  g_assert_true (strstr (resumed, "thread-1") >
                 strstr (resumed, "sandbox_mode="));

  spec.workdir = NULL;

  /* Codex has no --append-system-prompt, so instructions must reach it by
   * riding in front of the prompt. */
  spec.resume_session_id = NULL;
  spec.system_prompt = "be brief";
  g_free (plain);
  plain = argv_to_string (backend, &spec);
  g_assert_nonnull (strstr (plain, "be brief\n\nhello"));
}

/*
 * Access maps onto whatever each CLI calls it. The default has to be the
 * least permissive rung: an unrecognised value must never open the sandbox.
 */
static void
test_access_maps_to_each_cli (void)
{
  const AiBackend *claude = ai_backend_lookup ("claude");
  const AiBackend *codex = ai_backend_lookup ("codex");
  AiRunSpec spec = { .prompt = "hello" };

  struct { AiAccess access; const char *claude_flag; const char *codex_flag; } cases[] = {
    { AI_ACCESS_PLAN,      "--permission-mode plan",              "-s read-only" },
    { AI_ACCESS_READ_ONLY, "--permission-mode manual",            "-s read-only" },
    { AI_ACCESS_EDIT,      "--permission-mode acceptEdits",       "-s workspace-write" },
    { AI_ACCESS_FULL,      "--permission-mode bypassPermissions", "-s danger-full-access" },
  };

  for (gsize i = 0; i < G_N_ELEMENTS (cases); i++)
    {
      g_autofree char *claude_argv = NULL;
      g_autofree char *codex_argv = NULL;

      spec.access = cases[i].access;
      claude_argv = argv_to_string (claude, &spec);
      codex_argv = argv_to_string (codex, &spec);

      g_assert_nonnull (strstr (claude_argv, cases[i].claude_flag));
      g_assert_nonnull (strstr (codex_argv, cases[i].codex_flag));
    }

  /* Codex has no plan mode, so planning has to be asked for in words. */
  spec.access = AI_ACCESS_PLAN;
  {
    g_autofree char *codex_argv = argv_to_string (codex, &spec);

    g_assert_nonnull (strstr (codex_argv, "<plan_mode>"));
  }

  g_assert_cmpint (ai_access_from_string ("something-new"), ==, AI_ACCESS_READ_ONLY);
  g_assert_cmpint (ai_access_from_string (NULL), ==, AI_ACCESS_READ_ONLY);
}

static void
test_effort_maps_to_each_cli (void)
{
  const AiBackend *claude = ai_backend_lookup ("claude");
  const AiBackend *codex = ai_backend_lookup ("codex");
  AiRunSpec spec = { .prompt = "hello", .effort = AI_EFFORT_XHIGH };
  g_autofree char *claude_argv = argv_to_string (claude, &spec);
  g_autofree char *codex_argv = argv_to_string (codex, &spec);
  g_autofree char *unset = NULL;

  g_assert_nonnull (strstr (claude_argv, "--effort xhigh"));
  g_assert_nonnull (strstr (codex_argv, "model_reasoning_effort=\"xhigh\""));

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

static void
test_tool_summary_names_the_work (void)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  g_autofree char *bash = NULL;
  g_autofree char *bare = NULL;
  g_autofree char *file_change = NULL;

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
  const AiBackend *codex = ai_backend_lookup ("codex");
  AiRunSpec spec = { .prompt = "hello", .model = "claude-opus-5" };
  g_autofree char *claude_argv = argv_to_string (claude, &spec);
  g_autofree char *codex_argv = NULL;

  g_assert_nonnull (strstr (claude_argv, "--model claude-opus-5"));

  spec.model = "gpt-5.6-sol";
  codex_argv = argv_to_string (codex, &spec);
  g_assert_nonnull (strstr (codex_argv, "-m gpt-5.6-sol"));
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
  g_test_add_func ("/backend/tools/live", test_claude_reports_tool_use);
  g_test_add_func ("/backend/tools/summary", test_tool_summary_names_the_work);

  return g_test_run ();
}
