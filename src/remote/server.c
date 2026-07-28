#include "server.h"

#include "backend/backend.h"
#include "chat/chat-title.h"
#include "remote/terminal.h"
#include "remote/turn.h"
#include "remote/protocol.h"
#include "settings/agent-secrets.h"
#include "settings/folder-settings.h"
#include "util/git-info.h"
#include "util/worktree.h"

#include <gdk-pixbuf/gdk-pixbuf.h>
#include <json-glib/json-glib.h>
#include <string.h>
#include <glib/gstdio.h>
#include <errno.h>

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
  XdRemoteServer *server;      /* unowned; the server outlives connections */
  GDataInputStream *in;
  GOutputStream *out;
  GIOStream *stream;
  GQueue *outgoing;            /* complete JSON lines, oldest first */
  gsize outgoing_bytes;

  /*
   * What stops an in-flight read or write before the stream under it closes.
   *
   * Without this the fd went away while GIO still had a source polling it.
   * Linux reports that as POLLNVAL on the one entry and carries on, so it was
   * invisible there; BSD fails the whole poll with EBADF, which GLib treats as
   * fatal -- so the daemon only died of it on macOS.
   */
  GCancellable *cancellable;
  guint refs;
  gboolean authed;
  gboolean writing;
  gboolean closed;
} Connection;

struct _XdRemoteServer
{
  GObject parent_instance;

  XdStorage *storage;
  char *root_path;
  GSocketService *service;
  GTlsCertificate *certificate;
  guint16 port;

  /* Every connection currently open, so a turn can be shown to all of them.
   * Unowned: a connection takes itself out when its socket goes. */
  GPtrArray *connections;

  /* Turns in flight, by chat. One per chat is the rule, and this is what
   * enforces it. chat id -> XdDaemonTurn*. */
  GHashTable *turns;
  gboolean quiescing;
  GTask *quiesce_task;

  /* Installed slash commands learned from backend init events. backend id ->
   * GStrv. Kept after a turn ends so newly connected devices get suggestions
   * without starting another turn first. */
  GHashTable *command_sets;

  /* Shells live here, not on whichever device opened them. terminal id ->
   * XdRemoteTerminal*. */
  GHashTable *terminals;

  /* Changes made by anything else on this machine -- the window open here,
   * most of the time -- and the delay that coalesces them. */
  GFileMonitor *tree_watch;
  guint local_change_id;

  char *pairing_code;
  gint64 pairing_expires;      /* monotonic microseconds */
};

/* SQLite writes several times per statement, and a checkout touches everything
 * at once; one event out the far side is enough. */
#define LOCAL_CHANGE_DEBOUNCE_MS 400
#define MAX_QUEUED_OUTPUT (32u * 1024u * 1024u)

G_DEFINE_FINAL_TYPE (XdRemoteServer, xd_remote_server, G_TYPE_OBJECT)

static void read_next_request (Connection *connection);
static void connection_close (Connection *connection);

/* --- small json helpers ---------------------------------------------------- */

static const char *
member_string (JsonObject *object,
               const char *name)
{
  if (object == NULL || !json_object_has_member (object, name))
    return NULL;

  return json_object_get_string_member_with_default (object, name, NULL);
}

static Connection *
connection_ref (Connection *connection)
{
  connection->refs++;
  return connection;
}

static void
outgoing_free (GQueue *outgoing)
{
  g_queue_free_full (outgoing, g_free);
}

static void
connection_unref (Connection *connection)
{
  g_return_if_fail (connection->refs > 0);

  if (--connection->refs > 0)
    return;

  g_clear_pointer (&connection->outgoing, outgoing_free);
  g_clear_object (&connection->cancellable);
  g_clear_object (&connection->in);
  g_clear_object (&connection->stream);
  g_free (connection);
}

static void write_next_json (Connection *connection);

static void
on_json_written (GObject      *source,
                 GAsyncResult *result,
                 gpointer      user_data)
{
  Connection *connection = user_data;
  g_autoptr (GError) error = NULL;
  char *line = g_queue_pop_head (connection->outgoing);
  gsize length = line != NULL ? strlen (line) : 0;

  if (!g_output_stream_write_all_finish (G_OUTPUT_STREAM (source), result,
                                         NULL, &error) &&
      !connection->closed)
    connection_close (connection);

  connection->outgoing_bytes -= MIN (connection->outgoing_bytes, length);
  g_free (line);
  connection->writing = FALSE;
  write_next_json (connection);
  connection_unref (connection);
}

static void
write_next_json (Connection *connection)
{
  const char *line;

  if (connection->closed || connection->writing ||
      (line = g_queue_peek_head (connection->outgoing)) == NULL)
    return;

  connection->writing = TRUE;
  g_output_stream_write_all_async (
    connection->out, line, strlen (line), G_PRIORITY_DEFAULT,
    connection->cancellable, on_json_written, connection_ref (connection));
}

/*
 * Queue complete lines rather than blocking the daemon's main loop on a
 * device that is busy rendering. One slow client must not stall every remote
 * chat and make otherwise healthy connections appear dead.
 */
static void
send_line (Connection *connection,
           const char *text,
           gsize       length)
{
  char *line;

  if (connection->closed)
    return;

  if (length >= MAX_QUEUED_OUTPUT ||
      connection->outgoing_bytes > MAX_QUEUED_OUTPUT - length - 1)
    {
      connection_close (connection);
      return;
    }

  line = g_malloc (length + 2);
  memcpy (line, text, length);
  line[length] = '\n';
  line[length + 1] = '\0';
  g_queue_push_tail (connection->outgoing, line);
  connection->outgoing_bytes += length + 1;
  write_next_json (connection);
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

  send_line (connection, text, length);
}

/* --- telling every device ---------------------------------------------------
 *
 * A turn belongs to the chat, not to whoever started it: a reply is shown on
 * every device watching, in the order it arrives, because that is what makes
 * two machines the same machine. Connections that have not proven a token hear
 * nothing.
 */

static void
broadcast (XdRemoteServer *self,
           JsonBuilder    *builder)
{
  g_autoptr (JsonGenerator) generator = json_generator_new ();
  g_autoptr (JsonNode) root = json_builder_get_root (builder);
  g_autofree char *text = NULL;
  gsize length;

  json_generator_set_root (generator, root);
  text = json_generator_to_data (generator, &length);

  for (guint i = self->connections->len; i > 0; i--)
    {
      Connection *connection = g_ptr_array_index (self->connections, i - 1);

      if (!connection->authed)
        continue;

      send_line (connection, text, length);
    }
}

/* An event about one chat, carrying at most one string of its own. */
static void
broadcast_event (XdRemoteServer *self,
                 const char     *event,
                 const char     *chat_id,
                 const char     *name,
                 const char     *value)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "event");
  json_builder_add_string_value (builder, event);

  if (chat_id != NULL)
    {
      json_builder_set_member_name (builder, "chat");
      json_builder_add_string_value (builder, chat_id);
    }

  if (name != NULL)
    {
      json_builder_set_member_name (builder, name);
      json_builder_add_string_value (builder, value);
    }

  json_builder_end_object (builder);

  broadcast (self, builder);
}

/*
 * The tree has changed shape.
 *
 * Sent after anything that adds, removes, renames or moves: the device that
 * asked hears it along with everyone else, so there is one path to being up to
 * date rather than one for the client that acted and another for the rest.
 */
static void
broadcast_tree (XdRemoteServer *self)
{
  broadcast_event (self, "tree", NULL, NULL, NULL);
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

/* What a request that changed something answers with: that it worked, and the
 * id of whatever was made, for a client that wants to open it. */
static void
send_done (Connection *connection,
           const char *id)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);
  if (id != NULL)
    {
      json_builder_set_member_name (builder, "id");
      json_builder_add_string_value (builder, id);
    }
  json_builder_end_object (builder);

  send_json (connection, builder);
}

/* --- pairing --------------------------------------------------------------- */

char *
xd_remote_server_arm_pairing (XdRemoteServer *self,
                              guint           seconds)
{
  static const char alphabet[] = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  GString *code = g_string_new (NULL);

  g_return_val_if_fail (XD_IS_REMOTE_SERVER (self), NULL);

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
  XdRemoteServer *self = connection->server;
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

  if (!xd_storage_add_device (self->storage, hash, name, &error))
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
  name = xd_storage_device_name (connection->server->storage, hash);
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
  g_autofree char *dotfile = g_build_filename (path, ".xd.json", NULL);
  g_autofree char *legacy = g_build_filename (path, ".hy.json", NULL);
  g_autoptr (JsonParser) parser = json_parser_new ();
  JsonObject *root;

  if (!json_parser_load_from_file (parser, dotfile, NULL) &&
      !json_parser_load_from_file (parser, legacy, NULL))
    return NULL;

  root = json_node_get_object (json_parser_get_root (parser));

  return g_strdup (member_string (root, "id"));
}

static void
add_folder (XdRemoteServer *self,
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
      xd_storage_list_chats (self->storage, id, NULL);

    for (guint i = 0; rows != NULL && i < rows->len; i++)
      {
        const XdChat *chat = g_ptr_array_index (rows, i);

        json_builder_begin_object (chats);
        json_builder_set_member_name (chats, "id");
        json_builder_add_string_value (chats, chat->id);
        json_builder_set_member_name (chats, "folder");
        json_builder_add_string_value (chats, id);
        json_builder_set_member_name (chats, "title");
        json_builder_add_string_value (chats, chat->title);
        json_builder_set_member_name (chats, "backend");
        json_builder_add_string_value (chats, chat->backend);
        json_builder_set_member_name (chats, "working");
        json_builder_add_boolean_value (
          chats, g_hash_table_contains (self->turns, chat->id));
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
  XdRemoteServer *self = connection->server;
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

/* --- changing the tree ----------------------------------------------------- */

/*
 * The directory a folder id names, searched from the workspace root.
 *
 * A folder's id lives in a dotfile inside it, which is what lets it be renamed
 * and moved without the chats written against it noticing -- and is why an id
 * has to be looked for rather than computed.
 */
static char *
find_folder (const char *path,
             const char *id)
{
  g_autofree char *here = folder_id_for (path);
  g_autoptr (GDir) dir = NULL;
  const char *entry;

  if (here != NULL && g_strcmp0 (here, id) == 0)
    return g_strdup (path);

  dir = g_dir_open (path, 0, NULL);
  while (dir != NULL && (entry = g_dir_read_name (dir)) != NULL)
    {
      g_autofree char *child = g_build_filename (path, entry, NULL);
      char *found;

      if (entry[0] == '.' || !g_file_test (child, G_FILE_TEST_IS_DIR))
        continue;

      found = find_folder (child, id);
      if (found != NULL)
        return found;
    }

  return NULL;
}

/*
 * Resolves a folder argument to a place on disk.
 *
 * An absent id means the workspace root, which is where a new workspace goes.
 * Anything else that does not resolve is answered rather than ignored: the
 * client is looking at a tree that has moved on.
 */
static char *
folder_argument (Connection *connection,
                 JsonObject *request,
                 const char *name,
                 gboolean    root_allowed)
{
  XdRemoteServer *self = connection->server;
  const char *id = member_string (request, name);
  char *path;

  if (id == NULL)
    {
      if (root_allowed)
        return g_strdup (self->root_path);

      send_error (connection, "That request needs a folder.");
      return NULL;
    }

  path = find_folder (self->root_path, id);
  if (path == NULL)
    send_error (connection, "No such folder on the daemon.");

  return path;
}

/*
 * A name that can be a directory in the workspace tree.
 *
 * Hidden names are refused along with separators: the tree skips anything
 * beginning with a dot, so a folder called ".x" would be made and then never
 * appear -- and "." and ".." would be something else entirely.
 */
static gboolean
valid_folder_name (Connection *connection,
                   const char *name)
{
  if (name == NULL || *name == '\0' || *name == '.' ||
      strchr (name, G_DIR_SEPARATOR) != NULL)
    {
      send_error (connection, "A folder name cannot be empty or hidden, or "
                              "contain a path separator.");
      return FALSE;
    }

  return TRUE;
}

static void
handle_new_folder (Connection *connection,
                   JsonObject *request)
{
  const char *name = member_string (request, "name");
  g_autofree char *parent = NULL;
  g_autofree char *path = NULL;
  g_autoptr (XdFolderSettings) settings = NULL;
  g_autoptr (GFile) file = NULL;
  g_autoptr (GError) error = NULL;

  if (!valid_folder_name (connection, name))
    return;

  parent = folder_argument (connection, request, "parent", TRUE);
  if (parent == NULL)
    return;

  path = g_build_filename (parent, name, NULL);
  file = g_file_new_for_path (path);

  if (!g_file_make_directory (file, NULL, &error))
    {
      send_error (connection, error->message);
      return;
    }

  /* Minted here rather than left for the next scan, so the answer can name
   * the folder that was just made. */
  settings = xd_folder_settings_ensure (path, &error);
  if (settings == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, settings->id);
  broadcast_tree (connection->server);
}

static void
handle_rename_folder (Connection *connection,
                      JsonObject *request)
{
  const char *name = member_string (request, "name");
  g_autofree char *path = NULL;
  g_autoptr (GFile) file = NULL;
  g_autoptr (GFile) renamed = NULL;
  g_autoptr (GError) error = NULL;

  if (!valid_folder_name (connection, name))
    return;

  path = folder_argument (connection, request, "folder", FALSE);
  if (path == NULL)
    return;

  file = g_file_new_for_path (path);
  renamed = g_file_set_display_name (file, name, NULL, &error);
  if (renamed == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);
  broadcast_tree (connection->server);
}

static void
handle_move_folder (Connection *connection,
                    JsonObject *request)
{
  g_autofree char *path = NULL;
  g_autofree char *parent = NULL;
  g_autofree char *name = NULL;
  g_autofree char *destination_path = NULL;
  g_autofree char *inside = NULL;
  g_autoptr (GFile) source = NULL;
  g_autoptr (GFile) destination = NULL;
  g_autoptr (GError) error = NULL;

  path = folder_argument (connection, request, "folder", FALSE);
  if (path == NULL)
    return;

  parent = folder_argument (connection, request, "parent", TRUE);
  if (parent == NULL)
    return;

  /* A folder cannot hold itself, and moving one into its own subtree would
   * take the destination along with it. */
  inside = g_strconcat (path, G_DIR_SEPARATOR_S, NULL);
  if (g_strcmp0 (path, parent) == 0 || g_str_has_prefix (parent, inside))
    {
      send_error (connection, "A folder cannot be moved inside itself.");
      return;
    }

  name = g_path_get_basename (path);
  destination_path = g_build_filename (parent, name, NULL);

  source = g_file_new_for_path (path);
  destination = g_file_new_for_path (destination_path);

  if (g_file_query_exists (destination, NULL))
    {
      send_error (connection, "There is already a folder of that name there.");
      return;
    }

  /* A plain rename, since everything is under one root. Across filesystems
   * this fails rather than copying, which is the honest outcome. */
  if (!g_file_move (source, destination, G_FILE_COPY_NONE, NULL, NULL, NULL, &error))
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);
  broadcast_tree (connection->server);
}

static void
handle_trash_folder (Connection *connection,
                     JsonObject *request)
{
  g_autofree char *path = NULL;
  g_autoptr (GFile) file = NULL;
  g_autoptr (GError) error = NULL;

  path = folder_argument (connection, request, "folder", FALSE);
  if (path == NULL)
    return;

  file = g_file_new_for_path (path);

  /* The trash rather than a delete, as the local tree does: the daemon runs
   * unattended, and a mistaken click from another machine should be something
   * the person at this one can undo. */
  if (!g_file_trash (file, NULL, &error))
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);
  broadcast_tree (connection->server);
}

/*
 * The context belongs to the folder on the daemon, not to the device editing
 * it. Only the folder's own text crosses the wire; inherited context remains
 * the resolver's job when a turn starts.
 */
static void
handle_folder_context (Connection *connection,
                       JsonObject *request)
{
  g_autofree char *path = NULL;
  g_autoptr (XdFolderSettings) settings = NULL;
  g_autoptr (GError) error = NULL;
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  path = folder_argument (connection, request, "folder", FALSE);
  if (path == NULL)
    return;

  settings = xd_folder_settings_ensure (path, &error);
  if (settings == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);
  json_builder_set_member_name (builder, "context");
  if (settings->instructions != NULL)
    json_builder_add_string_value (builder, settings->instructions);
  else
    json_builder_add_null_value (builder);
  json_builder_end_object (builder);

  send_json (connection, builder);
}

static void
handle_set_folder_context (Connection *connection,
                           JsonObject *request)
{
  g_autofree char *path = NULL;
  g_autofree char *context = NULL;
  g_autoptr (XdFolderSettings) settings = NULL;
  g_autoptr (GError) error = NULL;
  JsonNode *node;

  path = folder_argument (connection, request, "folder", FALSE);
  if (path == NULL)
    return;

  if (!json_object_has_member (request, "context"))
    {
      send_error (connection, "set-folder-context needs context.");
      return;
    }

  node = json_object_get_member (request, "context");
  if (!JSON_NODE_HOLDS_NULL (node))
    {
      if (!JSON_NODE_HOLDS_VALUE (node) ||
          json_node_get_value_type (node) != G_TYPE_STRING)
        {
          send_error (connection, "Folder context must be text or null.");
          return;
        }

      context = json_node_dup_string (node);
      g_strstrip (context);
      if (*context == '\0')
        g_clear_pointer (&context, g_free);
    }

  settings = xd_folder_settings_ensure (path, &error);
  if (settings == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  g_free (settings->instructions);
  settings->instructions = g_steal_pointer (&context);

  if (!xd_folder_settings_save (settings, path, &error))
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);
}

/*
 * What a new chat in @path answers with.
 *
 * Resolved here rather than sent by the client, because the folder chain that
 * decides it lives on this machine: a client would be guessing at settings it
 * cannot read.
 */
static void
resolve_backend (XdRemoteServer  *self,
                 const char      *path,
                 char           **backend,
                 char           **model)
{
  g_autofree char *at = g_strdup (path);

  while (at != NULL)
    {
      g_autoptr (XdFolderSettings) settings = xd_folder_settings_load (at, NULL);
      char *parent;

      if (settings != NULL)
        {
          if (*backend == NULL && settings->backend != NULL)
            *backend = g_strdup (settings->backend);
          if (*model == NULL && settings->model != NULL)
            *model = g_strdup (settings->model);
        }

      if (g_strcmp0 (at, self->root_path) == 0)
        break;

      parent = g_path_get_dirname (at);

      /* The filesystem root is its own parent, and a workspace root written
       * with a trailing slash never matches the check above -- so the walk
       * stops when it stops moving rather than only where it is meant to. */
      if (g_strcmp0 (parent, at) == 0)
        {
          g_free (parent);
          break;
        }

      g_free (at);
      at = parent;
    }

  if (*backend == NULL)
    *backend = g_strdup ("claude");

  if (*model == NULL)
    {
      const AiBackend *definition = ai_backend_lookup (*backend);

      if (definition != NULL)
        *model = g_strdup (definition->default_model);
    }
}

static void
handle_new_chat (Connection *connection,
                 JsonObject *request)
{
  XdRemoteServer *self = connection->server;
  const char *folder_id = member_string (request, "folder");
  const char *title = member_string (request, "title");
  const char *workdir = member_string (request, "workdir");
  g_autofree char *path = NULL;
  g_autofree char *backend = NULL;
  g_autofree char *model = NULL;
  g_autofree char *chat_id = NULL;
  const char *effort = NULL;
  g_autoptr (GError) error = NULL;

  path = folder_argument (connection, request, "folder", FALSE);
  if (path == NULL)
    return;

  resolve_backend (self, path, &backend, &model);

  {
    const AiBackend *definition = ai_backend_lookup (backend);

    /* What the CLI on this machine would do if nothing said otherwise. */
    if (definition != NULL)
      effort = ai_effort_to_string (ai_backend_default_effort (definition));
  }

  /* Without one it inherits the folder's, which is resolved on this side
   * too -- and the one it is given was picked from this machine's own
   * directories, so it means something here. */
  chat_id = xd_storage_create_chat (self->storage, folder_id,
                                    title != NULL ? title : XD_CHAT_UNTITLED,
                                    backend, model, effort, workdir, &error);
  if (chat_id == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, chat_id);
}

static void
handle_rename_chat (Connection *connection,
                    JsonObject *request)
{
  const char *chat_id = member_string (request, "chat");
  const char *title = member_string (request, "title");
  g_autoptr (GError) error = NULL;

  if (chat_id == NULL || title == NULL || *title == '\0')
    {
      send_error (connection, "A chat needs an id and a title.");
      return;
    }

  if (!xd_storage_set_chat_title (connection->server->storage, chat_id, title,
                                  &error))
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);
  broadcast_tree (connection->server);
}

static void
handle_delete_chat (Connection *connection,
                    JsonObject *request)
{
  const char *chat_id = member_string (request, "chat");
  g_autoptr (GError) error = NULL;

  if (chat_id == NULL)
    {
      send_error (connection, "delete-chat needs a chat id");
      return;
    }

  if (!xd_storage_delete_chat (connection->server->storage, chat_id, &error))
    {
      send_error (connection, error->message);
      return;
    }

  {
    GHashTableIter iter;
    gpointer value;

    g_hash_table_iter_init (&iter, connection->server->terminals);
    while (g_hash_table_iter_next (&iter, NULL, &value))
      {
        XdRemoteTerminal *terminal = value;

        if (g_strcmp0 (xd_remote_terminal_get_chat_id (terminal), chat_id) == 0)
          xd_remote_terminal_close (terminal);
      }
  }

  send_done (connection, NULL);
  broadcast_tree (connection->server);
}

/*
 * The directories under a path on this machine.
 *
 * So a client can say where a chat runs: the directory it picks has to exist
 * on the side that will run the agent, and only that side can list it. Names
 * only, directories only, hidden ones left out -- it is a place-picker, not a
 * file browser, and a device that can start an agent here can already read far
 * more than this.
 *
 * An absent path means the daemon's home, which is where browsing starts.
 */
static void
handle_list_dir (Connection *connection,
                 JsonObject *request)
{
  const char *path = member_string (request, "path");
  g_autoptr (GPtrArray) names = g_ptr_array_new_with_free_func (g_free);
  g_autoptr (GDir) dir = NULL;
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (GError) error = NULL;
  const char *entry;

  if (path == NULL || *path == '\0')
    path = g_get_home_dir ();

  dir = g_dir_open (path, 0, &error);
  if (dir == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  while ((entry = g_dir_read_name (dir)) != NULL)
    {
      g_autofree char *child = g_build_filename (path, entry, NULL);

      if (entry[0] == '.' || !g_file_test (child, G_FILE_TEST_IS_DIR))
        continue;

      g_ptr_array_add (names, g_strdup (entry));
    }

  g_ptr_array_sort_values (names, (GCompareFunc) g_strcmp0);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);
  json_builder_set_member_name (builder, "path");
  json_builder_add_string_value (builder, path);

  json_builder_set_member_name (builder, "entries");
  json_builder_begin_array (builder);
  for (guint i = 0; i < names->len; i++)
    json_builder_add_string_value (builder, g_ptr_array_index (names, i));
  json_builder_end_array (builder);

  json_builder_end_object (builder);

  send_json (connection, builder);
}

/* --- files in a chat's working directory --------------------------------- */

#define FILE_PREVIEW_LIMIT (1024 * 1024)

typedef struct
{
  char *name;
  gboolean directory;
} BrowseEntry;

static void
browse_entry_free (BrowseEntry *entry)
{
  g_free (entry->name);
  g_free (entry);
}

static gint
compare_browse_entries (gconstpointer a,
                        gconstpointer b)
{
  const BrowseEntry *left = a;
  const BrowseEntry *right = b;

  if (left->directory != right->directory)
    return left->directory ? -1 : 1;

  return g_utf8_collate (left->name, right->name);
}

/*
 * A file request is relative to the chat's working directory.
 *
 * The device is authenticated and can already open a terminal there, but
 * keeping this operation inside the checkout prevents a stale or malformed
 * pane request from reading some unrelated absolute path.
 */
static char *
browse_path (const char  *workdir,
             const char  *relative,
             GError     **error)
{
  g_autofree char *root = NULL;
  g_autofree char *joined = NULL;
  g_autofree char *prefix = NULL;
  char *path;

  if (relative == NULL)
    relative = "";

  if (g_path_is_absolute (relative))
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                           "File paths must be relative to the working directory.");
      return NULL;
    }

  root = g_canonicalize_filename (workdir, NULL);
  joined = g_build_filename (root, relative, NULL);
  path = g_canonicalize_filename (joined, NULL);
  prefix = g_strconcat (root, G_DIR_SEPARATOR_S, NULL);

  if (g_strcmp0 (path, root) != 0 && !g_str_has_prefix (path, prefix))
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED,
                           "That file is outside the working directory.");
      g_free (path);
      return NULL;
    }

  return path;
}

static void
handle_file_browse (Connection *connection,
                    JsonObject *request)
{
  XdRemoteServer *self = connection->server;
  const char *chat_id = member_string (request, "chat");
  const char *action = member_string (request, "action");
  const char *relative = member_string (request, "path");
  g_autoptr (XdChat) chat = NULL;
  g_autoptr (XdDaemonTurn) resolver = NULL;
  g_autofree char *workdir = NULL;
  g_autofree char *path = NULL;
  g_autoptr (GError) error = NULL;

  if (chat_id == NULL || action == NULL)
    {
      send_error (connection, "file-browse needs a chat and action.");
      return;
    }

  chat = xd_storage_get_chat (self->storage, chat_id, &error);
  if (chat == NULL)
    {
      send_error (connection, error != NULL ? error->message : "No such chat.");
      return;
    }

  resolver = xd_daemon_turn_new (self->storage, self->root_path);
  workdir = xd_daemon_turn_resolve_workdir (resolver, chat);
  if (workdir == NULL)
    {
      send_error (connection, "This chat has no working directory.");
      return;
    }

  path = browse_path (workdir, relative, &error);
  if (path == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  if (g_strcmp0 (action, "list") == 0)
    {
      g_autoptr (GDir) dir = g_dir_open (path, 0, &error);
      g_autoptr (GPtrArray) entries =
        g_ptr_array_new_with_free_func ((GDestroyNotify) browse_entry_free);
      g_autoptr (JsonBuilder) builder = json_builder_new ();
      const char *name;

      if (dir == NULL)
        {
          send_error (connection, error->message);
          return;
        }

      while ((name = g_dir_read_name (dir)) != NULL)
        {
          g_autofree char *child = NULL;
          BrowseEntry *entry;

          if (name[0] == '.')
            continue;

          child = g_build_filename (path, name, NULL);
          entry = g_new0 (BrowseEntry, 1);
          entry->name = g_strdup (name);
          entry->directory = g_file_test (child, G_FILE_TEST_IS_DIR);
          g_ptr_array_add (entries, entry);
        }

      g_ptr_array_sort_values (entries, compare_browse_entries);

      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "ok");
      json_builder_add_boolean_value (builder, TRUE);
      json_builder_set_member_name (builder, "entries");
      json_builder_begin_array (builder);
      for (guint i = 0; i < entries->len; i++)
        {
          BrowseEntry *entry = g_ptr_array_index (entries, i);

          json_builder_begin_object (builder);
          json_builder_set_member_name (builder, "name");
          json_builder_add_string_value (builder, entry->name);
          json_builder_set_member_name (builder, "directory");
          json_builder_add_boolean_value (builder, entry->directory);
          json_builder_end_object (builder);
        }
      json_builder_end_array (builder);
      json_builder_end_object (builder);
      send_json (connection, builder);
      return;
    }

  if (g_strcmp0 (action, "read") == 0)
    {
      g_autoptr (GFile) file = g_file_new_for_path (path);
      g_autoptr (GFileInfo) info = NULL;
      g_autofree char *content = NULL;
      gsize length = 0;
      g_autoptr (JsonBuilder) builder = json_builder_new ();

      info = g_file_query_info (
        file,
        G_FILE_ATTRIBUTE_STANDARD_TYPE ","
        G_FILE_ATTRIBUTE_STANDARD_SIZE,
        G_FILE_QUERY_INFO_NONE, NULL, &error);
      if (info == NULL)
        {
          send_error (connection, error->message);
          return;
        }
      if (g_file_info_get_file_type (info) != G_FILE_TYPE_REGULAR)
        {
          send_error (connection, "Only regular files can be previewed.");
          return;
        }
      if (g_file_info_get_size (info) > FILE_PREVIEW_LIMIT)
        {
          send_error (connection, "Files larger than 1 MB are not previewed.");
          return;
        }
      if (!g_file_get_contents (path, &content, &length, &error))
        {
          send_error (connection, error->message);
          return;
        }
      if (length > FILE_PREVIEW_LIMIT)
        {
          send_error (connection, "Files larger than 1 MB are not previewed.");
          return;
        }
      if (memchr (content, '\0', length) != NULL ||
          !g_utf8_validate (content, length, NULL))
        {
          send_error (connection, "Binary files cannot be previewed as text.");
          return;
        }

      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "ok");
      json_builder_add_boolean_value (builder, TRUE);
      json_builder_set_member_name (builder, "content");
      json_builder_add_string_value (builder, content);
      json_builder_end_object (builder);
      send_json (connection, builder);
      return;
    }

  if (g_strcmp0 (action, "write") == 0)
    {
      const char *content = member_string (request, "content");
      g_autoptr (GFile) file = g_file_new_for_path (path);
      g_autoptr (GFileInfo) info = NULL;
      g_autoptr (JsonBuilder) builder = json_builder_new ();
      gsize length;

      if (content == NULL)
        {
          send_error (connection, "file-browse write needs content.");
          return;
        }

      length = strlen (content);
      if (length > FILE_PREVIEW_LIMIT)
        {
          send_error (connection, "Files larger than 1 MB cannot be saved here.");
          return;
        }

      info = g_file_query_info (
        file, G_FILE_ATTRIBUTE_STANDARD_TYPE,
        G_FILE_QUERY_INFO_NONE, NULL, &error);
      if (info == NULL)
        {
          send_error (connection, error->message);
          return;
        }
      if (g_file_info_get_file_type (info) != G_FILE_TYPE_REGULAR)
        {
          send_error (connection, "Only regular files can be edited.");
          return;
        }
      if (!g_file_set_contents (path, content, (gssize) length, &error))
        {
          send_error (connection, error->message);
          return;
        }

      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "ok");
      json_builder_add_boolean_value (builder, TRUE);
      json_builder_end_object (builder);
      send_json (connection, builder);
      return;
    }

  send_error (connection, "No such file-browse action.");
}

/*
 * Global agent secrets live on the machine that executes the agent.
 *
 * A paired client may manage their names and replace values, but reading the
 * store never sends values over the wire. An omitted value means "keep the
 * existing value", allowing a masked editor to save without first retrieving
 * credentials.
 */
static void
handle_agent_secrets (Connection *connection)
{
  g_autoptr (XdAgentSecrets) secrets = NULL;
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (GError) error = NULL;
  g_auto (GStrv) names = NULL;

  secrets = xd_agent_secrets_load (NULL, &error);
  if (secrets == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  names = xd_agent_secrets_names (secrets);
  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);
  json_builder_set_member_name (builder, "names");
  json_builder_begin_array (builder);
  for (gsize i = 0; names[i] != NULL; i++)
    json_builder_add_string_value (builder, names[i]);
  json_builder_end_array (builder);
  json_builder_end_object (builder);
  send_json (connection, builder);
}

static void
handle_set_agent_secrets (Connection *connection,
                          JsonObject *request)
{
  g_autoptr (XdAgentSecrets) secrets = NULL;
  g_autoptr (GHashTable) desired = NULL;
  g_autoptr (GError) error = NULL;
  g_auto (GStrv) old_names = NULL;
  JsonNode *entries_node;
  JsonArray *entries;

  entries_node = json_object_get_member (request, "entries");
  if (entries_node == NULL || !JSON_NODE_HOLDS_ARRAY (entries_node))
    {
      send_error (connection, "set-agent-secrets needs an entries array.");
      return;
    }

  secrets = xd_agent_secrets_load (NULL, &error);
  if (secrets == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  entries = json_node_get_array (entries_node);
  desired = g_hash_table_new (g_str_hash, g_str_equal);

  /* Validate the complete request before changing the in-memory copy. */
  for (guint i = 0; i < json_array_get_length (entries); i++)
    {
      JsonNode *entry_node = json_array_get_element (entries, i);
      JsonObject *entry;
      JsonNode *value_node;
      const char *name;
      const char *value = NULL;

      if (!JSON_NODE_HOLDS_OBJECT (entry_node))
        {
          send_error (connection, "Every secret entry must be an object.");
          return;
        }

      entry = json_node_get_object (entry_node);
      name = member_string (entry, "name");
      if (!xd_agent_secret_name_is_valid (name))
        {
          send_error (connection, "A secret has an invalid environment name.");
          return;
        }

      if (g_hash_table_contains (desired, name))
        {
          send_error (connection, "Secret names must be unique.");
          return;
        }

      value_node = json_object_get_member (entry, "value");
      if (value_node != NULL)
        {
          if (!JSON_NODE_HOLDS_VALUE (value_node) ||
              json_node_get_value_type (value_node) != G_TYPE_STRING)
            {
              send_error (connection, "A secret value must be text.");
              return;
            }

          value = json_node_get_string (value_node);
          if (value == NULL || *value == '\0')
            {
              send_error (connection, "A replacement secret needs a value.");
              return;
            }
        }
      else if (!xd_agent_secrets_contains (secrets, name))
        {
          send_error (connection, "A new secret needs a value.");
          return;
        }

      g_hash_table_add (desired, (gpointer) name);
    }

  old_names = xd_agent_secrets_names (secrets);
  for (gsize i = 0; old_names[i] != NULL; i++)
    if (!g_hash_table_contains (desired, old_names[i]))
      xd_agent_secrets_remove (secrets, old_names[i]);

  for (guint i = 0; i < json_array_get_length (entries); i++)
    {
      JsonObject *entry = json_array_get_object_element (entries, i);
      JsonNode *value_node = json_object_get_member (entry, "value");

      if (value_node != NULL &&
          !xd_agent_secrets_set (
            secrets, member_string (entry, "name"),
            json_node_get_string (value_node), &error))
        {
          send_error (connection, error->message);
          return;
        }
    }

  if (!xd_agent_secrets_save (secrets, &error))
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);
}

static void
handle_messages (Connection *connection,
                 JsonObject *request)
{
  const char *chat_id = member_string (request, "chat");
  g_autoptr (GPtrArray) rows = NULL;
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  gint64 requested =
    json_object_get_int_member_with_default (request, "limit", 0);
  XdDaemonTurn *turn;
  gint64 through_id;
  guint total = 0;

  if (chat_id == NULL)
    {
      send_error (connection, "messages needs a chat id");
      return;
    }

  turn = g_hash_table_lookup (connection->server->turns, chat_id);
  if (turn != NULL && !xd_daemon_turn_is_running (turn))
    turn = NULL;
  through_id = turn != NULL
    ? xd_daemon_turn_get_transcript_id (turn) : G_MAXINT64;

  if (turn != NULL || requested > 0)
    rows = xd_storage_list_recent_messages_through (
      connection->server->storage, chat_id, through_id,
      requested > 0 ? (guint) MIN (requested, G_MAXUINT) : G_MAXUINT,
      &total, NULL);
  else
    {
      rows = xd_storage_list_messages (
        connection->server->storage, chat_id, NULL);
      total = rows != NULL ? rows->len : 0;
    }

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);
  json_builder_set_member_name (builder, "total_messages");
  json_builder_add_int_value (builder, total);
  json_builder_set_member_name (builder, "last_message_id");
  json_builder_add_int_value (builder, turn != NULL ? through_id :
    xd_storage_last_message_id (connection->server->storage, chat_id));
  json_builder_set_member_name (builder, "messages");
  json_builder_begin_array (builder);

  for (guint i = 0; rows != NULL && i < rows->len; i++)
    {
      const XdMessage *message = g_ptr_array_index (rows, i);

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

/*
 * A transcript stores the path the daemon-side agent received. That path is
 * meaningless on a paired device, so hover previews fetch the bytes lazily.
 *
 * Only files materialized by remote paste handling are exposed. Authentication
 * alone must not turn this small UI endpoint into an arbitrary file reader.
 */
static void
handle_image_read (Connection *connection,
                   JsonObject *request)
{
  const char *path = member_string (request, "path");
  gboolean preview =
    json_object_get_boolean_member_with_default (request, "preview", FALSE);
  g_autofree char *directory = NULL;
  g_autofree char *canonical_directory = NULL;
  g_autofree char *canonical_path = NULL;
  g_autofree char *parent = NULL;
  g_autofree char *contents = NULL;
  g_autofree char *encoded = NULL;
  g_autoptr (GError) error = NULL;
  g_autoptr (GdkPixbuf) thumbnail = NULL;
  gsize length = 0;

  directory = g_build_filename (g_get_user_cache_dir (), "xd",
                                "remote-pasted", NULL);
  canonical_directory = g_canonicalize_filename (directory, NULL);

  if (path == NULL || !g_path_is_absolute (path))
    {
      send_error (connection, "image-read needs an image path.");
      return;
    }

  canonical_path = g_canonicalize_filename (path, NULL);
  parent = g_path_get_dirname (canonical_path);

  if (g_strcmp0 (parent, canonical_directory) != 0 ||
      g_file_test (canonical_path, G_FILE_TEST_IS_SYMLINK) ||
      !g_file_test (canonical_path, G_FILE_TEST_IS_REGULAR))
    {
      send_error (connection, "That image is not a remote paste.");
      return;
    }

  if (preview)
    {
      thumbnail = gdk_pixbuf_new_from_file_at_scale (
        canonical_path, 640, 360, TRUE, &error);
      if (thumbnail != NULL &&
          !gdk_pixbuf_save_to_buffer (
            thumbnail, &contents, &length, "png", &error, NULL))
        g_clear_object (&thumbnail);
    }
  else if (!g_file_get_contents (canonical_path, &contents, &length, &error))
    {
      send_error (connection, error->message);
      return;
    }

  if (preview && thumbnail == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  if (length < 8 || length > XD_REMOTE_MAX_IMAGE_BYTES ||
      memcmp (contents, "\x89PNG\r\n\x1a\n", 8) != 0)
    {
      send_error (connection, "That remote paste is not a valid PNG.");
      return;
    }

  encoded = g_base64_encode ((const guchar *) contents, length);

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "ok");
    json_builder_add_boolean_value (builder, TRUE);
    json_builder_set_member_name (builder, "mime");
    json_builder_add_string_value (builder, "image/png");
    json_builder_set_member_name (builder, "data");
    json_builder_add_string_value (builder, encoded);
    json_builder_end_object (builder);

    send_json (connection, builder);
  }
}

/* --- running a turn -------------------------------------------------------- */

typedef struct
{
  XdRemoteServer *server;   /* unowned; the server outlives its turns */
  char *chat_id;
  char *backend_id;
} Running;

static void
running_free (Running *running)
{
  g_free (running->chat_id);
  g_free (running->backend_id);
  g_free (running);
}

static void
add_commands (JsonBuilder       *builder,
              const char        *member,
              const char *const *commands)
{
  if (commands == NULL)
    return;

  json_builder_set_member_name (builder, member);
  json_builder_begin_array (builder);
  for (guint i = 0; commands[i] != NULL; i++)
    json_builder_add_string_value (builder, commands[i]);
  json_builder_end_array (builder);
}

static void
on_turn_commands (XdDaemonTurn    *turn,
                  const char *const *commands,
                  gpointer          user_data)
{
  Running *running = user_data;
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  g_hash_table_replace (running->server->command_sets,
                        g_strdup (running->backend_id),
                        g_strdupv ((char **) commands));

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "event");
  json_builder_add_string_value (builder, "commands");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, running->chat_id);
  json_builder_set_member_name (builder, "backend");
  json_builder_add_string_value (builder, running->backend_id);
  add_commands (builder, "commands", commands);
  json_builder_end_object (builder);

  broadcast (running->server, builder);
}

static void
on_turn_text (XdDaemonTurn *turn,
              const char   *delta,
              gpointer      user_data)
{
  Running *running = user_data;

  broadcast_event (running->server, "text", running->chat_id, "text", delta);
}

static char *
describe_turn_context (const char *workdir)
{
  g_autoptr (XdGitInfo) git = xd_git_info_for_path (workdir);
  g_autoptr (GString) text = g_string_new (NULL);
  g_autofree char *shown = NULL;

  if (workdir == NULL)
    return g_strdup ("No working directory");

  shown = g_str_has_prefix (workdir, g_get_home_dir ())
    ? g_strconcat ("~", workdir + strlen (g_get_home_dir ()), NULL)
    : g_strdup (workdir);

  if (git == NULL)
    return g_strdup_printf ("%s — not a repository", shown);

  if (git->branch != NULL)
    g_string_append_printf (text, "%s %s", git->detached ? "detached at" : "⎇",
                            git->branch);
  g_string_append_printf (text, "%s%s", text->len > 0 ? " · " : "", git->name);
  if (git->linked_worktree)
    g_string_append (text, " (worktree)");
  g_string_append_printf (text, " · %s", shown);

  return g_string_free (g_steal_pointer (&text), FALSE);
}

static void
on_turn_tool (XdDaemonTurn *turn,
              const char   *name,
              gpointer      user_data)
{
  Running *running = user_data;
  const char *workdir = xd_daemon_turn_get_workdir (turn);
  g_autofree char *context = describe_turn_context (workdir);
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "event");
  json_builder_add_string_value (builder, "tool");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, running->chat_id);
  json_builder_set_member_name (builder, "text");
  json_builder_add_string_value (builder, name);
  if (workdir != NULL)
    {
      json_builder_set_member_name (builder, "workdir");
      json_builder_add_string_value (builder, workdir);
    }
  json_builder_set_member_name (builder, "context");
  json_builder_add_string_value (builder, context);
  json_builder_end_object (builder);

  broadcast (running->server, builder);
}

static gboolean forget_turn (gpointer user_data);

static void
complete_quiesce (XdRemoteServer *self)
{
  g_autoptr (GTask) task = NULL;

  if (self->quiesce_task == NULL || g_hash_table_size (self->turns) != 0)
    return;

  task = g_steal_pointer (&self->quiesce_task);
  g_task_return_boolean (task, TRUE);
}

static void
on_turn_finished (XdDaemonTurn *turn,
                  gboolean      ok,
                  const char   *message,
                  gpointer      user_data)
{
  Running *running = user_data;
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "event");
  json_builder_add_string_value (builder, "turn-finished");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, running->chat_id);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, ok);
  json_builder_set_member_name (builder, "waiting");
  json_builder_add_boolean_value (
    builder, xd_daemon_turn_asked_user (turn));
  if (message != NULL)
    {
      json_builder_set_member_name (builder, "error");
      json_builder_add_string_value (builder, message);
    }
  json_builder_end_object (builder);

  broadcast (running->server, builder);

  g_idle_add (forget_turn, running);
}

/*
 * Starts one daemon-owned turn.
 *
 * Used by a direct send and by the persistent queue. Keeping both on this path
 * makes naming, storage and broadcasts identical whichever device supplied
 * the message.
 */
static gboolean
start_daemon_turn (XdRemoteServer  *self,
                   const char      *chat_id,
                   const char      *text,
                   GError         **error)
{
  XdDaemonTurn *turn;
  Running *running;
  g_autofree char *backend_id = NULL;

  if (self->quiescing)
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_BUSY,
                           "The daemon is preparing to restart.");
      return FALSE;
    }

  {
    g_autoptr (XdChat) chat =
      xd_storage_get_chat (self->storage, chat_id, error);

    if (chat == NULL)
      return FALSE;

    backend_id = g_strdup (chat->backend);

    if (chat->new_worktree)
      {
        g_autoptr (XdDaemonTurn) resolver =
          xd_daemon_turn_new (self->storage, self->root_path);
        g_autofree char *name = xd_chat_title_from_prompt (text);
        g_autofree char *source =
          xd_daemon_turn_resolve_workdir (resolver, chat);
        g_autofree char *worktree =
          xd_worktree_create (source, chat_id, name, error);

        if (worktree == NULL ||
            !xd_storage_use_worktree (
              self->storage, chat_id, worktree, error))
          return FALSE;
      }
  }

  /*
   * An unnamed chat takes its name from what was asked first.
   *
   * Done here rather than by whoever sent it: the daemon is the one writing
   * the message down, and a chat named on one device has to be named on all of
   * them. Before the turn starts, because starting it is what stores the
   * message this looks for the absence of.
   */
  {
    g_autoptr (XdChat) chat = xd_storage_get_chat (self->storage, chat_id, NULL);

    if (chat != NULL && g_strcmp0 (chat->title, XD_CHAT_UNTITLED) == 0 &&
        xd_storage_last_message_id (self->storage, chat_id) == 0)
      {
        g_autofree char *title = xd_chat_title_from_prompt (text);

        if (title != NULL &&
            xd_storage_set_chat_title (self->storage, chat_id, title, NULL))
          broadcast_tree (self);
      }
  }

  turn = xd_daemon_turn_new (self->storage, self->root_path);

  running = g_new0 (Running, 1);
  running->server = self;
  running->chat_id = g_strdup (chat_id);
  running->backend_id = g_steal_pointer (&backend_id);

  g_signal_connect (turn, "commands", G_CALLBACK (on_turn_commands), running);
  g_signal_connect (turn, "text", G_CALLBACK (on_turn_text), running);
  g_signal_connect (turn, "tool", G_CALLBACK (on_turn_tool), running);
  g_signal_connect (turn, "finished", G_CALLBACK (on_turn_finished), running);

  if (!xd_daemon_turn_start (turn, chat_id, text, error))
    {
      running_free (running);
      g_object_unref (turn);

      /* The message and the failure are both in the transcript now. */
      broadcast_event (self, "changed", chat_id, NULL, NULL);
      return FALSE;
    }

  g_hash_table_insert (self->turns, g_strdup (chat_id), turn);

  /* Everyone watching sees the message arrive and the work start, including
   * the device that sent it -- one path, so every screen agrees. */
  broadcast_event (self, "turn-started", chat_id, "label",
                   xd_daemon_turn_get_label (turn));

  return TRUE;
}

/*
 * What is waiting, in order.
 *
 * Every device shows the same queue, so the whole list travels rather than
 * only its head. "text" carries the oldest message as well: a client from
 * before the queue could hold more than one still shows something useful.
 */
static void
broadcast_queue (XdRemoteServer *self,
                 const char     *chat_id,
                 GPtrArray      *messages)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "event");
  json_builder_add_string_value (builder, "queued");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder, chat_id);

  if (messages != NULL && messages->len > 0)
    {
      json_builder_set_member_name (builder, "text");
      json_builder_add_string_value (builder, g_ptr_array_index (messages, 0));
    }

  json_builder_set_member_name (builder, "queue");
  json_builder_begin_array (builder);
  for (guint i = 0; messages != NULL && i < messages->len; i++)
    json_builder_add_string_value (builder, g_ptr_array_index (messages, i));
  json_builder_end_array (builder);

  json_builder_end_object (builder);

  broadcast (self, builder);
}

static void
broadcast_stored_queue (XdRemoteServer *self,
                        const char     *chat_id)
{
  g_autoptr (XdChat) chat = xd_storage_get_chat (self->storage, chat_id, NULL);

  broadcast_queue (self, chat_id, chat != NULL ? chat->queue : NULL);
}

/* Appended rather than replacing: a second thought waits behind the first. */
static gboolean
store_queued (Connection *connection,
              const char *chat_id,
              const char *text)
{
  g_autoptr (GError) error = NULL;

  if (!xd_storage_queue_append (connection->server->storage, chat_id,
                                text, &error))
    {
      send_error (connection, error->message);
      return FALSE;
    }

  send_done (connection, NULL);
  broadcast_stored_queue (connection->server, chat_id);

  return TRUE;
}

/* Consumes the persisted message before starting it, so reconnecting devices
 * cannot race each other into running it twice. */
static gboolean
start_queued (XdRemoteServer *self,
              const char     *chat_id)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *text = NULL;

  if (self->quiescing || g_hash_table_contains (self->turns, chat_id))
    return FALSE;

  if (!xd_storage_queue_take_first (self->storage, chat_id, &text, &error))
    {
      g_warning ("cannot consume the queued message: %s", error->message);
      return FALSE;
    }

  if (text == NULL)
    return FALSE;

  broadcast_stored_queue (self, chat_id);

  if (!start_daemon_turn (self, chat_id, text, &error))
    {
      g_warning ("cannot start the queued message: %s", error->message);
      return FALSE;
    }

  return TRUE;
}

static void
remove_paths (GPtrArray *paths)
{
  for (guint i = 0; paths != NULL && i < paths->len; i++)
    g_unlink (g_ptr_array_index (paths, i));
}

/*
 * Turns client-side image bytes into files the daemon-side agent can read.
 *
 * Supplied names are display metadata only. Generated names and a private
 * cache directory prevent path traversal and keep uploads out of repositories.
 */
static char *
materialize_message (JsonObject  *request,
                     const char  *text,
                     GError     **error)
{
  g_autoptr (GString) message = g_string_new (text != NULL ? text : "");
  g_autoptr (GPtrArray) paths = g_ptr_array_new_with_free_func (g_free);
  g_autofree char *directory = NULL;
  JsonNode *node;
  JsonArray *attachments;
  gsize total = 0;

  if (!json_object_has_member (request, "attachments"))
    return g_string_free (g_steal_pointer (&message), FALSE);

  node = json_object_get_member (request, "attachments");
  if (!JSON_NODE_HOLDS_ARRAY (node))
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                           "Message attachments must be an array.");
      return NULL;
    }

  attachments = json_node_get_array (node);
  if (json_array_get_length (attachments) == 0 ||
      json_array_get_length (attachments) > XD_REMOTE_MAX_IMAGES)
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                           "A message can contain between 1 and 4 images.");
      return NULL;
    }

  directory = g_build_filename (g_get_user_cache_dir (), "xd",
                                "remote-pasted", NULL);
  if (g_mkdir_with_parents (directory, 0700) != 0)
    {
      g_set_error (error, G_IO_ERROR, g_io_error_from_errno (errno),
                   "Cannot create the remote image cache: %s.",
                   g_strerror (errno));
      return NULL;
    }

  for (guint i = 0; i < json_array_get_length (attachments); i++)
    {
      JsonObject *attachment = json_array_get_object_element (attachments, i);
      const char *mime = member_string (attachment, "mime");
      const char *encoded = member_string (attachment, "data");
      g_autofree guchar *data = NULL;
      g_autofree char *uuid = NULL;
      g_autofree char *name = NULL;
      char *path;
      gsize length = 0;
      gsize encoded_limit = ((XD_REMOTE_MAX_IMAGE_BYTES + 2) / 3) * 4;

      if (attachment == NULL || g_strcmp0 (mime, "image/png") != 0 ||
          encoded == NULL || strlen (encoded) > encoded_limit)
        {
          g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                               "Only PNG images up to 10 MiB can be sent.");
          remove_paths (paths);
          return NULL;
        }

      data = g_base64_decode (encoded, &length);
      if (length < 8 ||
          memcmp (data, "\x89PNG\r\n\x1a\n", 8) != 0 ||
          length > XD_REMOTE_MAX_IMAGE_BYTES ||
          total > XD_REMOTE_MAX_IMAGES_BYTES - length)
        {
          g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                               "The attached images are invalid or too large.");
          remove_paths (paths);
          return NULL;
        }
      total += length;

      uuid = g_uuid_string_random ();
      name = g_strdup_printf ("paste-%s.png", uuid);
      path = g_build_filename (directory, name, NULL);

      if (!g_file_set_contents (path, (const char *) data, length, error))
        {
          remove_paths (paths);
          g_free (path);
          return NULL;
        }
      g_chmod (path, 0600);
      g_ptr_array_add (paths, path);

      if (message->len > 0)
        g_string_append_c (message, '\n');
      g_string_append_printf (message, "[image: %s]", path);
    }

  return g_string_free (g_steal_pointer (&message), FALSE);
}

/* The turn is over, but it is over inside one of its own signals: dropping it
 * there would take the session down while it was still emitting. */
static gboolean
forget_turn (gpointer user_data)
{
  Running *running = user_data;

  g_hash_table_remove (running->server->turns, running->chat_id);
  if (running->server->quiescing)
    complete_quiesce (running->server);
  else
    start_queued (running->server, running->chat_id);
  running_free (running);

  return G_SOURCE_REMOVE;
}

static void
handle_send (Connection *connection,
             JsonObject *request)
{
  XdRemoteServer *self = connection->server;
  const char *chat_id = member_string (request, "chat");
  const char *text = member_string (request, "text");
  g_autofree char *message = NULL;
  g_autoptr (GError) error = NULL;

  if (chat_id == NULL ||
      ((text == NULL || *text == '\0') &&
       !json_object_has_member (request, "attachments")))
    {
      send_error (connection, "A message needs a chat and something to say.");
      return;
    }

  message = materialize_message (request, text, &error);
  if (message == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  /*
   * A client can send before the turn-started event reaches it. Decide at the
   * daemon, where the current state is authoritative: preserve that text as
   * the one queued instruction instead of rejecting a valid steer attempt.
   * This also keeps two devices from starting agents in the same directory.
   */
  if (g_hash_table_contains (self->turns, chat_id))
    {
      store_queued (connection, chat_id, message);
      return;
    }

  if (self->quiescing)
    {
      store_queued (connection, chat_id, message);
      return;
    }

  if (!start_daemon_turn (self, chat_id, message, &error))
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);
}

static void
handle_cancel (Connection *connection,
               JsonObject *request)
{
  const char *chat_id = member_string (request, "chat");
  XdDaemonTurn *turn;

  if (chat_id == NULL)
    {
      send_error (connection, "cancel needs a chat id");
      return;
    }

  turn = g_hash_table_lookup (connection->server->turns, chat_id);
  if (turn != NULL)
    xd_daemon_turn_cancel (turn);
  else
    {
      /*
       * A client can miss the working event while its queued instruction has
       * already reached this daemon. Treat cancel as the steer action it is:
       * when nothing needs stopping, start that instruction immediately.
       */
      start_queued (connection->server, chat_id);
    }

  send_done (connection, NULL);
}

static void
handle_queue (Connection *connection,
              JsonObject *request,
              gboolean    drop)
{
  const char *chat_id = member_string (request, "chat");
  const char *text = drop ? NULL : member_string (request, "text");

  if (chat_id == NULL || (!drop && (text == NULL || *text == '\0')))
    {
      send_error (connection, drop ? "drop-queue needs a chat id"
                                   : "A queued message needs a chat and text.");
      return;
    }

  if (!drop)
    {
      store_queued (connection, chat_id, text);
      return;
    }

  /*
   * Dropping one waiting message, or all of them.
   *
   * A client that says which position it means discards that message; one that
   * says nothing discards the queue, which is what the old single-message
   * discard did.
   */
  {
    g_autoptr (GError) error = NULL;
    gboolean ok;

    if (json_object_has_member (request, "index"))
      ok = xd_storage_queue_remove (
        connection->server->storage, chat_id,
        (guint) MAX (json_object_get_int_member (request, "index"), 0), &error);
    else
      ok = xd_storage_set_queue (
        connection->server->storage, chat_id, NULL, &error);

    if (!ok)
      {
        send_error (connection, error->message);
        return;
      }

    send_done (connection, NULL);
    broadcast_stored_queue (connection->server, chat_id);
  }
}

/* --- read-only repository views ------------------------------------------- */

#define MAX_REMOTE_DIFF_BYTES (8 * 1024 * 1024)

static const char *DIFF_BASE_SCRIPT =
  "for ref in \"$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD)\" "
  "origin/main origin/master main master; do "
  "  [ -n \"$ref\" ] || continue; "
  "  git rev-parse --verify --quiet \"$ref\" >/dev/null && { echo \"$ref\"; exit 0; }; "
  "done";

static const char *DIFF_WORKING_SCRIPT =
  "git --no-pager diff HEAD; "
  "git ls-files --others --exclude-standard -z | "
  "xargs -0 -n 1 sh -c "
  "'[ \"$#\" -eq 0 ] || "
  "git --no-pager diff --no-index -- /dev/null \"$1\"' sh";

static gboolean
diff_base_is_safe (const char *base)
{
  if (base == NULL || *base == '\0' || strlen (base) > 256)
    return FALSE;

  for (const char *at = base; *at != '\0'; at++)
    {
      if (!g_ascii_isalnum (*at) &&
          *at != '-' && *at != '_' && *at != '.' && *at != '/')
        return FALSE;
    }

  return TRUE;
}

static gboolean
diff_path_is_safe (const char *workdir,
                   const char *repository,
                   const char *path)
{
  g_autofree char *canonical = NULL;
  g_autofree char *prefix = NULL;

  if (path == NULL || *path == '\0' || g_path_is_absolute (path))
    return FALSE;

  canonical = g_canonicalize_filename (path, workdir);
  prefix = g_strconcat (repository, G_DIR_SEPARATOR_S, NULL);

  return g_strcmp0 (canonical, repository) == 0 ||
         g_str_has_prefix (canonical, prefix);
}

static char *
read_diff_output (const char        *workdir,
                  const char *const *argv,
                  GError           **error)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GSubprocess) process = NULL;
  g_autofree char *stderr_text = NULL;
  char *output = NULL;

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                        G_SUBPROCESS_FLAGS_STDERR_PIPE);
  g_subprocess_launcher_set_cwd (launcher, workdir);
  process = g_subprocess_launcher_spawnv (launcher, argv, error);
  if (process == NULL)
    return NULL;

  if (!g_subprocess_communicate_utf8 (process, NULL, NULL, &output,
                                      &stderr_text, error))
    return NULL;

  /*
   * `git diff --no-index` exits 1 when it found a difference. The output is
   * still the answer, so process status is deliberately not treated as an
   * error. An empty failed command is different: surface git's explanation.
   */
  if ((output == NULL || *output == '\0') &&
      !g_subprocess_get_successful (process) &&
      stderr_text != NULL && *stderr_text != '\0')
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_FAILED,
                           g_strstrip (stderr_text));
      g_free (output);
      return NULL;
    }

  if (output != NULL && strlen (output) > MAX_REMOTE_DIFF_BYTES)
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_NO_SPACE,
                           "That diff is too large to send over the remote connection.");
      g_free (output);
      return NULL;
    }

  return output != NULL ? output : g_strdup ("");
}

static void
handle_diff_read (Connection *connection,
                  JsonObject *request)
{
  XdRemoteServer *self = connection->server;
  const char *chat_id = member_string (request, "chat");
  const char *read = member_string (request, "read");
  const char *path = member_string (request, "path");
  const char *base = member_string (request, "base");
  g_autoptr (XdChat) chat = NULL;
  g_autoptr (XdDaemonTurn) resolver = NULL;
  g_autoptr (XdGitInfo) repository = NULL;
  g_autofree char *workdir = NULL;
  g_autofree char *range = NULL;
  g_autofree char *output = NULL;
  g_autoptr (GError) error = NULL;
  const char *const *argv = NULL;
  const char *base_argv[] = { "sh", "-c", DIFF_BASE_SCRIPT, NULL };
  const char *working_status_argv[] = {
    "git", "status", "--porcelain", "--untracked-files=all", NULL
  };
  const char *branch_status_argv[] = {
    "git", "--no-pager", "diff", "--name-status", NULL, NULL
  };
  const char *working_all_argv[] = {
    "sh", "-c", DIFF_WORKING_SCRIPT, NULL
  };
  const char *branch_all_argv[] = {
    "git", "--no-pager", "diff", NULL, NULL
  };
  const char *working_file_argv[] = {
    "git", "--no-pager", "diff", "HEAD", "--", path, NULL
  };
  const char *untracked_file_argv[] = {
    "git", "--no-pager", "diff", "--no-index", "--", "/dev/null", path, NULL
  };
  const char *branch_file_argv[] = {
    "git", "--no-pager", "diff", NULL, "--", path, NULL
  };

  if (chat_id == NULL || read == NULL)
    {
      send_error (connection, "diff-read needs a chat and read type.");
      return;
    }

  chat = xd_storage_get_chat (self->storage, chat_id, &error);
  if (chat == NULL)
    {
      send_error (connection, error != NULL ? error->message : "No such chat.");
      return;
    }

  resolver = xd_daemon_turn_new (self->storage, self->root_path);
  workdir = xd_daemon_turn_resolve_workdir (resolver, chat);
  repository = xd_git_info_for_path (workdir);
  if (workdir == NULL || repository == NULL)
    {
      send_error (connection, "This chat is not in a Git repository.");
      return;
    }

  if (g_strcmp0 (read, "base") == 0)
    {
      argv = base_argv;
    }
  else if (g_strcmp0 (read, "working-status") == 0)
    {
      argv = working_status_argv;
    }
  else if (g_strcmp0 (read, "branch-status") == 0)
    {
      if (!diff_base_is_safe (base))
        {
          send_error (connection, "A valid base branch is required.");
          return;
        }

      range = g_strdup_printf ("%s...HEAD", base);
      branch_status_argv[4] = range;
      argv = branch_status_argv;
    }
  else if (g_strcmp0 (read, "working-all") == 0)
    {
      argv = working_all_argv;
    }
  else if (g_strcmp0 (read, "branch-all") == 0)
    {
      if (!diff_base_is_safe (base))
        {
          send_error (connection, "A valid base branch is required.");
          return;
        }

      range = g_strdup_printf ("%s...HEAD", base);
      branch_all_argv[3] = range;
      argv = branch_all_argv;
    }
  else if (g_strcmp0 (read, "working-file") == 0 ||
           g_strcmp0 (read, "untracked-file") == 0 ||
           g_strcmp0 (read, "branch-file") == 0)
    {
      if (!diff_path_is_safe (workdir, repository->root, path))
        {
          send_error (connection, "That diff path is outside the repository.");
          return;
        }

      if (g_strcmp0 (read, "working-file") == 0)
        argv = working_file_argv;
      else if (g_strcmp0 (read, "untracked-file") == 0)
        argv = untracked_file_argv;
      else
        {
          if (!diff_base_is_safe (base))
            {
              send_error (connection, "A valid base branch is required.");
              return;
            }

          range = g_strdup_printf ("%s...HEAD", base);
          branch_file_argv[3] = range;
          argv = branch_file_argv;
        }
    }
  else
    {
      send_error (connection, "No such diff read type.");
      return;
    }

  output = read_diff_output (workdir, argv, &error);
  if (output == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "ok");
    json_builder_add_boolean_value (builder, TRUE);
    json_builder_set_member_name (builder, "output");
    json_builder_add_string_value (builder, output);
    json_builder_end_object (builder);
    send_json (connection, builder);
  }
}

/* --- terminals ------------------------------------------------------------ */

static XdRemoteTerminal *
terminal_argument (Connection *connection,
                   JsonObject *request)
{
  const char *id = member_string (request, "terminal");
  XdRemoteTerminal *terminal;

  if (id == NULL)
    {
      send_error (connection, "A terminal id is required.");
      return NULL;
    }

  terminal = g_hash_table_lookup (connection->server->terminals, id);
  if (terminal == NULL)
    {
      send_error (connection, "No such terminal.");
      return NULL;
    }

  return terminal;
}

static void
on_terminal_output (XdRemoteTerminal *terminal,
                    GBytes          *bytes,
                    gpointer         user_data)
{
  XdRemoteServer *self = user_data;
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autofree char *encoded = NULL;
  gsize length;
  const guint8 *data = g_bytes_get_data (bytes, &length);

  encoded = g_base64_encode (data, length);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "event");
  json_builder_add_string_value (builder, "terminal-output");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder,
                                 xd_remote_terminal_get_chat_id (terminal));
  json_builder_set_member_name (builder, "terminal");
  json_builder_add_string_value (builder, xd_remote_terminal_get_id (terminal));
  json_builder_set_member_name (builder, "data");
  json_builder_add_string_value (builder, encoded);
  json_builder_end_object (builder);

  broadcast (self, builder);
}

static void
on_terminal_closed (XdRemoteTerminal *terminal,
                    gpointer          user_data)
{
  XdRemoteServer *self = user_data;
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autofree char *id = g_strdup (xd_remote_terminal_get_id (terminal));

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "event");
  json_builder_add_string_value (builder, "terminal-closed");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder,
                                 xd_remote_terminal_get_chat_id (terminal));
  json_builder_set_member_name (builder, "terminal");
  json_builder_add_string_value (builder, id);
  json_builder_end_object (builder);

  broadcast (self, builder);
  g_hash_table_remove (self->terminals, id);
}

static void
broadcast_terminal_geometry (XdRemoteServer   *self,
                             XdRemoteTerminal *terminal)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "event");
  json_builder_add_string_value (builder, "terminal-resized");
  json_builder_set_member_name (builder, "chat");
  json_builder_add_string_value (builder,
                                 xd_remote_terminal_get_chat_id (terminal));
  json_builder_set_member_name (builder, "terminal");
  json_builder_add_string_value (builder, xd_remote_terminal_get_id (terminal));
  json_builder_set_member_name (builder, "columns");
  json_builder_add_int_value (builder,
                              xd_remote_terminal_get_columns (terminal));
  json_builder_set_member_name (builder, "rows");
  json_builder_add_int_value (builder, xd_remote_terminal_get_rows (terminal));
  json_builder_end_object (builder);

  broadcast (self, builder);
}

static void
handle_terminal_list (Connection *connection,
                      JsonObject *request)
{
  const char *chat_id = member_string (request, "chat");
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  GHashTableIter iter;
  gpointer value;

  if (chat_id == NULL)
    {
      send_error (connection, "terminal-list needs a chat id.");
      return;
    }

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);
  json_builder_set_member_name (builder, "terminals");
  json_builder_begin_array (builder);

  g_hash_table_iter_init (&iter, connection->server->terminals);
  while (g_hash_table_iter_next (&iter, NULL, &value))
    {
      XdRemoteTerminal *terminal = value;
      GPtrArray *replay;

      if (g_strcmp0 (xd_remote_terminal_get_chat_id (terminal), chat_id) != 0)
        continue;
      if (xd_remote_terminal_is_closing (terminal))
        continue;

      replay = xd_remote_terminal_get_replay (terminal);

      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "id");
      json_builder_add_string_value (builder, xd_remote_terminal_get_id (terminal));
      json_builder_set_member_name (builder, "title");
      json_builder_add_string_value (builder,
                                     xd_remote_terminal_get_title (terminal));
      json_builder_set_member_name (builder, "columns");
      json_builder_add_int_value (builder,
                                  xd_remote_terminal_get_columns (terminal));
      json_builder_set_member_name (builder, "rows");
      json_builder_add_int_value (builder,
                                  xd_remote_terminal_get_rows (terminal));
      json_builder_set_member_name (builder, "replay");
      json_builder_begin_array (builder);
      for (guint j = 0; replay != NULL && j < replay->len; j++)
        {
          const XdTerminalReplayItem *item = g_ptr_array_index (replay, j);

          json_builder_begin_object (builder);
          if (item->data != NULL)
            {
              g_autofree char *encoded = NULL;
              gsize length;
              const guint8 *data = g_bytes_get_data (item->data, &length);

              encoded = g_base64_encode (data, length);
              json_builder_set_member_name (builder, "data");
              json_builder_add_string_value (builder, encoded);
            }
          else
            {
              json_builder_set_member_name (builder, "columns");
              json_builder_add_int_value (builder, item->columns);
              json_builder_set_member_name (builder, "rows");
              json_builder_add_int_value (builder, item->rows);
            }
          json_builder_end_object (builder);
        }
      json_builder_end_array (builder);
      json_builder_end_object (builder);
    }

  json_builder_end_array (builder);
  json_builder_end_object (builder);
  send_json (connection, builder);
}

static void
handle_terminal_open (Connection *connection,
                      JsonObject *request)
{
  XdRemoteServer *self = connection->server;
  const char *chat_id = member_string (request, "chat");
  g_autoptr (XdChat) chat = NULL;
  g_autoptr (XdDaemonTurn) resolver = NULL;
  g_autofree char *workdir = NULL;
  g_autoptr (XdRemoteTerminal) terminal = NULL;
  g_autoptr (GError) error = NULL;
  guint columns = (guint) CLAMP (
    json_object_get_int_member_with_default (request, "columns", 80), 1, 1000);
  guint rows = (guint) CLAMP (
    json_object_get_int_member_with_default (request, "rows", 24), 1, 1000);
  gboolean reuse =
    json_object_get_boolean_member_with_default (request, "reuse", FALSE);

  if (chat_id == NULL)
    {
      send_error (connection, "terminal-open needs a chat id.");
      return;
    }

  if (reuse)
    {
      GHashTableIter iter;
      gpointer value;

      g_hash_table_iter_init (&iter, self->terminals);
      while (g_hash_table_iter_next (&iter, NULL, &value))
        {
          XdRemoteTerminal *existing = value;

          if (!xd_remote_terminal_is_closing (existing) &&
              g_strcmp0 (xd_remote_terminal_get_chat_id (existing), chat_id) == 0)
            {
              send_done (connection, xd_remote_terminal_get_id (existing));
              return;
            }
        }
    }

  chat = xd_storage_get_chat (self->storage, chat_id, &error);
  if (chat == NULL)
    {
      send_error (connection, error != NULL ? error->message : "No such chat.");
      return;
    }

  resolver = xd_daemon_turn_new (self->storage, self->root_path);
  workdir = xd_daemon_turn_resolve_workdir (resolver, chat);
  if (workdir == NULL)
    {
      send_error (connection, "This chat has no working directory.");
      return;
    }

  terminal = xd_remote_terminal_new (chat_id, workdir, columns, rows, &error);
  if (terminal == NULL)
    {
      send_error (connection, error->message);
      return;
    }

  g_signal_connect (terminal, "output", G_CALLBACK (on_terminal_output), self);
  g_signal_connect (terminal, "closed", G_CALLBACK (on_terminal_closed), self);
  g_hash_table_insert (self->terminals,
                       g_strdup (xd_remote_terminal_get_id (terminal)),
                       g_object_ref (terminal));

  /*
   * Reply first. Output is read on the next main-loop dispatch, so the opener
   * always has a tab to feed before the first terminal-output event arrives.
   */
  send_done (connection, xd_remote_terminal_get_id (terminal));

  {
    g_autoptr (JsonBuilder) builder = json_builder_new ();

    json_builder_begin_object (builder);
    json_builder_set_member_name (builder, "event");
    json_builder_add_string_value (builder, "terminal-opened");
    json_builder_set_member_name (builder, "chat");
    json_builder_add_string_value (builder, chat_id);
    json_builder_set_member_name (builder, "terminal");
    json_builder_add_string_value (builder, xd_remote_terminal_get_id (terminal));
    json_builder_set_member_name (builder, "title");
    json_builder_add_string_value (builder, xd_remote_terminal_get_title (terminal));
    json_builder_set_member_name (builder, "columns");
    json_builder_add_int_value (builder,
                                xd_remote_terminal_get_columns (terminal));
    json_builder_set_member_name (builder, "rows");
    json_builder_add_int_value (builder, xd_remote_terminal_get_rows (terminal));
    json_builder_end_object (builder);

    broadcast (self, builder);
  }
}

static void
handle_terminal_input (Connection *connection,
                       JsonObject *request)
{
  XdRemoteTerminal *terminal = terminal_argument (connection, request);
  const char *encoded = member_string (request, "data");
  g_autofree guchar *data = NULL;
  g_autoptr (GError) error = NULL;
  gsize length = 0;

  if (terminal == NULL)
    return;
  if (encoded == NULL)
    {
      send_error (connection, "terminal-input needs data.");
      return;
    }

  data = g_base64_decode (encoded, &length);
  if (!xd_remote_terminal_write (terminal, data, length, &error))
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);
}

static void
handle_terminal_resize (Connection *connection,
                        JsonObject *request)
{
  XdRemoteTerminal *terminal = terminal_argument (connection, request);
  g_autoptr (GError) error = NULL;
  guint columns = (guint) CLAMP (
    json_object_get_int_member_with_default (request, "columns", 80), 1, 1000);
  guint rows = (guint) CLAMP (
    json_object_get_int_member_with_default (request, "rows", 24), 1, 1000);

  if (terminal == NULL)
    return;

  if (!xd_remote_terminal_resize (terminal, columns, rows, &error))
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);
  broadcast_terminal_geometry (connection->server, terminal);
}

static void
handle_terminal_kill (Connection *connection,
                      JsonObject *request)
{
  const char *id = member_string (request, "terminal");
  XdRemoteTerminal *terminal;

  if (id == NULL)
    {
      send_error (connection, "A terminal id is required.");
      return;
    }

  /* Idempotent so a client can safely retry after losing the acknowledgement. */
  terminal = g_hash_table_lookup (connection->server->terminals, id);
  if (terminal != NULL)
    xd_remote_terminal_close (terminal);
  send_done (connection, NULL);
}

/* --- one chat's settings ---------------------------------------------------- */

static void
add_worktrees (JsonBuilder *builder,
               const char  *workdir)
{
  g_autoptr (GPtrArray) worktrees = xd_worktree_list (workdir, NULL);

  if (worktrees == NULL)
    return;

  json_builder_set_member_name (builder, "worktrees");
  json_builder_begin_array (builder);
  for (guint i = 0; i < worktrees->len; i++)
    {
      XdWorktreeInfo *item = g_ptr_array_index (worktrees, i);

      json_builder_begin_object (builder);
      json_builder_set_member_name (builder, "path");
      json_builder_add_string_value (builder, item->path);
      if (item->branch != NULL)
        {
          json_builder_set_member_name (builder, "branch");
          json_builder_add_string_value (builder, item->branch);
        }
      json_builder_set_member_name (builder, "detached");
      json_builder_add_boolean_value (builder, item->detached);
      json_builder_set_member_name (builder, "main");
      json_builder_add_boolean_value (builder, item->main);
      json_builder_set_member_name (builder, "current");
      json_builder_add_boolean_value (builder, item->current);
      json_builder_end_object (builder);
    }
  json_builder_end_array (builder);
}

static void
handle_chat (Connection *connection,
             JsonObject *request)
{
  const char *chat_id = member_string (request, "chat");
  g_autoptr (XdChat) chat = NULL;
  g_autoptr (GError) error = NULL;
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  XdRemoteServer *self = connection->server;

  if (chat_id == NULL)
    {
      send_error (connection, "chat needs a chat id");
      return;
    }

  chat = xd_storage_get_chat (self->storage, chat_id, &error);
  if (chat == NULL)
    {
      send_error (connection, error != NULL ? error->message : "No such chat.");
      return;
    }

  /* A daemon restart ends the old process, not the user's next instruction.
   * Opening the chat is enough to resume its persisted queue. */
  if (chat->queue->len > 0 &&
      !g_hash_table_contains (self->turns, chat_id) &&
      start_queued (self, chat_id))
    {
      g_clear_pointer (&chat, xd_chat_free);
      chat = xd_storage_get_chat (self->storage, chat_id, &error);
      if (chat == NULL)
        {
          send_error (connection, error != NULL ? error->message : "No such chat.");
          return;
        }
    }

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);

  json_builder_set_member_name (builder, "title");
  json_builder_add_string_value (builder, chat->title);
  json_builder_set_member_name (builder, "backend");
  json_builder_add_string_value (builder, chat->backend);
  add_commands (
    builder, "commands",
    g_hash_table_lookup (self->command_sets, chat->backend));
  json_builder_set_member_name (builder, "plan");
  json_builder_add_boolean_value (builder, chat->plan);
  if (chat->queue->len > 0)
    {
      json_builder_set_member_name (builder, "queued");
      json_builder_add_string_value (builder, g_ptr_array_index (chat->queue, 0));
    }
  json_builder_set_member_name (builder, "queue");
  json_builder_begin_array (builder);
  for (guint i = 0; i < chat->queue->len; i++)
    json_builder_add_string_value (builder, g_ptr_array_index (chat->queue, i));
  json_builder_end_array (builder);
  {
    XdDaemonTurn *turn = g_hash_table_lookup (self->turns, chat_id);

    if (turn != NULL && !xd_daemon_turn_is_running (turn))
      turn = NULL;

    json_builder_set_member_name (builder, "working");
    json_builder_add_boolean_value (builder, turn != NULL);

    /* The turn so far, which is nowhere else yet: a device opening this chat
     * now joins the reply in progress, in the order it happened. */
    if (turn != NULL)
      {
        GPtrArray *items = xd_daemon_turn_get_items (turn);
        const char *segment = xd_daemon_turn_get_segment (turn);

        json_builder_set_member_name (builder, "label");
        json_builder_add_string_value (builder, xd_daemon_turn_get_label (turn));
        json_builder_set_member_name (builder, "working_for");
        json_builder_add_int_value (builder, xd_daemon_turn_get_elapsed (turn));

        json_builder_set_member_name (builder, "items");
        json_builder_begin_array (builder);
        for (guint i = 0; items != NULL && i < items->len; i++)
          {
            const XdTurnItem *item = g_ptr_array_index (items, i);

            json_builder_begin_object (builder);
            json_builder_set_member_name (builder, "tool");
            json_builder_add_boolean_value (builder, item->tool);
            json_builder_set_member_name (builder, "text");
            json_builder_add_string_value (builder, item->text);
            json_builder_end_object (builder);
          }
        json_builder_end_array (builder);

        if (segment != NULL && *segment != '\0')
          {
            json_builder_set_member_name (builder, "segment");
            json_builder_add_string_value (builder, segment);
          }
      }
  }

  if (chat->model != NULL)
    {
      json_builder_set_member_name (builder, "model");
      json_builder_add_string_value (builder, chat->model);
    }
  if (chat->effort != NULL)
    {
      json_builder_set_member_name (builder, "effort");
      json_builder_add_string_value (builder, chat->effort);
    }
  if (chat->access != NULL)
    {
      json_builder_set_member_name (builder, "access");
      json_builder_add_string_value (builder, chat->access);
    }

  {
    const AiBackend *backend = ai_backend_lookup (chat->backend);
    const char *model = chat->model != NULL
      ? chat->model : backend != NULL ? backend->default_model : NULL;
    guint64 used = 0;
    guint64 window = 0;

    if (xd_storage_get_context_usage (
          self->storage, chat->id, chat->backend, model, &used, &window))
      {
        json_builder_set_member_name (builder, "context_used");
        json_builder_add_int_value (builder, used);
        json_builder_set_member_name (builder, "context_window");
        json_builder_add_int_value (builder, window);
      }
  }

  json_builder_set_member_name (builder, "new_worktree");
  json_builder_add_boolean_value (builder, chat->new_worktree);
  json_builder_set_member_name (builder, "has_messages");
  json_builder_add_boolean_value (
    builder, xd_storage_last_message_id (self->storage, chat->id) > 0);

  /* Where it runs, resolved on this side: the folder chain that decides it is
   * here, and the client has no way to read it. */
  {
    g_autoptr (XdDaemonTurn) resolver = NULL;
    g_autofree char *workdir = NULL;

    resolver = xd_daemon_turn_new (self->storage, self->root_path);
    workdir = xd_daemon_turn_resolve_workdir (resolver, chat);

    if (workdir != NULL)
      {
        g_autoptr (XdGitInfo) git = xd_git_info_for_path (workdir);

        json_builder_set_member_name (builder, "workdir");
        json_builder_add_string_value (builder, workdir);
        json_builder_set_member_name (builder, "linked_worktree");
        json_builder_add_boolean_value (
          builder, git != NULL && git->linked_worktree);
        add_worktrees (builder, workdir);
      }
  }

  json_builder_end_object (builder);

  send_json (connection, builder);
}

static gboolean
use_existing_worktree (XdRemoteServer  *self,
                       const char      *chat_id,
                       const char      *requested,
                       GError         **error)
{
  g_autoptr (XdChat) chat = NULL;
  g_autoptr (XdDaemonTurn) resolver = NULL;
  g_autofree char *workdir = NULL;
  g_autoptr (GPtrArray) worktrees = NULL;

  if (requested == NULL || *requested == '\0')
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                   "An existing worktree path is required.");
      return FALSE;
    }

  chat = xd_storage_get_chat (self->storage, chat_id, error);
  if (chat == NULL)
    return FALSE;

  resolver = xd_daemon_turn_new (self->storage, self->root_path);
  workdir = xd_daemon_turn_resolve_workdir (resolver, chat);
  worktrees = xd_worktree_list (workdir, error);
  if (worktrees == NULL)
    return FALSE;

  for (guint i = 0; i < worktrees->len; i++)
    {
      XdWorktreeInfo *item = g_ptr_array_index (worktrees, i);

      if (xd_worktree_path_equal (item->path, requested))
        return xd_storage_use_existing_worktree (
          self->storage, chat_id, item->path, error);
    }

  g_set_error (error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
               "That path is not a worktree of this repository.");
  return FALSE;
}

static void
handle_set_option (Connection *connection,
                   JsonObject *request)
{
  XdRemoteServer *self = connection->server;
  const char *chat_id = member_string (request, "chat");
  const char *option = member_string (request, "option");
  const char *value = member_string (request, "value");
  g_autoptr (GError) error = NULL;
  gboolean ok;

  if (chat_id == NULL || option == NULL)
    {
      send_error (connection, "set-option needs a chat and an option.");
      return;
    }

  if (g_strcmp0 (option, "model") == 0)
    ok = xd_storage_set_model (self->storage, chat_id, value, &error);
  else if (g_strcmp0 (option, "effort") == 0)
    ok = xd_storage_set_effort (self->storage, chat_id, value, &error);
  else if (g_strcmp0 (option, "access") == 0)
    ok = xd_storage_set_access (self->storage, chat_id, value, &error);
  else if (g_strcmp0 (option, "plan") == 0)
    ok = xd_storage_set_plan (self->storage, chat_id,
                              g_strcmp0 (value, "true") == 0, &error);
  else if (g_strcmp0 (option, "backend") == 0)
    ok = xd_storage_set_backend (self->storage, chat_id, value, &error);
  else if (g_strcmp0 (option, "new-worktree") == 0)
    ok = xd_storage_set_new_worktree (
      self->storage, chat_id, g_strcmp0 (value, "true") == 0, &error);
  else if (g_strcmp0 (option, "workspace") == 0)
    ok = use_existing_worktree (self, chat_id, value, &error);
  else
    {
      send_error (connection, "No such option.");
      return;
    }

  if (!ok)
    {
      send_error (connection, error->message);
      return;
    }

  send_done (connection, NULL);

  /* Which model answers is part of what a chat is, so every device showing it
   * has to hear that it changed. */
  broadcast_event (self, "changed", chat_id, NULL, NULL);
}

/* --- changes made on this machine ------------------------------------------- */

/*
 * The daemon is not the only thing writing here.
 *
 * A window open on this machine works on the same database and the same
 * directories, and a message sent there is as real as one sent from a phone.
 * Watching for it is what keeps the two the same: the database is watched for
 * writes, the tree for folders coming and going, and anything either of them
 * shows becomes an event like any other.
 *
 * Coalesced, because SQLite writes several times per statement and a git
 * checkout can touch a thousand directories.
 */
static gboolean
on_local_change_settled (gpointer user_data)
{
  XdRemoteServer *self = user_data;

  self->local_change_id = 0;

  broadcast_tree (self);
  broadcast_event (self, "changed", NULL, NULL, NULL);

  return G_SOURCE_REMOVE;
}

static void
queue_local_change (XdRemoteServer *self)
{
  g_clear_handle_id (&self->local_change_id, g_source_remove);
  self->local_change_id = g_timeout_add (LOCAL_CHANGE_DEBOUNCE_MS,
                                         on_local_change_settled, self);
}

static void
on_local_change (GFileMonitor      *monitor,
                 GFile             *file,
                 GFile             *other_file,
                 GFileMonitorEvent  event,
                 gpointer           user_data)
{
  XdRemoteServer *self = user_data;

  queue_local_change (self);
}

static void
on_local_storage_change (XdStorage *storage,
                         gpointer   user_data)
{
  queue_local_change (user_data);
}

static void
watch_for_local_changes (XdRemoteServer *self)
{
  g_autoptr (GError) error = NULL;

  /* The database directory rather than the file: in WAL mode the writes land
   * beside it, in a journal the file itself never mentions. */
  /* The database says when it changes; everyone reading it listens the same
   * way, this side included. */
  xd_storage_watch (self->storage);
  g_signal_connect (self->storage, "changed",
                    G_CALLBACK (on_local_storage_change), self);

  {
    g_autoptr (GFile) file = g_file_new_for_path (self->root_path);

    self->tree_watch = g_file_monitor_directory (file, G_FILE_MONITOR_WATCH_MOVES,
                                                 NULL, &error);
    if (self->tree_watch == NULL)
      g_warning ("cannot watch the workspaces: %s", error->message);
    else
      g_signal_connect (self->tree_watch, "changed",
                        G_CALLBACK (on_local_change), self);
  }
}

/* --- the conversation ------------------------------------------------------ */

static void
connection_close (Connection *connection)
{
  if (connection->closed)
    return;

  connection->closed = TRUE;
  connection->authed = FALSE;

  /* NULL when the server went first and let its connections know. */
  if (connection->server != NULL)
    g_ptr_array_remove_fast (connection->server->connections, connection);

  /* Before the close, never after: the point is that no source is left
   * polling the fd this is about to take away. */
  g_cancellable_cancel (connection->cancellable);

  if (connection->stream != NULL)
    g_io_stream_close (connection->stream, NULL, NULL);
}

static void
connection_free (Connection *connection)
{
  connection_close (connection);
  connection_unref (connection);
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
  else if (g_strcmp0 (op, "image-read") == 0)
    handle_image_read (connection, request);
  else if (g_strcmp0 (op, "list-dir") == 0)
    handle_list_dir (connection, request);
  else if (g_strcmp0 (op, "file-browse") == 0)
    handle_file_browse (connection, request);
  else if (g_strcmp0 (op, "agent-secrets") == 0)
    handle_agent_secrets (connection);
  else if (g_strcmp0 (op, "set-agent-secrets") == 0)
    handle_set_agent_secrets (connection, request);
  /* The daemon is the only writer: a client sends what it wants done and this
   * is where it is done, so two of them acting at once are ordered here rather
   * than racing in the database. */
  else if (g_strcmp0 (op, "new-folder") == 0)
    handle_new_folder (connection, request);
  else if (g_strcmp0 (op, "rename-folder") == 0)
    handle_rename_folder (connection, request);
  else if (g_strcmp0 (op, "move-folder") == 0)
    handle_move_folder (connection, request);
  else if (g_strcmp0 (op, "trash-folder") == 0)
    handle_trash_folder (connection, request);
  else if (g_strcmp0 (op, "folder-context") == 0)
    handle_folder_context (connection, request);
  else if (g_strcmp0 (op, "set-folder-context") == 0)
    handle_set_folder_context (connection, request);
  else if (g_strcmp0 (op, "new-chat") == 0)
    handle_new_chat (connection, request);
  else if (g_strcmp0 (op, "rename-chat") == 0)
    handle_rename_chat (connection, request);
  else if (g_strcmp0 (op, "delete-chat") == 0)
    handle_delete_chat (connection, request);
  else if (g_strcmp0 (op, "chat") == 0)
    handle_chat (connection, request);
  else if (g_strcmp0 (op, "set-option") == 0)
    handle_set_option (connection, request);
  else if (g_strcmp0 (op, "send") == 0)
    handle_send (connection, request);
  else if (g_strcmp0 (op, "queue") == 0)
    handle_queue (connection, request, FALSE);
  else if (g_strcmp0 (op, "drop-queue") == 0)
    handle_queue (connection, request, TRUE);
  else if (g_strcmp0 (op, "cancel") == 0)
    handle_cancel (connection, request);
  else if (g_strcmp0 (op, "diff-read") == 0)
    handle_diff_read (connection, request);
  else if (g_strcmp0 (op, "terminal-list") == 0)
    handle_terminal_list (connection, request);
  else if (g_strcmp0 (op, "terminal-open") == 0)
    handle_terminal_open (connection, request);
  else if (g_strcmp0 (op, "terminal-input") == 0)
    handle_terminal_input (connection, request);
  else if (g_strcmp0 (op, "terminal-resize") == 0)
    handle_terminal_resize (connection, request);
  else if (g_strcmp0 (op, "terminal-kill") == 0)
    handle_terminal_kill (connection, request);
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

  /* The daemon is going, or this socket is: either way there is nobody left to
   * answer for, and reading on would be reading for a server that is gone. */
  if (line == NULL || connection->server == NULL)
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
                                       connection->cancellable,
                                       on_line_read, connection);
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
  XdRemoteServer *self = user_data;
  Connection *connection;
  g_autoptr (GIOStream) tls = NULL;

  tls = g_tls_server_connection_new (G_IO_STREAM (socket_connection),
                                     self->certificate, NULL);
  if (tls == NULL)
    return TRUE;

  connection = g_new0 (Connection, 1);
  connection->refs = 1;
  connection->server = self;
  connection->cancellable = g_cancellable_new ();
  connection->outgoing = g_queue_new ();
  g_ptr_array_add (self->connections, connection);
  connection->stream = g_object_ref (tls);
  connection->in = g_data_input_stream_new (g_io_stream_get_input_stream (tls));
  connection->out = g_io_stream_get_output_stream (tls);

  g_tls_connection_handshake_async (G_TLS_CONNECTION (tls), G_PRIORITY_DEFAULT,
                                    connection->cancellable, on_handshake,
                                    connection);

  return TRUE;
}

/* --- lifecycle ------------------------------------------------------------- */

XdRemoteServer *
xd_remote_server_new (XdStorage        *storage,
                      const char       *root_path,
                      guint16           port,
                      GTlsCertificate  *certificate,
                      GError          **error)
{
  g_autoptr (XdRemoteServer) self = NULL;

  g_return_val_if_fail (XD_IS_STORAGE (storage), NULL);
  g_return_val_if_fail (root_path != NULL, NULL);
  g_return_val_if_fail (G_IS_TLS_CERTIFICATE (certificate), NULL);

  self = g_object_new (XD_TYPE_REMOTE_SERVER, NULL);
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

  watch_for_local_changes (self);

  return g_steal_pointer (&self);
}

guint16
xd_remote_server_get_port (XdRemoteServer *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_SERVER (self), 0);

  return self->port;
}

void
xd_remote_server_quiesce_async (XdRemoteServer     *self,
                                GCancellable       *cancellable,
                                GAsyncReadyCallback callback,
                                gpointer            user_data)
{
  g_autoptr (GPtrArray) chat_ids = NULL;
  g_autoptr (GError) error = NULL;
  GHashTableIter iter;
  gpointer key;
  gpointer value;

  g_return_if_fail (XD_IS_REMOTE_SERVER (self));

  if (self->quiescing)
    {
      g_task_report_new_error (
        self, callback, user_data, xd_remote_server_quiesce_async,
        G_IO_ERROR, G_IO_ERROR_PENDING,
        "The daemon is already preparing to restart.");
      return;
    }

  chat_ids = g_ptr_array_new_with_free_func (g_free);
  g_hash_table_iter_init (&iter, self->turns);
  while (g_hash_table_iter_next (&iter, &key, NULL))
    g_ptr_array_add (chat_ids, g_strdup (key));

  if (!xd_storage_mark_resumes (self->storage, chat_ids, &error))
    {
      g_task_report_error (
        self, callback, user_data, xd_remote_server_quiesce_async,
        g_steal_pointer (&error));
      return;
    }

  self->quiescing = TRUE;
  self->quiesce_task = g_task_new (self, cancellable, callback, user_data);
  g_task_set_source_tag (self->quiesce_task,
                         xd_remote_server_quiesce_async);

  g_hash_table_iter_init (&iter, self->turns);
  while (g_hash_table_iter_next (&iter, NULL, &value))
    xd_daemon_turn_cancel (value);

  complete_quiesce (self);
}

gboolean
xd_remote_server_quiesce_finish (XdRemoteServer *self,
                                 GAsyncResult   *result,
                                 GError        **error)
{
  g_return_val_if_fail (XD_IS_REMOTE_SERVER (self), FALSE);
  g_return_val_if_fail (g_task_is_valid (result, self), FALSE);

  return g_task_propagate_boolean (G_TASK (result), error);
}

gboolean
xd_remote_server_resume_interrupted (XdRemoteServer *self,
                                     GError        **error)
{
  g_autoptr (GPtrArray) chat_ids = NULL;
  g_autoptr (GError) first_error = NULL;

  g_return_val_if_fail (XD_IS_REMOTE_SERVER (self), FALSE);

  chat_ids = xd_storage_take_resumes (self->storage, error);
  if (chat_ids == NULL)
    return FALSE;

  self->quiescing = FALSE;

  for (guint i = 0; i < chat_ids->len; i++)
    {
      const char *chat_id = g_ptr_array_index (chat_ids, i);
      g_autoptr (GError) start_error = NULL;

      if (!start_daemon_turn (
            self, chat_id,
            "Resume the work interrupted by the daemon update.",
            &start_error))
        {
          g_warning ("cannot resume chat %s after update: %s",
                     chat_id, start_error->message);
          if (first_error == NULL)
            first_error = g_steal_pointer (&start_error);
        }
    }

  if (first_error != NULL)
    {
      g_propagate_error (error, g_steal_pointer (&first_error));
      return FALSE;
    }

  return TRUE;
}

static void
xd_remote_server_dispose (GObject *object)
{
  XdRemoteServer *self = XD_REMOTE_SERVER (object);

  if (self->service != NULL)
    g_socket_service_stop (self->service);

  if (self->quiesce_task != NULL)
    {
      g_task_return_new_error (self->quiesce_task, G_IO_ERROR,
                               G_IO_ERROR_CANCELLED,
                               "The daemon stopped before it quiesced.");
      g_clear_object (&self->quiesce_task);
    }

  /*
   * The connections outlive this object otherwise.
   *
   * Each one is owned by the read it has in flight, which finishes long after
   * the server has been let go -- and answers by reaching back into it. Closing
   * the stream ends that read, and the missing server is how the connection
   * knows not to look for one.
   */
  if (self->connections != NULL)
    {
      for (guint i = 0; i < self->connections->len; i++)
        {
          Connection *connection = g_ptr_array_index (self->connections, i);

          connection->server = NULL;
          connection->authed = FALSE;
          connection->closed = TRUE;

          /* connection_close would remove from the array being walked, so it
           * is done by hand here -- including stopping the pending reads. */
          g_cancellable_cancel (connection->cancellable);

          if (connection->stream != NULL)
            g_io_stream_close (connection->stream, NULL, NULL);
        }
    }

  g_clear_handle_id (&self->local_change_id, g_source_remove);
  g_clear_object (&self->tree_watch);
  if (self->storage != NULL)
    g_signal_handlers_disconnect_by_data (self->storage, self);
  g_clear_pointer (&self->turns, g_hash_table_unref);
  g_clear_pointer (&self->command_sets, g_hash_table_unref);
  if (self->terminals != NULL)
    {
      GHashTableIter iter;
      gpointer value;

      g_hash_table_iter_init (&iter, self->terminals);
      while (g_hash_table_iter_next (&iter, NULL, &value))
        {
          XdRemoteTerminal *terminal = value;

          g_signal_handlers_disconnect_by_data (terminal, self);
          xd_remote_terminal_close (terminal);
        }
    }
  g_clear_pointer (&self->terminals, g_hash_table_unref);
  g_clear_pointer (&self->connections, g_ptr_array_unref);
  g_clear_object (&self->service);
  g_clear_object (&self->certificate);
  g_clear_object (&self->storage);
  g_clear_pointer (&self->root_path, g_free);
  g_clear_pointer (&self->pairing_code, g_free);

  G_OBJECT_CLASS (xd_remote_server_parent_class)->dispose (object);
}

static void
xd_remote_server_class_init (XdRemoteServerClass *klass)
{
  G_OBJECT_CLASS (klass)->dispose = xd_remote_server_dispose;
}

static void
xd_remote_server_init (XdRemoteServer *self)
{
  self->connections = g_ptr_array_new ();
  self->turns = g_hash_table_new_full (g_str_hash, g_str_equal,
                                       g_free, g_object_unref);
  self->command_sets = g_hash_table_new_full (
    g_str_hash, g_str_equal, g_free, (GDestroyNotify) g_strfreev);
  self->terminals = g_hash_table_new_full (g_str_hash, g_str_equal,
                                           g_free, g_object_unref);
}
