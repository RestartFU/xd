#include "terminal-panel.h"

/*
 * Windows client intentionally ships without an embedded terminal. Keeping
 * this API lets chat code remain platform-neutral while its button is hidden.
 */

struct _XdTerminalPanel
{
  AdwBin parent_instance;
};

G_DEFINE_FINAL_TYPE (XdTerminalPanel, xd_terminal_panel, ADW_TYPE_BIN)

enum
{
  SIGNAL_CLOSE_REQUESTED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

XdTerminalPanel *
xd_terminal_panel_new (void)
{
  return g_object_new (XD_TYPE_TERMINAL_PANEL, NULL);
}

void
xd_terminal_panel_set_remote (XdTerminalPanel *self,
                              XdRemoteClient  *client)
{
  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));
  g_return_if_fail (client == NULL || XD_IS_REMOTE_CLIENT (client));
}

void
xd_terminal_panel_set_chat (XdTerminalPanel *self,
                            const char      *chat_id)
{
  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));
}

void
xd_terminal_panel_set_workdir (XdTerminalPanel *self,
                               const char      *workdir)
{
  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));
}

void
xd_terminal_panel_start (XdTerminalPanel *self)
{
  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));
}

void
xd_terminal_panel_activate (XdTerminalPanel *self)
{
  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));
}

void
xd_terminal_panel_forget_chat (XdTerminalPanel *self,
                               const char      *chat_id)
{
  g_return_if_fail (XD_IS_TERMINAL_PANEL (self));
}

static void
xd_terminal_panel_class_init (XdTerminalPanelClass *klass)
{
  signals[SIGNAL_CLOSE_REQUESTED] =
    g_signal_new ("close-requested", G_TYPE_FROM_CLASS (klass),
                  G_SIGNAL_RUN_LAST, 0, NULL, NULL, NULL, G_TYPE_NONE, 0);
}

static void
xd_terminal_panel_init (XdTerminalPanel *self)
{
}
