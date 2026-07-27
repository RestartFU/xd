#pragma once

#include <gtk/gtk.h>

#include "remote/remote-tree.h"

G_BEGIN_DECLS

/*
 * Edits global agent environment secrets.
 *
 * @remote is NULL for this machine. Otherwise only names are fetched from the
 * daemon; stored values never leave the machine where agents execute.
 */
void xd_agent_secrets_dialog_present (GtkWidget    *parent,
                                      XdRemoteTree *remote);

G_END_DECLS
