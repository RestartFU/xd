#pragma once

#include <adwaita.h>

#include "remote/client.h"

G_BEGIN_DECLS

/*
 * Called once pairing has gone through, with a client that is connected and
 * carries the token and the pinned certificate the caller should store.
 *
 * Not called at all when the user changes their mind or pairing fails: the
 * dialog says what went wrong itself, since that is where the user is looking.
 */
typedef void (*XdRemotePairedCallback) (XdRemoteClient *client,
                                        gpointer        user_data);

/*
 * Asks for a daemon and the code it printed.
 *
 * @settings is read for what was paired with last, so re-pairing a daemon that
 * was reinstalled does not mean typing its address again. Storing the result is
 * the caller's, since it is the caller that will connect on the next start.
 */
void xd_remote_pair_dialog_present (GtkWidget              *parent,
                                    GSettings              *settings,
                                    XdRemotePairedCallback  callback,
                                    gpointer                user_data);

G_END_DECLS
