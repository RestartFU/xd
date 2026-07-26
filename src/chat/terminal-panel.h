#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define HY_TYPE_TERMINAL_PANEL (hy_terminal_panel_get_type ())

G_DECLARE_FINAL_TYPE (HyTerminalPanel, hy_terminal_panel, HY, TERMINAL_PANEL, AdwBin)

/*
 * A stack of shell sessions, grouped per chat.
 *
 * Each chat keeps its own tabs of terminals, alive across chat switches for
 * as long as the app runs. Closing a tab kills its session outright: the
 * terminal's pty goes with it, and the kernel hangs up the shell.
 *
 * Emits "close-requested" when the last session of the current chat is
 * killed, so whoever owns the toggle can take the panel off screen.
 */

HyTerminalPanel *hy_terminal_panel_new         (void);

/* Which chat's sessions are on screen. NULL shows none. */
void             hy_terminal_panel_set_chat    (HyTerminalPanel *self,
                                                const char      *chat_id);

/* Where new sessions start. Existing sessions are left where they are. */
void             hy_terminal_panel_set_workdir (HyTerminalPanel *self,
                                                const char      *workdir);

/* Ensures the current chat has at least one session running. */
void             hy_terminal_panel_start       (HyTerminalPanel *self);

/* Starts if needed, and takes the keyboard. */
void             hy_terminal_panel_activate    (HyTerminalPanel *self);

/* Kills every session the chat has; for when the chat itself is deleted. */
void             hy_terminal_panel_forget_chat (HyTerminalPanel *self,
                                                const char      *chat_id);

G_END_DECLS
