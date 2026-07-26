#include "client.h"

#include <string.h>

/*
 * One connection, held open, and a queue of what has been asked over it.
 *
 * Everything here is asynchronous except the writes: a request is one short
 * line, which the socket takes without waiting, and the daemon's own side
 * writes its replies the same way.
 *
 * A dropped line is not an error the user has to act on -- a laptop sleeps,
 * a network changes -- so it is retried on a timer, and whoever is showing the
 * remote hears about it through ::opened and ::closed instead.
 */

/*
 * How long before a dropped connection is tried again.
 *
 * Flat rather than widening: a daemon is a machine the user knows about and
 * expects to come back -- a laptop opened, a network rejoined -- and a delay
 * that grew would leave the tree looking dead for minutes after it was there
 * again. One connection attempt every few seconds costs nothing on either end.
 */
#define RECONNECT_SECONDS 5

G_DEFINE_QUARK (xd-remote-error-quark, xd_remote_error)

typedef enum
{
  GREETING_HELLO,
  GREETING_PAIR,
} Greeting;

/*
 * One request waiting for its answer.
 *
 * A greeting is a call like any other, which is why pairing and saying hello
 * need no state of their own: they are the first thing written on a connection
 * and therefore the first thing answered.
 */
typedef struct
{
  GTask *task;          /* NULL for a hello nobody asked for */
  gboolean greeting;
} Call;

struct _XdRemoteClient
{
  GObject parent_instance;

  char *host;
  guint16 port;
  char *certificate_pem;
  char *token;
  char *device_name;

  GSocketClient *socket_client;
  GIOStream *stream;            /* the TLS connection, while there is one */
  GDataInputStream *in;
  GOutputStream *out;           /* unowned: it belongs to the stream */
  GCancellable *cancellable;    /* cancels this connection, not the client */

  GQueue *pending;              /* Call*, oldest first */

  Greeting greeting;
  char *pair_code;
  char *pair_name;
  GTask *pair_task;             /* parked until the greeting can carry it */

  gboolean running;             /* keep the line up */
  gboolean connected;           /* a greeting has been accepted */
  guint reconnect_id;
};

enum
{
  SIGNAL_OPENED,
  SIGNAL_CLOSED,
  SIGNAL_EVENT,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (XdRemoteClient, xd_remote_client, G_TYPE_OBJECT)

static void connect_now (XdRemoteClient *self);
static void read_next_line (XdRemoteClient *self);

/* --- calls ---------------------------------------------------------------- */

static void
call_free (Call *call)
{
  g_clear_object (&call->task);
  g_free (call);
}

static void
call_fail (Call   *call,
           GError *error)
{
  if (call->task != NULL)
    g_task_return_error (call->task, g_error_copy (error));

  call_free (call);
}

/*
 * Tears the connection down and tells everyone waiting on it why.
 *
 * Pending calls cannot be carried over to the next connection: the daemon
 * never saw some of them, and the ones it did answer in a reply we did not
 * read. Failing them is the honest outcome, and callers retry on ::opened.
 */
static void
drop_connection (XdRemoteClient *self,
                 GError         *why)
{
  gboolean was_connected = self->connected;
  Call *call;

  self->connected = FALSE;

  g_cancellable_cancel (self->cancellable);
  g_clear_object (&self->cancellable);
  self->cancellable = g_cancellable_new ();

  g_clear_object (&self->in);
  self->out = NULL;
  g_clear_object (&self->stream);

  while ((call = g_queue_pop_head (self->pending)) != NULL)
    call_fail (call, why);

  /* A pairing attempt that never got as far as its greeting. There is nothing
   * to retry with: a pairing code is spent by the connection that offered it,
   * whether or not that connection lasted long enough to hear back. */
  if (self->pair_task != NULL)
    {
      g_autoptr (GTask) task = g_steal_pointer (&self->pair_task);

      self->running = FALSE;
      self->greeting = GREETING_HELLO;
      g_task_return_error (task, g_error_copy (why));
    }

  if (was_connected)
    g_signal_emit (self, signals[SIGNAL_CLOSED], 0, why->message);
}

static gboolean
on_reconnect_elapsed (gpointer user_data)
{
  XdRemoteClient *self = user_data;

  self->reconnect_id = 0;
  connect_now (self);

  return G_SOURCE_REMOVE;
}

static void
schedule_reconnect (XdRemoteClient *self)
{
  if (!self->running || self->reconnect_id != 0)
    return;

  self->reconnect_id = g_timeout_add_seconds (RECONNECT_SECONDS,
                                              on_reconnect_elapsed, self);
}

static void
fail_connection (XdRemoteClient *self,
                 GError         *why)
{
  drop_connection (self, why);
  schedule_reconnect (self);
}

/* --- writing -------------------------------------------------------------- */

static void
send_call (XdRemoteClient *self,
           Call           *call,
           JsonNode       *request)
{
  g_autoptr (JsonGenerator) generator = json_generator_new ();
  g_autoptr (GError) error = NULL;
  g_autofree char *text = NULL;
  gsize length;

  g_queue_push_tail (self->pending, call);

  json_generator_set_root (generator, request);
  text = json_generator_to_data (generator, &length);

  /* One short line, which the socket buffer takes without blocking; the
   * daemon writes its side of the conversation the same way. */
  if (!g_output_stream_write_all (self->out, text, length, NULL, NULL, &error) ||
      !g_output_stream_write_all (self->out, "\n", 1, NULL, NULL, &error))
    fail_connection (self, error);
}

static void
send_greeting (XdRemoteClient *self)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;
  Call *call = g_new0 (Call, 1);

  call->greeting = TRUE;

  /* Whoever asked to pair is waiting on this exact request, so the task rides
   * along with it and is answered by the daemon's reply to the greeting. */
  if (self->greeting == GREETING_PAIR)
    call->task = g_steal_pointer (&self->pair_task);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");

  if (self->greeting == GREETING_PAIR)
    {
      json_builder_add_string_value (builder, "pair");
      json_builder_set_member_name (builder, "code");
      json_builder_add_string_value (builder, self->pair_code);
      json_builder_set_member_name (builder, "name");
      json_builder_add_string_value (builder, self->pair_name);
    }
  else
    {
      json_builder_add_string_value (builder, "hello");
      json_builder_set_member_name (builder, "token");
      json_builder_add_string_value (builder, self->token);
    }

  json_builder_end_object (builder);
  request = json_builder_get_root (builder);

  send_call (self, call, request);
}

/* --- reading -------------------------------------------------------------- */

static void
complete_call (XdRemoteClient *self,
               Call           *call,
               JsonObject     *reply)
{
  gboolean ok = json_object_get_boolean_member_with_default (reply, "ok", FALSE);
  g_autoptr (GError) error = NULL;

  if (!ok)
    {
      const char *message =
        json_object_get_string_member_with_default (reply, "error", NULL);

      error = g_error_new_literal (XD_REMOTE_ERROR, XD_REMOTE_ERROR_REFUSED,
                                   message != NULL ? message
                                                   : "The daemon refused the request.");
    }

  if (!call->greeting)
    {
      if (error != NULL)
        call_fail (call, error);
      else if (call->task != NULL)
        g_task_return_pointer (call->task, json_object_ref (reply),
                               (GDestroyNotify) json_object_unref);
      else
        call_free (call);

      return;
    }

  /* A refused greeting is a wrong token or a spent pairing code, and neither
   * gets better by trying again: there is nothing to reconnect for. */
  if (error != NULL)
    {
      self->running = FALSE;
      call_fail (call, error);
      drop_connection (self, error);
      return;
    }

  if (json_object_has_member (reply, "token"))
    {
      g_free (self->token);
      self->token = g_strdup (json_object_get_string_member (reply, "token"));
    }

  if (json_object_has_member (reply, "device"))
    {
      g_free (self->device_name);
      self->device_name = g_strdup (json_object_get_string_member (reply, "device"));
    }

  /* Pairing is a greeting of its own, so from here on the connection is an
   * ordinary one and is kept up like any other. */
  self->greeting = GREETING_HELLO;
  self->connected = TRUE;

  if (call->task != NULL)
    g_task_return_boolean (call->task, TRUE);
  call_free (call);

  g_signal_emit (self, signals[SIGNAL_OPENED], 0);
}

static void
on_line_read (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  g_autoptr (XdRemoteClient) self = user_data;
  g_autoptr (GError) error = NULL;
  g_autofree char *line = NULL;
  g_autoptr (JsonParser) parser = NULL;
  JsonObject *reply;
  Call *call;

  line = g_data_input_stream_read_line_finish_utf8 (G_DATA_INPUT_STREAM (source),
                                                    result, NULL, &error);

  /* A read left over from a connection that has already been torn down: the
   * queue it belonged to is gone, and so is the reason to care. */
  if (G_DATA_INPUT_STREAM (source) != self->in)
    return;

  if (line == NULL)
    {
      if (error == NULL)
        error = g_error_new_literal (XD_REMOTE_ERROR, XD_REMOTE_ERROR_DISCONNECTED,
                                     "The daemon closed the connection.");
      fail_connection (self, error);
      return;
    }

  if (*line == '\0')
    {
      read_next_line (self);
      return;
    }

  parser = json_parser_new ();
  if (!json_parser_load_from_data (parser, line, -1, NULL) ||
      !JSON_NODE_HOLDS_OBJECT (json_parser_get_root (parser)))
    {
      g_autoptr (GError) bad = g_error_new_literal (XD_REMOTE_ERROR,
                                                    XD_REMOTE_ERROR_PROTOCOL,
                                                    "The daemon sent something "
                                                    "that is not a JSON object.");
      fail_connection (self, bad);
      return;
    }

  reply = json_node_get_object (json_parser_get_root (parser));

  /*
   * What the daemon says without being asked.
   *
   * It must not be handed to the call at the head of the queue: an event
   * arriving between a request and its answer would otherwise be read as that
   * answer, and every reply after it would belong to the wrong caller.
   */
  if (json_object_has_member (reply, "event"))
    {
      g_signal_emit (self, signals[SIGNAL_EVENT], 0, reply);
    }
  else
    {
      call = g_queue_pop_head (self->pending);

      if (call == NULL)
        g_debug ("unanswerable line from %s: %s", self->host, line);
      else
        complete_call (self, call, reply);
    }

  /* Answering may have ended the connection -- a refused greeting does -- and
   * there is nothing left to read from when it has. */
  if (self->in == G_DATA_INPUT_STREAM (source))
    read_next_line (self);
}

static void
read_next_line (XdRemoteClient *self)
{
  g_data_input_stream_read_line_async (self->in, G_PRIORITY_DEFAULT,
                                       self->cancellable, on_line_read,
                                       g_object_ref (self));
}

/* --- connecting ----------------------------------------------------------- */

/*
 * The pin, which is the whole of our certificate checking.
 *
 * The daemon signs its own certificate, so the flags GLib hands us say only
 * that -- nothing about whether this is the machine we paired with. That
 * question is answered by comparing against what we saw at pairing time.
 */
static gboolean
on_accept_certificate (GTlsConnection       *connection,
                       GTlsCertificate      *peer,
                       GTlsCertificateFlags  errors,
                       gpointer              user_data)
{
  XdRemoteClient *self = user_data;
  g_autoptr (GTlsCertificate) pinned = NULL;

  if (self->certificate_pem == NULL)
    {
      /* Pinning anything at any other moment would trust whoever answered. */
      if (self->greeting != GREETING_PAIR)
        return FALSE;

      g_object_get (peer, "certificate-pem", &self->certificate_pem, NULL);

      return self->certificate_pem != NULL;
    }

  pinned = g_tls_certificate_new_from_pem (self->certificate_pem, -1, NULL);

  return pinned != NULL && g_tls_certificate_is_same (peer, pinned);
}

static void
on_handshake (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  g_autoptr (XdRemoteClient) self = user_data;
  g_autoptr (GError) error = NULL;

  if (G_IO_STREAM (source) != self->stream)
    return;

  if (!g_tls_connection_handshake_finish (G_TLS_CONNECTION (source), result, &error))
    {
      /* A certificate we refused is the interesting failure, and worth saying
       * plainly: the daemon is reachable, but it is not the one we paired
       * with, which is what an attacker in the middle looks like. */
      if (g_error_matches (error, G_TLS_ERROR, G_TLS_ERROR_BAD_CERTIFICATE) ||
          g_error_matches (error, G_TLS_ERROR, G_TLS_ERROR_CERTIFICATE_REQUIRED))
        {
          g_autoptr (GError) pin = g_error_new (XD_REMOTE_ERROR,
                                               XD_REMOTE_ERROR_CERTIFICATE,
                                               "%s is not the daemon this device "
                                               "paired with: its certificate has "
                                               "changed.", self->host);

          /* Retrying cannot help, and would keep offering the token. */
          self->running = FALSE;
          drop_connection (self, pin);
          return;
        }

      fail_connection (self, error);
      return;
    }

  self->in = g_data_input_stream_new (g_io_stream_get_input_stream (self->stream));
  self->out = g_io_stream_get_output_stream (self->stream);

  read_next_line (self);
  send_greeting (self);
}

static void
on_socket_connected (GObject      *source,
                     GAsyncResult *result,
                     gpointer      user_data)
{
  g_autoptr (XdRemoteClient) self = user_data;
  g_autoptr (GSocketConnection) socket = NULL;
  g_autoptr (GError) error = NULL;
  GIOStream *tls;

  socket = g_socket_client_connect_to_host_finish (G_SOCKET_CLIENT (source),
                                                   result, &error);
  if (socket == NULL)
    {
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        fail_connection (self, error);
      return;
    }

  tls = g_tls_client_connection_new (G_IO_STREAM (socket), NULL, &error);
  if (tls == NULL)
    {
      fail_connection (self, error);
      return;
    }

  self->stream = tls;

  g_signal_connect (tls, "accept-certificate",
                    G_CALLBACK (on_accept_certificate), self);
  g_tls_connection_handshake_async (G_TLS_CONNECTION (tls), G_PRIORITY_DEFAULT,
                                    self->cancellable, on_handshake,
                                    g_object_ref (self));
}

static void
connect_now (XdRemoteClient *self)
{
  if (self->stream != NULL)
    return;

  g_socket_client_connect_to_host_async (self->socket_client, self->host,
                                         self->port, self->cancellable,
                                         on_socket_connected,
                                         g_object_ref (self));
}

/* --- public API ----------------------------------------------------------- */

XdRemoteClient *
xd_remote_client_new (const char *host,
                      guint16     port)
{
  XdRemoteClient *self;

  g_return_val_if_fail (host != NULL && *host != '\0', NULL);

  self = g_object_new (XD_TYPE_REMOTE_CLIENT, NULL);
  self->host = g_strdup (host);
  self->port = port != 0 ? port : 4001;

  return self;
}

const char *
xd_remote_client_get_host (XdRemoteClient *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_CLIENT (self), NULL);

  return self->host;
}

guint16
xd_remote_client_get_port (XdRemoteClient *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_CLIENT (self), 0);

  return self->port;
}

void
xd_remote_client_set_certificate (XdRemoteClient *self,
                                  const char     *pem)
{
  g_return_if_fail (XD_IS_REMOTE_CLIENT (self));

  g_free (self->certificate_pem);
  self->certificate_pem = (pem != NULL && *pem != '\0') ? g_strdup (pem) : NULL;
}

const char *
xd_remote_client_get_certificate (XdRemoteClient *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_CLIENT (self), NULL);

  return self->certificate_pem;
}

void
xd_remote_client_set_token (XdRemoteClient *self,
                            const char     *token)
{
  g_return_if_fail (XD_IS_REMOTE_CLIENT (self));

  g_free (self->token);
  self->token = (token != NULL && *token != '\0') ? g_strdup (token) : NULL;
}

const char *
xd_remote_client_get_token (XdRemoteClient *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_CLIENT (self), NULL);

  return self->token;
}

const char *
xd_remote_client_get_device_name (XdRemoteClient *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_CLIENT (self), NULL);

  return self->device_name;
}

gboolean
xd_remote_client_is_connected (XdRemoteClient *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_CLIENT (self), FALSE);

  return self->connected;
}

void
xd_remote_client_pair_async (XdRemoteClient      *self,
                             const char          *code,
                             const char          *device_name,
                             GCancellable        *cancellable,
                             GAsyncReadyCallback  callback,
                             gpointer             user_data)
{
  GTask *task;

  g_return_if_fail (XD_IS_REMOTE_CLIENT (self));
  g_return_if_fail (code != NULL && *code != '\0');

  task = g_task_new (self, cancellable, callback, user_data);
  g_task_set_source_tag (task, xd_remote_client_pair_async);

  /* Pairing starts from nothing, so anything half-open is in the way: the
   * code is spent by the first connection that offers it. */
  {
    g_autoptr (GError) restarting =
      g_error_new_literal (XD_REMOTE_ERROR, XD_REMOTE_ERROR_DISCONNECTED,
                           "Pairing restarted the connection.");

    drop_connection (self, restarting);
  }

  g_clear_handle_id (&self->reconnect_id, g_source_remove);

  g_free (self->pair_code);
  self->pair_code = g_strdup (code);
  g_free (self->pair_name);
  self->pair_name = g_strdup (device_name != NULL ? device_name : g_get_host_name ());
  self->greeting = GREETING_PAIR;

  /* Keeping the line up from here means the tree can be read the moment
   * pairing lands, without a second round of connecting. */
  self->running = TRUE;

  /* Nothing is written until the handshake finishes, so the task waits on the
   * client and is picked up by the greeting when there is a line to write it
   * on -- or failed with the connection, if there never is. */
  self->pair_task = task;

  connect_now (self);
}

gboolean
xd_remote_client_pair_finish (XdRemoteClient  *self,
                              GAsyncResult    *result,
                              GError         **error)
{
  g_return_val_if_fail (g_task_is_valid (result, self), FALSE);

  return g_task_propagate_boolean (G_TASK (result), error);
}

void
xd_remote_client_start (XdRemoteClient *self)
{
  g_return_if_fail (XD_IS_REMOTE_CLIENT (self));

  /* Both are set when pairing succeeded and were stored with it; without them
   * there is nothing to say and nothing to check the daemon against. */
  g_return_if_fail (self->token != NULL);
  g_return_if_fail (self->certificate_pem != NULL);

  self->running = TRUE;
  self->greeting = GREETING_HELLO;

  connect_now (self);
}

void
xd_remote_client_stop (XdRemoteClient *self)
{
  g_autoptr (GError) error = NULL;

  g_return_if_fail (XD_IS_REMOTE_CLIENT (self));

  self->running = FALSE;
  g_clear_handle_id (&self->reconnect_id, g_source_remove);

  error = g_error_new_literal (XD_REMOTE_ERROR, XD_REMOTE_ERROR_DISCONNECTED,
                               "Disconnected from the daemon.");
  drop_connection (self, error);
}

void
xd_remote_client_call_async (XdRemoteClient      *self,
                             JsonNode            *request,
                             GCancellable        *cancellable,
                             GAsyncReadyCallback  callback,
                             gpointer             user_data)
{
  GTask *task;
  Call *call;

  g_return_if_fail (XD_IS_REMOTE_CLIENT (self));
  g_return_if_fail (request != NULL);

  task = g_task_new (self, cancellable, callback, user_data);
  g_task_set_source_tag (task, xd_remote_client_call_async);

  if (!self->connected)
    {
      g_task_return_new_error (task, XD_REMOTE_ERROR,
                               XD_REMOTE_ERROR_DISCONNECTED,
                               "Not connected to %s.", self->host);
      g_object_unref (task);
      return;
    }

  call = g_new0 (Call, 1);
  call->task = task;

  send_call (self, call, request);
}

JsonObject *
xd_remote_client_call_finish (XdRemoteClient  *self,
                              GAsyncResult    *result,
                              GError         **error)
{
  g_return_val_if_fail (g_task_is_valid (result, self), NULL);

  return g_task_propagate_pointer (G_TASK (result), error);
}

void
xd_remote_client_call_op_async (XdRemoteClient      *self,
                                const char          *op,
                                const char          *argument_name,
                                const char          *argument_value,
                                GCancellable        *cancellable,
                                GAsyncReadyCallback  callback,
                                gpointer             user_data)
{
  g_autoptr (JsonBuilder) builder = json_builder_new ();
  g_autoptr (JsonNode) request = NULL;

  g_return_if_fail (XD_IS_REMOTE_CLIENT (self));
  g_return_if_fail (op != NULL);

  json_builder_begin_object (builder);
  json_builder_set_member_name (builder, "op");
  json_builder_add_string_value (builder, op);

  if (argument_name != NULL)
    {
      json_builder_set_member_name (builder, argument_name);
      json_builder_add_string_value (builder, argument_value);
    }

  json_builder_end_object (builder);
  request = json_builder_get_root (builder);

  xd_remote_client_call_async (self, request, cancellable, callback, user_data);
}

/* --- GObject -------------------------------------------------------------- */

static void
xd_remote_client_dispose (GObject *object)
{
  XdRemoteClient *self = XD_REMOTE_CLIENT (object);

  /* No ::closed on the way out: the object is already going, and a handler
   * that took a reference to it would be holding a corpse. */
  self->connected = FALSE;
  xd_remote_client_stop (self);

  g_clear_object (&self->cancellable);
  g_clear_object (&self->socket_client);
  g_clear_pointer (&self->pending, g_queue_free);

  G_OBJECT_CLASS (xd_remote_client_parent_class)->dispose (object);
}

static void
xd_remote_client_finalize (GObject *object)
{
  XdRemoteClient *self = XD_REMOTE_CLIENT (object);

  g_clear_pointer (&self->host, g_free);
  g_clear_pointer (&self->certificate_pem, g_free);
  g_clear_pointer (&self->token, g_free);
  g_clear_pointer (&self->device_name, g_free);
  g_clear_pointer (&self->pair_code, g_free);
  g_clear_pointer (&self->pair_name, g_free);

  G_OBJECT_CLASS (xd_remote_client_parent_class)->finalize (object);
}

static void
xd_remote_client_class_init (XdRemoteClientClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = xd_remote_client_dispose;
  object_class->finalize = xd_remote_client_finalize;

  /* A greeting was accepted. Fires again on every reconnection, because the
   * daemon may have moved on while we were away and whatever was read from it
   * has to be read again. */
  signals[SIGNAL_OPENED] =
    g_signal_new ("opened", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 0);

  /* The line went, with what went wrong. */
  signals[SIGNAL_CLOSED] =
    g_signal_new ("closed", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRING);

  /* Something happened over there: a turn saying something, a tree that
   * changed, a chat that was edited. Carries the daemon's own object. */
  signals[SIGNAL_EVENT] =
    g_signal_new ("event", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, JSON_TYPE_OBJECT);
}

static void
xd_remote_client_init (XdRemoteClient *self)
{
  self->socket_client = g_socket_client_new ();
  self->cancellable = g_cancellable_new ();
  self->pending = g_queue_new ();
}
