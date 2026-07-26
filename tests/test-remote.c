#include "remote/server.h"
#include "storage/storage.h"

#include <json-glib/json-glib.h>
#include <string.h>

/*
 * The whole first exchange, over a real socket and real TLS: pair with the
 * armed code, come back with the token, read the tree and a chat's messages.
 * The client half runs in a thread with blocking IO while the main thread
 * pumps the server's loop, which is the same shape a real client has.
 */

typedef struct
{
  guint16 port;
  char *code;
  char *chat_id;
  gboolean done;
  gboolean ok;
  char *failure;
} Exchange;

static JsonObject *
round_trip (GDataInputStream *in,
            GOutputStream    *out,
            const char       *request,
            JsonParser       *parser)
{
  g_autofree char *line = NULL;

  g_output_stream_write_all (out, request, strlen (request), NULL, NULL, NULL);
  g_output_stream_write_all (out, "\n", 1, NULL, NULL, NULL);

  line = g_data_input_stream_read_line_utf8 (in, NULL, NULL, NULL);
  if (line == NULL || !json_parser_load_from_data (parser, line, -1, NULL))
    return NULL;

  return json_node_get_object (json_parser_get_root (parser));
}

static gboolean
accept_anything (GTlsConnection       *connection,
                 GTlsCertificate      *certificate,
                 GTlsCertificateFlags  errors,
                 gpointer              user_data)
{
  /* The test is its own certificate authority. */
  return TRUE;
}

static gpointer
client_thread (gpointer user_data)
{
  Exchange *exchange = user_data;
  g_autoptr (GSocketClient) client = g_socket_client_new ();
  g_autoptr (GSocketConnection) socket = NULL;
  g_autoptr (GIOStream) tls = NULL;
  g_autoptr (GDataInputStream) in = NULL;
  g_autoptr (JsonParser) parser = json_parser_new ();
  GOutputStream *out;
  JsonObject *reply;
  g_autofree char *token = NULL;

  #define FAIL(why) G_STMT_START { \
    exchange->failure = g_strdup (why); exchange->done = TRUE; return NULL; \
  } G_STMT_END

  socket = g_socket_client_connect_to_host (client, "127.0.0.1",
                                            exchange->port, NULL, NULL);
  if (socket == NULL)
    FAIL ("connect");

  tls = g_tls_client_connection_new (G_IO_STREAM (socket), NULL, NULL);
  if (tls == NULL)
    FAIL ("tls new");
  g_signal_connect (tls, "accept-certificate",
                    G_CALLBACK (accept_anything), NULL);
  if (!g_tls_connection_handshake (G_TLS_CONNECTION (tls), NULL, NULL))
    FAIL ("handshake");

  in = g_data_input_stream_new (g_io_stream_get_input_stream (tls));
  out = g_io_stream_get_output_stream (tls);

  /* A request before authenticating must be refused. */
  reply = round_trip (in, out, "{\"op\":\"tree\"}", parser);
  if (reply == NULL || json_object_get_boolean_member (reply, "ok"))
    FAIL ("unauthenticated tree was allowed");

  /* Pair with the armed code. */
  {
    g_autofree char *pair = g_strdup_printf (
      "{\"op\":\"pair\",\"code\":\"%s\",\"name\":\"test-device\"}",
      exchange->code);

    reply = round_trip (in, out, pair, parser);
    if (reply == NULL || !json_object_get_boolean_member (reply, "ok"))
      FAIL ("pair refused");
    token = g_strdup (json_object_get_string_member (reply, "token"));
  }

  /* The code is one-use: pairing again must fail. */
  {
    g_autofree char *pair = g_strdup_printf (
      "{\"op\":\"pair\",\"code\":\"%s\",\"name\":\"again\"}", exchange->code);

    reply = round_trip (in, out, pair, parser);
    if (reply == NULL || json_object_get_boolean_member (reply, "ok"))
      FAIL ("pairing code was reusable");
  }

  /* The token proves the device on a fresh conversation. */
  {
    g_autofree char *hello = g_strdup_printf (
      "{\"op\":\"hello\",\"token\":\"%s\"}", token);

    reply = round_trip (in, out, hello, parser);
    if (reply == NULL || !json_object_get_boolean_member (reply, "ok"))
      FAIL ("hello with token refused");
    if (g_strcmp0 (json_object_get_string_member (reply, "device"),
                   "test-device") != 0)
      FAIL ("device name lost");
  }

  reply = round_trip (in, out, "{\"op\":\"tree\"}", parser);
  if (reply == NULL || !json_object_get_boolean_member (reply, "ok"))
    FAIL ("tree refused");
  if (json_array_get_length (json_object_get_array_member (reply, "folders")) != 1)
    FAIL ("folder count wrong");
  if (json_array_get_length (json_object_get_array_member (reply, "chats")) != 1)
    FAIL ("chat count wrong");

  {
    g_autofree char *messages = g_strdup_printf (
      "{\"op\":\"messages\",\"chat\":\"%s\"}", exchange->chat_id);
    JsonArray *rows;

    reply = round_trip (in, out, messages, parser);
    if (reply == NULL || !json_object_get_boolean_member (reply, "ok"))
      FAIL ("messages refused");
    rows = json_object_get_array_member (reply, "messages");
    if (json_array_get_length (rows) != 2)
      FAIL ("message count wrong");
    if (g_strcmp0 (json_object_get_string_member (
                     json_array_get_object_element (rows, 1), "content"),
                   "hello from the daemon") != 0)
      FAIL ("message content wrong");
  }

  exchange->ok = TRUE;
  exchange->done = TRUE;
  return NULL;
  #undef FAIL
}

static void
test_pair_hello_tree (void)
{
  g_autofree char *dir = g_dir_make_tmp ("hy-remote-XXXXXX", NULL);
  g_autofree char *db_path = g_build_filename (dir, "chats.db", NULL);
  g_autofree char *root = g_build_filename (dir, "Workspaces", NULL);
  g_autofree char *folder = g_build_filename (root, "Zeno", NULL);
  g_autofree char *dotfile = g_build_filename (folder, ".hy.json", NULL);
  g_autofree char *cert_path = g_build_filename (dir, "cert.pem", NULL);
  g_autofree char *key_path = g_build_filename (dir, "key.pem", NULL);
  g_autoptr (HyStorage) storage = NULL;
  g_autoptr (GTlsCertificate) certificate = NULL;
  g_autoptr (HyRemoteServer) server = NULL;
  g_autoptr (GError) error = NULL;
  Exchange exchange = { 0 };
  GThread *thread;

  /* A workspace with one folder and one chat with two messages. */
  g_assert_cmpint (g_mkdir_with_parents (folder, 0700), ==, 0);
  g_assert_true (g_file_set_contents (dotfile,
                                      "{\"id\": \"folder-1\"}", -1, NULL));

  storage = hy_storage_new (db_path, &error);
  g_assert_no_error (error);

  exchange.chat_id = hy_storage_create_chat (storage, "folder-1", "remote chat",
                                             "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);
  g_assert_true (hy_storage_append_message (storage, exchange.chat_id, "user",
                                            "anyone there?", NULL, NULL, &error));
  g_assert_true (hy_storage_append_message (storage, exchange.chat_id, "assistant",
                                            "hello from the daemon", NULL,
                                            "Claude · High", &error));

  /* A throwaway certificate; the daemon mints its own the same way. */
  {
    g_autofree char *command = g_strdup_printf (
      "openssl req -x509 -newkey rsa:2048 -keyout %s -out %s "
      "-days 1 -nodes -subj /CN=test", key_path, cert_path);
    g_assert_true (g_spawn_command_line_sync (command, NULL, NULL, NULL, NULL));
  }
  certificate = g_tls_certificate_new_from_files (cert_path, key_path, &error);
  g_assert_no_error (error);

  server = hy_remote_server_new (storage, root, 0, certificate, &error);
  g_assert_no_error (error);

  exchange.port = hy_remote_server_get_port (server);
  exchange.code = hy_remote_server_arm_pairing (server, 60);

  thread = g_thread_new ("client", client_thread, &exchange);
  while (!exchange.done)
    g_main_context_iteration (NULL, TRUE);
  g_thread_join (thread);

  if (exchange.failure != NULL)
    g_error ("client failed at: %s", exchange.failure);
  g_assert_true (exchange.ok);

  g_free (exchange.code);
  g_free (exchange.chat_id);
  g_free (exchange.failure);
}

int
main (int argc, char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/remote/pair-hello-tree", test_pair_hello_tree);

  return g_test_run ();
}
