#include "remote/client.h"
#include "remote/remote-tree.h"
#include "remote/server.h"
#include "storage/storage.h"

#include <json-glib/json-glib.h>
#include <string.h>

/*
 * Both halves of remote xd, over a real socket and real TLS.
 *
 * The first test speaks the wire by hand, so the daemon's answers are pinned
 * down as text rather than as whatever the client happens to read. The rest
 * drive the client itself: pairing, the certificate it pins while doing so,
 * the tree it builds out of the reply, and the token it comes back with.
 */

/* --- a daemon to talk to --------------------------------------------------- */

typedef struct
{
  char *dir;
  char *root;
  char *chat_id;
  XdStorage *storage;
  GTlsCertificate *certificate;
  XdRemoteServer *server;
  guint16 port;
} Daemon;

/* Throwaway and self-signed, which is what the daemon mints for itself. */
static GTlsCertificate *
make_certificate (const char *dir,
                  const char *name)
{
  g_autofree char *cert_path = g_strdup_printf ("%s/%s-cert.pem", dir, name);
  g_autofree char *key_path = g_strdup_printf ("%s/%s-key.pem", dir, name);
  g_autofree char *command = NULL;
  g_autoptr (GError) error = NULL;
  GTlsCertificate *certificate;

  command = g_strdup_printf ("openssl req -x509 -newkey rsa:2048 -keyout %s "
                             "-out %s -days 1 -nodes -subj /CN=%s",
                             key_path, cert_path, name);
  g_assert_true (g_spawn_command_line_sync (command, NULL, NULL, NULL, NULL));

  certificate = g_tls_certificate_new_from_files (cert_path, key_path, &error);
  g_assert_no_error (error);

  return certificate;
}

/* A workspace with one folder and one chat with two messages, served. */
static void
daemon_start (Daemon *daemon)
{
  g_autofree char *db_path = NULL;
  g_autofree char *folder = NULL;
  g_autofree char *dotfile = NULL;
  g_autoptr (GError) error = NULL;

  daemon->dir = g_dir_make_tmp ("xd-remote-XXXXXX", NULL);
  daemon->root = g_build_filename (daemon->dir, "Workspaces", NULL);

  folder = g_build_filename (daemon->root, "Zeno", NULL);
  dotfile = g_build_filename (folder, ".xd.json", NULL);
  g_assert_cmpint (g_mkdir_with_parents (folder, 0700), ==, 0);
  g_assert_true (g_file_set_contents (dotfile, "{\"id\": \"folder-1\"}", -1, NULL));

  db_path = g_build_filename (daemon->dir, "chats.db", NULL);
  daemon->storage = xd_storage_new (db_path, &error);
  g_assert_no_error (error);

  daemon->chat_id = xd_storage_create_chat (daemon->storage, "folder-1",
                                            "remote chat", "claude",
                                            NULL, NULL, NULL, &error);
  g_assert_no_error (error);
  g_assert_true (xd_storage_append_message (daemon->storage, daemon->chat_id, "user",
                                            "anyone there?", NULL, NULL, &error));
  g_assert_true (xd_storage_append_message (daemon->storage, daemon->chat_id,
                                            "assistant", "hello from the daemon",
                                            NULL, "Claude · High", &error));

  daemon->certificate = make_certificate (daemon->dir, "daemon");
  daemon->server = xd_remote_server_new (daemon->storage, daemon->root, 0,
                                         daemon->certificate, &error);
  g_assert_no_error (error);

  daemon->port = xd_remote_server_get_port (daemon->server);
}

static void
daemon_stop (Daemon *daemon)
{
  g_clear_object (&daemon->server);
  g_clear_object (&daemon->certificate);
  g_clear_object (&daemon->storage);
  g_clear_pointer (&daemon->chat_id, g_free);
  g_clear_pointer (&daemon->root, g_free);
  g_clear_pointer (&daemon->dir, g_free);
}

/* --- waiting on the loop --------------------------------------------------- */

typedef struct
{
  gboolean done;
  gboolean timed_out;
  gboolean ok;
  char *failure;
} Wait;

static gboolean
on_wait_elapsed (gpointer user_data)
{
  Wait *wait = user_data;

  wait->timed_out = TRUE;

  return G_SOURCE_REMOVE;
}

/* Everything here is local, so a wait this long only ends one way: hung. */
static void
wait_for (Wait *wait)
{
  guint id = g_timeout_add_seconds (10, on_wait_elapsed, wait);

  while (!wait->done && !wait->timed_out)
    g_main_context_iteration (NULL, TRUE);

  if (!wait->timed_out)
    g_source_remove (id);

  g_assert_false (wait->timed_out);
}

static void
on_done (Wait *wait)
{
  wait->done = TRUE;
}

/* --- the wire, by hand ----------------------------------------------------- */

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

/*
 * The whole first exchange as the daemon sees it: pair with the armed code,
 * come back with the token, read the tree and a chat's messages. The client
 * half runs in a thread with blocking IO while the main thread pumps the
 * server's loop.
 */
static void
test_pair_hello_tree (void)
{
  Daemon daemon = { 0 };
  Exchange exchange = { 0 };
  GThread *thread;

  daemon_start (&daemon);

  exchange.port = daemon.port;
  exchange.chat_id = g_strdup (daemon.chat_id);
  exchange.code = xd_remote_server_arm_pairing (daemon.server, 60);

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
  daemon_stop (&daemon);
}

/* --- the client ------------------------------------------------------------ */

static void
on_paired (GObject      *source,
           GAsyncResult *result,
           gpointer      user_data)
{
  Wait *wait = user_data;
  g_autoptr (GError) error = NULL;

  wait->ok = xd_remote_client_pair_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (!wait->ok)
    wait->failure = g_strdup (error->message);

  wait->done = TRUE;
}

static void
on_messages (GObject      *source,
             GAsyncResult *result,
             gpointer      user_data)
{
  Wait *wait = user_data;
  g_autoptr (JsonObject) reply = NULL;
  g_autoptr (GError) error = NULL;
  JsonArray *rows;

  reply = xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (reply == NULL)
    {
      wait->failure = g_strdup (error->message);
      wait->done = TRUE;
      return;
    }

  rows = json_object_get_array_member (reply, "messages");
  g_assert_cmpuint (json_array_get_length (rows), ==, 2);
  g_assert_cmpstr (json_object_get_string_member (
                     json_array_get_object_element (rows, 1), "content"), ==,
                   "hello from the daemon");

  wait->ok = TRUE;
  wait->done = TRUE;
}

/*
 * The row at @position under @folder, borrowed: the folder's own list is what
 * keeps it alive, which is how every other holder of a node treats it.
 */
static XdNode *
child_at (XdNode *folder,
          guint   position)
{
  GListModel *children = G_LIST_MODEL (xd_node_get_children (folder));
  g_autoptr (XdNode) child = NULL;

  g_assert_cmpuint (g_list_model_get_n_items (children), >, position);

  child = g_list_model_get_item (children, position);

  return child;
}

static guint
child_count (XdNode *folder)
{
  return g_list_model_get_n_items (G_LIST_MODEL (xd_node_get_children (folder)));
}

/*
 * Pairing as the client does it, and what it makes of the answer.
 *
 * The certificate is pinned here and nowhere else, so this is also where it is
 * checked that pairing came away with one at all: without it every later
 * connection has nothing to compare against.
 */
static void
test_client_pairs_and_reads_the_tree (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *code = NULL;
  XdNode *folder;
  XdNode *chat;
  Wait pairing = { 0 };
  Wait loading = { 0 };
  Wait messages = { 0 };

  daemon_start (&daemon);
  code = xd_remote_server_arm_pairing (daemon.server, 60);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  xd_remote_client_pair_async (client, code, "test-device", NULL,
                               on_paired, &pairing);
  wait_for (&pairing);

  if (!pairing.ok)
    g_error ("pairing failed: %s", pairing.failure);

  g_assert_nonnull (xd_remote_client_get_token (client));
  g_assert_nonnull (xd_remote_client_get_certificate (client));
  g_assert_true (xd_remote_client_is_connected (client));

  /* Made against a client that is already up, so it asks straight away. */
  tree = xd_remote_tree_new (client);
  g_signal_connect_swapped (tree, "loaded", G_CALLBACK (on_done), &loading);
  wait_for (&loading);

  g_assert_cmpstr (xd_node_get_name (xd_remote_tree_get_root (tree)), ==,
                   "127.0.0.1");

  folder = child_at (xd_remote_tree_get_root (tree), 0);
  g_assert_cmpint (xd_node_get_kind (folder), ==, XD_NODE_FOLDER);
  g_assert_cmpstr (xd_node_get_name (folder), ==, "Zeno");
  g_assert_cmpstr (xd_node_get_folder_id (folder), ==, "folder-1");

  chat = child_at (folder, 0);
  g_assert_cmpint (xd_node_get_kind (chat), ==, XD_NODE_CHAT);
  g_assert_cmpstr (xd_node_get_name (chat), ==, "remote chat");
  g_assert_cmpstr (xd_node_get_chat_id (chat), ==, daemon.chat_id);

  g_assert_true (xd_remote_tree_lookup_chat (tree, daemon.chat_id) == chat);
  g_assert_true (xd_remote_tree_owns (tree, chat));

  /* The chat's transcript, which is what opening one asks for. */
  xd_remote_client_call_op_async (client, "messages", "chat", daemon.chat_id,
                                  NULL, on_messages, &messages);
  wait_for (&messages);
  if (!messages.ok)
    g_error ("messages failed: %s", messages.failure);

  g_free (pairing.failure);
  g_free (messages.failure);
  daemon_stop (&daemon);
}

/*
 * The token gets a device back in, and both halves of the check turn away the
 * ones that should not be.
 *
 * All three clients are started together and the one that should connect is
 * waited on, so the two that should not have had at least as long to try. The
 * wrong certificate never gets past the handshake -- a daemon presenting one
 * this device did not pair with is not that daemon, whatever it says
 * afterwards -- and the wrong token gets as far as being told no.
 */
static void
test_token_reconnects_and_strangers_are_turned_away (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) paired = NULL;
  g_autoptr (XdRemoteClient) returning = NULL;
  g_autoptr (XdRemoteClient) stranger = NULL;
  g_autoptr (XdRemoteClient) untrusted = NULL;
  g_autoptr (GTlsCertificate) other = NULL;
  g_autofree char *other_pem = NULL;
  g_autofree char *code = NULL;
  Wait pairing = { 0 };
  Wait opened = { 0 };

  daemon_start (&daemon);
  code = xd_remote_server_arm_pairing (daemon.server, 60);

  paired = xd_remote_client_new ("127.0.0.1", daemon.port);
  xd_remote_client_pair_async (paired, code, "test-device", NULL,
                               on_paired, &pairing);
  wait_for (&pairing);
  if (!pairing.ok)
    g_error ("pairing failed: %s", pairing.failure);

  /* A device coming back after being closed: what it kept, and nothing else. */
  returning = xd_remote_client_new ("127.0.0.1", daemon.port);
  xd_remote_client_set_token (returning, xd_remote_client_get_token (paired));
  xd_remote_client_set_certificate (returning,
                                    xd_remote_client_get_certificate (paired));

  /* The same address and a valid token, expecting a different certificate. */
  other = make_certificate (daemon.dir, "stranger");
  g_object_get (other, "certificate-pem", &other_pem, NULL);

  stranger = xd_remote_client_new ("127.0.0.1", daemon.port);
  xd_remote_client_set_token (stranger, xd_remote_client_get_token (paired));
  xd_remote_client_set_certificate (stranger, other_pem);

  /* The right daemon, and a token it has never issued. */
  untrusted = xd_remote_client_new ("127.0.0.1", daemon.port);
  xd_remote_client_set_token (untrusted, "not-a-token");
  xd_remote_client_set_certificate (untrusted,
                                    xd_remote_client_get_certificate (paired));

  g_signal_connect_swapped (returning, "opened", G_CALLBACK (on_done), &opened);

  xd_remote_client_start (stranger);
  xd_remote_client_start (untrusted);
  xd_remote_client_start (returning);

  wait_for (&opened);

  g_assert_true (xd_remote_client_is_connected (returning));
  g_assert_cmpstr (xd_remote_client_get_device_name (returning), ==, "test-device");
  g_assert_false (xd_remote_client_is_connected (stranger));
  g_assert_false (xd_remote_client_is_connected (untrusted));

  g_free (pairing.failure);
  daemon_stop (&daemon);
}

typedef struct
{
  Wait wait;
  XdNode *chat;         /* whatever the signal handed over; held */
} Created;

/*
 * Answers for ::chat-created and ::chat-removed alike.
 *
 * The node is referenced rather than borrowed: a chat that has been deleted is
 * alive only for as long as the tree is saying so, which is exactly this call.
 */
static void
on_chat_signal (XdRemoteTree *tree,
                XdNode       *chat,
                gpointer      user_data)
{
  Created *created = user_data;

  g_set_object (&created->chat, chat);
  created->wait.done = TRUE;
}

static void
on_failed (XdRemoteTree *tree,
           const char   *heading,
           const char   *message,
           gpointer      user_data)
{
  Wait *wait = user_data;

  wait->failure = g_strdup_printf ("%s: %s", heading, message);
  wait->done = TRUE;
}

/* Pairs, and hands back a tree that has been read once. */
static XdRemoteTree *
paired_tree (Daemon         *daemon,
             XdRemoteClient *client)
{
  g_autofree char *code = xd_remote_server_arm_pairing (daemon->server, 60);
  XdRemoteTree *tree;
  Wait pairing = { 0 };
  Wait loading = { 0 };

  xd_remote_client_pair_async (client, code, "test-device", NULL,
                               on_paired, &pairing);
  wait_for (&pairing);
  if (!pairing.ok)
    g_error ("pairing failed: %s", pairing.failure);

  tree = xd_remote_tree_new (client);
  g_signal_connect_swapped (tree, "loaded", G_CALLBACK (on_done), &loading);
  wait_for (&loading);
  g_signal_handlers_disconnect_by_data (tree, &loading);

  return tree;
}

/*
 * Waits for the tree to be read back, which is what every change ends with.
 *
 * A refusal ends the wait too, and says so: without that the test would sit
 * out its whole timeout and report only that nothing happened.
 */
static void
wait_for_reload (XdRemoteTree *tree)
{
  Wait wait = { 0 };

  g_signal_connect_swapped (tree, "loaded", G_CALLBACK (on_done), &wait);
  g_signal_connect (tree, "failed", G_CALLBACK (on_failed), &wait);

  wait_for (&wait);

  g_signal_handlers_disconnect_by_data (tree, &wait);

  if (wait.failure != NULL)
    g_error ("the daemon refused: %s", wait.failure);
}

/*
 * Managing the daemon's tree from a client.
 *
 * Every one of these goes the same way: the client says what it wants done,
 * the daemon does it, and the tree comes back read from the daemon rather than
 * imagined here -- so what is asserted is what the daemon actually has.
 */
static void
test_folders_and_chats_are_managed_from_the_client (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  XdNode *root;
  XdNode *zeno;
  XdNode *made;

  daemon_start (&daemon);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);
  root = xd_remote_tree_get_root (tree);

  /* A workspace at the top level of the remote. */
  xd_remote_tree_create_folder (tree, NULL, "Lunar");
  wait_for_reload (tree);

  g_assert_cmpuint (child_count (root), ==, 2);
  made = child_at (root, 0);
  g_assert_cmpstr (xd_node_get_name (made), ==, "Lunar");
  g_assert_nonnull (xd_node_get_folder_id (made));
  {
    g_autofree char *path = g_build_filename (daemon.root, "Lunar", NULL);

    g_assert_true (g_file_test (path, G_FILE_TEST_IS_DIR));
  }

  /* A folder inside one, then renamed. */
  xd_remote_tree_create_folder (tree, made, "Proxy");
  wait_for_reload (tree);
  g_assert_cmpuint (child_count (made), ==, 1);

  {
    XdNode *proxy = child_at (made, 0);

    g_assert_cmpstr (xd_node_get_name (proxy), ==, "Proxy");

    xd_remote_tree_rename_folder (tree, proxy, "Gateway");
    wait_for_reload (tree);

    /* The same node, because the folder's id travelled with the directory. */
    g_assert_true (child_at (made, 0) == proxy);
    g_assert_cmpstr (xd_node_get_name (proxy), ==, "Gateway");

    /* Moved under the workspace that was there from the start. */
    zeno = child_at (root, 1);
    g_assert_cmpstr (xd_node_get_name (zeno), ==, "Zeno");

    xd_remote_tree_move_folder (tree, proxy, zeno);
    wait_for_reload (tree);

    g_assert_cmpuint (child_count (made), ==, 0);
    g_assert_true (child_at (zeno, 0) == proxy);
    {
      g_autofree char *path = g_build_filename (daemon.root, "Zeno", "Gateway", NULL);

      g_assert_true (g_file_test (path, G_FILE_TEST_IS_DIR));
    }
  }

  /* A chat in it, which the tree hands over once it exists. */
  {
    Created created = { 0 };
    g_autofree char *chat_id = NULL;

    g_signal_connect (tree, "chat-created", G_CALLBACK (on_chat_signal), &created);
    xd_remote_tree_create_chat (tree, zeno, "from the client");
    wait_for (&created.wait);
    g_signal_handlers_disconnect_by_data (tree, &created);

    g_assert_nonnull (created.chat);
    g_assert_cmpstr (xd_node_get_name (created.chat), ==, "from the client");
    g_assert_true (xd_remote_tree_owns (tree, created.chat));

    chat_id = g_strdup (xd_node_get_chat_id (created.chat));

    /* The backend came from the daemon, which is the side that can read the
     * folder chain and knows which CLIs are installed. */
    {
      g_autoptr (XdChat) record =
        xd_storage_get_chat (daemon.storage, chat_id, NULL);

      g_assert_nonnull (record);
      g_assert_cmpstr (record->backend, ==, "claude");
      g_assert_nonnull (record->model);
    }

    xd_remote_tree_rename_chat (tree, created.chat, "renamed from the client");
    wait_for_reload (tree);
    g_assert_cmpstr (xd_node_get_name (created.chat), ==, "renamed from the client");

    /* Deleting it takes the row away, and says so for whoever is reading it. */
    {
      Created removed = { 0 };

      g_signal_connect (tree, "chat-removed", G_CALLBACK (on_chat_signal), &removed);
      xd_remote_tree_delete_chat (tree, created.chat);
      wait_for (&removed.wait);
      g_signal_handlers_disconnect_by_data (tree, &removed);

      g_assert_true (removed.chat == created.chat);
      g_assert_null (xd_remote_tree_lookup_chat (tree, chat_id));

      /* What is left is the folder moved in and the chat Zeno started with:
       * the delete took one row, not the folder's contents. */
      g_assert_cmpuint (child_count (zeno), ==, 2);

      g_clear_object (&removed.chat);
    }

    g_clear_object (&created.chat);
  }

  /* And a folder into the trash, gone from the daemon's disk. */
  xd_remote_tree_trash_folder (tree, made);
  wait_for_reload (tree);

  g_assert_cmpuint (child_count (root), ==, 1);
  {
    g_autofree char *path = g_build_filename (daemon.root, "Lunar", NULL);

    g_assert_false (g_file_test (path, G_FILE_TEST_EXISTS));
  }

  daemon_stop (&daemon);
}

/*
 * A daemon that will not do something says why, and nothing moves.
 *
 * The client cannot know in advance -- the tree it is looking at may have been
 * changed from another device a moment ago -- so a refusal has to arrive as an
 * answer rather than as a client-side guess that got lucky.
 */
static void
test_a_refused_change_is_reported (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  XdNode *zeno;
  Wait failure = { 0 };

  daemon_start (&daemon);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);

  zeno = child_at (xd_remote_tree_get_root (tree), 0);
  g_signal_connect (tree, "failed", G_CALLBACK (on_failed), &failure);

  /* A second folder of the same name, in the same place. */
  xd_remote_tree_create_folder (tree, NULL, "Zeno");
  wait_for (&failure);

  g_assert_nonnull (failure.failure);
  g_assert_true (g_str_has_prefix (failure.failure, "Could not create the folder"));

  /* And the tree is as it was. */
  g_assert_cmpuint (child_count (xd_remote_tree_get_root (tree)), ==, 1);
  g_assert_cmpstr (xd_node_get_name (zeno), ==, "Zeno");

  g_free (failure.failure);
  daemon_stop (&daemon);
}

/*
 * A remote that is not answering says so on its own row.
 *
 * This is what a window opened while the daemon is off looks like, and without
 * it the remote is a row with nothing under it -- indistinguishable from one
 * that is connected and empty.
 */
static void
test_a_remote_that_is_not_answering_shows_offline (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  XdNode *root;

  daemon_start (&daemon);

  /* Nothing has been connected to yet, which is the state a client is in for
   * as long as the machine it is pointed at is not there. */
  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = xd_remote_tree_new (client);
  root = xd_remote_tree_get_root (tree);

  g_assert_cmpint (xd_node_get_state (root), ==, XD_NODE_OFFLINE);
  g_assert_cmpstr (xd_node_get_icon_name (root), ==, "network-offline-symbolic");

  /* And stops saying so once the daemon answers. */
  {
    g_autofree char *code = xd_remote_server_arm_pairing (daemon.server, 60);
    Wait pairing = { 0 };
    Wait loading = { 0 };

    g_signal_connect_swapped (tree, "loaded", G_CALLBACK (on_done), &loading);

    xd_remote_client_pair_async (client, code, "test-device", NULL,
                                 on_paired, &pairing);
    wait_for (&pairing);
    if (!pairing.ok)
      g_error ("pairing failed: %s", pairing.failure);

    wait_for (&loading);
    g_signal_handlers_disconnect_by_data (tree, &loading);
    g_free (pairing.failure);
  }

  g_assert_cmpint (xd_node_get_state (root), ==, XD_NODE_IDLE);
  g_assert_cmpstr (xd_node_get_icon_name (root), ==, "network-server-symbolic");
  g_assert_cmpuint (child_count (root), ==, 1);

  daemon_stop (&daemon);
}

/*
 * Two devices on one daemon, without either asking.
 *
 * What one changes the other is told about: the daemon says what it did to
 * everything connected, so being up to date is not something a client has to
 * remember to do. Nothing here polls -- the second tree is waited on, and it
 * reloads because it was told to.
 */
static void
test_two_devices_stay_in_step (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) first = NULL;
  g_autoptr (XdRemoteClient) second = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autoptr (XdRemoteTree) watching = NULL;
  XdNode *root;

  daemon_start (&daemon);

  first = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, first);

  second = xd_remote_client_new ("127.0.0.1", daemon.port);
  watching = paired_tree (&daemon, second);

  root = xd_remote_tree_get_root (watching);
  g_assert_cmpuint (child_count (root), ==, 1);

  /* One device acts... */
  xd_remote_tree_create_folder (tree, NULL, "Lunar");

  /* ...and the other finds out on its own. */
  wait_for_reload (watching);

  g_assert_cmpuint (child_count (root), ==, 2);
  g_assert_cmpstr (xd_node_get_name (child_at (root, 0)), ==, "Lunar");

  daemon_stop (&daemon);
}

/*
 * A change made on the daemon's own machine reaches the devices watching it.
 *
 * The window open there writes to the same database and the same directories,
 * and a folder made in it is as real as one made from a phone -- so the daemon
 * watches its own disk and says what it sees. Here that is stood in for by
 * making the folder directly, which is all the local window does.
 */
static void
test_local_changes_reach_the_devices (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  XdNode *root;

  daemon_start (&daemon);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);
  root = xd_remote_tree_get_root (tree);

  g_assert_cmpuint (child_count (root), ==, 1);

  {
    g_autofree char *folder = g_build_filename (daemon.root, "Made Here", NULL);
    g_autofree char *dotfile = g_build_filename (folder, ".xd.json", NULL);

    g_assert_cmpint (g_mkdir_with_parents (folder, 0700), ==, 0);
    g_assert_true (g_file_set_contents (dotfile, "{\"id\": \"folder-2\"}", -1, NULL));
  }

  wait_for_reload (tree);

  g_assert_cmpuint (child_count (root), ==, 2);

  daemon_stop (&daemon);
}

int
main (int argc, char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/remote/pair-hello-tree", test_pair_hello_tree);
  g_test_add_func ("/remote/client-pairs-and-reads-the-tree",
                   test_client_pairs_and_reads_the_tree);
  g_test_add_func ("/remote/token-reconnects-and-strangers-are-turned-away",
                   test_token_reconnects_and_strangers_are_turned_away);
  g_test_add_func ("/remote/folders-and-chats-are-managed-from-the-client",
                   test_folders_and_chats_are_managed_from_the_client);
  g_test_add_func ("/remote/a-refused-change-is-reported",
                   test_a_refused_change_is_reported);
  g_test_add_func ("/remote/a-remote-that-is-not-answering-shows-offline",
                   test_a_remote_that_is_not_answering_shows_offline);
  g_test_add_func ("/remote/two-devices-stay-in-step",
                   test_two_devices_stay_in_step);
  g_test_add_func ("/remote/local-changes-reach-the-devices",
                   test_local_changes_reach_the_devices);

  return g_test_run ();
}
