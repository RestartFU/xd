#include "codex-app-server.h"

#include "codex-backend.h"

#include <stdlib.h>

#define STDERR_LIMIT 8192

typedef struct _CodexServer CodexServer;
typedef struct _CodexServerClass CodexServerClass;

typedef enum
{
  REQUEST_INITIALIZE,
  REQUEST_OPEN_THREAD,
  REQUEST_START_TURN,
  REQUEST_INTERRUPT,
} RequestKind;

typedef struct
{
  RequestKind kind;
  XdCodexTurn *turn;
} PendingRequest;

struct _XdCodexTurn
{
  gint refs;
  CodexServer *server;
  char *prompt;
  char *model;
  char *instructions;
  char *resume_id;
  char *workdir;
  GStrv allowed_environment_names;
  AiEffort effort;
  AiAccess access;
  char *thread_id;
  char *turn_id;
  GHashTable *streamed_messages;
  GHashTable *started_commands;
  AiEventFunc event_callback;
  XdCodexTurnFinishedFunc finished_callback;
  gpointer user_data;
  char *backend_error;
  gboolean stopping;
  gboolean finished;
};

struct _CodexServer
{
  GObject parent_instance;
  const AiBackend *backend;
  char *pool_key;
  GSubprocess *process;
  GOutputStream *stdin_pipe;
  GDataInputStream *stdout_stream;
  GDataInputStream *stderr_stream;
  GCancellable *cancellable;
  GString *stderr_text;
  GQueue writes;
  GQueue waiting;
  GHashTable *pending;
  GHashTable *turns;
  guint64 next_id;
  gboolean writing;
  gboolean ready;
  gboolean failed;
};

struct _CodexServerClass
{
  GObjectClass parent_class;
};

G_DEFINE_TYPE (CodexServer, codex_server, G_TYPE_OBJECT)
G_DEFINE_AUTOPTR_CLEANUP_FUNC (CodexServer, g_object_unref)

static GHashTable *server_pool;

static XdCodexTurn *turn_ref (XdCodexTurn *turn);
static void turn_unref (XdCodexTurn *turn);
static void server_open_turn (CodexServer *server, XdCodexTurn *turn);
static void server_fail (CodexServer *server, const char *message);
static void read_next_line (CodexServer *server);

static const char *
json_string (JsonObject *object,
             const char *name)
{
  if (object == NULL)
    return NULL;

  return json_object_get_string_member_with_default (object, name, NULL);
}

static JsonObject *
json_child (JsonObject *object,
            const char *name)
{
  return ai_json_get_object (object, name);
}

static JsonObject *
object_new (void)
{
  return json_object_new ();
}

static JsonNode *
object_node (JsonObject *object)
{
  JsonNode *node = json_node_new (JSON_NODE_OBJECT);

  json_node_take_object (node, object);
  return node;
}

static char *
serialize_line (JsonObject *object)
{
  g_autoptr (JsonGenerator) generator = json_generator_new ();
  g_autoptr (JsonNode) root = object_node (object);
  g_autofree char *json = NULL;

  json_generator_set_root (generator, root);
  json = json_generator_to_data (generator, NULL);
  return g_strconcat (json, "\n", NULL);
}

static void
on_write_done (GObject      *source,
               GAsyncResult *result,
               gpointer      user_data)
{
  g_autoptr (CodexServer) server = user_data;
  g_autoptr (GError) error = NULL;
  GBytes *bytes;

  if (!g_output_stream_write_all_finish (G_OUTPUT_STREAM (source), result,
                                         NULL, &error))
    {
      server->writing = FALSE;
      server_fail (server, error->message);
      return;
    }

  bytes = g_queue_pop_head (&server->writes);
  g_bytes_unref (bytes);
  server->writing = FALSE;

  if (!g_queue_is_empty (&server->writes))
    {
      gsize length;
      const void *data;

      bytes = g_queue_peek_head (&server->writes);
      data = g_bytes_get_data (bytes, &length);
      server->writing = TRUE;
      g_output_stream_write_all_async (server->stdin_pipe, data, length,
                                       G_PRIORITY_DEFAULT, server->cancellable,
                                       on_write_done, g_object_ref (server));
    }
}

static void
server_queue_object (CodexServer *server,
                     JsonObject  *object)
{
  g_autofree char *line = NULL;
  GBytes *bytes;

  if (server->failed)
    {
      json_object_unref (object);
      return;
    }

  line = serialize_line (object);
  bytes = g_bytes_new (line, strlen (line));
  g_queue_push_tail (&server->writes, bytes);

  if (!server->writing)
    {
      gsize length;
      const void *data = g_bytes_get_data (bytes, &length);

      server->writing = TRUE;
      g_output_stream_write_all_async (server->stdin_pipe, data, length,
                                       G_PRIORITY_DEFAULT, server->cancellable,
                                       on_write_done, g_object_ref (server));
    }
}

static void
pending_request_free (gpointer data)
{
  PendingRequest *request = data;

  if (request->turn != NULL)
    turn_unref (request->turn);
  g_free (request);
}

static guint64
server_request (CodexServer *server,
                const char  *method,
                JsonObject  *params,
                RequestKind  kind,
                XdCodexTurn *turn)
{
  PendingRequest *request = g_new0 (PendingRequest, 1);
  JsonObject *root = object_new ();
  guint64 id = ++server->next_id;

  request->kind = kind;
  request->turn = turn != NULL ? turn_ref (turn) : NULL;
  g_hash_table_insert (server->pending, g_memdup2 (&id, sizeof id), request);

  json_object_set_int_member (root, "id", id);
  json_object_set_string_member (root, "method", method);
  json_object_set_object_member (root, "params", params);
  server_queue_object (server, root);

  return id;
}

static void
server_notify (CodexServer *server,
               const char  *method,
               JsonObject  *params)
{
  JsonObject *root = object_new ();

  json_object_set_string_member (root, "method", method);
  json_object_set_object_member (root, "params", params);
  server_queue_object (server, root);
}

static XdCodexTurn *
turn_ref (XdCodexTurn *turn)
{
  g_atomic_int_inc (&turn->refs);
  return turn;
}

static void
turn_unref (XdCodexTurn *turn)
{
  if (!g_atomic_int_dec_and_test (&turn->refs))
    return;

  g_clear_object (&turn->server);
  g_free (turn->prompt);
  g_free (turn->model);
  g_free (turn->instructions);
  g_free (turn->resume_id);
  g_free (turn->workdir);
  g_strfreev (turn->allowed_environment_names);
  g_free (turn->thread_id);
  g_free (turn->turn_id);
  g_free (turn->backend_error);
  g_hash_table_unref (turn->streamed_messages);
  g_hash_table_unref (turn->started_commands);
  g_free (turn);
}

static void
turn_emit (XdCodexTurn *turn,
           AiEventType  type,
           const char  *text,
           guint64      used,
           guint64      window)
{
  AiEvent event = {
    .type = type,
    .session_id = type == AI_EVENT_SESSION_STARTED ? turn->thread_id : NULL,
    .text = text,
    .context_used = used,
    .context_window = window,
  };

  if (turn->event_callback != NULL)
    turn->event_callback (&event, turn->user_data);
}

static void
turn_finish (XdCodexTurn *turn,
             gboolean     success,
             const char  *message)
{
  CodexServer *server;

  if (turn->finished)
    return;

  turn->finished = TRUE;
  server = turn->server;

  if (turn->thread_id != NULL)
    g_hash_table_remove (server->turns, turn->thread_id);

  if (turn->finished_callback != NULL)
    turn->finished_callback (success, message, turn->user_data);
}

static JsonObject *
thread_params (XdCodexTurn *turn)
{
  JsonObject *params = object_new ();

  json_object_set_string_member (params, "approvalPolicy", "never");
  json_object_set_string_member (
    params, "sandbox",
    turn->access == AI_ACCESS_FULL ? "danger-full-access"
    : turn->access == AI_ACCESS_EDIT ? "workspace-write" : "read-only");
  if (turn->model != NULL)
    json_object_set_string_member (params, "model", turn->model);
  if (turn->workdir != NULL)
    json_object_set_string_member (params, "cwd", turn->workdir);
  if (turn->instructions != NULL)
    json_object_set_string_member (params, "developerInstructions",
                                   turn->instructions);
  if (turn->allowed_environment_names != NULL)
    {
      JsonObject *config = object_new ();
      JsonObject *policy = object_new ();
      JsonArray *include_only = json_array_new ();

      for (gsize i = 0; turn->allowed_environment_names[i] != NULL; i++)
        json_array_add_string_element (include_only,
                                       turn->allowed_environment_names[i]);
      json_object_set_string_member (policy, "inherit", "all");
      json_object_set_boolean_member (policy, "ignore_default_excludes", TRUE);
      json_object_set_array_member (policy, "include_only", include_only);
      json_object_set_object_member (config, "shell_environment_policy", policy);
      json_object_set_object_member (params, "config", config);
    }

  return params;
}

static void
server_start_turn (CodexServer *server,
                   XdCodexTurn *turn)
{
  JsonObject *params = object_new ();
  JsonObject *input = object_new ();
  JsonObject *sandbox = object_new ();
  JsonArray *inputs = json_array_new ();

  json_object_set_string_member (params, "threadId", turn->thread_id);
  json_object_set_string_member (params, "approvalPolicy", "never");
  json_object_set_string_member (params, "effort",
                                 ai_effort_to_string (turn->effort));
  if (turn->model != NULL)
    json_object_set_string_member (params, "model", turn->model);
  if (turn->workdir != NULL)
    json_object_set_string_member (params, "cwd", turn->workdir);

  json_object_set_string_member (
    sandbox, "type", xd_codex_sandbox_policy_type (turn->access));
  if (turn->access == AI_ACCESS_EDIT && turn->workdir != NULL)
    {
      JsonArray *roots = json_array_new ();

      json_array_add_string_element (roots, turn->workdir);
      json_object_set_array_member (sandbox, "writableRoots", roots);
      json_object_set_boolean_member (sandbox, "networkAccess", FALSE);
    }
  json_object_set_object_member (params, "sandboxPolicy", sandbox);

  json_object_set_string_member (input, "type", "text");
  json_object_set_string_member (input, "text", turn->prompt);
  json_array_add_object_element (inputs, input);
  json_object_set_array_member (params, "input", inputs);

  server_request (server, "turn/start", params, REQUEST_START_TURN, turn);
}

static void
server_open_turn (CodexServer *server,
                  XdCodexTurn *turn)
{
  JsonObject *params = thread_params (turn);

  if (turn->resume_id != NULL)
    {
      json_object_set_string_member (params, "threadId", turn->resume_id);
      server_request (server, "thread/resume", params,
                      REQUEST_OPEN_THREAD, turn);
    }
  else
    {
      server_request (server, "thread/start", params,
                      REQUEST_OPEN_THREAD, turn);
    }
}

static void
server_interrupt_turn (XdCodexTurn *turn)
{
  JsonObject *params;

  if (turn->turn_id == NULL || turn->finished)
    return;

  params = object_new ();
  json_object_set_string_member (params, "threadId", turn->thread_id);
  json_object_set_string_member (params, "turnId", turn->turn_id);
  server_request (turn->server, "turn/interrupt", params,
                  REQUEST_INTERRUPT, turn);
}

static const char *
item_summary_name (const char *type)
{
  if (g_strcmp0 (type, "commandExecution") == 0)
    return "command_execution";
  if (g_strcmp0 (type, "fileChange") == 0)
    return "file_change";
  if (g_strcmp0 (type, "mcpToolCall") == 0)
    return "mcp_tool_call";
  if (g_strcmp0 (type, "collabAgentToolCall") == 0)
    return "collab_agent_tool_call";
  if (g_strcmp0 (type, "webSearch") == 0)
    return "web_search";
  if (g_strcmp0 (type, "imageView") == 0)
    return "image_view";
  return type;
}

static gboolean
item_is_quiet (const char *type)
{
  return g_strcmp0 (type, "agentMessage") == 0 ||
         g_strcmp0 (type, "userMessage") == 0 ||
         g_strcmp0 (type, "reasoning") == 0 ||
         g_strcmp0 (type, "hookPrompt") == 0 ||
         g_strcmp0 (type, "subAgentActivity") == 0;
}

static void
handle_item (XdCodexTurn *turn,
             JsonObject  *params,
             gboolean     started)
{
  JsonObject *item = json_child (params, "item");
  const char *type = json_string (item, "type");
  const char *id = json_string (item, "id");

  if (item == NULL || type == NULL)
    return;

  if (g_strcmp0 (type, "agentMessage") == 0)
    {
      const char *text = json_string (item, "text");

      if (!started && text != NULL && id != NULL &&
          !g_hash_table_contains (turn->streamed_messages, id))
        turn_emit (turn, AI_EVENT_TEXT_DELTA, text, 0, 0);
      return;
    }

  if (item_is_quiet (type))
    return;

  if (g_strcmp0 (type, "commandExecution") == 0)
    {
      if (!started || id == NULL)
        return;
      if (!g_hash_table_add (turn->started_commands, g_strdup (id)))
        return;
    }
  else if (started)
    return;

  {
    g_autofree char *summary =
      ai_tool_summary (item_summary_name (type), item);

    turn_emit (turn, AI_EVENT_TOOL_USE, summary, 0, 0);
  }
}

static void
handle_notification (CodexServer *server,
                     JsonObject  *root)
{
  const char *method = json_string (root, "method");
  JsonObject *params = json_child (root, "params");
  const char *thread_id = json_string (params, "threadId");
  XdCodexTurn *turn = thread_id != NULL
    ? g_hash_table_lookup (server->turns, thread_id) : NULL;

  if (method == NULL || params == NULL)
    return;

  if (g_strcmp0 (method, "item/agentMessage/delta") == 0 && turn != NULL)
    {
      const char *delta = json_string (params, "delta");
      const char *item_id = json_string (params, "itemId");

      if (item_id != NULL)
        g_hash_table_add (turn->streamed_messages, g_strdup (item_id));
      if (delta != NULL)
        turn_emit (turn, AI_EVENT_TEXT_DELTA, delta, 0, 0);
    }
  else if (g_strcmp0 (method, "item/started") == 0 && turn != NULL)
    handle_item (turn, params, TRUE);
  else if (g_strcmp0 (method, "item/completed") == 0 && turn != NULL)
    handle_item (turn, params, FALSE);
  else if (g_strcmp0 (method, "thread/tokenUsage/updated") == 0 && turn != NULL)
    {
      JsonObject *usage = json_child (params, "tokenUsage");
      JsonObject *last = json_child (usage, "last");
      guint64 used = last != NULL
        ? MAX (json_object_get_int_member_with_default (last, "totalTokens", 0), 0)
        : 0;
      guint64 window = usage != NULL
        ? MAX (json_object_get_int_member_with_default (
                 usage, "modelContextWindow", 0), 0)
        : 0;

      turn_emit (turn, AI_EVENT_USAGE, NULL, used, window);
    }
  else if (g_strcmp0 (method, "error") == 0 && turn != NULL)
    {
      JsonObject *error = json_child (params, "error");
      const char *message = json_string (error, "message");
      gboolean retrying = json_object_get_boolean_member_with_default (
        params, "willRetry", FALSE);

      if (!retrying && message != NULL && !turn->stopping)
        {
          g_free (turn->backend_error);
          turn->backend_error = g_strdup (message);
        }
    }
  else if (g_strcmp0 (method, "turn/completed") == 0 && turn != NULL)
    {
      JsonObject *completed = json_child (params, "turn");
      JsonObject *error = json_child (completed, "error");
      const char *status = json_string (completed, "status");
      const char *message = json_string (error, "message");

      if (turn->stopping || g_strcmp0 (status, "interrupted") == 0)
        turn_finish (turn, TRUE, NULL);
      else if (g_strcmp0 (status, "failed") == 0)
        turn_finish (turn, FALSE,
                     message != NULL ? message : turn->backend_error);
      else if (g_strcmp0 (status, "completed") == 0)
        {
          turn_emit (turn, AI_EVENT_RESULT, NULL, 0, 0);
          turn_finish (turn, turn->backend_error == NULL,
                       turn->backend_error);
        }
      else
        turn_finish (turn, FALSE, "Codex returned an unknown turn status");
    }
  else if ((g_strcmp0 (method, "item/commandExecution/requestApproval") == 0 ||
            g_strcmp0 (method, "item/fileChange/requestApproval") == 0))
    {
      JsonObject *response = object_new ();
      JsonObject *result = object_new ();
      gint64 id = json_object_get_int_member_with_default (root, "id", -1);

      if (id >= 0)
        {
          json_object_set_int_member (response, "id", id);
          json_object_set_string_member (result, "decision", "cancel");
          json_object_set_object_member (response, "result", result);
          server_queue_object (server, response);
        }
      else
        {
          json_object_unref (response);
          json_object_unref (result);
        }
    }
  else if (json_object_has_member (root, "id"))
    {
      JsonObject *response = object_new ();
      JsonObject *error = object_new ();
      gint64 id = json_object_get_int_member_with_default (root, "id", -1);

      json_object_set_int_member (response, "id", id);
      json_object_set_int_member (error, "code", -32601);
      json_object_set_string_member (error, "message",
                                     "xd does not support this server request");
      json_object_set_object_member (response, "error", error);
      server_queue_object (server, response);
    }
}

static void
handle_response (CodexServer *server,
                 JsonObject  *root)
{
  guint64 id = json_object_get_int_member_with_default (root, "id", 0);
  PendingRequest *request = g_hash_table_lookup (server->pending, &id);
  JsonObject *result = json_child (root, "result");
  JsonObject *error = json_child (root, "error");
  if (request == NULL)
    return;

  if (error != NULL)
    {
      const char *message = json_string (error, "message");

      if (request->kind == REQUEST_INITIALIZE)
        server_fail (server, message != NULL ? message
                                             : "Codex app-server initialization failed");
      else if (request->turn != NULL)
        turn_finish (request->turn, FALSE,
                     message != NULL ? message : "Codex app-server request failed");
      g_hash_table_remove (server->pending, &id);
      return;
    }

  switch (request->kind)
    {
    case REQUEST_INITIALIZE:
      {
        JsonObject *params = object_new ();

        server->ready = TRUE;
        server_notify (server, "initialized", params);
        while (!g_queue_is_empty (&server->waiting))
          {
            XdCodexTurn *turn = g_queue_pop_head (&server->waiting);

            server_open_turn (server, turn);
            turn_unref (turn);
          }
      }
      break;

    case REQUEST_OPEN_THREAD:
      if (request->turn != NULL)
        {
          JsonObject *thread = json_child (result, "thread");
          const char *thread_id = json_string (thread, "id");

          if (thread_id == NULL)
            {
              turn_finish (request->turn, FALSE,
                           "Codex app-server returned no thread id");
              break;
            }

          g_free (request->turn->thread_id);
          request->turn->thread_id = g_strdup (thread_id);
          g_hash_table_replace (server->turns, g_strdup (thread_id),
                                turn_ref (request->turn));
          turn_emit (request->turn, AI_EVENT_SESSION_STARTED, NULL, 0, 0);
          server_start_turn (server, request->turn);
        }
      break;

    case REQUEST_START_TURN:
      if (request->turn != NULL)
        {
          JsonObject *turn = json_child (result, "turn");
          const char *turn_id = json_string (turn, "id");

          if (turn_id != NULL)
            {
              g_free (request->turn->turn_id);
              request->turn->turn_id = g_strdup (turn_id);
            }
          if (request->turn->stopping)
            server_interrupt_turn (request->turn);
        }
      break;

    case REQUEST_INTERRUPT:
      break;
    }

  g_hash_table_remove (server->pending, &id);
}

static void
on_line_read (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  g_autoptr (CodexServer) server = user_data;
  g_autoptr (GError) error = NULL;
  g_autofree char *line = NULL;
  g_autoptr (JsonParser) parser = NULL;
  JsonNode *root;
  gsize length = 0;

  line = g_data_input_stream_read_line_finish_utf8 (
    G_DATA_INPUT_STREAM (source), result, &length, &error);
  if (error != NULL)
    {
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        server_fail (server, error->message);
      return;
    }
  if (line == NULL)
    {
      if (!server->failed)
        server_fail (server, "Codex app-server closed unexpectedly");
      return;
    }

  parser = json_parser_new ();
  if (json_parser_load_from_data (parser, line, length, NULL) &&
      (root = json_parser_get_root (parser)) != NULL &&
      JSON_NODE_HOLDS_OBJECT (root))
    {
      JsonObject *object = json_node_get_object (root);

      if (json_object_has_member (object, "id") &&
          (json_object_has_member (object, "result") ||
           json_object_has_member (object, "error")))
        handle_response (server, object);
      else
        handle_notification (server, object);
    }

  if (!server->failed)
    read_next_line (server);
}

static void
read_next_line (CodexServer *server)
{
  g_data_input_stream_read_line_async (
    server->stdout_stream, G_PRIORITY_DEFAULT, server->cancellable,
    on_line_read, g_object_ref (server));
}

static void
on_stderr_line (GObject      *source,
                GAsyncResult *result,
                gpointer      user_data)
{
  g_autoptr (CodexServer) server = user_data;
  g_autofree char *line = NULL;

  line = g_data_input_stream_read_line_finish_utf8 (
    G_DATA_INPUT_STREAM (source), result, NULL, NULL);
  if (line == NULL)
    return;

  if (server->stderr_text->len < STDERR_LIMIT)
    g_string_append_printf (server->stderr_text, "%s\n", line);

  g_data_input_stream_read_line_async (
    server->stderr_stream, G_PRIORITY_LOW, server->cancellable,
    on_stderr_line, g_object_ref (server));
}

static void
server_fail (CodexServer *server,
             const char  *message)
{
  g_autoptr (GHashTable) turns = NULL;
  GHashTableIter iter;
  gpointer value;
  const char *tail;

  if (server->failed)
    return;
  server->failed = TRUE;
  g_cancellable_cancel (server->cancellable);
  if (server->process != NULL)
    g_subprocess_force_exit (server->process);

  g_strstrip (server->stderr_text->str);
  tail = *server->stderr_text->str != '\0' ? server->stderr_text->str : message;
  turns = g_hash_table_new_full (g_direct_hash, g_direct_equal, NULL,
                                  (GDestroyNotify) turn_unref);

  for (GList *link = server->waiting.head; link != NULL; link = link->next)
    if (!g_hash_table_contains (turns, link->data))
      g_hash_table_add (turns, turn_ref (link->data));
  g_hash_table_iter_init (&iter, server->turns);
  while (g_hash_table_iter_next (&iter, NULL, &value))
    if (!g_hash_table_contains (turns, value))
      g_hash_table_add (turns, turn_ref (value));
  g_hash_table_iter_init (&iter, server->pending);
  while (g_hash_table_iter_next (&iter, NULL, &value))
    {
      PendingRequest *request = value;

      if (request->turn != NULL &&
          !g_hash_table_contains (turns, request->turn))
        g_hash_table_add (turns, turn_ref (request->turn));
    }

  g_queue_clear_full (&server->waiting, (GDestroyNotify) turn_unref);
  g_hash_table_remove_all (server->turns);
  g_hash_table_remove_all (server->pending);

  if (server_pool != NULL)
    g_hash_table_remove (server_pool, server->pool_key);

  g_hash_table_iter_init (&iter, turns);
  while (g_hash_table_iter_next (&iter, &value, NULL))
    turn_finish (value, FALSE,
                 tail != NULL ? tail : "Codex app-server failed");
}

static void
codex_server_dispose (GObject *object)
{
  CodexServer *server = (CodexServer *) object;

  g_cancellable_cancel (server->cancellable);
  if (server->process != NULL)
    g_subprocess_force_exit (server->process);
  g_clear_object (&server->stdout_stream);
  g_clear_object (&server->stderr_stream);
  g_clear_object (&server->stdin_pipe);
  g_clear_object (&server->process);
  g_clear_object (&server->cancellable);

  G_OBJECT_CLASS (codex_server_parent_class)->dispose (object);
}

static void
codex_server_finalize (GObject *object)
{
  CodexServer *server = (CodexServer *) object;

  g_free (server->pool_key);
  g_string_free (server->stderr_text, TRUE);
  g_queue_clear_full (&server->writes, (GDestroyNotify) g_bytes_unref);
  g_queue_clear_full (&server->waiting, (GDestroyNotify) turn_unref);
  g_hash_table_unref (server->pending);
  g_hash_table_unref (server->turns);

  G_OBJECT_CLASS (codex_server_parent_class)->finalize (object);
}

static void
codex_server_class_init (CodexServerClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = codex_server_dispose;
  object_class->finalize = codex_server_finalize;
}

static void
codex_server_init (CodexServer *server)
{
  server->cancellable = g_cancellable_new ();
  server->stderr_text = g_string_new (NULL);
  g_queue_init (&server->writes);
  g_queue_init (&server->waiting);
  server->pending = g_hash_table_new_full (g_int64_hash, g_int64_equal, g_free,
                                           pending_request_free);
  server->turns = g_hash_table_new_full (g_str_hash, g_str_equal, g_free,
                                         (GDestroyNotify) turn_unref);
}

static gint
environment_compare (gconstpointer a,
                     gconstpointer b)
{
  const char *const *left = a;
  const char *const *right = b;

  return g_strcmp0 (*left, *right);
}

static char *
environment_key (const AiBackend *backend,
                 GStrv            environment)
{
  g_autoptr (GChecksum) checksum = g_checksum_new (G_CHECKSUM_SHA256);
  g_auto (GStrv) sorted = g_strdupv (environment);

  qsort (sorted, g_strv_length (sorted), sizeof (char *),
         environment_compare);
  g_checksum_update (checksum, (const guchar *) backend->program,
                     strlen (backend->program));
  for (gsize i = 0; sorted[i] != NULL; i++)
    {
      g_checksum_update (checksum, (const guchar *) "\0", 1);
      g_checksum_update (checksum, (const guchar *) sorted[i],
                         strlen (sorted[i]));
    }

  return g_strdup (g_checksum_get_string (checksum));
}

static gboolean
name_looks_secret (const char *name)
{
  g_autofree char *lower = g_ascii_strdown (name, -1);

  return strstr (lower, "key") != NULL ||
         strstr (lower, "secret") != NULL ||
         strstr (lower, "token") != NULL;
}

static GStrv
allowed_environment_names (GStrv environment,
                           GStrv secret_names)
{
  g_autoptr (GPtrArray) names = NULL;

  if (secret_names == NULL || secret_names[0] == NULL)
    return NULL;

  names = g_ptr_array_new_with_free_func (g_free);
  for (gsize i = 0; environment[i] != NULL; i++)
    {
      const char *equals = strchr (environment[i], '=');
      g_autofree char *name = equals != NULL
        ? g_strndup (environment[i], equals - environment[i])
        : g_strdup (environment[i]);

      if (!name_looks_secret (name) ||
          g_strv_contains ((const char *const *) secret_names, name))
        g_ptr_array_add (names, g_steal_pointer (&name));
    }
  g_ptr_array_add (names, NULL);
  return (GStrv) g_ptr_array_free (g_steal_pointer (&names), FALSE);
}

static CodexServer *
server_new (const AiBackend *backend,
            const AiRunSpec *spec,
            GStrv            environment,
            const char      *key,
            GError         **error)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GPtrArray) argv = NULL;
  CodexServer *server = g_object_new (codex_server_get_type (), NULL);
  JsonObject *params;
  JsonObject *client;

  server->backend = backend;
  server->pool_key = g_strdup (key);
  argv = backend->build_argv (backend, spec);
  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDIN_PIPE |
                                        G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                        G_SUBPROCESS_FLAGS_STDERR_PIPE);
  g_subprocess_launcher_set_environ (launcher, environment);
  server->process = g_subprocess_launcher_spawnv (
    launcher, (const char *const *) argv->pdata, error);
  if (server->process == NULL)
    {
      g_object_unref (server);
      return NULL;
    }

  server->stdin_pipe =
    g_object_ref (g_subprocess_get_stdin_pipe (server->process));
  server->stdout_stream = g_data_input_stream_new (
    g_subprocess_get_stdout_pipe (server->process));
  server->stderr_stream = g_data_input_stream_new (
    g_subprocess_get_stderr_pipe (server->process));
  g_buffered_input_stream_set_buffer_size (
    G_BUFFERED_INPUT_STREAM (server->stdout_stream), 1 << 20);
  read_next_line (server);
  g_data_input_stream_read_line_async (
    server->stderr_stream, G_PRIORITY_LOW, server->cancellable,
    on_stderr_line, g_object_ref (server));

  params = object_new ();
  client = object_new ();
  json_object_set_string_member (client, "name", "xd");
  json_object_set_string_member (client, "title", "xd");
  json_object_set_string_member (client, "version", XD_VERSION_STRING);
  json_object_set_object_member (params, "clientInfo", client);
  server_request (server, "initialize", params, REQUEST_INITIALIZE, NULL);

  return server;
}

XdCodexTurn *
xd_codex_app_server_start (const AiBackend          *backend,
                           const AiRunSpec          *spec,
                           GStrv                     environment,
                           GStrv                     secret_names,
                           AiEventFunc               event_callback,
                           XdCodexTurnFinishedFunc   finished_callback,
                           gpointer                  user_data,
                           GError                  **error)
{
  g_autofree char *key = NULL;
  CodexServer *server;
  XdCodexTurn *turn;

  g_return_val_if_fail (backend != NULL, NULL);
  g_return_val_if_fail (spec != NULL, NULL);
  g_return_val_if_fail (environment != NULL, NULL);

  if (server_pool == NULL)
    server_pool = g_hash_table_new_full (g_str_hash, g_str_equal, g_free,
                                         g_object_unref);

  key = environment_key (backend, environment);
  server = g_hash_table_lookup (server_pool, key);
  if (server == NULL)
    {
      server = server_new (backend, spec, environment, key, error);
      if (server == NULL)
        return NULL;
      g_hash_table_insert (server_pool, g_strdup (key), server);
    }

  turn = g_new0 (XdCodexTurn, 1);
  turn->refs = 1;
  turn->server = g_object_ref (server);
  turn->prompt = g_strdup (spec->prompt);
  turn->model = g_strdup (spec->model);
  turn->instructions = xd_codex_developer_instructions (spec);
  turn->resume_id = g_strdup (spec->resume_session_id);
  turn->workdir = g_strdup (spec->workdir);
  turn->allowed_environment_names =
    allowed_environment_names (environment, secret_names);
  turn->effort = spec->effort;
  turn->access = spec->access;
  turn->streamed_messages = g_hash_table_new_full (
    g_str_hash, g_str_equal, g_free, NULL);
  turn->started_commands = g_hash_table_new_full (
    g_str_hash, g_str_equal, g_free, NULL);
  turn->event_callback = event_callback;
  turn->finished_callback = finished_callback;
  turn->user_data = user_data;

  if (server->ready)
    server_open_turn (server, turn);
  else
    g_queue_push_tail (&server->waiting, turn_ref (turn));

  return turn;
}

void
xd_codex_turn_cancel (XdCodexTurn *turn)
{
  g_return_if_fail (turn != NULL);

  if (turn->finished || turn->stopping)
    return;
  turn->stopping = TRUE;
  server_interrupt_turn (turn);
}

void
xd_codex_turn_detach (XdCodexTurn *turn)
{
  g_return_if_fail (turn != NULL);

  turn->event_callback = NULL;
  turn->finished_callback = NULL;
  turn->user_data = NULL;
}

void
xd_codex_turn_free (XdCodexTurn *turn)
{
  if (turn != NULL)
    turn_unref (turn);
}

void
xd_codex_app_server_shutdown_all (void)
{
  g_autoptr (GPtrArray) servers = NULL;
  GHashTableIter iter;
  gpointer value;

  if (server_pool == NULL)
    return;

  servers = g_ptr_array_new_with_free_func (g_object_unref);
  g_hash_table_iter_init (&iter, server_pool);
  while (g_hash_table_iter_next (&iter, NULL, &value))
    g_ptr_array_add (servers, g_object_ref (value));

  for (guint i = 0; i < servers->len; i++)
    server_fail (g_ptr_array_index (servers, i), "Codex app-server stopped");

  g_clear_pointer (&server_pool, g_hash_table_unref);
}
