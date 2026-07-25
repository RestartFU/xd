#pragma once

#include <adwaita.h>

#include "tree/hy-node.h"

G_BEGIN_DECLS

/*
 * Edits one folder's .hy.json. Anything left blank is inherited from the
 * folder above, and the rows say where the inherited value came from.
 *
 * Changes are written when the dialog closes.
 */
void hy_folder_settings_dialog_present (GtkWidget *parent,
                                        HyNode    *folder,
                                        GSettings *app_settings);

G_END_DECLS
