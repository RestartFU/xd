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

  spec.resume_session_id = "thread-1";
  resumed = argv_to_string (backend, &spec);
  g_assert_nonnull (strstr (resumed, "exec resume thread-1"));

  /* Codex has no --append-system-prompt, so instructions must reach it by
   * riding in front of the prompt. */
  spec.resume_session_id = NULL;
  spec.system_prompt = "be brief";
  g_free (plain);
  plain = argv_to_string (backend, &spec);
  g_assert_nonnull (strstr (plain, "be brief\n\nhello"));
}

static void
test_unknown_backend (void)
{
  g_assert_null (ai_backend_lookup ("gpt-9"));
  g_assert_null (ai_backend_lookup (NULL));
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

  return g_test_run ();
}
