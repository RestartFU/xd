#include "chat/chat-session.h"
#include "settings/agent-secrets.h"

#include <glib/gstdio.h>
#include <stdio.h>

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
  GStrv commands;
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

extern const AiBackend xd_claude_backend;
extern const AiBackend xd_codex_backend;

static void
stub_parse_object (AiParser    *parser,
                   JsonObject  *root,
                   AiEventFunc  callback,
                   gpointer     user_data)
{
  xd_claude_backend.parse_object (parser, root, callback, user_data);
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
  .program = "xd-definitely-not-installed",
  .build_argv = stub_build_argv,
  .parse_object = stub_parse_object,
};

static void
codex_parse_object (AiParser    *parser,
                    JsonObject  *root,
                    AiEventFunc  callback,
                    gpointer     user_data)
{
  xd_codex_backend.parse_object (parser, root, callback, user_data);
}

static const AiBackend codex_stub_backend = {
  .id = "codex-stub",
  .display_name = "Codex Stub",
  .program = "cat",
  .build_argv = stub_build_argv,
  .parse_object = codex_parse_object,
};

static char *secret_child_program;
static char *secret_system_prompt;

static GPtrArray *
secret_build_argv (const AiBackend *self,
                   const AiRunSpec *spec)
{
  GPtrArray *argv = g_ptr_array_new_with_free_func (g_free);

  g_free (secret_system_prompt);
  secret_system_prompt = g_strdup (spec->system_prompt);

  g_ptr_array_add (argv, g_strdup (secret_child_program));
  g_ptr_array_add (argv, g_strdup ("--secret-child"));
  g_ptr_array_add (argv, g_strdup (spec->prompt));
  g_ptr_array_add (argv, NULL);

  return argv;
}

static const AiBackend secret_backend = {
  .id = "secret-stub",
  .display_name = "Secret Stub",
  .program = "test-session",
  .build_argv = secret_build_argv,
  .parse_object = stub_parse_object,
};

static void
on_session_started (XdChatSession *session,
                    const char    *session_id,
                    gpointer       user_data)
{
  Run *run = user_data;

  g_free (run->session_id);
  run->session_id = g_strdup (session_id);
}

static void
on_commands (XdChatSession    *session,
             const char *const *commands,
             gpointer          user_data)
{
  Run *run = user_data;

  g_strfreev (run->commands);
  run->commands = g_strdupv ((char **) commands);
}

static void
on_text_delta (XdChatSession *session,
               const char    *delta,
               gpointer       user_data)
{
  Run *run = user_data;

  g_string_append (run->text, delta);
}

static void
on_finished (XdChatSession *session,
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
          XdChatSession *session)
{
  run->loop = g_main_loop_new (NULL, FALSE);
  run->text = g_string_new (NULL);

  g_signal_connect (session, "session-started", G_CALLBACK (on_session_started), run);
  g_signal_connect (session, "commands", G_CALLBACK (on_commands), run);
  g_signal_connect (session, "text-delta", G_CALLBACK (on_text_delta), run);
  g_signal_connect (session, "finished", G_CALLBACK (on_finished), run);
}

static void
run_clear (Run *run)
{
  g_main_loop_unref (run->loop);
  g_string_free (run->text, TRUE);
  g_free (run->session_id);
  g_strfreev (run->commands);
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
  g_autoptr (XdChatSession) session = xd_chat_session_new (&stub_backend);
  g_autoptr (GError) error = NULL;
  g_autofree char *fixture = NULL;
  AiRunSpec spec = { 0 };
  Run run = { 0 };

  fixture = g_build_filename (g_getenv ("G_TEST_SRCDIR"), "fixtures",
                              "claude-stream.jsonl", NULL);
  spec.prompt = fixture;

  run_init (&run, session);

  g_assert_true (xd_chat_session_start (session, &spec, &error));
  g_assert_no_error (error);

  g_timeout_add_seconds (10, on_timeout, &run);
  g_main_loop_run (run.loop);

  g_assert_true (run.finished);
  g_assert_true (run.success);
  g_assert_cmpstr (run.text->str, ==, "hello from hy");
  g_assert_cmpstr (run.session_id, ==, "653dbf2a-6521-4412-9ac9-81b4d94160e7");
  g_assert_true (g_strv_contains (
    (const char *const *) run.commands, "simplify"));

  run_clear (&run);
}

/* The most likely failure in the wild is simply not having the CLI, and the
 * message has to say so instead of leaking a spawn error code. */
static void
test_missing_program_explains_itself (void)
{
  g_autoptr (XdChatSession) session = xd_chat_session_new (&missing_backend);
  g_autoptr (GError) error = NULL;
  AiRunSpec spec = { .prompt = "hello" };

  g_assert_false (xd_chat_session_start (session, &spec, &error));
  g_assert_error (error, G_SPAWN_ERROR, G_SPAWN_ERROR_NOENT);
  g_assert_nonnull (strstr (error->message, "not in PATH"));
}

static void
test_nonzero_exit_is_a_failure (void)
{
  g_autoptr (XdChatSession) session = NULL;
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

  session = xd_chat_session_new (&failing);
  spec.prompt = "/definitely/not/a/file";

  run_init (&run, session);

  g_assert_true (xd_chat_session_start (session, &spec, &error));
  g_assert_no_error (error);

  g_timeout_add_seconds (10, on_timeout, &run);
  g_main_loop_run (run.loop);

  g_assert_true (run.finished);
  g_assert_false (run.success);
  /* cat's own complaint is what reaches the user. */
  g_assert_cmpuint (strlen (run.message), >, 0);

  run_clear (&run);
}

static void
test_recoverable_backend_error_does_not_end_turn (void)
{
  g_autoptr (XdChatSession) session =
    xd_chat_session_new (&codex_stub_backend);
  g_autoptr (GError) error = NULL;
  g_autofree char *fixture = NULL;
  AiRunSpec spec = { 0 };
  Run run = { 0 };

  fixture = g_build_filename (g_getenv ("G_TEST_SRCDIR"), "fixtures",
                              "codex-recoverable-error.jsonl", NULL);
  spec.prompt = fixture;
  run_init (&run, session);

  g_assert_true (xd_chat_session_start (session, &spec, &error));
  g_assert_no_error (error);

  g_timeout_add_seconds (10, on_timeout, &run);
  g_main_loop_run (run.loop);

  g_assert_true (run.finished);
  g_assert_true (run.success);
  g_assert_cmpstr (run.text->str, ==, "still working");

  run_clear (&run);
}

static void
test_agent_secret_reaches_process_not_prompt (void)
{
  g_autoptr (XdChatSession) session = xd_chat_session_new (&secret_backend);
  g_autoptr (XdAgentSecrets) secrets = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *directory = NULL;
  g_autofree char *path = NULL;
  g_autofree char *scope_directory = NULL;
  g_autofree char *scope_digest = NULL;
  g_autofree char *scope_filename = NULL;
  g_autofree char *scope_path = NULL;
  g_autofree char *fixture = NULL;
  const char *folder_ids[] = { "session-folder", NULL };
  AiRunSpec spec = { .system_prompt = "existing instructions" };
  Run run = { 0 };

  directory = g_dir_make_tmp ("xd-session-secrets-XXXXXX", &error);
  g_assert_no_error (error);
  path = g_build_filename (directory, "agent-secrets.json", NULL);
  g_setenv ("XD_AGENT_SECRETS_FILE", path, TRUE);
  secrets = xd_agent_secrets_load_for_folder (folder_ids[0], &error);
  g_assert_no_error (error);
  g_assert_true (
    xd_agent_secrets_set (secrets, "XD_TEST_TOKEN", "super-secret", &error));
  g_assert_true (xd_agent_secrets_save (secrets, &error));
  g_assert_no_error (error);

  fixture = g_build_filename (g_getenv ("G_TEST_SRCDIR"), "fixtures",
                              "claude-stream.jsonl", NULL);
  spec.prompt = fixture;
  spec.folder_ids = folder_ids;

  run_init (&run, session);
  g_assert_true (xd_chat_session_start (session, &spec, &error));
  g_assert_no_error (error);

  g_timeout_add_seconds (10, on_timeout, &run);
  g_main_loop_run (run.loop);

  g_assert_true (run.finished);
  g_assert_true (run.success);
  g_assert_nonnull (strstr (secret_system_prompt, "existing instructions"));
  g_assert_nonnull (strstr (secret_system_prompt, "XD_TEST_TOKEN"));
  g_assert_null (strstr (secret_system_prompt, "super-secret"));

  run_clear (&run);
  g_unsetenv ("XD_AGENT_SECRETS_FILE");
  scope_directory = g_strconcat (path, ".d", NULL);
  scope_digest =
    g_compute_checksum_for_string (G_CHECKSUM_SHA256, folder_ids[0], -1);
  scope_filename = g_strconcat (scope_digest, ".json", NULL);
  scope_path = g_build_filename (scope_directory, scope_filename, NULL);
  g_remove (scope_path);
  g_rmdir (scope_directory);
  g_remove (path);
  g_rmdir (directory);
}

int
main (int   argc,
      char *argv[])
{
  if (argc == 3 && g_strcmp0 (argv[1], "--secret-child") == 0)
    {
      g_autofree char *contents = NULL;
      gsize length = 0;

      if (g_strcmp0 (g_getenv ("XD_TEST_TOKEN"), "super-secret") != 0 ||
          !g_file_get_contents (argv[2], &contents, &length, NULL))
        return 9;

      return fwrite (contents, 1, length, stdout) == length ? 0 : 10;
    }

  secret_child_program = argv[0];
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/session/streams", test_streams_a_transcript);
  g_test_add_func ("/session/missing-program", test_missing_program_explains_itself);
  g_test_add_func ("/session/nonzero-exit", test_nonzero_exit_is_a_failure);
  g_test_add_func ("/session/recoverable-backend-error",
                   test_recoverable_backend_error_does_not_end_turn);
  g_test_add_func ("/session/agent-secret-environment",
                   test_agent_secret_reaches_process_not_prompt);

  {
    int status = g_test_run ();

    g_free (secret_system_prompt);
    return status;
  }
}
