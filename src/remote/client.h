#pragma once

#include <gio/gio.h>
#include <json-glib/json-glib.h>

G_BEGIN_DECLS

#define XD_REMOTE_ERROR (xd_remote_error_quark ())

typedef enum
{
  XD_REMOTE_ERROR_REFUSED,        /* the daemon answered, and said no */
  XD_REMOTE_ERROR_DISCONNECTED,   /* the line went, or never came up */
  XD_REMOTE_ERROR_CERTIFICATE,    /* not the daemon we paired with */
  XD_REMOTE_ERROR_PROTOCOL,       /* an answer we cannot read */
} XdRemoteError;

GQuark xd_remote_error_quark (void);

#define XD_TYPE_REMOTE_CLIENT (xd_remote_client_get_type ())
G_DECLARE_FINAL_TYPE (XdRemoteClient, xd_remote_client, XD, REMOTE_CLIENT, GObject)

/*
 * The client side of remote xd: one TLS connection held open to a daemon,
 * newline-delimited JSON in both directions.
 *
 * The daemon's certificate is self-signed, so there is no authority to ask
 * about it. It is pinned the moment we pair -- trust on first use, as SSH does
 * with host keys -- and every connection afterwards refuses anything else. A
 * device token is remote code execution on that machine, so a certificate that
 * changed is a connection that does not happen rather than a prompt.
 *
 * The wire has no request ids, so the daemon's replies come back in the order
 * the requests went out: calls queue, and each reply answers the head of that
 * queue.
 */

XdRemoteClient *xd_remote_client_new   (const char *host,
                                        guint16     port);

const char     *xd_remote_client_get_host (XdRemoteClient *self);
guint16         xd_remote_client_get_port (XdRemoteClient *self);

/*
 * The pinned certificate, as PEM.
 *
 * Leaving it unset only works while pairing, which is the one moment there is
 * nothing yet to compare against.
 */
void            xd_remote_client_set_certificate (XdRemoteClient *self,
                                                  const char     *pem);
const char     *xd_remote_client_get_certificate (XdRemoteClient *self);

/* The device token pairing handed back. It does not expire. */
void            xd_remote_client_set_token (XdRemoteClient *self,
                                            const char     *token);
const char     *xd_remote_client_get_token (XdRemoteClient *self);

/* The name the daemon greeted us with; NULL until a greeting is accepted. */
const char     *xd_remote_client_get_device_name (XdRemoteClient *self);

gboolean        xd_remote_client_is_connected (XdRemoteClient *self);

/*
 * Trades a pairing code for a device token, pinning the certificate offered.
 *
 * @device_name is what the daemon lists this machine as; NULL uses the
 * hostname. On success the token and the certificate can be read back and
 * stored, and the connection is already authenticated -- pairing is a greeting
 * of its own, not a step before one.
 */
void            xd_remote_client_pair_async  (XdRemoteClient      *self,
                                              const char          *code,
                                              const char          *device_name,
                                              GCancellable        *cancellable,
                                              GAsyncReadyCallback  callback,
                                              gpointer             user_data);
gboolean        xd_remote_client_pair_finish (XdRemoteClient  *self,
                                              GAsyncResult    *result,
                                              GError         **error);

/*
 * Connects, says hello with the stored token, and keeps the connection open.
 *
 * A dropped line is retried every few seconds rather than reported as the end
 * of the remote: a laptop that slept should come back on its own. ::opened
 * fires each time a greeting is accepted -- on the first connection and on
 * every reconnection, because whatever was read before may have moved on --
 * and ::closed when the line goes.
 */
void            xd_remote_client_start (XdRemoteClient *self);
void            xd_remote_client_stop  (XdRemoteClient *self);

/*
 * One request, one reply.
 *
 * @request is an object carrying at least "op". The reply object is returned
 * whole; a daemon that answered "ok": false becomes an error carrying what it
 * said, so callers read replies rather than checking them.
 *
 * Cancelling only stops the answer from being delivered. The request has been
 * sent, and its reply still has to be read off the connection, because what a
 * reply answers is decided by its position rather than by anything in it.
 */
void            xd_remote_client_call_async  (XdRemoteClient      *self,
                                              JsonNode            *request,
                                              GCancellable        *cancellable,
                                              GAsyncReadyCallback  callback,
                                              gpointer             user_data);
JsonObject     *xd_remote_client_call_finish (XdRemoteClient  *self,
                                              GAsyncResult    *result,
                                              GError         **error);

/* The common shape: an op with no arguments beyond its name. */
void            xd_remote_client_call_op_async (XdRemoteClient      *self,
                                                const char          *op,
                                                const char          *argument_name,
                                                const char          *argument_value,
                                                GCancellable        *cancellable,
                                                GAsyncReadyCallback  callback,
                                                gpointer             user_data);

G_END_DECLS
