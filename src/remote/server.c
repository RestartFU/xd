#include "server.h"

#include "backend/backend.h"
#include "chat/chat-title.h"
#include "remote/turn.h"
#include "settings/folder-settings.h"

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
  XdRemoteServer *server;      /* unowned; the server outlives connections */
  GDataInputStream *in;
  GOutputStream *out;
  GIOStream *stream;
  gboolean authed;
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

  /* Changes made by anything else on this machine -- the window open here,
   * most of the time -- and the delay that coalesces them. */
  GFileMonitor *database_watch;
  GFileMonitor *tree_watch;
  guint local_change_id;

  char *pairing_code;
  gint64 pairing_expires;      /* monotonic microseconds */
};

/* SQLite writes several times per statement, and a checkout touches everything
 * at once; one event out the far side is enough. */
#define LOCAL_CHANGE_DEBOUNCE_MS 400

G_DEFINE_FINAL_TYPE (XdRemoteServer, xd_remote_server, G_TYPE_OBJECT)

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

  for (guint i = 0; i < self->connections->len; i++)
    {
      Connection *connection = g_ptr_array_index (self->connections, i);

      if (!connection->authed)
        continue;

      /* Errors are ignored on purpose: a connection that cannot be written to
       * is one whose read side is about to notice the same thing and clean
       * itself up. */
      g_output_stream_write_all (connection->out, text, length, NULL, NULL, NULL);
      g_output_stream_write_all (connection->out, "\n", 1, NULL, NULL, NULL);
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

  /* No working directory: the chat inherits the folder's, which is resolved
   * on this side too. */
  chat_id = xd_storage_create_chat (self->storage, folder_id,
                                    title != NULL ? title : XD_CHAT_UNTITLED,
                                    backend, model, effort, NULL, &error);
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

  send_done (connection, NULL);
  broadcast_tree (connection->server);
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

  rows = xd_storage_list_messages (connection->server->storage, chat_id, NULL);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);
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

/* --- running a turn -------------------------------------------------------- */

typedef struct
{
  XdRemoteServer *server;   /* unowned; the server outlives its turns */
  char *chat_id;
} Running;

static void
running_free (Running *running)
{
  g_free (running->chat_id);
  g_free (running);
}

static void
on_turn_text (XdDaemonTurn *turn,
              const char   *delta,
              gpointer      user_data)
{
  Running *running = user_data;

  broadcast_event (running->server, "text", running->chat_id, "text", delta);
}

static void
on_turn_tool (XdDaemonTurn *turn,
              const char   *name,
              gpointer      user_data)
{
  Running *running = user_data;

  broadcast_event (running->server, "tool", running->chat_id, "text", name);
}

/* The turn is over, but it is over inside one of its own signals: dropping it
 * here would take the session down while it is still emitting. */
static gboolean
forget_turn (gpointer user_data)
{
  Running *running = user_data;

  g_hash_table_remove (running->server->turns, running->chat_id);
  running_free (running);

  return G_SOURCE_REMOVE;
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
  if (message != NULL)
    {
      json_builder_set_member_name (builder, "error");
      json_builder_add_string_value (builder, message);
    }
  json_builder_end_object (builder);

  broadcast (running->server, builder);

  g_idle_add (forget_turn, running);
}

static void
handle_send (Connection *connection,
             JsonObject *request)
{
  XdRemoteServer *self = connection->server;
  const char *chat_id = member_string (request, "chat");
  const char *text = member_string (request, "text");
  g_autoptr (GError) error = NULL;
  XdDaemonTurn *turn;
  Running *running;

  if (chat_id == NULL || text == NULL || *text == '\0')
    {
      send_error (connection, "A message needs a chat and something to say.");
      return;
    }

  /* One turn per chat, enforced here rather than by each client: two devices
   * sending at once would otherwise be two agents in the same directory. */
  if (g_hash_table_contains (self->turns, chat_id))
    {
      send_error (connection, "That chat is already working.");
      return;
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

  g_signal_connect (turn, "text", G_CALLBACK (on_turn_text), running);
  g_signal_connect (turn, "tool", G_CALLBACK (on_turn_tool), running);
  g_signal_connect (turn, "finished", G_CALLBACK (on_turn_finished), running);

  if (!xd_daemon_turn_start (turn, chat_id, text, &error))
    {
      send_error (connection, error->message);
      running_free (running);
      g_object_unref (turn);

      /* The message and the failure are both in the transcript now. */
      broadcast_event (self, "changed", chat_id, NULL, NULL);
      return;
    }

  g_hash_table_insert (self->turns, g_strdup (chat_id), turn);

  send_done (connection, NULL);

  /* Everyone watching sees the message arrive and the work start, including
   * the device that sent it -- one path, so every screen agrees. */
  broadcast_event (self, "turn-started", chat_id, "label",
                   xd_daemon_turn_get_label (turn));
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

  send_done (connection, NULL);
}

/* --- one chat's settings ---------------------------------------------------- */

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

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "ok");
  json_builder_add_boolean_value (builder, TRUE);

  json_builder_set_member_name (builder, "title");
  json_builder_add_string_value (builder, chat->title);
  json_builder_set_member_name (builder, "backend");
  json_builder_add_string_value (builder, chat->backend);
  json_builder_set_member_name (builder, "plan");
  json_builder_add_boolean_value (builder, chat->plan);
  json_builder_set_member_name (builder, "working");
  json_builder_add_boolean_value (builder,
                                  g_hash_table_contains (self->turns, chat_id));

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

  /* Where it runs, resolved on this side: the folder chain that decides it is
   * here, and the client has no way to read it. */
  {
    g_autoptr (XdDaemonTurn) resolver = NULL;
    g_autofree char *workdir = NULL;

    resolver = xd_daemon_turn_new (self->storage, self->root_path);
    workdir = xd_daemon_turn_resolve_workdir (resolver, chat);

    if (workdir != NULL)
      {
        json_builder_set_member_name (builder, "workdir");
        json_builder_add_string_value (builder, workdir);
      }
  }

  json_builder_end_object (builder);

  send_json (connection, builder);
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
on_local_change (GFileMonitor      *monitor,
                 GFile             *file,
                 GFile             *other_file,
                 GFileMonitorEvent  event,
                 gpointer           user_data)
{
  XdRemoteServer *self = user_data;

  g_clear_handle_id (&self->local_change_id, g_source_remove);
  self->local_change_id = g_timeout_add (LOCAL_CHANGE_DEBOUNCE_MS,
                                         on_local_change_settled, self);
}

static void
watch_for_local_changes (XdRemoteServer *self)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *db_dir = NULL;

  /* The database directory rather than the file: in WAL mode the writes land
   * beside it, in a journal the file itself never mentions. */
  db_dir = g_path_get_dirname (xd_storage_get_path (self->storage));

  {
    g_autoptr (GFile) file = g_file_new_for_path (db_dir);

    self->database_watch = g_file_monitor_directory (file, G_FILE_MONITOR_NONE,
                                                     NULL, &error);
    if (self->database_watch == NULL)
      g_warning ("cannot watch the database: %s", error->message);
    else
      g_signal_connect (self->database_watch, "changed",
                        G_CALLBACK (on_local_change), self);
  }

  g_clear_error (&error);

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
connection_free (Connection *connection)
{
  /* NULL when the server went first and let its connections know. */
  if (connection->server != NULL)
    g_ptr_array_remove_fast (connection->server->connections, connection);

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
  else if (g_strcmp0 (op, "cancel") == 0)
    handle_cancel (connection, request);
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
  XdRemoteServer *self = user_data;
  Connection *connection;
  g_autoptr (GIOStream) tls = NULL;

  tls = g_tls_server_connection_new (G_IO_STREAM (socket_connection),
                                     self->certificate, NULL);
  if (tls == NULL)
    return TRUE;

  connection = g_new0 (Connection, 1);
  connection->server = self;
  g_ptr_array_add (self->connections, connection);
  connection->stream = g_object_ref (tls);
  connection->in = g_data_input_stream_new (g_io_stream_get_input_stream (tls));
  connection->out = g_io_stream_get_output_stream (tls);

  g_tls_connection_handshake_async (G_TLS_CONNECTION (tls), G_PRIORITY_DEFAULT,
                                    NULL, on_handshake, connection);

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

static void
xd_remote_server_dispose (GObject *object)
{
  XdRemoteServer *self = XD_REMOTE_SERVER (object);

  if (self->service != NULL)
    g_socket_service_stop (self->service);

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

          if (connection->stream != NULL)
            g_io_stream_close (connection->stream, NULL, NULL);
        }
    }

  g_clear_handle_id (&self->local_change_id, g_source_remove);
  g_clear_object (&self->database_watch);
  g_clear_object (&self->tree_watch);
  g_clear_pointer (&self->turns, g_hash_table_unref);
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
}
