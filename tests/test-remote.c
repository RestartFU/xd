#include "remote/client.h"
#include "remote/remote-tree.h"
#include "remote/server.h"
#include "remote/turn.h"
#include "remote/protocol.h"
#include "settings/agent-secrets.h"
#include "settings/folder-settings.h"
#include "storage/storage.h"
#include "util/worktree.h"

#include <glib/gstdio.h>
#include <json-glib/json-glib.h>
#include <signal.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

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

/*
 * Throwaway and self-signed, which is what the daemon mints for itself.
 *
 * An elliptic key rather than RSA: it is the same thing to everything here,
 * and it costs a fraction of the work -- which matters because generating one
 * is the only thing these tests do that they cannot interrupt, and they do it
 * for every daemon.
 */
static GTlsCertificate *
make_certificate (const char *dir,
                  const char *name)
{
  g_autofree char *cert_path = g_strdup_printf ("%s/%s-cert.pem", dir, name);
  g_autofree char *key_path = g_strdup_printf ("%s/%s-key.pem", dir, name);
  g_autofree char *command = NULL;
  g_autoptr (GError) error = NULL;
  GTlsCertificate *certificate;

  command = g_strdup_printf ("openssl req -x509 -newkey ec "
                             "-pkeyopt ec_paramgen_curve:prime256v1 -keyout %s "
                             "-out %s -days 1 -nodes -subj /CN=%s",
                             key_path, cert_path, name);
  g_assert_true (g_spawn_command_line_sync (command, NULL, NULL, NULL, NULL));

  certificate = g_tls_certificate_new_from_files (cert_path, key_path, &error);
  g_assert_no_error (error);

  return certificate;
}

/* Every daemon can use the same one; only the test about a certificate that
 * changed needs a second. */
static GTlsCertificate *
daemon_certificate (const char *dir)
{
  static GTlsCertificate *shared = NULL;

  if (shared == NULL)
    shared = make_certificate (dir, "daemon");

  return g_object_ref (shared);
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

  daemon->certificate = daemon_certificate (daemon->dir);
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

static void
run_in_directory (const char        *directory,
                  const char *const *argv)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_SILENCE |
                                        G_SUBPROCESS_FLAGS_STDERR_PIPE);
  g_subprocess_launcher_set_cwd (launcher, directory);
  process = g_subprocess_launcher_spawnv (launcher, argv, &error);
  g_assert_no_error (error);
  g_assert_nonnull (process);
  g_assert_true (g_subprocess_wait_check (process, NULL, &error));
  g_assert_no_error (error);
}

/* --- waiting on the loop --------------------------------------------------- */

typedef struct
{
  gboolean done;
  gboolean timed_out;
  gboolean ok;
  char *failure;
} Wait;

typedef struct
{
  Wait wait;
  JsonObject *reply;
} RemoteReply;

static void call_remote_request (XdRemoteClient *client,
                                 JsonBuilder    *builder,
                                 RemoteReply    *waiting);

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

/*
 * Sends a request and reads the answer to it.
 *
 * Events are stepped over on the way. The daemon talks without being asked --
 * a turn saying something, a tree that changed under it -- and one of those
 * arriving between a request and its reply is not the reply. Any client has to
 * do this; reading the next line and hoping is how the answers end up belonging
 * to the wrong questions.
 */
static JsonObject *
round_trip (GDataInputStream *in,
            GOutputStream    *out,
            const char       *request,
            JsonParser       *parser)
{
  g_output_stream_write_all (out, request, strlen (request), NULL, NULL, NULL);
  g_output_stream_write_all (out, "\n", 1, NULL, NULL, NULL);

  for (;;)
    {
      g_autofree char *line = NULL;
      JsonObject *object;

      line = g_data_input_stream_read_line_utf8 (in, NULL, NULL, NULL);
      if (line == NULL || !json_parser_load_from_data (parser, line, -1, NULL))
        return NULL;

      object = json_node_get_object (json_parser_get_root (parser));

      if (!json_object_has_member (object, "event"))
        return object;
    }
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
      "{\"op\":\"messages\",\"chat\":\"%s\",\"limit\":1}", exchange->chat_id);
    JsonArray *rows;

    reply = round_trip (in, out, messages, parser);
    if (reply == NULL || !json_object_get_boolean_member (reply, "ok"))
      FAIL ("messages refused");
    if (json_object_get_int_member_with_default (
          reply, "last_message_id", 0) != 2)
      FAIL ("message revision wrong");
    if (json_object_get_int_member_with_default (
          reply, "total_messages", 0) != 2)
      FAIL ("total message count wrong");
    rows = json_object_get_array_member (reply, "messages");
    if (json_array_get_length (rows) != 1)
      FAIL ("message limit ignored");
    if (g_strcmp0 (json_object_get_string_member (
                     json_array_get_object_element (rows, 0), "content"),
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

  /* The client half of this one blocks on reads in a thread of its own, so a
   * daemon that never answers would otherwise be a test that never ends. */
  {
    Wait wait = { 0 };
    guint id = g_timeout_add_seconds (30, on_wait_elapsed, &wait);

    while (!exchange.done && !wait.timed_out)
      g_main_context_iteration (NULL, TRUE);

    if (!wait.timed_out)
      g_source_remove (id);

    g_assert_false (wait.timed_out);
  }

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

typedef struct
{
  Wait wait;
  XdRemoteTree *tree;          /* unowned */
  GStrv names;
} AgentSecretsWait;

static void
on_agent_secrets_read (GObject      *source,
                       GAsyncResult *result,
                       gpointer      user_data)
{
  AgentSecretsWait *waiting = user_data;
  g_autoptr (GError) error = NULL;

  waiting->names = xd_remote_tree_get_agent_secrets_finish (
    waiting->tree, result, &error);
  if (waiting->names == NULL)
    waiting->wait.failure = g_strdup (error->message);
  else
    waiting->wait.ok = TRUE;
  waiting->wait.done = TRUE;
}

static void
on_agent_secrets_saved (GObject      *source,
                        GAsyncResult *result,
                        gpointer      user_data)
{
  AgentSecretsWait *waiting = user_data;
  g_autoptr (GError) error = NULL;

  waiting->wait.ok = xd_remote_tree_set_agent_secrets_finish (
    waiting->tree, result, &error);
  if (!waiting->wait.ok)
    waiting->wait.failure = g_strdup (error->message);
  waiting->wait.done = TRUE;
}

static void
test_agent_secrets_are_managed_without_reading_values (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *path = NULL;
  g_autofree char *old_override = g_strdup (
    g_getenv ("XD_AGENT_SECRETS_FILE"));
  g_autoptr (XdAgentSecrets) stored = NULL;
  g_autoptr (GError) error = NULL;
  g_auto (GStrv) environment = NULL;

  daemon_start (&daemon);
  path = g_build_filename (daemon.dir, "agent-secrets.json", NULL);
  g_setenv ("XD_AGENT_SECRETS_FILE", path, TRUE);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);

  {
    AgentSecretsWait read = { .tree = tree };

    xd_remote_tree_get_agent_secrets_async (
      tree, NULL, on_agent_secrets_read, &read);
    wait_for (&read.wait);
    g_assert_true (read.wait.ok);
    g_assert_null (read.names[0]);
    g_strfreev (read.names);
    g_free (read.wait.failure);
  }

  {
    const XdAgentSecretUpdate entries[] = {
      { "CLOUDFLARE_API_TOKEN", "never-return-this-value" },
    };
    AgentSecretsWait saved = { .tree = tree };

    xd_remote_tree_set_agent_secrets_async (
      tree, entries, G_N_ELEMENTS (entries), NULL,
      on_agent_secrets_saved, &saved);
    wait_for (&saved.wait);
    g_assert_true (saved.wait.ok);
    g_free (saved.wait.failure);
  }

  {
    AgentSecretsWait read = { .tree = tree };

    xd_remote_tree_get_agent_secrets_async (
      tree, NULL, on_agent_secrets_read, &read);
    wait_for (&read.wait);
    g_assert_true (read.wait.ok);
    g_assert_cmpstr (read.names[0], ==, "CLOUDFLARE_API_TOKEN");
    g_assert_null (read.names[1]);
    g_assert_cmpstr (read.names[0], !=, "never-return-this-value");
    g_strfreev (read.names);
    g_free (read.wait.failure);
  }

  /* A masked editor saves blank existing values as "keep". */
  {
    const XdAgentSecretUpdate entries[] = {
      { "CLOUDFLARE_API_TOKEN", NULL },
    };
    AgentSecretsWait saved = { .tree = tree };

    xd_remote_tree_set_agent_secrets_async (
      tree, entries, G_N_ELEMENTS (entries), NULL,
      on_agent_secrets_saved, &saved);
    wait_for (&saved.wait);
    g_assert_true (saved.wait.ok);
    g_free (saved.wait.failure);
  }

  stored = xd_agent_secrets_load (path, &error);
  g_assert_no_error (error);
  environment = g_new0 (char *, 1);
  environment = xd_agent_secrets_apply_environment (stored, environment);
  g_assert_cmpstr (
    g_environ_getenv (environment, "CLOUDFLARE_API_TOKEN"),
    ==, "never-return-this-value");

  /* Omitting a name is the explicit deletion mechanism. */
  {
    AgentSecretsWait saved = { .tree = tree };

    xd_remote_tree_set_agent_secrets_async (
      tree, NULL, 0, NULL, on_agent_secrets_saved, &saved);
    wait_for (&saved.wait);
    g_assert_true (saved.wait.ok);
    g_free (saved.wait.failure);
  }

  g_clear_pointer (&stored, xd_agent_secrets_free);
  stored = xd_agent_secrets_load (path, &error);
  g_assert_no_error (error);
  {
    g_auto (GStrv) names = xd_agent_secrets_names (stored);

    g_assert_null (names[0]);
  }

  if (old_override != NULL)
    g_setenv ("XD_AGENT_SECRETS_FILE", old_override, TRUE);
  else
    g_unsetenv ("XD_AGENT_SECRETS_FILE");
  daemon_stop (&daemon);
}

/*
 * Waits until the tree is what the test expects.
 *
 * Not for "a reload happened": the daemon reloads for its own reasons too --
 * it watches the database it writes to -- so one change can produce several,
 * and waiting for the first lands on whichever arrived, which may be older
 * than the thing being waited for.
 */
static void
wait_until (gboolean (*ready) (gpointer),
            gpointer  data)
{
  Wait wait = { 0 };
  guint id = g_timeout_add_seconds (10, on_wait_elapsed, &wait);

  while (!ready (data) && !wait.timed_out)
    g_main_context_iteration (NULL, TRUE);

  if (!wait.timed_out)
    g_source_remove (id);

  g_assert_false (wait.timed_out);
}

static void
iterate_for (guint milliseconds)
{
  Wait wait = { 0 };

  g_timeout_add (milliseconds, on_wait_elapsed, &wait);
  while (!wait.timed_out)
    g_main_context_iteration (NULL, TRUE);
}

typedef struct
{
  XdNode *node;
  XdStorage *storage;
  const char *chat_id;
  guint count;
  const char *name;
} Expected;

static gboolean
has_children (gpointer data)
{
  const Expected *expected = data;

  return child_count (expected->node) == expected->count;
}

static gboolean
stored_chat_is_named (gpointer data)
{
  const Expected *expected = data;
  g_autoptr (XdChat) chat =
    xd_storage_get_chat (expected->storage, expected->chat_id, NULL);

  return chat != NULL && g_strcmp0 (chat->title, expected->name) == 0;
}

/* Waits for @node to hold exactly @count rows. */
static void
wait_for_children (XdNode *node,
                   guint   count)
{
  Expected expected = { .node = node, .count = count };

  wait_until (has_children, &expected);
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
  wait_for_children (root, 2);

  made = child_at (root, 0);
  g_assert_cmpstr (xd_node_get_name (made), ==, "Lunar");
  g_assert_nonnull (xd_node_get_folder_id (made));
  {
    g_autofree char *path = g_build_filename (daemon.root, "Lunar", NULL);

    g_assert_true (g_file_test (path, G_FILE_TEST_IS_DIR));
  }

  /* A folder inside one, then renamed. */
  xd_remote_tree_create_folder (tree, made, "Proxy");
  wait_for_children (made, 1);

  {
    XdNode *proxy = child_at (made, 0);

    g_assert_cmpstr (xd_node_get_name (proxy), ==, "Proxy");

    xd_remote_tree_rename_folder (tree, proxy, "Gateway");
    g_assert_cmpstr (xd_node_get_name (proxy), ==, "Gateway");

    /* The same node, because the folder's id travelled with the directory. */
    g_assert_true (child_at (made, 0) == proxy);

    /* Moved under the workspace that was there from the start. */
    zeno = child_at (root, 1);
    g_assert_cmpstr (xd_node_get_name (zeno), ==, "Zeno");

    xd_remote_tree_move_folder (tree, proxy, zeno);
    wait_for_children (made, 0);

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
    xd_remote_tree_create_chat (tree, zeno, "from the client", NULL);
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

    {
      Expected stored = {
        .storage = daemon.storage,
        .chat_id = chat_id,
        .name = "renamed from the client",
      };

      xd_remote_tree_rename_chat (
        tree, created.chat, "renamed from the client");

      /* The UI changes now; the daemon remains the durable source of truth. */
      g_assert_cmpstr (
        xd_node_get_name (created.chat), ==, "renamed from the client");
      wait_until (stored_chat_is_named, &stored);
    }

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
  wait_for_children (root, 1);

  {
    g_autofree char *path = g_build_filename (daemon.root, "Lunar", NULL);

    g_assert_false (g_file_test (path, G_FILE_TEST_EXISTS));
  }

  daemon_stop (&daemon);
}

static void
set_remote_agent_option (XdRemoteClient *client,
                         const char     *chat_id,
                         const char     *option,
                         const char     *value)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  RemoteReply reply = { 0 };

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, "set-option");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, chat_id);
  json_builder_set_member_name (builder, "option");
  json_builder_add_string_value (builder, option);
  json_builder_set_member_name (builder, "value");
  json_builder_add_string_value (builder, value);
  json_builder_end_object (builder);

  call_remote_request (client, builder, &reply);
  json_object_unref (reply.reply);
  g_free (reply.wait.failure);
}

static void
test_remote_new_chat_inherits_last_changed_agent (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *chat_id = NULL;
  g_autoptr (XdChat) chat = NULL;
  XdNode *folder;
  Created created = { 0 };

  daemon_start (&daemon);
  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);
  folder = child_at (xd_remote_tree_get_root (tree), 0);

  set_remote_agent_option (client, daemon.chat_id, "backend", "codex");
  set_remote_agent_option (client, daemon.chat_id, "model", "gpt-5.6-codex");
  set_remote_agent_option (client, daemon.chat_id, "effort", "xhigh");
  set_remote_agent_option (client, daemon.chat_id, "access", "full");
  set_remote_agent_option (client, daemon.chat_id, "plan", "true");

  g_signal_connect (tree, "chat-created", G_CALLBACK (on_chat_signal), &created);
  xd_remote_tree_create_chat (tree, folder, "inherits agent", NULL);
  wait_for (&created.wait);
  g_signal_handlers_disconnect_by_data (tree, &created);

  g_assert_nonnull (created.chat);
  chat_id = g_strdup (xd_node_get_chat_id (created.chat));
  chat = xd_storage_get_chat (daemon.storage, chat_id, NULL);
  g_assert_nonnull (chat);
  g_assert_cmpstr (chat->backend, ==, "codex");
  g_assert_cmpstr (chat->model, ==, "gpt-5.6-codex");
  g_assert_cmpstr (chat->effort, ==, "xhigh");
  g_assert_cmpstr (chat->access, ==, "full");
  g_assert_true (chat->plan);

  g_clear_object (&created.chat);
  daemon_stop (&daemon);
}

/*
 * A folder's own context can be edited from another machine without exposing
 * its path. Clearing it restores inheritance; parent context is deliberately
 * not flattened into this reply.
 */
static void
test_folder_context_is_managed_from_the_client (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *folder_path = NULL;
  XdNode *folder;

  daemon_start (&daemon);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);
  folder = child_at (xd_remote_tree_get_root (tree), 0);
  folder_path = g_build_filename (daemon.root, "Zeno", NULL);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    RemoteReply read = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "folder-context");
    json_builder_set_member_name (builder, "folder");
    json_builder_add_string_value (builder, xd_node_get_folder_id (folder));
    json_builder_end_object (builder);
    call_remote_request (client, builder, &read);

    g_assert_true (read.wait.ok);
    g_assert_true (json_object_get_null_member (read.reply, "context"));
    json_object_unref (read.reply);
  }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    g_autoptr (XdFolderSettings) settings = NULL;
    RemoteReply saved = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "set-folder-context");
    json_builder_set_member_name (builder, "folder");
    json_builder_add_string_value (builder, xd_node_get_folder_id (folder));
    json_builder_set_member_name (builder, "context");
    json_builder_add_string_value (builder, "  Prefer small patches.  ");
    json_builder_end_object (builder);
    call_remote_request (client, builder, &saved);

    g_assert_true (saved.wait.ok);
    settings = xd_folder_settings_load (folder_path, NULL);
    g_assert_cmpstr (settings->instructions, ==, "Prefer small patches.");
    json_object_unref (saved.reply);
  }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    RemoteReply read = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "folder-context");
    json_builder_set_member_name (builder, "folder");
    json_builder_add_string_value (builder, xd_node_get_folder_id (folder));
    json_builder_end_object (builder);
    call_remote_request (client, builder, &read);

    g_assert_true (read.wait.ok);
    g_assert_cmpstr (
      json_object_get_string_member (read.reply, "context"), ==,
      "Prefer small patches.");
    json_object_unref (read.reply);
  }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    g_autoptr (XdFolderSettings) settings = NULL;
    RemoteReply cleared = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "set-folder-context");
    json_builder_set_member_name (builder, "folder");
    json_builder_add_string_value (builder, xd_node_get_folder_id (folder));
    json_builder_set_member_name (builder, "context");
    json_builder_add_null_value (builder);
    json_builder_end_object (builder);
    call_remote_request (client, builder, &cleared);

    g_assert_true (cleared.wait.ok);
    settings = xd_folder_settings_load (folder_path, NULL);
    g_assert_null (settings->instructions);
    json_object_unref (cleared.reply);
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
  wait_for_children (root, 2);

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

  wait_for_children (root, 2);

  daemon_stop (&daemon);
}

/*
 * An unnamed chat takes its name from the first thing asked in it.
 *
 * Done by the daemon, because the daemon is what writes the message down --
 * a chat named from one device has to be named on all of them. The turn itself
 * cannot run here (there is no CLI in a test image) and does not need to: the
 * naming happens before it starts, which is the only way the message it is
 * named after is the first one.
 */
static void
test_a_first_message_names_the_chat (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *chat_id = NULL;
  g_autoptr (GError) error = NULL;
  Wait sending = { 0 };

  daemon_start (&daemon);

  chat_id = xd_storage_create_chat (daemon.storage, "folder-1", "New Chat",
                                    "claude", NULL, NULL, NULL, &error);
  g_assert_no_error (error);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    g_autoptr (JsonNode) request = NULL;

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "send");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, chat_id);
    json_builder_set_member_name (builder, "text");
    json_builder_add_string_value (builder,
                                   "make the sidebar stop flickering\nand also this");
    json_builder_end_object (builder);
    request = json_builder_get_root (builder);

    xd_remote_client_call_async (client, request, NULL, on_messages, &sending);
    wait_for (&sending);
  }

  {
    g_autoptr (XdChat) chat = xd_storage_get_chat (daemon.storage, chat_id, NULL);

    g_assert_nonnull (chat);

    /* The first line only: a pasted stack trace should not become the title. */
    g_assert_cmpstr (chat->title, ==, "make the sidebar stop flickering");
  }

  g_free (sending.failure);
  daemon_stop (&daemon);
}

typedef struct
{
  XdStorage *storage;
  const char *chat_id;
} StoredTurn;

static gboolean
turn_was_stored (gpointer user_data)
{
  StoredTurn *turn = user_data;
  g_autoptr (GPtrArray) messages =
    xd_storage_list_messages (turn->storage, turn->chat_id, NULL);

  if (messages == NULL || messages->len == 0)
    return FALSE;

  return g_strcmp0 (
    ((XdMessage *) g_ptr_array_index (messages, messages->len - 1))->role,
    "duration") == 0;
}

static void
test_images_are_uploaded_to_the_daemon (void)
{
  static const guint8 png[] = {
    0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n',
  };
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *bin_dir = NULL;
  g_autofree char *program = NULL;
  g_autofree char *old_path = NULL;
  g_autofree char *test_path = NULL;
  g_autofree char *encoded = NULL;
  g_autofree char *uploaded = NULL;
  g_autofree char *contents = NULL;
  g_autofree guchar *preview_data = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  gsize preview_length = 0;
  RemoteReply sent = { 0 };
  RemoteReply preview = { 0 };
  RemoteReply options = { 0 };
  StoredTurn stored;

  daemon_start (&daemon);

  bin_dir = g_build_filename (daemon.dir, "bin", NULL);
  program = g_build_filename (bin_dir, "claude", NULL);
  g_assert_cmpint (g_mkdir_with_parents (bin_dir, 0700), ==, 0);
  g_assert_true (g_file_set_contents (
    program,
    "#!/bin/sh\n"
    "printf '%s\\n' "
    "'{\"type\":\"system\",\"subtype\":\"init\","
    "\"session_id\":\"test-image-upload\","
    "\"slash_commands\":[\"simplify\",\"review\"]}' "
    "'{\"type\":\"result\",\"result\":\"ok\","
    "\"session_id\":\"test-image-upload\",\"is_error\":false}'\n",
    -1, NULL));
  g_assert_cmpint (chmod (program, 0700), ==, 0);

  old_path = g_strdup (g_getenv ("PATH"));
  test_path = g_strdup_printf ("%s:%s", bin_dir,
                               old_path != NULL ? old_path : "");
  g_setenv ("PATH", test_path, TRUE);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);
  encoded = g_base64_encode (png, sizeof png);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "send");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "text");
    json_builder_add_string_value (builder, "inspect this");
    json_builder_set_member_name (builder, "attachments");
    json_builder_begin_array (builder);
    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "name");
    json_builder_add_string_value (builder, "screenshot.png");
    json_builder_set_member_name (builder, "mime");
    json_builder_add_string_value (builder, "image/png");
    json_builder_set_member_name (builder, "data");
    json_builder_add_string_value (builder, encoded);
    json_builder_end_object (builder);
    json_builder_end_array (builder);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &sent);
  }

  stored.storage = daemon.storage;
  stored.chat_id = daemon.chat_id;
  wait_until (turn_was_stored, &stored);
  /* ::finished stores the duration, then the server removes the turn on an
   * idle callback. Let that callback release its server-owned state before
   * this test tears the server down. */
  while (g_main_context_pending (NULL))
    g_main_context_iteration (NULL, FALSE);

  /* Command metadata survives the turn on the daemon, so a device opening
   * this chat later gets the same installed command list. */
  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    JsonArray *commands;

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "chat");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &options);
    commands = json_object_get_array_member (options.reply, "commands");
    g_assert_nonnull (commands);
    g_assert_cmpstr (json_array_get_string_element (commands, 0),
                     ==, "simplify");
    g_assert_cmpstr (json_array_get_string_element (commands, 1),
                     ==, "review");
  }

  messages = xd_storage_list_messages (daemon.storage, daemon.chat_id, NULL);
  for (guint i = 0; i < messages->len; i++)
    {
      XdMessage *message = g_ptr_array_index (messages, i);
      const char *start;
      const char *end;

      if (g_strcmp0 (message->role, "user") != 0 ||
          (start = strstr (message->content, "[image: ")) == NULL)
        continue;

      start += strlen ("[image: ");
      end = strchr (start, ']');
      g_assert_nonnull (end);
      uploaded = g_strndup (start, end - start);
      break;
    }

  g_assert_nonnull (uploaded);
  g_assert_true (g_file_get_contents (uploaded, &contents, NULL, NULL));
  g_assert_cmpmem (contents, sizeof png, png, sizeof png);

  /* The daemon path in the transcript does not exist on a paired device.
   * Preview bytes are read lazily through the authenticated connection. */
  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    const char *preview_encoded;

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "image-read");
    json_builder_set_member_name (builder, "path");
    json_builder_add_string_value (builder, uploaded);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &preview);
    g_assert_cmpstr (
      json_object_get_string_member_with_default (preview.reply, "mime", NULL),
      ==, "image/png");
    preview_encoded =
      json_object_get_string_member_with_default (preview.reply, "data", NULL);
    g_assert_nonnull (preview_encoded);
    preview_data = g_base64_decode (preview_encoded, &preview_length);
    g_assert_cmpmem (preview_data, preview_length, png, sizeof png);
  }

  unlink (uploaded);
  if (old_path != NULL)
    g_setenv ("PATH", old_path, TRUE);
  else
    g_unsetenv ("PATH");

  json_object_unref (sent.reply);
  json_object_unref (preview.reply);
  json_object_unref (options.reply);
  g_free (sent.wait.failure);
  g_free (preview.wait.failure);
  g_free (options.wait.failure);
  daemon_stop (&daemon);
}

typedef struct
{
  Wait wait;
  char *path;
  char *failure;
  GStrv entries;
} Listed;

static void
on_dir_listed (const char        *path,
               const char *const *entries,
               const char        *trouble,
               gpointer           user_data)
{
  Listed *listed = user_data;

  listed->failure = g_strdup (trouble);
  listed->path = g_strdup (path);
  listed->entries = g_strdupv ((char **) entries);
  listed->wait.done = TRUE;
}

/*
 * Browsing the daemon's directories.
 *
 * A chat that runs over there has to be pointed at a directory over there, and
 * only that side can say what is on it -- so choosing where a chat works needs
 * the daemon to answer for its own disk.
 */
static void
test_the_daemon_lists_its_directories (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  Listed listed = { 0 };

  daemon_start (&daemon);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);

  xd_remote_tree_list_dir (tree, daemon.root, on_dir_listed, &listed);
  wait_for (&listed.wait);

  if (listed.failure != NULL)
    g_error ("the daemon would not list %s: %s", daemon.root, listed.failure);

  g_assert_cmpstr (listed.path, ==, daemon.root);
  g_assert_nonnull (listed.entries);
  g_assert_true (g_strv_contains ((const char * const *) listed.entries, "Zeno"));

  g_free (listed.path);
  g_free (listed.failure);
  g_strfreev (listed.entries);
  daemon_stop (&daemon);
}

static void
test_remote_files_are_browsed_and_read (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *folder = NULL;
  g_autofree char *source_dir = NULL;
  g_autofree char *note = NULL;
  RemoteReply listed = { 0 };
  RemoteReply read = { 0 };
  gboolean saw_source = FALSE;
  gboolean saw_note = FALSE;

  daemon_start (&daemon);
  folder = g_build_filename (daemon.root, "Zeno", NULL);
  source_dir = g_build_filename (folder, "src", NULL);
  note = g_build_filename (folder, "notes.txt", NULL);
  g_assert_cmpint (g_mkdir_with_parents (source_dir, 0700), ==, 0);
  g_assert_true (g_file_set_contents (note, "remote preview\n", -1, NULL));

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    JsonArray *entries;

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "file-browse");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "action");
    json_builder_add_string_value (builder, "list");
    json_builder_set_member_name (builder, "path");
    json_builder_add_string_value (builder, "");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &listed);
    entries = json_object_get_array_member (listed.reply, "entries");
    g_assert_nonnull (entries);

    for (guint i = 0; i < json_array_get_length (entries); i++)
      {
        JsonObject *entry = json_array_get_object_element (entries, i);
        const char *name =
          json_object_get_string_member_with_default (entry, "name", "");
        gboolean directory =
          json_object_get_boolean_member_with_default (
            entry, "directory", FALSE);

        if (g_strcmp0 (name, "src") == 0 && directory)
          saw_source = TRUE;
        if (g_strcmp0 (name, "notes.txt") == 0 && !directory)
          saw_note = TRUE;
      }
  }

  g_assert_true (saw_source);
  g_assert_true (saw_note);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "file-browse");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "action");
    json_builder_add_string_value (builder, "read");
    json_builder_set_member_name (builder, "path");
    json_builder_add_string_value (builder, "notes.txt");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &read);
    g_assert_cmpstr (
      json_object_get_string_member_with_default (
        read.reply, "content", NULL),
      ==, "remote preview\n");
  }

  json_object_unref (listed.reply);
  json_object_unref (read.reply);
  g_free (listed.wait.failure);
  g_free (read.wait.failure);
  g_remove (note);
  g_rmdir (source_dir);
  daemon_stop (&daemon);
}

static void
on_terminal_reply (GObject      *source,
                   GAsyncResult *result,
                   gpointer      user_data)
{
  RemoteReply *waiting = user_data;
  g_autoptr (GError) error = NULL;

  waiting->reply =
    xd_remote_client_call_finish (XD_REMOTE_CLIENT (source), result, &error);
  if (waiting->reply == NULL)
    waiting->wait.failure = g_strdup (error->message);
  else
    waiting->wait.ok = TRUE;
  waiting->wait.done = TRUE;
}

typedef struct
{
  char *terminal_id;
  GString *output;
  guint columns;
  guint rows;
  gboolean closed;
} TerminalEvents;

static void
on_terminal_event (XdRemoteClient *client,
                   JsonObject     *event,
                   gpointer        user_data)
{
  TerminalEvents *events = user_data;
  const char *name =
    json_object_get_string_member_with_default (event, "event", NULL);
  const char *id =
    json_object_get_string_member_with_default (event, "terminal", NULL);

  if (events->terminal_id == NULL || g_strcmp0 (id, events->terminal_id) != 0)
    return;

  if (g_strcmp0 (name, "terminal-output") == 0)
    {
      const char *encoded =
        json_object_get_string_member_with_default (event, "data", "");
      g_autofree guchar *data = NULL;
      gsize length = 0;

      data = g_base64_decode (encoded, &length);
      g_string_append_len (events->output, (const char *) data, length);
    }
  else if (g_strcmp0 (name, "terminal-closed") == 0)
    {
      events->closed = TRUE;
    }
  else if (g_strcmp0 (name, "terminal-resized") == 0)
    {
      events->columns = (guint)
        json_object_get_int_member_with_default (event, "columns", 0);
      events->rows = (guint)
        json_object_get_int_member_with_default (event, "rows", 0);
    }
}

static gboolean
terminal_printed_marker (gpointer user_data)
{
  TerminalEvents *events = user_data;

  return strstr (events->output->str, "REMOTE_TERMINAL_OK") != NULL;
}

static gboolean
terminal_ignored_hup (gpointer user_data)
{
  TerminalEvents *events = user_data;

  return strstr (events->output->str, "HUP_READY") != NULL;
}

static gboolean
terminal_printed_tail (gpointer user_data)
{
  TerminalEvents *events = user_data;

  return strstr (events->output->str, "TAIL_MARKER") != NULL;
}

static gboolean
terminal_printed_after_resize (gpointer user_data)
{
  TerminalEvents *events = user_data;

  return strstr (events->output->str, "AFTER_RESIZE") != NULL;
}

static gboolean
terminal_resize_job_ready (gpointer user_data)
{
  TerminalEvents *events = user_data;

  return strstr (events->output->str, "RESIZE_JOB_READY") != NULL;
}

static gboolean
replay_contains (JsonArray  *replay,
                 const char *marker)
{
  g_autoptr (GString) output = g_string_new (NULL);

  for (guint i = 0; replay != NULL && i < json_array_get_length (replay); i++)
    {
      JsonObject *item = json_array_get_object_element (replay, i);
      const char *encoded =
        json_object_get_string_member_with_default (item, "data", NULL);
      g_autofree guchar *data = NULL;
      gsize length = 0;

      if (encoded == NULL)
        continue;
      data = g_base64_decode (encoded, &length);
      g_string_append_len (output, (const char *) data, length);
    }

  return strstr (output->str, marker) != NULL;
}

static gboolean
replay_crosses_resize (JsonArray *replay)
{
  g_autoptr (GString) output = g_string_new (NULL);
  gssize resize_at = -1;
  const char *before;
  const char *after;

  for (guint i = 0; replay != NULL && i < json_array_get_length (replay); i++)
    {
      JsonObject *item = json_array_get_object_element (replay, i);
      const char *encoded =
        json_object_get_string_member_with_default (item, "data", NULL);

      if (encoded != NULL)
        {
          g_autofree guchar *data = NULL;
          gsize length = 0;

          data = g_base64_decode (encoded, &length);
          g_string_append_len (output, (const char *) data, length);
        }
      else if (json_object_get_int_member_with_default (item, "columns", 0) == 120 &&
               json_object_get_int_member_with_default (item, "rows", 0) == 40)
        {
          resize_at = (gssize) output->len;
        }
    }

  before = strstr (output->str, "REMOTE_TERMINAL_OK");
  after = strstr (output->str, "AFTER_RESIZE");

  return before != NULL && after != NULL && resize_at >= 0 &&
         before < output->str + resize_at && after >= output->str + resize_at;
}

static gboolean
terminal_was_closed (gpointer user_data)
{
  return ((TerminalEvents *) user_data)->closed;
}

static gboolean
terminal_was_resized (gpointer user_data)
{
  TerminalEvents *events = user_data;

  return events->columns == 120 && events->rows == 40;
}

static void
call_remote_request (XdRemoteClient *client,
                     JsonBuilder    *builder,
                     RemoteReply    *waiting)
{
  g_autoptr (JsonNode) request = json_builder_get_root (builder);

  xd_remote_client_call_async (client, request, NULL, on_terminal_reply, waiting);
  wait_for (&waiting->wait);
  if (!waiting->wait.ok)
    g_error ("terminal request failed: %s", waiting->wait.failure);
}

static void
test_remote_workspace_choice_is_persisted (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *chat_id = NULL;
  g_autofree char *tracked = NULL;
  g_autofree char *existing = NULL;
  g_autofree char *listed_existing = NULL;
  g_autoptr (XdChat) stored = NULL;
  g_autoptr (GError) error = NULL;
  RemoteReply changed = { 0 };
  RemoteReply selected = { 0 };
  RemoteReply options = { 0 };
  const char *init[] = { "git", "init", "-q", "-b", "main", NULL };
  const char *add[] = { "git", "add", "tracked.txt", NULL };
  const char *commit[] = {
    "git", "-c", "user.name=xd tests", "-c", "user.email=xd@example.com",
    "commit", "-q", "-m", "initial", NULL
  };
  const char *add_worktree[] = {
    "git", "worktree", "add", "-q", "-b", "existing", NULL, "HEAD", NULL
  };

  daemon_start (&daemon);
  tracked = g_build_filename (daemon.root, "tracked.txt", NULL);
  existing = g_build_filename (daemon.dir, "existing-worktree", NULL);
  add_worktree[6] = existing;
  run_in_directory (daemon.root, init);
  g_assert_true (g_file_set_contents (tracked, "tracked\n", -1, &error));
  run_in_directory (daemon.root, add);
  run_in_directory (daemon.root, commit);
  run_in_directory (daemon.root, add_worktree);

  chat_id = xd_storage_create_chat (
    daemon.storage, "folder-1", "fresh", "claude",
    NULL, NULL, daemon.root, &error);
  g_assert_true (xd_storage_set_context_usage (
    daemon.storage, chat_id, "claude", "claude-opus-5",
    42000, 1000000, &error));
  g_assert_no_error (error);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "set-option");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, chat_id);
    json_builder_set_member_name (builder, "option");
    json_builder_add_string_value (builder, "new-worktree");
    json_builder_set_member_name (builder, "value");
    json_builder_add_string_value (builder, "true");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &changed);
  }

  stored = xd_storage_get_chat (daemon.storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_true (stored->new_worktree);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "chat");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, chat_id);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &options);
  }

  g_assert_true (json_object_get_boolean_member (options.reply, "new_worktree"));
  g_assert_false (json_object_get_boolean_member (options.reply, "has_messages"));
  g_assert_cmpint (json_object_get_int_member (options.reply, "context_used"),
                   ==, 42000);
  g_assert_cmpint (json_object_get_int_member (options.reply, "context_window"),
                   ==, 1000000);
  {
    JsonArray *worktrees =
      json_object_get_array_member (options.reply, "worktrees");

    g_assert_cmpuint (json_array_get_length (worktrees), ==, 2);
    g_assert_true (json_object_get_boolean_member (
      json_array_get_object_element (worktrees, 0), "current"));
    listed_existing = g_strdup (json_object_get_string_member (
      json_array_get_object_element (worktrees, 1), "path"));
    g_assert_true (xd_worktree_path_equal (listed_existing, existing));
  }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "set-option");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, chat_id);
    json_builder_set_member_name (builder, "option");
    json_builder_add_string_value (builder, "workspace");
    json_builder_set_member_name (builder, "value");
    json_builder_add_string_value (builder, listed_existing);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &selected);
  }

  g_clear_pointer (&stored, xd_chat_free);
  stored = xd_storage_get_chat (daemon.storage, chat_id, &error);
  g_assert_no_error (error);
  g_assert_false (stored->new_worktree);
  g_assert_cmpstr (stored->workdir, ==, listed_existing);

  json_object_unref (changed.reply);
  json_object_unref (selected.reply);
  json_object_unref (options.reply);
  g_free (changed.wait.failure);
  g_free (selected.wait.failure);
  g_free (options.wait.failure);
  daemon_stop (&daemon);
}

/*
 * Diff controls on a remote chat read the daemon's checkout, never a path on
 * the client. Complete branch and working-tree patches travel as ordinary
 * authenticated replies, including untracked files.
 */
static void
test_remote_diff_reads_the_daemon_repository (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *repository = NULL;
  g_autofree char *tracked = NULL;
  g_autofree char *new_dir = NULL;
  g_autofree char *new_file = NULL;
  g_autoptr (GError) error = NULL;
  RemoteReply base = { 0 };
  RemoteReply branch = { 0 };
  RemoteReply status = { 0 };
  RemoteReply diff = { 0 };
  RemoteReply untracked = { 0 };
  RemoteReply branch_all = { 0 };
  RemoteReply working_all = { 0 };
  const char *init[] = { "git", "init", "-q", "-b", "main", NULL };
  const char *switch_branch[] = { "git", "switch", "-q", "-c", "feature", NULL };
  const char *add[] = { "git", "add", "tracked.txt", NULL };
  const char *commit[] = {
    "git", "-c", "user.name=xd tests", "-c", "user.email=xd@example.com",
    "commit", "-q", "-m", "initial", NULL
  };

  daemon_start (&daemon);
  repository = g_build_filename (daemon.root, "Zeno", NULL);
  tracked = g_build_filename (repository, "tracked.txt", NULL);
  new_dir = g_build_filename (repository, "nested", NULL);
  new_file = g_build_filename (new_dir, "new.txt", NULL);

  run_in_directory (repository, init);
  g_assert_true (g_file_set_contents (tracked, "before\n", -1, &error));
  g_assert_no_error (error);
  run_in_directory (repository, add);
  run_in_directory (repository, commit);
  run_in_directory (repository, switch_branch);
  g_assert_true (g_file_set_contents (tracked, "branch\n", -1, &error));
  g_assert_no_error (error);
  run_in_directory (repository, add);
  run_in_directory (repository, commit);
  g_assert_true (g_file_set_contents (tracked, "after\n", -1, &error));
  g_assert_no_error (error);
  g_assert_cmpint (g_mkdir_with_parents (new_dir, 0700), ==, 0);
  g_assert_true (g_file_set_contents (new_file, "new\n", -1, &error));
  g_assert_no_error (error);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "diff-read");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "read");
    json_builder_add_string_value (builder, "base");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &base);
  }

  g_assert_cmpstr (json_object_get_string_member (base.reply, "output"), ==,
                   "main\n");

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "diff-read");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "read");
    json_builder_add_string_value (builder, "branch-status");
    json_builder_set_member_name (builder, "base");
    json_builder_add_string_value (builder, "main");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &branch);
  }

  g_assert_nonnull (strstr (
    json_object_get_string_member (branch.reply, "output"), "tracked.txt"));

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "diff-read");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "read");
    json_builder_add_string_value (builder, "working-status");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &status);
  }

  g_assert_nonnull (strstr (
    json_object_get_string_member (status.reply, "output"), "tracked.txt"));
  g_assert_nonnull (strstr (
    json_object_get_string_member (status.reply, "output"),
    "?? nested/new.txt"));

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "diff-read");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "read");
    json_builder_add_string_value (builder, "working-file");
    json_builder_set_member_name (builder, "path");
    json_builder_add_string_value (builder, "tracked.txt");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &diff);
  }

  {
    const char *output = json_object_get_string_member (diff.reply, "output");

    g_assert_nonnull (strstr (output, "-branch"));
    g_assert_nonnull (strstr (output, "+after"));
  }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "diff-read");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "read");
    json_builder_add_string_value (builder, "untracked-file");
    json_builder_set_member_name (builder, "path");
    json_builder_add_string_value (builder, "nested/new.txt");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &untracked);
  }

  g_assert_nonnull (strstr (
    json_object_get_string_member (untracked.reply, "output"), "+new"));

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "diff-read");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "read");
    json_builder_add_string_value (builder, "branch-all");
    json_builder_set_member_name (builder, "base");
    json_builder_add_string_value (builder, "main");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &branch_all);
  }

  {
    const char *output =
      json_object_get_string_member (branch_all.reply, "output");

    g_assert_nonnull (strstr (output, "-before"));
    g_assert_nonnull (strstr (output, "+branch"));
  }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "diff-read");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "read");
    json_builder_add_string_value (builder, "working-all");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &working_all);
  }

  {
    const char *output =
      json_object_get_string_member (working_all.reply, "output");

    g_assert_nonnull (strstr (output, "-branch"));
    g_assert_nonnull (strstr (output, "+after"));
    g_assert_nonnull (strstr (output, "nested/new.txt"));
    g_assert_nonnull (strstr (output, "+new"));
  }

  json_object_unref (base.reply);
  json_object_unref (branch.reply);
  json_object_unref (status.reply);
  json_object_unref (diff.reply);
  json_object_unref (untracked.reply);
  json_object_unref (branch_all.reply);
  json_object_unref (working_all.reply);
  g_free (base.wait.failure);
  g_free (branch.wait.failure);
  g_free (status.wait.failure);
  g_free (diff.wait.failure);
  g_free (untracked.wait.failure);
  g_free (branch_all.wait.failure);
  g_free (working_all.wait.failure);
  daemon_stop (&daemon);
}

/*
 * A pty lives on the daemon, accepts input over the authenticated line, and
 * keeps enough output for a second device joining after the command ran.
 */
static void
test_remote_terminal_is_shared_and_replayable (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  TerminalEvents events = { 0 };
  RemoteReply opened = { 0 };

  daemon_start (&daemon);
  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);
  events.output = g_string_new (NULL);
  g_signal_connect (client, "event", G_CALLBACK (on_terminal_event), &events);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-open");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "columns");
    json_builder_add_int_value (builder, 100);
    json_builder_set_member_name (builder, "rows");
    json_builder_add_int_value (builder, 30);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &opened);
    events.terminal_id =
      g_strdup (json_object_get_string_member (opened.reply, "id"));
  }

  {
    static const char command[] = "printf '\\nREMOTE_TERMINAL_OK\\n'\n";
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    g_autofree char *encoded =
      g_base64_encode ((const guint8 *) command, strlen (command));
    RemoteReply written = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-input");
    json_builder_set_member_name (builder, "terminal");
    json_builder_add_string_value (builder, events.terminal_id);
    json_builder_set_member_name (builder, "data");
    json_builder_add_string_value (builder, encoded);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &written);
    json_object_unref (written.reply);
    g_free (written.wait.failure);
  }

  wait_until (terminal_printed_marker, &events);

  {
    static const char command[] =
      "sh -c 'printf \"\\nRESIZE_JOB_READY\\n\"; "
      "while :; do sleep 1; done'\n";
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    g_autofree char *encoded =
      g_base64_encode ((const guint8 *) command, strlen (command));
    RemoteReply written = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-input");
    json_builder_set_member_name (builder, "terminal");
    json_builder_add_string_value (builder, events.terminal_id);
    json_builder_set_member_name (builder, "data");
    json_builder_add_string_value (builder, encoded);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &written);
    json_object_unref (written.reply);
    g_free (written.wait.failure);
  }

  wait_until (terminal_resize_job_ready, &events);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    RemoteReply listed = { 0 };
    JsonArray *rows;
    JsonObject *row;
    JsonArray *replay;

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-list");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &listed);
    rows = json_object_get_array_member (listed.reply, "terminals");
    g_assert_cmpuint (json_array_get_length (rows), ==, 1);
    row = json_array_get_object_element (rows, 0);
    g_assert_cmpstr (json_object_get_string_member (row, "id"), ==,
                     events.terminal_id);
    g_assert_cmpint (json_object_get_int_member (row, "columns"), ==, 100);
    g_assert_cmpint (json_object_get_int_member (row, "rows"), ==, 30);
    replay = json_object_get_array_member (row, "replay");
    g_assert_true (replay_contains (replay, "REMOTE_TERMINAL_OK"));

    json_object_unref (listed.reply);
    g_free (listed.wait.failure);
  }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    RemoteReply resized = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-resize");
    json_builder_set_member_name (builder, "terminal");
    json_builder_add_string_value (builder, events.terminal_id);
    json_builder_set_member_name (builder, "columns");
    json_builder_add_int_value (builder, 120);
    json_builder_set_member_name (builder, "rows");
    json_builder_add_int_value (builder, 40);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &resized);
    json_object_unref (resized.reply);
    g_free (resized.wait.failure);
  }

  wait_until (terminal_was_resized, &events);

  {
    /* The literal output marker is deliberately not present in the input:
     * the terminal echoes typed commands even while the foreground job owns
     * it. */
    static const char command[] =
      "printf '\\nDETACHED_%s\\n' AFTER_RESIZE\n";
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    g_autofree char *encoded =
      g_base64_encode ((const guint8 *) command, strlen (command));
    RemoteReply written = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-input");
    json_builder_set_member_name (builder, "terminal");
    json_builder_add_string_value (builder, events.terminal_id);
    json_builder_set_member_name (builder, "data");
    json_builder_add_string_value (builder, encoded);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &written);
    json_object_unref (written.reply);
    g_free (written.wait.failure);
  }

  /*
   * The foreground command must still own the pty after resizing. If the
   * shell observed it being suspended, it returns to its prompt and executes
   * the probe above.
   */
  iterate_for (500);
  g_assert_null (strstr (events.output->str, "DETACHED_AFTER_RESIZE"));

  {
    static const char command[] = "\003printf '\\nAFTER_RESIZE\\n'\n";
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    g_autofree char *encoded =
      g_base64_encode ((const guint8 *) command, strlen (command));
    RemoteReply written = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-input");
    json_builder_set_member_name (builder, "terminal");
    json_builder_add_string_value (builder, events.terminal_id);
    json_builder_set_member_name (builder, "data");
    json_builder_add_string_value (builder, encoded);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &written);
    json_object_unref (written.reply);
    g_free (written.wait.failure);
  }

  wait_until (terminal_printed_after_resize, &events);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    RemoteReply listed = { 0 };
    JsonArray *rows;
    JsonObject *row;

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-list");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &listed);
    rows = json_object_get_array_member (listed.reply, "terminals");
    row = json_array_get_object_element (rows, 0);
    g_assert_true (replay_crosses_resize (
      json_object_get_array_member (row, "replay")));
    json_object_unref (listed.reply);
    g_free (listed.wait.failure);
  }

  {
    static const char command[] =
      "trap '' HUP; printf '\\nHUP_READY\\n'; sleep 30\n";
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    g_autofree char *encoded =
      g_base64_encode ((const guint8 *) command, strlen (command));
    RemoteReply written = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-input");
    json_builder_set_member_name (builder, "terminal");
    json_builder_add_string_value (builder, events.terminal_id);
    json_builder_set_member_name (builder, "data");
    json_builder_add_string_value (builder, encoded);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &written);
    json_object_unref (written.reply);
    g_free (written.wait.failure);
  }

  wait_until (terminal_ignored_hup, &events);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    RemoteReply killed = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-kill");
    json_builder_set_member_name (builder, "terminal");
    json_builder_add_string_value (builder, events.terminal_id);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &killed);
    json_object_unref (killed.reply);
    g_free (killed.wait.failure);
  }

  wait_until (terminal_was_closed, &events);

  /*
   * A second shell emits more than one read callback's budget and exits.
   * Its tail must arrive before terminal-closed.
   */
  events.closed = FALSE;
  g_string_truncate (events.output, 0);
  g_clear_pointer (&events.terminal_id, g_free);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    RemoteReply second = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-open");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &second);
    events.terminal_id =
      g_strdup (json_object_get_string_member (second.reply, "id"));
    json_object_unref (second.reply);
    g_free (second.wait.failure);
  }

  {
    static const char command[] =
      "head -c 200000 /dev/zero | tr '\\0' X; "
      "printf '\\nTAIL_MARKER\\n'; exit\n";
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    g_autofree char *encoded =
      g_base64_encode ((const guint8 *) command, strlen (command));
    RemoteReply written = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "terminal-input");
    json_builder_set_member_name (builder, "terminal");
    json_builder_add_string_value (builder, events.terminal_id);
    json_builder_set_member_name (builder, "data");
    json_builder_add_string_value (builder, encoded);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &written);
    json_object_unref (written.reply);
    g_free (written.wait.failure);
  }

  wait_until (terminal_was_closed, &events);
  g_assert_true (terminal_printed_tail (&events));

  g_signal_handlers_disconnect_by_data (client, &events);
  json_object_unref (opened.reply);
  g_free (opened.wait.failure);
  g_free (events.terminal_id);
  g_string_free (events.output, TRUE);
  daemon_stop (&daemon);
}

typedef struct
{
  const char *chat_id;
  char *text;
  guint changes;
} QueueEvents;

static void
on_queue_event (XdRemoteClient *client,
                JsonObject     *event,
                gpointer        user_data)
{
  QueueEvents *seen = user_data;
  const char *name =
    json_object_get_string_member_with_default (event, "event", NULL);
  const char *chat_id =
    json_object_get_string_member_with_default (event, "chat", NULL);

  if (g_strcmp0 (name, "queued") != 0 ||
      g_strcmp0 (chat_id, seen->chat_id) != 0)
    return;

  g_free (seen->text);
  seen->text = g_strdup (
    json_object_get_string_member_with_default (event, "text", NULL));
  seen->changes++;
}

static gboolean
both_queues_changed (gpointer user_data)
{
  QueueEvents *seen = user_data;

  return seen[0].changes > 0 && seen[1].changes > 0;
}

static gboolean
queue_changed (gpointer user_data)
{
  return ((QueueEvents *) user_data)->changes > 0;
}

typedef struct
{
  const char *chat_id;
  gboolean queue_cleared;
  gboolean turn_started;
} SteerEvents;

static void
on_steer_event (XdRemoteClient *client,
                JsonObject     *event,
                gpointer        user_data)
{
  SteerEvents *seen = user_data;
  const char *name =
    json_object_get_string_member_with_default (event, "event", NULL);
  const char *chat_id =
    json_object_get_string_member_with_default (event, "chat", NULL);

  if (g_strcmp0 (chat_id, seen->chat_id) != 0)
    return;

  if (g_strcmp0 (name, "queued") == 0 &&
      !json_object_has_member (event, "text"))
    seen->queue_cleared = TRUE;
  else if (g_strcmp0 (name, "turn-started") == 0)
    seen->turn_started = TRUE;
}

static gboolean
steer_started_queued_turn (gpointer user_data)
{
  SteerEvents *seen = user_data;

  return seen->queue_cleared && seen->turn_started;
}

/*
 * A send raced against turn-started used to fail with "already working".
 * Daemon knows the real state, so it must accept that send as queued steer
 * text even when the client still thinks the chat is idle.
 */
static void
test_send_during_turn_queues (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *bin_dir = NULL;
  g_autofree char *program = NULL;
  g_autofree char *old_path = NULL;
  g_autofree char *test_path = NULL;
  RemoteReply started = { 0 };
  RemoteReply steered = { 0 };
  QueueEvents seen = { 0 };

  daemon_start (&daemon);
  seen.chat_id = daemon.chat_id;

  bin_dir = g_build_filename (daemon.dir, "bin", NULL);
  program = g_build_filename (bin_dir, "claude", NULL);
  g_assert_cmpint (g_mkdir_with_parents (bin_dir, 0700), ==, 0);
  g_assert_true (g_file_set_contents (
    program,
    "#!/bin/sh\n"
    "printf '%s\\n' "
    "'{\"type\":\"system\",\"subtype\":\"init\","
    "\"session_id\":\"test-send-race\"}'\n"
    "exec sleep 30\n",
    -1, NULL));
  g_assert_cmpint (chmod (program, 0700), ==, 0);

  old_path = g_strdup (g_getenv ("PATH"));
  test_path = g_strdup_printf ("%s:%s", bin_dir,
                               old_path != NULL ? old_path : "");
  g_setenv ("PATH", test_path, TRUE);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);
  g_signal_connect (client, "event", G_CALLBACK (on_queue_event), &seen);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "send");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "text");
    json_builder_add_string_value (builder, "keep working");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &started);
  }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "send");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "text");
    json_builder_add_string_value (builder, "steer now");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &steered);
  }

  wait_until (queue_changed, &seen);
  g_assert_cmpstr (seen.text, ==, "steer now");

  {
    g_autoptr (XdChat) stored =
      xd_storage_get_chat (daemon.storage, daemon.chat_id, NULL);

    g_assert_cmpstr (stored->queued, ==, "steer now");
  }

  if (old_path != NULL)
    g_setenv ("PATH", old_path, TRUE);
  else
    g_unsetenv ("PATH");

  g_signal_handlers_disconnect_by_data (client, &seen);
  json_object_unref (started.reply);
  json_object_unref (steered.reply);
  g_free (started.wait.failure);
  g_free (steered.wait.failure);
  g_free (seen.text);
  daemon_stop (&daemon);
}

/*
 * The client may know about a queued instruction before it knows a remote turn
 * stopped. Clicking steer must still do work: cancel is sent regardless of the
 * client's stale state, and an idle daemon promotes the persisted queue.
 */
static void
test_steer_starts_an_idle_remote_queue (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) client = NULL;
  g_autoptr (XdRemoteTree) tree = NULL;
  g_autofree char *bin_dir = NULL;
  g_autofree char *program = NULL;
  g_autofree char *old_path = NULL;
  g_autofree char *test_path = NULL;
  RemoteReply queued = { 0 };
  RemoteReply steered = { 0 };
  SteerEvents seen = { 0 };

  daemon_start (&daemon);
  seen.chat_id = daemon.chat_id;

  bin_dir = g_build_filename (daemon.dir, "bin", NULL);
  program = g_build_filename (bin_dir, "claude", NULL);
  g_assert_cmpint (g_mkdir_with_parents (bin_dir, 0700), ==, 0);
  g_assert_true (g_file_set_contents (
    program,
    "#!/bin/sh\n"
    "printf '%s\\n' "
    "'{\"type\":\"system\",\"subtype\":\"init\","
    "\"session_id\":\"test-steered\"}'\n"
    "exec sleep 30\n",
    -1, NULL));
  g_assert_cmpint (chmod (program, 0700), ==, 0);

  old_path = g_strdup (g_getenv ("PATH"));
  test_path = g_strdup_printf ("%s:%s", bin_dir,
                               old_path != NULL ? old_path : "");
  g_setenv ("PATH", test_path, TRUE);

  client = xd_remote_client_new ("127.0.0.1", daemon.port);
  tree = paired_tree (&daemon, client);
  g_signal_connect (client, "event", G_CALLBACK (on_steer_event), &seen);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "queue");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "text");
    json_builder_add_string_value (builder, "follow up now");
    json_builder_end_object (builder);

    call_remote_request (client, builder, &queued);
  }

  {
    g_autoptr (XdChat) stored =
      xd_storage_get_chat (daemon.storage, daemon.chat_id, NULL);

    g_assert_cmpstr (stored->queued, ==, "follow up now");
  }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "cancel");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_end_object (builder);

    call_remote_request (client, builder, &steered);
  }

  wait_until (steer_started_queued_turn, &seen);

  {
    g_autoptr (XdChat) stored =
      xd_storage_get_chat (daemon.storage, daemon.chat_id, NULL);

    g_assert_null (stored->queued);
  }

  if (old_path != NULL)
    g_setenv ("PATH", old_path, TRUE);
  else
    g_unsetenv ("PATH");

  g_signal_handlers_disconnect_by_data (client, &seen);
  json_object_unref (queued.reply);
  json_object_unref (steered.reply);
  g_free (queued.wait.failure);
  g_free (steered.wait.failure);
  daemon_stop (&daemon);
}

/*
 * A device joining after work started must learn that state from the tree
 * snapshot. It never saw the earlier turn-started event.
 */
static void
test_a_joining_device_sees_an_active_turn (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdRemoteClient) sender = NULL;
  g_autoptr (XdRemoteClient) joining = NULL;
  g_autoptr (XdRemoteTree) sender_tree = NULL;
  g_autoptr (XdRemoteTree) joining_tree = NULL;
  g_autofree char *bin_dir = NULL;
  g_autofree char *program = NULL;
  g_autofree char *old_path = NULL;
  g_autofree char *test_path = NULL;
  RemoteReply started = { 0 };
  QueueEvents queues[2] = { 0 };
  XdNode *chat;

  daemon_start (&daemon);
  queues[0].chat_id = daemon.chat_id;
  queues[1].chat_id = daemon.chat_id;

  /* Keep a real daemon turn alive without requiring an installed CLI. */
  bin_dir = g_build_filename (daemon.dir, "bin", NULL);
  program = g_build_filename (bin_dir, "claude", NULL);
  g_assert_cmpint (g_mkdir_with_parents (bin_dir, 0700), ==, 0);
  g_assert_true (g_file_set_contents (
    program,
    "#!/bin/sh\n"
    "printf '%s\\n' "
    "'{\"type\":\"system\",\"subtype\":\"init\","
    "\"session_id\":\"test-running\"}'\n"
    "exec sleep 30\n",
    -1, NULL));
  g_assert_cmpint (chmod (program, 0700), ==, 0);

  old_path = g_strdup (g_getenv ("PATH"));
  test_path = g_strdup_printf ("%s:%s", bin_dir,
                               old_path != NULL ? old_path : "");
  g_setenv ("PATH", test_path, TRUE);

  sender = xd_remote_client_new ("127.0.0.1", daemon.port);
  sender_tree = paired_tree (&daemon, sender);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "send");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "text");
    json_builder_add_string_value (builder, "keep working");
    json_builder_end_object (builder);

    call_remote_request (sender, builder, &started);
  }

  if (old_path != NULL)
    g_setenv ("PATH", old_path, TRUE);
  else
    g_unsetenv ("PATH");

  /* This tree is created after turn-started, so only its snapshot can know. */
  joining = xd_remote_client_new ("127.0.0.1", daemon.port);
  joining_tree = paired_tree (&daemon, joining);
  chat = xd_remote_tree_lookup_chat (joining_tree, daemon.chat_id);

  g_assert_nonnull (chat);
  g_assert_cmpint (xd_node_get_state (chat), ==, XD_NODE_WORKING);

  /* Queue belongs to daemon's chat: both devices receive the same update and
   * a fresh chat snapshot carries it too. */
  g_signal_connect (sender, "event", G_CALLBACK (on_queue_event), &queues[0]);
  g_signal_connect (joining, "event", G_CALLBACK (on_queue_event), &queues[1]);
  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    RemoteReply queued = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "queue");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_set_member_name (builder, "text");
    json_builder_add_string_value (builder, "follow up");
    json_builder_end_object (builder);

    call_remote_request (sender, builder, &queued);
    json_object_unref (queued.reply);
    g_free (queued.wait.failure);
  }
  wait_until (both_queues_changed, queues);
  g_assert_cmpstr (queues[0].text, ==, "follow up");
  g_assert_cmpstr (queues[1].text, ==, "follow up");

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();
    RemoteReply options = { 0 };

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "op");
    json_builder_add_string_value (builder, "chat");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, daemon.chat_id);
    json_builder_end_object (builder);

    call_remote_request (joining, builder, &options);
    g_assert_true (json_object_has_member (options.reply, "working_for"));
    g_assert_cmpint (json_object_get_int_member (options.reply, "working_for"),
                     >=, 0);
    g_assert_cmpstr (json_object_get_string_member (options.reply, "queued"),
                     ==, "follow up");
    json_object_unref (options.reply);
    g_free (options.wait.failure);
  }

  {
    g_autoptr (XdChat) stored =
      xd_storage_get_chat (daemon.storage, daemon.chat_id, NULL);

    g_assert_cmpstr (stored->queued, ==, "follow up");
  }

  g_signal_handlers_disconnect_by_data (sender, &queues[0]);
  g_signal_handlers_disconnect_by_data (joining, &queues[1]);
  g_free (queues[0].text);
  g_free (queues[1].text);
  json_object_unref (started.reply);
  g_free (started.wait.failure);
  daemon_stop (&daemon);
}

typedef struct
{
  Wait tool;
  Wait text;
  Wait finished;
  char *tool_summary;
  GString *said;
} InterruptedTurn;

static void
on_interrupted_turn_tool (XdDaemonTurn *turn,
                          const char   *summary,
                          gpointer      user_data)
{
  InterruptedTurn *seen = user_data;

  g_free (seen->tool_summary);
  seen->tool_summary = g_strdup (summary);
  seen->tool.done = TRUE;
}

static void
on_interrupted_turn_text (XdDaemonTurn *turn,
                          const char   *delta,
                          gpointer      user_data)
{
  InterruptedTurn *seen = user_data;

  g_string_append (seen->said, delta);
  if (strstr (seen->said->str, "after tool") != NULL)
    seen->text.done = TRUE;
}

static void
on_interrupted_turn_finished (XdDaemonTurn *turn,
                              gboolean      success,
                              const char   *message,
                              gpointer      user_data)
{
  InterruptedTurn *seen = user_data;

  seen->finished.ok = success;
  seen->finished.failure = g_strdup (message);
  seen->finished.done = TRUE;
}

/*
 * Interrupting is still a completed timeline. Speech, tools, and measured
 * duration must all survive reopening the database in the order they happened.
 */
static void
test_an_interrupted_turn_keeps_its_timeline (void)
{
  Daemon daemon = { 0 };
  g_autoptr (XdDaemonTurn) turn = NULL;
  g_autoptr (XdStorage) reopened = NULL;
  g_autoptr (GPtrArray) messages = NULL;
  g_autofree char *bin_dir = NULL;
  g_autofree char *program = NULL;
  g_autofree char *old_path = NULL;
  g_autofree char *test_path = NULL;
  g_autofree char *db_path = NULL;
  g_autofree char *chat_id = NULL;
  g_autoptr (GError) error = NULL;
  InterruptedTurn seen = { 0 };

  daemon_start (&daemon);

  bin_dir = g_build_filename (daemon.dir, "bin", NULL);
  program = g_build_filename (bin_dir, "claude", NULL);
  g_assert_cmpint (g_mkdir_with_parents (bin_dir, 0700), ==, 0);
  g_assert_true (g_file_set_contents (
    program,
    "#!/bin/sh\n"
    "printf '%s\\n' "
    "'{\"type\":\"system\",\"subtype\":\"init\","
    "\"session_id\":\"test-interrupted\"}'\n"
    "printf '%s\\n' "
    "'{\"type\":\"assistant\",\"message\":{\"content\":["
    "{\"type\":\"text\",\"text\":\"before tool\"}]}}'\n"
    "printf '%s\\n' "
    "'{\"type\":\"assistant\",\"message\":{\"content\":["
    "{\"type\":\"tool_use\",\"name\":\"Read\","
    "\"input\":{\"file_path\":\"src/main.c\"}}]}}'\n"
    "printf '%s\\n' "
    "'{\"type\":\"assistant\",\"message\":{\"content\":["
    "{\"type\":\"text\",\"text\":\"after tool\"}]}}'\n"
    "exec sleep 30\n",
    -1, NULL));
  g_assert_cmpint (chmod (program, 0700), ==, 0);

  old_path = g_strdup (g_getenv ("PATH"));
  test_path = g_strdup_printf ("%s:%s", bin_dir,
                               old_path != NULL ? old_path : "");
  g_setenv ("PATH", test_path, TRUE);

  seen.said = g_string_new (NULL);
  turn = xd_daemon_turn_new (daemon.storage, daemon.root);
  g_signal_connect (turn, "tool",
                    G_CALLBACK (on_interrupted_turn_tool), &seen);
  g_signal_connect (turn, "text",
                    G_CALLBACK (on_interrupted_turn_text), &seen);
  g_signal_connect (turn, "finished",
                    G_CALLBACK (on_interrupted_turn_finished), &seen);

  g_assert_true (xd_daemon_turn_start (turn, daemon.chat_id,
                                       "inspect it", &error));
  g_assert_no_error (error);

  if (old_path != NULL)
    g_setenv ("PATH", old_path, TRUE);
  else
    g_unsetenv ("PATH");

  wait_for (&seen.tool);
  wait_for (&seen.text);
  g_assert_nonnull (strstr (seen.tool_summary, "src/main.c"));

  xd_daemon_turn_cancel (turn);
  wait_for (&seen.finished);
  g_assert_true (seen.finished.ok);

  db_path = g_build_filename (daemon.dir, "chats.db", NULL);
  chat_id = g_strdup (daemon.chat_id);
  g_clear_object (&turn);
  daemon_stop (&daemon);

  reopened = xd_storage_new (db_path, &error);
  g_assert_no_error (error);
  messages = xd_storage_list_messages (reopened, chat_id, &error);
  g_assert_no_error (error);

  g_assert_cmpuint (messages->len, ==, 7);
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 2))->role,
                   ==, "user");
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 3))->role,
                   ==, "assistant");
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 3))->content,
                   ==, "before tool");
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 4))->role,
                   ==, "tool");
  g_assert_nonnull (strstr (
    ((XdMessage *) g_ptr_array_index (messages, 4))->content, "src/main.c"));
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 5))->role,
                   ==, "assistant");
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 5))->content,
                   ==, "after tool");
  g_assert_cmpstr (((XdMessage *) g_ptr_array_index (messages, 6))->role,
                   ==, "duration");
  g_assert_cmpint (g_ascii_strtoll (
                     ((XdMessage *) g_ptr_array_index (messages, 6))->content,
                     NULL, 10), >=, 0);

  g_free (seen.tool_summary);
  g_free (seen.finished.failure);
  g_string_free (seen.said, TRUE);
}

/*
 * Which test is running, and a watchdog that says so.
 *
 * These talk over real sockets to a real daemon, and a wedged one on a machine
 * nobody can reach is otherwise a timeout with nothing in it: the whole
 * binary is killed and the log says only that it took too long. This turns
 * that into the name of the test it was in.
 */
static const char *running_test = "(none)";

static void
on_stuck (int signal_number)
{
  /* Only what is safe to call from a signal handler. */
  const char *prefix = "\n*** test-remote is stuck in: ";

  (void) !write (STDERR_FILENO, prefix, strlen (prefix));
  (void) !write (STDERR_FILENO, running_test, strlen (running_test));
  (void) !write (STDERR_FILENO, "\n", 1);

  _exit (99);
}

typedef struct
{
  const char *name;
  void (*run) (void);
} Test;

static void
run_test (gconstpointer data)
{
  const Test *test = data;

  running_test = test->name;
  test->run ();
}

/* Registered through the trampoline above, so the watchdog knows the name. */
#define ADD(path, fn) G_STMT_START {           \
    static const Test test = { path, fn };     \
    g_test_add_data_func (path, &test, run_test); \
  } G_STMT_END

int
main (int argc, char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  /* Under meson's own 180; overridable so the watchdog itself can be tried. */
  {
    const char *seconds = g_getenv ("XD_TEST_WATCHDOG");

    signal (SIGALRM, on_stuck);
    alarm (seconds != NULL ? (guint) g_ascii_strtoull (seconds, NULL, 10) : 150);
  }

  ADD ("/remote/pair-hello-tree", test_pair_hello_tree);
  ADD ("/remote/client-pairs-and-reads-the-tree", test_client_pairs_and_reads_the_tree);
  ADD ("/remote/token-reconnects-and-strangers-are-turned-away", test_token_reconnects_and_strangers_are_turned_away);
  ADD ("/remote/folders-and-chats-are-managed-from-the-client", test_folders_and_chats_are_managed_from_the_client);
  ADD ("/remote/new-chat-inherits-last-changed-agent", test_remote_new_chat_inherits_last_changed_agent);
  ADD ("/remote/folder-context-is-managed-from-the-client", test_folder_context_is_managed_from_the_client);
  ADD ("/remote/agent-secrets-are-managed-without-reading-values", test_agent_secrets_are_managed_without_reading_values);
  ADD ("/remote/a-refused-change-is-reported", test_a_refused_change_is_reported);
  ADD ("/remote/a-remote-that-is-not-answering-shows-offline", test_a_remote_that_is_not_answering_shows_offline);
  ADD ("/remote/two-devices-stay-in-step", test_two_devices_stay_in_step);
  ADD ("/remote/local-changes-reach-the-devices", test_local_changes_reach_the_devices);
  ADD ("/remote/a-first-message-names-the-chat", test_a_first_message_names_the_chat);
  ADD ("/remote/images-are-uploaded-to-the-daemon", test_images_are_uploaded_to_the_daemon);
  ADD ("/remote/the-daemon-lists-its-directories", test_the_daemon_lists_its_directories);
  ADD ("/remote/files-are-browsed-and-read", test_remote_files_are_browsed_and_read);
  ADD ("/remote/workspace-choice-is-persisted", test_remote_workspace_choice_is_persisted);
  ADD ("/remote/diff-reads-the-daemon-repository", test_remote_diff_reads_the_daemon_repository);
  ADD ("/remote/terminal-is-shared-and-replayable", test_remote_terminal_is_shared_and_replayable);
  ADD ("/remote/send-during-turn-queues", test_send_during_turn_queues);
  ADD ("/remote/steer-starts-an-idle-remote-queue", test_steer_starts_an_idle_remote_queue);
  ADD ("/remote/a-joining-device-sees-an-active-turn", test_a_joining_device_sees_an_active_turn);
  ADD ("/remote/an-interrupted-turn-keeps-its-timeline", test_an_interrupted_turn_keeps_its_timeline);

  return g_test_run ();
}
