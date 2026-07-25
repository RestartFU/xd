#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * The environment as it was before hy's launcher rewrote it.
 *
 * hy runs out of a bundle carrying its own GTK, fonts and settings schemas.
 * A shell started under that environment would hand it to everything it runs,
 * so anything graphical launched from the terminal would load the bundle's
 * libraries instead of the host's. The launcher records the host values under
 * HY_HOST_* before overriding them; this hands them back.
 */
GStrv hy_host_environ (void);

G_END_DECLS
