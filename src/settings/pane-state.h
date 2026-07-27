#pragma once

#include <glib.h>

/*
 * Returns a new a{su} map with @key set to @state.
 *
 * GVariantDict cannot preserve this schema: it always produces a{sv}, which
 * GSettings rejects for the pane-state key.
 */
GVariant *xd_pane_state_update (GVariant   *states,
                                const char *key,
                                guint       state);
