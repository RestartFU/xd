#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define HY_TYPE_TERMINAL_PANEL (hy_terminal_panel_get_type ())

G_DECLARE_FINAL_TYPE (HyTerminalPanel, hy_terminal_panel, HY, TERMINAL_PANEL, AdwBin)

HyTerminalPanel *hy_terminal_panel_new         (void);

/*
 * Points the terminal at @workdir.
 *
 * A shell that is already running is left where it is: it may hold a session
 * the user is in the middle of, and moving it would mean either killing that
 * or typing behind their back. The directory applies to the next shell.
 */
void             hy_terminal_panel_set_workdir (HyTerminalPanel *self,
                                                const char      *workdir);

/*
 * Starts the shell if none is running.
 *
 * Separate from taking the keyboard so a panel restored at startup can come
 * back with its shell running without stealing focus from the composer. Does
 * nothing until a working directory is known, so the shell never starts in
 * whichever directory hy happened to be launched from.
 */
void             hy_terminal_panel_start       (HyTerminalPanel *self);

/* Starts the shell if none is running, and takes the keyboard. */
void             hy_terminal_panel_activate    (HyTerminalPanel *self);

G_END_DECLS
