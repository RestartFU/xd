#include "discord-presence.h"

#include <json-glib/json-glib.h>
#include <string.h>

#ifdef G_OS_WIN32
#include <windows.h>
#else
#include <errno.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#endif

#define DISCORD_APPLICATION_ID "1531361363522490489"
#define DISCORD_RETRY_USEC     (15 * G_USEC_PER_SEC)
#define DISCORD_IO_TIMEOUT_MS  2000
#define DISCORD_MAX_FRAME      (1024 * 1024)

typedef enum
{
  DISCORD_OPCODE_HANDSHAKE = 0,
  DISCORD_OPCODE_FRAME = 1,
  DISCORD_OPCODE_CLOSE = 2,
  DISCORD_OPCODE_PING = 3,
  DISCORD_OPCODE_PONG = 4,
} DiscordOpcode;

typedef struct
{
  gboolean stop;
  char *state;
} PresenceMessage;

typedef struct
{
#ifdef G_OS_WIN32
  HANDLE handle;
#else
  int fd;
#endif
} DiscordConnection;

struct _XdDiscordPresence
{
  GAsyncQueue *messages;
  GThread *thread;
  gint64 started_at;
};

static PresenceMessage *
presence_message_new (const char *state)
{
  PresenceMessage *message = g_new0 (PresenceMessage, 1);

  message->state = g_strdup (state);
  return message;
}

static void
presence_message_free (PresenceMessage *message)
{
  g_free (message->state);
  g_free (message);
}

static char *
json_builder_to_string (JsonBuilder *builder)
{
  g_autoptr (JsonGenerator) generator = json_generator_new ();
  JsonNode *root = json_builder_get_root (builder);
  char *json;

  json_generator_set_root (generator, root);
  json = json_generator_to_data (generator, NULL);
  json_node_free (root);

  return json;
}

char *
xd_discord_presence_build_activity (const char *state,
                                    gint64      started_at,
                                    guint32     process_id,
                                    guint64     nonce)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autofree char *nonce_string = g_strdup_printf ("%" G_GUINT64_FORMAT, nonce);

  g_return_val_if_fail (state != NULL && *state != '\0', NULL);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "cmd");
  json_builder_add_string_value (builder, "SET_ACTIVITY");
  json_builder_set_member_name (builder, "args");
  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "pid");
  json_builder_add_int_value (builder, process_id);
  json_builder_set_member_name (builder, "activity");
  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "details");
  json_builder_add_string_value (builder, "Building with AI");
  json_builder_set_member_name (builder, "state");
  json_builder_add_string_value (builder, state);
  json_builder_set_member_name (builder, "timestamps");
  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "start");
  json_builder_add_int_value (builder, started_at);
  json_builder_end_object (builder);
  json_builder_end_object (builder);
  json_builder_end_object (builder);
  json_builder_set_member_name (builder, "nonce");
  json_builder_add_string_value (builder, nonce_string);
  json_builder_end_object (builder);

  return json_builder_to_string (builder);
}

static char *
build_handshake (void)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "v");
  json_builder_add_int_value (builder, 1);
  json_builder_set_member_name (builder, "client_id");
  json_builder_add_string_value (builder, DISCORD_APPLICATION_ID);
  json_builder_end_object (builder);

  return json_builder_to_string (builder);
}

static char *
build_clear_activity (guint32 process_id,
                      guint64 nonce)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autofree char *nonce_string = g_strdup_printf ("%" G_GUINT64_FORMAT, nonce);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "cmd");
  json_builder_add_string_value (builder, "SET_ACTIVITY");
  json_builder_set_member_name (builder, "args");
  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "pid");
  json_builder_add_int_value (builder, process_id);
  json_builder_set_member_name (builder, "activity");
  json_builder_add_null_value (builder);
  json_builder_end_object (builder);
  json_builder_set_member_name (builder, "nonce");
  json_builder_add_string_value (builder, nonce_string);
  json_builder_end_object (builder);

  return json_builder_to_string (builder);
}

static void
connection_init (DiscordConnection *connection)
{
#ifdef G_OS_WIN32
  connection->handle = INVALID_HANDLE_VALUE;
#else
  connection->fd = -1;
#endif
}

static gboolean
connection_is_open (DiscordConnection *connection)
{
#ifdef G_OS_WIN32
  return connection->handle != INVALID_HANDLE_VALUE;
#else
  return connection->fd >= 0;
#endif
}

static void
connection_close (DiscordConnection *connection)
{
#ifdef G_OS_WIN32
  if (connection->handle != INVALID_HANDLE_VALUE)
    CloseHandle (connection->handle);
  connection->handle = INVALID_HANDLE_VALUE;
#else
  if (connection->fd >= 0)
    close (connection->fd);
  connection->fd = -1;
#endif
}

#ifdef G_OS_WIN32

static gboolean
connection_open (DiscordConnection *connection)
{
  for (guint i = 0; i < 10; i++)
    {
      g_autofree gunichar2 *pipe_name = NULL;
      g_autofree char *utf8_name =
        g_strdup_printf ("\\\\?\\pipe\\discord-ipc-%u", i);
      DWORD mode = PIPE_READMODE_BYTE;

      pipe_name = g_utf8_to_utf16 (utf8_name, -1, NULL, NULL, NULL);
      if (!WaitNamedPipeW ((LPCWSTR) pipe_name, 50))
        continue;

      connection->handle =
        CreateFileW ((LPCWSTR) pipe_name,
                     GENERIC_READ | GENERIC_WRITE,
                     0, NULL, OPEN_EXISTING, 0, NULL);
      if (connection->handle == INVALID_HANDLE_VALUE)
        continue;

      if (SetNamedPipeHandleState (connection->handle, &mode, NULL, NULL))
        return TRUE;

      connection_close (connection);
    }

  return FALSE;
}

static gboolean
connection_write_all (DiscordConnection *connection,
                      const guint8      *data,
                      gsize              length)
{
  while (length > 0)
    {
      DWORD written = 0;

      if (!WriteFile (connection->handle, data,
                      (DWORD) MIN (length, G_MAXUINT32),
                      &written, NULL) ||
          written == 0)
        return FALSE;

      data += written;
      length -= written;
    }

  return TRUE;
}

static gboolean
connection_read_all (DiscordConnection *connection,
                     guint8            *data,
                     gsize              length)
{
  gint64 deadline =
    g_get_monotonic_time () + DISCORD_IO_TIMEOUT_MS * G_TIME_SPAN_MILLISECOND;

  while (length > 0)
    {
      DWORD available = 0;
      DWORD read = 0;

      if (!PeekNamedPipe (connection->handle, NULL, 0, NULL, &available, NULL))
        return FALSE;
      if (available == 0)
        {
          if (g_get_monotonic_time () >= deadline)
            return FALSE;
          g_usleep (10 * G_TIME_SPAN_MILLISECOND);
          continue;
        }

      if (!ReadFile (connection->handle, data,
                     (DWORD) MIN (length, (gsize) available),
                     &read, NULL) ||
          read == 0)
        return FALSE;

      data += read;
      length -= read;
    }

  return TRUE;
}

static guint32
current_process_id (void)
{
  return GetCurrentProcessId ();
}

#else

static gboolean
try_unix_socket (DiscordConnection *connection,
                 const char        *directory,
                 guint              index)
{
  struct sockaddr_un address = { 0 };
  g_autofree char *path = NULL;
  int fd;

  if (directory == NULL || *directory == '\0')
    return FALSE;

  path = g_strdup_printf ("%s/discord-ipc-%u", directory, index);
  if (strlen (path) >= sizeof address.sun_path)
    return FALSE;

  fd = socket (AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0)
    return FALSE;

  address.sun_family = AF_UNIX;
  g_strlcpy (address.sun_path, path, sizeof address.sun_path);

  if (connect (fd, (struct sockaddr *) &address, sizeof address) != 0)
    {
      close (fd);
      return FALSE;
    }

#ifdef SO_NOSIGPIPE
  {
    int enabled = 1;

    setsockopt (fd, SOL_SOCKET, SO_NOSIGPIPE, &enabled, sizeof enabled);
  }
#endif

  connection->fd = fd;
  return TRUE;
}

static gboolean
connection_open (DiscordConnection *connection)
{
  const char *directories[] = {
    g_getenv ("XDG_RUNTIME_DIR"),
    g_getenv ("TMPDIR"),
    g_getenv ("TMP"),
    g_getenv ("TEMP"),
    "/tmp",
  };

  for (guint i = 0; i < G_N_ELEMENTS (directories); i++)
    {
      if (directories[i] == NULL)
        continue;

      for (guint previous = 0; previous < i; previous++)
        if (g_strcmp0 (directories[i], directories[previous]) == 0)
          goto next_directory;

      for (guint socket_index = 0; socket_index < 10; socket_index++)
        if (try_unix_socket (connection, directories[i], socket_index))
          return TRUE;

next_directory:
      continue;
    }

  return FALSE;
}

static gboolean
wait_for_fd (int   fd,
             short events)
{
  struct pollfd poll_fd = {
    .fd = fd,
    .events = events,
  };
  int result;

  do
    result = poll (&poll_fd, 1, DISCORD_IO_TIMEOUT_MS);
  while (result < 0 && errno == EINTR);

  return result > 0 && (poll_fd.revents & events) != 0;
}

static gboolean
connection_write_all (DiscordConnection *connection,
                      const guint8      *data,
                      gsize              length)
{
  while (length > 0)
    {
      ssize_t written;

      if (!wait_for_fd (connection->fd, POLLOUT))
        return FALSE;

      written = send (connection->fd, data, length,
#ifdef MSG_NOSIGNAL
                      MSG_NOSIGNAL
#else
                      0
#endif
                      );
      if (written < 0 && errno == EINTR)
        continue;
      if (written <= 0)
        return FALSE;

      data += written;
      length -= written;
    }

  return TRUE;
}

static gboolean
connection_read_all (DiscordConnection *connection,
                     guint8            *data,
                     gsize              length)
{
  while (length > 0)
    {
      ssize_t bytes_read;

      if (!wait_for_fd (connection->fd, POLLIN))
        return FALSE;

      bytes_read = read (connection->fd, data, length);
      if (bytes_read < 0 && errno == EINTR)
        continue;
      if (bytes_read <= 0)
        return FALSE;

      data += bytes_read;
      length -= bytes_read;
    }

  return TRUE;
}

static guint32
current_process_id (void)
{
  return (guint32) getpid ();
}

#endif

static gboolean
send_frame (DiscordConnection *connection,
            DiscordOpcode      opcode,
            const char        *payload)
{
  guint32 header[2];
  gsize length = strlen (payload);

  if (length > DISCORD_MAX_FRAME)
    return FALSE;

  header[0] = GUINT32_TO_LE ((guint32) opcode);
  header[1] = GUINT32_TO_LE ((guint32) length);

  return connection_write_all (connection, (guint8 *) header, sizeof header) &&
         connection_write_all (connection, (const guint8 *) payload, length);
}

static gboolean
read_frame (DiscordConnection *connection,
            DiscordOpcode     *opcode,
            char             **payload)
{
  guint32 header[2];
  guint32 length;
  char *data;

  if (!connection_read_all (connection, (guint8 *) header, sizeof header))
    return FALSE;

  *opcode = (DiscordOpcode) GUINT32_FROM_LE (header[0]);
  length = GUINT32_FROM_LE (header[1]);
  if (length > DISCORD_MAX_FRAME)
    return FALSE;

  data = g_malloc (length + 1);
  if (!connection_read_all (connection, (guint8 *) data, length))
    {
      g_free (data);
      return FALSE;
    }

  data[length] = '\0';
  *payload = data;
  return TRUE;
}

static gboolean
read_reply (DiscordConnection *connection)
{
  for (guint i = 0; i < 8; i++)
    {
      DiscordOpcode opcode;
      g_autofree char *payload = NULL;

      if (!read_frame (connection, &opcode, &payload))
        return FALSE;

      if (opcode == DISCORD_OPCODE_PING)
        {
          if (!send_frame (connection, DISCORD_OPCODE_PONG, payload))
            return FALSE;
          continue;
        }

      if (opcode == DISCORD_OPCODE_FRAME)
        {
          g_autoptr (JsonParser) parser = json_parser_new ();
          JsonNode *root;
          JsonObject *object;

          if (!json_parser_load_from_data (parser, payload, -1, NULL))
            return FALSE;

          root = json_parser_get_root (parser);
          if (!JSON_NODE_HOLDS_OBJECT (root))
            return FALSE;

          object = json_node_get_object (root);
          return !json_object_has_member (object, "evt") ||
                 g_strcmp0 (json_object_get_string_member (object, "evt"),
                            "ERROR") != 0;
        }

      return FALSE;
    }

  return FALSE;
}

static gboolean
connect_and_handshake (DiscordConnection *connection)
{
  g_autofree char *handshake = NULL;

  if (!connection_open (connection))
    return FALSE;

  handshake = build_handshake ();
  if (send_frame (connection, DISCORD_OPCODE_HANDSHAKE, handshake) &&
      read_reply (connection))
    return TRUE;

  connection_close (connection);
  return FALSE;
}

static gboolean
send_activity (DiscordConnection *connection,
               const char        *state,
               gint64             started_at,
               guint64            nonce)
{
  g_autofree char *activity =
    xd_discord_presence_build_activity (state, started_at,
                                        current_process_id (), nonce);

  return send_frame (connection, DISCORD_OPCODE_FRAME, activity) &&
         read_reply (connection);
}

static gpointer
presence_thread (gpointer data)
{
  XdDiscordPresence *self = data;
  DiscordConnection connection;
  g_autofree char *state = g_strdup ("Browsing workspaces");
  guint64 nonce = 1;
  gboolean stop = FALSE;

  connection_init (&connection);

  while (!stop)
    {
      PresenceMessage *message =
        g_async_queue_timeout_pop (self->messages, DISCORD_RETRY_USEC);

      if (message != NULL)
        {
          do
            {
              if (message->stop)
                stop = TRUE;
              else
                {
                  g_free (state);
                  state = g_strdup (message->state);
                }

              presence_message_free (message);
              message = g_async_queue_try_pop (self->messages);
            }
          while (message != NULL);
        }

      if (stop)
        break;

      if (!connection_is_open (&connection) &&
          !connect_and_handshake (&connection))
        continue;

      if (!send_activity (&connection, state, self->started_at, nonce++))
        connection_close (&connection);
    }

  if (connection_is_open (&connection))
    {
      g_autofree char *clear =
        build_clear_activity (current_process_id (), nonce);

      send_frame (&connection, DISCORD_OPCODE_FRAME, clear);
      connection_close (&connection);
    }

  return NULL;
}

XdDiscordPresence *
xd_discord_presence_new (void)
{
  XdDiscordPresence *self = g_new0 (XdDiscordPresence, 1);

  self->messages = g_async_queue_new_full (
    (GDestroyNotify) presence_message_free);
  self->started_at = g_get_real_time () / G_USEC_PER_SEC;
  self->thread = g_thread_new ("discord-presence", presence_thread, self);

  g_async_queue_push (self->messages,
                      presence_message_new ("Browsing workspaces"));
  return self;
}

void
xd_discord_presence_set_state (XdDiscordPresence *self,
                               const char        *state)
{
  g_return_if_fail (self != NULL);
  g_return_if_fail (state != NULL && *state != '\0');

  g_async_queue_push (self->messages, presence_message_new (state));
}

void
xd_discord_presence_free (XdDiscordPresence *self)
{
  PresenceMessage *message;

  if (self == NULL)
    return;

  message = presence_message_new (NULL);
  message->stop = TRUE;
  g_async_queue_push (self->messages, message);

  g_thread_join (self->thread);
  g_async_queue_unref (self->messages);
  g_free (self);
}
