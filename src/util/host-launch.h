#pragma once

#include <gio/gio.h>

G_BEGIN_DECLS

/*
 * Opens a terminal in @workdir.
 *
 * hy runs out of a bundle carrying its own GTK, fonts and settings schemas,
 * and a terminal that inherited those would load the wrong ones. The child is
 * given the host environment back before it starts.
 */
gboolean hy_open_terminal (const char  *workdir,
                           GError     **error);

G_END_DECLS
