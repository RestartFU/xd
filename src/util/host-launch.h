#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * The environment as it was before xd's launcher rewrote it.
 *
 * xd runs out of a bundle carrying its own GTK, fonts and settings schemas.
 * A shell started under that environment would hand it to everything it runs,
 * so anything graphical launched from the terminal would load the bundle's
 * libraries instead of the host's. The launcher records the host values under
 * XD_HOST_* before overriding them; this hands them back.
 */
GStrv xd_host_environ (void);

/* Opens a URI with the host desktop instead of the bundled GTK environment. */
void  xd_host_open_uri (const char *uri);

G_END_DECLS
