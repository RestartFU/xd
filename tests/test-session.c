#include "chat/chat-session.h"
#include "backend/codex-app-server.h"
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
  char *last_tool;
  guint64 context_used;
  guint64 context_window;
  guint timeout_id;
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
static char *app_server_child_program;
static char *app_server_count_file;

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

static GPtrArray *
app_server_build_argv (const AiBackend *self,
                       const AiRunSpec *spec)
{
  GPtrArray *argv = g_ptr_array_new_with_free_func (g_free);

  g_ptr_array_add (argv, g_strdup (app_server_child_program));
  g_ptr_array_add (argv, g_strdup ("--app-server-child"));
  g_ptr_array_add (argv, NULL);
  return argv;
}

static const AiBackend app_server_backend = {
  .id = "app-server-stub",
  .display_name = "App Server Stub",
  .program = "test-session-app-server",
  .transport = AI_TRANSPORT_CODEX_APP_SERVER,
  .build_argv = app_server_build_argv,
  .parse_object = codex_parse_object,
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
on_tool_use (XdChatSession *session,
             const char    *summary,
             gpointer       user_data)
{
  Run *run = user_data;

  g_free (run->last_tool);
  run->last_tool = g_strdup (summary);
}

static void
on_usage (XdChatSession *session,
          guint64        context_used,
          guint64        context_window,
          gpointer       user_data)
{
  Run *run = user_data;

  run->context_used = context_used;
  run->context_window = context_window;
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

  g_clear_handle_id (&run->timeout_id, g_source_remove);
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
  g_signal_connect (session, "tool-use", G_CALLBACK (on_tool_use), run);
  g_signal_connect (session, "usage", G_CALLBACK (on_usage), run);
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
  g_free (run->last_tool);
}

/* Stops a wedged test rather than letting the suite hang forever. */
static gboolean
on_timeout (gpointer user_data)
{
  Run *run = user_data;

  run->timeout_id = 0;
  g_main_loop_quit (run->loop);

  return G_SOURCE_REMOVE;
}

static void
run_wait (Run *run)
{
  run->timeout_id = g_timeout_add_seconds (10, on_timeout, run);
  g_main_loop_run (run->loop);
  g_clear_handle_id (&run->timeout_id, g_source_remove);
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

  run_wait (&run);

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

  run_wait (&run);

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

  run_wait (&run);

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

  run_wait (&run);

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

static gboolean
cancel_session (gpointer user_data)
{
  xd_chat_session_cancel (user_data);
  return G_SOURCE_REMOVE;
}

static guint
count_lines (const char *text)
{
  guint lines = 0;

  for (const char *at = text; at != NULL && *at != '\0'; at++)
    if (*at == '\n')
      lines++;
  return lines;
}

static void
test_app_server_streams_resumes_and_interrupts (void)
{
  g_autoptr (XdChatSession) first =
    xd_chat_session_new (&app_server_backend);
  g_autoptr (XdChatSession) second =
    xd_chat_session_new (&app_server_backend);
  g_autoptr (XdChatSession) cancelled =
    xd_chat_session_new (&app_server_backend);
  g_autoptr (GError) error = NULL;
  g_autoptr (XdAgentSecrets) secrets = NULL;
  g_autofree char *secrets_directory = NULL;
  g_autofree char *secrets_path = NULL;
  g_autofree char *count = NULL;
  AiRunSpec spec = {
    .prompt = "hello",
    .model = "gpt-test",
    .system_prompt = "test instructions",
    .workdir = "/tmp",
    .effort = AI_EFFORT_XHIGH,
    .access = AI_ACCESS_EDIT,
  };
  Run first_run = { 0 };
  Run second_run = { 0 };
  Run cancel_run = { 0 };

  secrets_directory = g_dir_make_tmp ("xd-app-server-secrets-XXXXXX", &error);
  g_assert_no_error (error);
  secrets_path =
    g_build_filename (secrets_directory, "agent-secrets.json", NULL);
  g_setenv ("XD_AGENT_SECRETS_FILE", secrets_path, TRUE);
  secrets = xd_agent_secrets_load (NULL, &error);
  g_assert_no_error (error);
  g_assert_true (
    xd_agent_secrets_set (secrets, "XD_TEST_TOKEN", "server-secret", &error));
  g_assert_true (xd_agent_secrets_save (secrets, &error));
  g_assert_no_error (error);

  run_init (&first_run, first);
  g_assert_true (xd_chat_session_start (first, &spec, &error));
  g_assert_no_error (error);
  run_wait (&first_run);

  g_assert_true (first_run.finished);
  g_assert_true (first_run.success);
  g_assert_cmpstr (first_run.text->str, ==, "hello world");
  g_assert_cmpstr (first_run.session_id, ==, "thread-test");
  g_assert_cmpstr (first_run.last_tool, ==, "$ printf hi");
  g_assert_cmpuint (first_run.context_used, ==, 42);
  g_assert_cmpuint (first_run.context_window, ==, 272000);

  spec.resume_session_id = first_run.session_id;
  run_init (&second_run, second);
  g_assert_true (xd_chat_session_start (second, &spec, &error));
  g_assert_no_error (error);
  run_wait (&second_run);
  g_assert_true (second_run.finished);
  g_assert_true (second_run.success);
  g_assert_cmpstr (second_run.session_id, ==, "thread-test");

  spec.prompt = "wait";
  run_init (&cancel_run, cancelled);
  g_assert_true (xd_chat_session_start (cancelled, &spec, &error));
  g_assert_no_error (error);
  g_idle_add (cancel_session, cancelled);
  run_wait (&cancel_run);
  g_assert_true (cancel_run.finished);
  g_assert_true (cancel_run.success);

  g_assert_true (g_file_get_contents (app_server_count_file, &count, NULL,
                                      &error));
  g_assert_no_error (error);
  g_assert_cmpuint (count_lines (count), ==, 1);

  run_clear (&cancel_run);
  run_clear (&second_run);
  run_clear (&first_run);
  xd_codex_app_server_shutdown_all ();
  g_unsetenv ("XD_AGENT_SECRETS_FILE");
  g_remove (secrets_path);
  g_rmdir (secrets_directory);
}

static int
run_app_server_child (void)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  g_autofree char *line = NULL;
  size_t capacity = 0;
  FILE *count;

  if (g_strcmp0 (g_getenv ("XD_TEST_TOKEN"), "server-secret") != 0)
    return 19;

  count = g_fopen (g_getenv ("XD_TEST_APP_SERVER_COUNT_FILE"), "a");
  if (count == NULL)
    return 20;
  fputs ("server\n", count);
  fclose (count);

  while (getline (&line, &capacity, stdin) >= 0)
    {
      JsonNode *root_node;
      JsonObject *root;
      JsonObject *params;
      const char *method;
      gint64 id;

      if (!json_parser_load_from_data (parser, line, -1, NULL))
        return 21;
      root_node = json_parser_get_root (parser);
      if (!JSON_NODE_HOLDS_OBJECT (root_node))
        return 22;
      root = json_node_get_object (root_node);
      method = json_object_get_string_member_with_default (
        root, "method", NULL);
      params = ai_json_get_object (root, "params");
      id = json_object_get_int_member_with_default (root, "id", -1);

      if (g_strcmp0 (method, "initialize") == 0)
        g_print ("{\"id\":%" G_GINT64_FORMAT ",\"result\":{}}\n", id);
      else if (g_strcmp0 (method, "initialized") == 0)
        continue;
      else if (g_strcmp0 (method, "thread/start") == 0 ||
               g_strcmp0 (method, "thread/resume") == 0)
        {
          JsonObject *config = ai_json_get_object (params, "config");
          JsonObject *policy =
            ai_json_get_object (config, "shell_environment_policy");
          JsonArray *include_only = policy != NULL
            ? json_object_get_array_member (policy, "include_only") : NULL;
          const char *instructions =
            json_object_get_string_member_with_default (
              params, "developerInstructions", NULL);
          const char *thread_id =
            json_object_get_string_member_with_default (
              params, "threadId", "thread-test");

          gboolean includes_secret = FALSE;

          for (guint i = 0;
               include_only != NULL && i < json_array_get_length (include_only);
               i++)
            if (g_strcmp0 (json_array_get_string_element (include_only, i),
                           "XD_TEST_TOKEN") == 0)
              includes_secret = TRUE;

          if (instructions == NULL ||
              strstr (instructions, "test instructions") == NULL ||
              strstr (instructions, "Co-authored-by: Codex") == NULL ||
              !includes_secret ||
              !json_object_get_boolean_member_with_default (
                policy, "ignore_default_excludes", FALSE) ||
              g_strcmp0 (json_object_get_string_member_with_default (
                           params, "approvalPolicy", NULL), "never") != 0)
            return 23;

          g_print ("{\"id\":%" G_GINT64_FORMAT
                   ",\"result\":{\"thread\":{\"id\":\"%s\"}}}\n",
                   id, thread_id);
        }
      else if (g_strcmp0 (method, "turn/start") == 0)
        {
          JsonArray *inputs = json_object_get_array_member (params, "input");
          JsonObject *input = json_array_get_object_element (inputs, 0);
          JsonObject *sandbox = ai_json_get_object (params, "sandboxPolicy");
          const char *prompt =
            json_object_get_string_member_with_default (input, "text", NULL);

          if (g_strcmp0 (json_object_get_string_member_with_default (
                           params, "model", NULL), "gpt-test") != 0 ||
              g_strcmp0 (json_object_get_string_member_with_default (
                           params, "effort", NULL), "xhigh") != 0 ||
              g_strcmp0 (json_object_get_string_member_with_default (
                           params, "cwd", NULL), "/tmp") != 0 ||
              g_strcmp0 (json_object_get_string_member_with_default (
                           sandbox, "type", NULL), "workspaceWrite") != 0)
            return 24;

          g_print ("{\"id\":%" G_GINT64_FORMAT
                   ",\"result\":{\"turn\":{\"id\":\"turn-test\"}}}\n", id);
          g_print ("{\"method\":\"turn/started\",\"params\":{"
                   "\"threadId\":\"thread-test\","
                   "\"turn\":{\"id\":\"turn-test\",\"status\":\"inProgress\"}}}\n");

          if (g_strcmp0 (prompt, "wait") != 0)
            {
              g_print ("{\"method\":\"item/agentMessage/delta\",\"params\":{"
                       "\"threadId\":\"thread-test\",\"turnId\":\"turn-test\","
                       "\"itemId\":\"message-test\",\"delta\":\"hello \"}}\n");
              g_print ("{\"method\":\"item/started\",\"params\":{"
                       "\"threadId\":\"thread-test\",\"turnId\":\"turn-test\","
                       "\"item\":{\"type\":\"commandExecution\","
                       "\"id\":\"command-test\",\"command\":\"printf hi\"}}}\n");
              g_print ("{\"method\":\"item/completed\",\"params\":{"
                       "\"threadId\":\"thread-test\",\"turnId\":\"turn-test\","
                       "\"item\":{\"type\":\"commandExecution\","
                       "\"id\":\"command-test\",\"command\":\"printf hi\"}}}\n");
              g_print ("{\"method\":\"item/agentMessage/delta\",\"params\":{"
                       "\"threadId\":\"thread-test\",\"turnId\":\"turn-test\","
                       "\"itemId\":\"message-test\",\"delta\":\"world\"}}\n");
              g_print ("{\"method\":\"item/completed\",\"params\":{"
                       "\"threadId\":\"thread-test\",\"turnId\":\"turn-test\","
                       "\"item\":{\"type\":\"agentMessage\","
                       "\"id\":\"message-test\",\"text\":\"hello world\"}}}\n");
              g_print ("{\"method\":\"thread/tokenUsage/updated\",\"params\":{"
                       "\"threadId\":\"thread-test\",\"turnId\":\"turn-test\","
                       "\"tokenUsage\":{\"last\":{\"totalTokens\":42},"
                       "\"modelContextWindow\":272000}}}\n");
              g_print ("{\"method\":\"turn/completed\",\"params\":{"
                       "\"threadId\":\"thread-test\","
                       "\"turn\":{\"id\":\"turn-test\","
                       "\"status\":\"completed\"}}}\n");
            }
        }
      else if (g_strcmp0 (method, "turn/interrupt") == 0)
        {
          g_print ("{\"id\":%" G_GINT64_FORMAT ",\"result\":{}}\n", id);
          g_print ("{\"method\":\"turn/completed\",\"params\":{"
                   "\"threadId\":\"thread-test\","
                   "\"turn\":{\"id\":\"turn-test\","
                   "\"status\":\"interrupted\"}}}\n");
        }

      fflush (stdout);
    }

  return 0;
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
  if (argc == 2 && g_strcmp0 (argv[1], "--app-server-child") == 0)
    return run_app_server_child ();

  secret_child_program = argv[0];
  app_server_child_program = argv[0];
  app_server_count_file =
    g_build_filename (g_get_tmp_dir (), "xd-app-server-count.txt", NULL);
  g_remove (app_server_count_file);
  g_setenv ("XD_TEST_APP_SERVER_COUNT_FILE", app_server_count_file, TRUE);
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/session/streams", test_streams_a_transcript);
  g_test_add_func ("/session/missing-program", test_missing_program_explains_itself);
  g_test_add_func ("/session/nonzero-exit", test_nonzero_exit_is_a_failure);
  g_test_add_func ("/session/recoverable-backend-error",
                   test_recoverable_backend_error_does_not_end_turn);
  g_test_add_func ("/session/agent-secret-environment",
                   test_agent_secret_reaches_process_not_prompt);
  g_test_add_func ("/session/codex-app-server",
                   test_app_server_streams_resumes_and_interrupts);

  {
    int status = g_test_run ();

    xd_codex_app_server_shutdown_all ();
    g_remove (app_server_count_file);
    g_free (app_server_count_file);
    g_free (secret_system_prompt);
    return status;
  }
}
