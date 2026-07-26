#pragma once

#include <gio/gio.h>

#include "storage/storage.h"

G_BEGIN_DECLS

#define XD_TYPE_REMOTE_SERVER (xd_remote_server_get_type ())
G_DECLARE_FINAL_TYPE (XdRemoteServer, xd_remote_server, XD, REMOTE_SERVER, GObject)

/*
 * The daemon side of remote xd: TLS on a port, newline-delimited JSON.
 *
 * Pairing trades a short-lived code for a device token that never expires;
 * only the token's hash touches the database. Everything a client reads
 * comes through here -- the daemon is the only writer.
 */

XdRemoteServer *xd_remote_server_new       (XdStorage        *storage,
                                            const char       *root_path,
                                            guint16           port,
                                            GTlsCertificate  *certificate,
                                            GError          **error);

/* The port actually bound, for when 0 asked the kernel to pick. */
guint16         xd_remote_server_get_port  (XdRemoteServer *self);

/* Arms a single-use pairing code valid for @seconds, and returns it. */
char           *xd_remote_server_arm_pairing (XdRemoteServer *self,
                                              guint           seconds);

G_END_DECLS
