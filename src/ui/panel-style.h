#pragma once

#include <gtk/gtk.h>

/*
 * Loads the shared shell used by xd's focused, undecorated utility windows.
 *
 * Widgets opt in with .xd-panel and its bar/action classes. Content-specific
 * styling stays with the widget that owns that content.
 */
void xd_panel_style_ensure (void);
