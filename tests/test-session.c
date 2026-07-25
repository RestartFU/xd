#include "chat/chat-session.h"

/*
 * Exercises the spawn/read/parse/finish path without going near a real AI CLI:
 * the stub backend replays a captured transcript through `cat`, so the test is
 * hermetic and needs no credentials.
 */

typedef struct
{
  GMainLoop *loop;
  GString *text;
  char *session_id;
  gboolean finished;
  gboolean success;
  char *message;
} Run;

static GPtrArray *
stub_build_argv (const AiBackend *self,
                 const AiRunSpec *spec)
{
  GPtrArray *argv = g_ptr_array_new_with_free_func (g_free);

  /* The prompt carries the fixture to replay. */
  g_ptr_array_add (argv, g_strdup (self->program));
  g_ptr_array_add (argv, g_strdup (spec->prompt));
  g_ptr_array_add (argv, NULL);

  return argv;
}

extern const AiBackend hy_claude_backend;

static void
stub_parse_object (AiParser    *parser,
                   JsonObject  *root,
                   AiEventFunc  callback,
                   gpointer     user_data)
{
  hy_claude_backend.parse_object (parser, root, callback, user_data);
}

static const AiBackend stub_backend = {
  .id = "stub",
  .display_name = "Stub",
  .program = "cat",
  .build_argv = stub_build_argv,
  .parse_object = stub_parse_object,
};

static const AiBackend missing_backend = {
  .id = "missing",
  .display_name = "Missing",
  .program = "hy-definitely-not-installed",
  .build_argv = stub_build_argv,
  .parse_object = stub_parse_object,
};

static void
on_session_started (HyChatSession *session,
                    const char    *session_id,
                    gpointer       user_data)
{
  Run *run = user_data;

  g_free (run->session_id);
  run->session_id = g_strdup (session_id);
}

static void
on_text_delta (HyChatSession *session,
               const char    *delta,
               gpointer       user_data)
{
  Run *run = user_data;

  g_string_append (run->text, delta);
}

static void
on_finished (HyChatSession *session,
             gboolean       success,
             const char    *message,
             gpointer       user_data)
{
  Run *run = user_data;

  run->finished = TRUE;
  run->success = success;
  run->message = g_strdup (message);

  g_main_loop_quit (run->loop);
}

static void
run_init (Run           *run,
          HyChatSession *session)
{
  run->loop = g_main_loop_new (NULL, FALSE);
  run->text = g_string_new (NULL);

  g_signal_connect (session, "session-started", G_CALLBACK (on_session_started), run);
  g_signal_connect (session, "text-delta", G_CALLBACK (on_text_delta), run);
  g_signal_connect (session, "finished", G_CALLBACK (on_finished), run);
}

static void
run_clear (Run *run)
{
  g_main_loop_unref (run->loop);
  g_string_free (run->text, TRUE);
  g_free (run->session_id);
  g_free (run->message);
}

/* Stops a wedged test rather than letting the suite hang forever. */
static gboolean
on_timeout (gpointer user_data)
{
  Run *run = user_data;

  g_main_loop_quit (run->loop);

  return G_SOURCE_REMOVE;
}

static void
test_streams_a_transcript (void)
{
  g_autoptr (HyChatSession) session = hy_chat_session_new (&stub_backend);
  g_autoptr (GError) error = NULL;
  g_autofree char *fixture = NULL;
  AiRunSpec spec = { 0 };
  Run run = { 0 };

  fixture = g_build_filename (g_getenv ("G_TEST_SRCDIR"), "fixtures",
                              "claude-stream.jsonl", NULL);
  spec.prompt = fixture;

  run_init (&run, session);

  g_assert_true (hy_chat_session_start (session, &spec, &error));
  g_assert_no_error (error);

  g_timeout_add_seconds (10, on_timeout, &run);
  g_main_loop_run (run.loop);

  g_assert_true (run.finished);
  g_assert_true (run.success);
  g_assert_cmpstr (run.text->str, ==, "hello from hy");
  g_assert_cmpstr (run.session_id, ==, "653dbf2a-6521-4412-9ac9-81b4d94160e7");

  run_clear (&run);
}

/* The most likely failure in the wild is simply not having the CLI, and the
 * message has to say so instead of leaking a spawn error code. */
static void
test_missing_program_explains_itself (void)
{
  g_autoptr (HyChatSession) session = hy_chat_session_new (&missing_backend);
  g_autoptr (GError) error = NULL;
  AiRunSpec spec = { .prompt = "hello" };

  g_assert_false (hy_chat_session_start (session, &spec, &error));
  g_assert_error (error, G_SPAWN_ERROR, G_SPAWN_ERROR_NOENT);
  g_assert_nonnull (strstr (error->message, "not in PATH"));
}

static void
test_nonzero_exit_is_a_failure (void)
{
  g_autoptr (HyChatSession) session = NULL;
  g_autoptr (GError) error = NULL;
  AiRunSpec spec = { 0 };
  Run run = { 0 };
  static const AiBackend failing = {
    .id = "failing",
    .display_name = "Failing",
    .program = "cat",
    .build_argv = stub_build_argv,
    .parse_object = stub_parse_object,
  };

  session = hy_chat_session_new (&failing);
  spec.prompt = "/definitely/not/a/file";

  run_init (&run, session);

  g_assert_true (hy_chat_session_start (session, &spec, &error));
  g_assert_no_error (error);

  g_timeout_add_seconds (10, on_timeout, &run);
  g_main_loop_run (run.loop);

  g_assert_true (run.finished);
  g_assert_false (run.success);
  /* cat's own complaint is what reaches the user. */
  g_assert_cmpuint (strlen (run.message), >, 0);

  run_clear (&run);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/session/streams", test_streams_a_transcript);
  g_test_add_func ("/session/missing-program", test_missing_program_explains_itself);
  g_test_add_func ("/session/nonzero-exit", test_nonzero_exit_is_a_failure);

  return g_test_run ();
}
