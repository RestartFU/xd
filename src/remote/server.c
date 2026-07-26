#include "server.h"

#include <json-glib/json-glib.h>
#include <string.h>

/*
 * One connection, one line-oriented JSON conversation.
 *
 * The framing is the same one the CLI parsers read all day: a JSON object
 * per line. Requests carry an "op"; every reply carries "ok", and errors say
 * why. Nothing here is stateful beyond whether the connection has proven a
 * token, so a dropped socket costs nothing to re-open.
 */

typedef struct
{
  HyRemoteServer *server;      /* unowned; the server outlives connections */
  GDataInputStream *in;
  GOutputStream *out;
  GIOStream *stream;
  gboolean authed;
} Connection;

struct _HyRemoteServer
{
  GObject parent_instance;

  HyStorage *storage;
  char *root_path;
  GSocketService *service;
  GTlsCertificate *certificate;
  guint16 port;

  char *pairing_code;
  gint64 pairing_expires;      /* monotonic microseconds */
};

G_DEFINE_FINAL_TYPE (HyRemoteServer, hy_remote_server, G_TYPE_OBJECT)

static void read_next_request (Connection *connection);

/* --- small json helpers ---------------------------------------------------- */

static const char *
member_string (JsonObject *object,
               const char *name)
{
  if (object == NULL || !json_object_has_member (object, name))
    return NULL;

  return json_object_get_string_member_with_default (object, name, NULL);
}

static void
send_json (Connection  *connection,
           JsonBuilder *builder)
{
  g_autoptr (JsonGenerator) generator = json_generator_new ();
  g_autoptr (JsonNode) root = json_builder_get_root (builder);
  g_autofree char *text = NULL;
  gsize length;

  json_generator_set_root (generator, root);
  text = json_generator_to_data (generator, &length);

  g_output_stream_write_all (connection->out, text, length, NULL, NULL, NULL);
  g_output_stream_write_all (connection->out, "\n", 1, NULL, NULL, NULL);
}

static void
send_error (Connection *connection,
            const char *message)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, FALSE);
  json_builder_set_member_name (builder, "error");
  json_builder_add_string_value (builder, message);
  json_builder_end_object (builder);

  send_json (connection, builder);
}

/* --- pairing --------------------------------------------------------------- */

char *
hy_remote_server_arm_pairing (HyRemoteServer *self,
                              guint           seconds)
{
  static const char alphabet[] = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  GString *code = g_string_new (NULL);

  g_return_val_if_fail (HY_IS_REMOTE_SERVER (self), NULL);

  for (int i = 0; i < 8; i++)
    {
      if (i == 4)
        g_string_append_c (code, '-');
      g_string_append_c (code,
                         alphabet[g_random_int_range (0, sizeof alphabet - 1)]);
    }

  g_free (self->pairing_code);
  self->pairing_code = g_strdup (code->str);
  self->pairing_expires = g_get_monotonic_time () +
                          (gint64) seconds * G_USEC_PER_SEC;

  return g_string_free (code, FALSE);
}

static char *
token_hash (const char *token)
{
  return g_compute_checksum_for_string (G_CHECKSUM_SHA256, token, -1);
}

static void
handle_pair (Connection *connection,
             JsonObject *request)
{
  HyRemoteServer *self = connection->server;
  const char *code = member_string (request, "code");
  const char *name = member_string (request, "name");
  g_autofree char *token = NULL;
  g_autofree char *hash = NULL;
  g_autoptr (GError) error = NULL;
  guint8 raw[32];

  if (self->pairing_code == NULL ||
      g_get_monotonic_time () > self->pairing_expires ||
      g_strcmp0 (code, self->pairing_code) != 0)
    {
      send_error (connection, "No such pairing code. Run the server with --pair.");
      return;
    }

  /* One use: right or wrong, the code is spent. */
  g_clear_pointer (&self->pairing_code, g_free);

  for (gsize i = 0; i < sizeof raw; i++)
    raw[i] = (guint8) g_random_int_range (0, 256);
  token = g_base64_encode (raw, sizeof raw);
  hash = token_hash (token);

  if (!hy_storage_add_device (self->storage, hash, name, &error))
    {
      send_error (connection, error->message);
      return;
    }

  connection->authed = TRUE;

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "ok");
    json_builder_add_boolean_value (builder, TRUE);
    json_builder_set_member_name (builder, "token");
    json_builder_add_string_value (builder, token);
    json_builder_end_object (builder);

    send_json (connection, builder);
  }
}

static void
handle_hello (Connection *connection,
              JsonObject *request)
{
  const char *token = member_string (request, "token");
  g_autofree char *hash = NULL;
  g_autofree char *name = NULL;

  if (token == NULL)
    {
      send_error (connection, "hello needs a token");
      return;
    }

  hash = token_hash (token);
  name = hy_storage_device_name (connection->server->storage, hash);
  if (name == NULL)
    {
      send_error (connection, "Unknown device. Pair first.");
      return;
    }

  connection->authed = TRUE;

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "ok");
    json_builder_add_boolean_value (builder, TRUE);
    json_builder_set_member_name (builder, "device");
    json_builder_add_string_value (builder, name);
    json_builder_set_member_name (builder, "version");
    json_builder_add_int_value (builder, 1);
    json_builder_end_object (builder);

    send_json (connection, builder);
  }
}

/* --- the tree -------------------------------------------------------------- */

/* Reads a folder's id out of its dotfile, as the fs tree does. */
static char *
folder_id_for (const char *path)
{
  g_autofree char *dotfile = g_build_filename (path, ".hy.json", NULL);
  g_autoptr (JsonParser) parser = json_parser_new ();
  JsonObject *root;

  if (!json_parser_load_from_file (parser, dotfile, NULL))
    return NULL;

  root = json_node_get_object (json_parser_get_root (parser));

  return g_strdup (member_string (root, "id"));
}

static void
add_folder (HyRemoteServer *self,
            JsonBuilder    *folders,
            JsonBuilder    *chats,
            const char     *path,
            const char     *parent_id)
{
  g_autofree char *id = folder_id_for (path);
  g_autofree char *name = g_path_get_basename (path);
  g_autoptr (GDir) dir = NULL;
  const char *entry;

  if (id == NULL)
    return;

  json_builder_begin_object (folders);
  json_builder_set_member_name (folders, "id");
  json_builder_add_string_value (folders, id);
  json_builder_set_member_name (folders, "name");
  json_builder_add_string_value (folders, name);
  if (parent_id != NULL)
    {
      json_builder_set_member_name (folders, "parent");
      json_builder_add_string_value (folders, parent_id);
    }
  json_builder_end_object (folders);

  {
    g_autoptr (GPtrArray) rows =
      hy_storage_list_chats (self->storage, id, NULL);

    for (guint i = 0; rows != NULL && i < rows->len; i++)
      {
        const HyChat *chat = g_ptr_array_index (rows, i);

        json_builder_begin_object (chats);
        json_builder_set_member_name (chats, "id");
        json_builder_add_string_value (chats, chat->id);
        json_builder_set_member_name (chats, "folder");
        json_builder_add_string_value (chats, id);
        json_builder_set_member_name (chats, "title");
        json_builder_add_string_value (chats, chat->title);
        json_builder_set_member_name (chats, "backend");
        json_builder_add_string_value (chats, chat->backend);
        json_builder_end_object (chats);
      }
  }

  dir = g_dir_open (path, 0, NULL);
  while (dir != NULL && (entry = g_dir_read_name (dir)) != NULL)
    {
      g_autofree char *child = g_build_filename (path, entry, NULL);

      if (entry[0] != '.' && g_file_test (child, G_FILE_TEST_IS_DIR))
        add_folder (self, folders, chats, child, id);
    }
}

static void
handle_tree (Connection *connection)
{
  HyRemoteServer *self = connection->server;
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonBuilder) folders = json_builder_new ();
  g_autoptr (JsonBuilder) chats = json_builder_new ();
  g_autoptr (GDir) dir = NULL;
  const char *entry;

  json_builder_begin_array (folders);
  json_builder_begin_array (chats);

  dir = g_dir_open (self->root_path, 0, NULL);
  while (dir != NULL && (entry = g_dir_read_name (dir)) != NULL)
    {
      g_autofree char *child = g_build_filename (self->root_path, entry, NULL);

      if (entry[0] != '.' && g_file_test (child, G_FILE_TEST_IS_DIR))
        add_folder (self, folders, chats, child, NULL);
    }

  json_builder_end_array (folders);
  json_builder_end_array (chats);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);
  json_builder_set_member_name (builder, "folders");
  json_builder_add_value (builder, json_builder_get_root (folders));
  json_builder_set_member_name (builder, "chats");
  json_builder_add_value (builder, json_builder_get_root (chats));
  json_builder_end_object (builder);

  send_json (connection, builder);
}

static void
handle_messages (Connection *connection,
                 JsonObject *request)
{
  const char *chat_id = member_string (request, "chat");
  g_autoptr (GPtrArray) rows = NULL;
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  if (chat_id == NULL)
    {
      send_error (connection, "messages needs a chat id");
      return;
    }

  rows = hy_storage_list_messages (connection->server->storage, chat_id, NULL);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);
  json_builder_set_member_name (builder, "messages");
  json_builder_begin_array (builder);

  for (guint i = 0; rows != NULL && i < rows->len; i++)
    {
      const HyMessage *message = g_ptr_array_index (rows, i);

      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "role");
      json_builder_add_string_value (builder, message->role);
      json_builder_set_member_name (builder, "content");
      json_builder_add_string_value (builder, message->content);
      json_builder_set_member_name (builder, "at");
      json_builder_add_int_value (builder, message->created_at);
      if (message->label != NULL)
        {
          json_builder_set_member_name (builder, "label");
          json_builder_add_string_value (builder, message->label);
        }
      json_builder_end_object (builder);
    }

  json_builder_end_array (builder);
  json_builder_end_object (builder);

  send_json (connection, builder);
}

/* --- the conversation ------------------------------------------------------ */

static void
connection_free (Connection *connection)
{
  g_clear_object (&connection->in);
  g_clear_object (&connection->stream);
  g_free (connection);
}

static void
dispatch (Connection *connection,
          const char *line)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  JsonObject *request;
  const char *op;

  if (!json_parser_load_from_data (parser, line, -1, NULL) ||
      !JSON_NODE_HOLDS_OBJECT (json_parser_get_root (parser)))
    {
      send_error (connection, "Not a JSON object");
      return;
    }

  request = json_node_get_object (json_parser_get_root (parser));
  op = member_string (request, "op");

  if (g_strcmp0 (op, "pair") == 0)
    {
      handle_pair (connection, request);
      return;
    }

  if (g_strcmp0 (op, "hello") == 0)
    {
      handle_hello (connection, request);
      return;
    }

  /* Everything below is for devices that have proven a token. */
  if (!connection->authed)
    {
      send_error (connection, "Not authenticated. Say hello first.");
      return;
    }

  if (g_strcmp0 (op, "tree") == 0)
    handle_tree (connection);
  else if (g_strcmp0 (op, "messages") == 0)
    handle_messages (connection, request);
  else if (g_strcmp0 (op, "ping") == 0)
    {
      g_autoptr (JsonBuilder) builder = json_builder_new ();

      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "ok");
      json_builder_add_boolean_value (builder, TRUE);
      json_builder_end_object (builder);
      send_json (connection, builder);
    }
  else
    send_error (connection, "Unknown op");
}

static void
on_line_read (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  Connection *connection = user_data;
  g_autofree char *line = NULL;

  line = g_data_input_stream_read_line_finish_utf8 (G_DATA_INPUT_STREAM (source),
                                                    result, NULL, NULL);
  if (line == NULL)
    {
      connection_free (connection);
      return;
    }

  if (*line != '\0')
    dispatch (connection, line);

  read_next_request (connection);
}

static void
read_next_request (Connection *connection)
{
  g_data_input_stream_read_line_async (connection->in, G_PRIORITY_DEFAULT,
                                       NULL, on_line_read, connection);
}

static void
on_handshake (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  Connection *connection = user_data;

  if (!g_tls_connection_handshake_finish (G_TLS_CONNECTION (source), result, NULL))
    {
      connection_free (connection);
      return;
    }

  read_next_request (connection);
}

static gboolean
on_incoming (GSocketService    *service,
             GSocketConnection *socket_connection,
             GObject           *source_object,
             gpointer           user_data)
{
  HyRemoteServer *self = user_data;
  Connection *connection;
  g_autoptr (GIOStream) tls = NULL;

  tls = g_tls_server_connection_new (G_IO_STREAM (socket_connection),
                                     self->certificate, NULL);
  if (tls == NULL)
    return TRUE;

  connection = g_new0 (Connection, 1);
  connection->server = self;
  connection->stream = g_object_ref (tls);
  connection->in = g_data_input_stream_new (g_io_stream_get_input_stream (tls));
  connection->out = g_io_stream_get_output_stream (tls);

  g_tls_connection_handshake_async (G_TLS_CONNECTION (tls), G_PRIORITY_DEFAULT,
                                    NULL, on_handshake, connection);

  return TRUE;
}

/* --- lifecycle ------------------------------------------------------------- */

HyRemoteServer *
hy_remote_server_new (HyStorage        *storage,
                      const char       *root_path,
                      guint16           port,
                      GTlsCertificate  *certificate,
                      GError          **error)
{
  g_autoptr (HyRemoteServer) self = NULL;

  g_return_val_if_fail (HY_IS_STORAGE (storage), NULL);
  g_return_val_if_fail (root_path != NULL, NULL);
  g_return_val_if_fail (G_IS_TLS_CERTIFICATE (certificate), NULL);

  self = g_object_new (HY_TYPE_REMOTE_SERVER, NULL);
  self->storage = g_object_ref (storage);
  self->root_path = g_strdup (root_path);
  self->certificate = g_object_ref (certificate);
  self->service = g_socket_service_new ();

  if (port != 0)
    {
      if (!g_socket_listener_add_inet_port (G_SOCKET_LISTENER (self->service),
                                            port, NULL, error))
        return NULL;
      self->port = port;
    }
  else
    {
      self->port = g_socket_listener_add_any_inet_port (
        G_SOCKET_LISTENER (self->service), NULL, error);
      if (self->port == 0)
        return NULL;
    }

  g_signal_connect (self->service, "incoming", G_CALLBACK (on_incoming), self);
  g_socket_service_start (self->service);

  return g_steal_pointer (&self);
}

guint16
hy_remote_server_get_port (HyRemoteServer *self)
{
  g_return_val_if_fail (HY_IS_REMOTE_SERVER (self), 0);

  return self->port;
}

static void
hy_remote_server_dispose (GObject *object)
{
  HyRemoteServer *self = HY_REMOTE_SERVER (object);

  if (self->service != NULL)
    g_socket_service_stop (self->service);
  g_clear_object (&self->service);
  g_clear_object (&self->certificate);
  g_clear_object (&self->storage);
  g_clear_pointer (&self->root_path, g_free);
  g_clear_pointer (&self->pairing_code, g_free);

  G_OBJECT_CLASS (hy_remote_server_parent_class)->dispose (object);
}

static void
hy_remote_server_class_init (HyRemoteServerClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = hy_remote_server_dispose;
}

static void
hy_remote_server_init (HyRemoteServer *self)
{
}
