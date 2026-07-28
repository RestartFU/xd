#include "terminal.h"

/*
 * The Windows daemon serves everything except a shell.
 *
 * A pty is the one thing this file cannot supply: Windows has no forkpty, and
 * ConPTY is a different enough shape that pretending otherwise here would only
 * move the problem. The client already ships this way -- see
 * chat/terminal-panel-stub.c -- so the daemon matches it: opening a terminal
 * fails with a reason, and every other op is served normally.
 *
 * Keeping the type rather than compiling the calls out is what lets server.c
 * stay platform-neutral. Sessions are never created, so the getters below are
 * unreachable rather than wrong.
 */

struct _XdRemoteTerminal
{
  GObject parent_instance;
};

G_DEFINE_FINAL_TYPE (XdRemoteTerminal, xd_remote_terminal, G_TYPE_OBJECT)

enum
{
  SIGNAL_OUTPUT,
  SIGNAL_CLOSED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

XdRemoteTerminal *
xd_remote_terminal_new (const char  *chat_id,
                        const char  *workdir,
                        guint        columns,
                        guint        rows,
                        GError     **error)
{
  g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                       "This machine cannot host terminals.");
  return NULL;
}

const char *
xd_remote_terminal_get_id (XdRemoteTerminal *self)
{
  return NULL;
}

const char *
xd_remote_terminal_get_chat_id (XdRemoteTerminal *self)
{
  return NULL;
}

const char *
xd_remote_terminal_get_title (XdRemoteTerminal *self)
{
  return NULL;
}

guint
xd_remote_terminal_get_columns (XdRemoteTerminal *self)
{
  return 0;
}

guint
xd_remote_terminal_get_rows (XdRemoteTerminal *self)
{
  return 0;
}

gboolean
xd_remote_terminal_is_closing (XdRemoteTerminal *self)
{
  return TRUE;
}

GPtrArray *
xd_remote_terminal_get_replay (XdRemoteTerminal *self)
{
  return NULL;
}

gboolean
xd_remote_terminal_write (XdRemoteTerminal *self,
                          const guint8     *data,
                          gsize             length,
                          GError          **error)
{
  g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                       "This machine cannot host terminals.");
  return FALSE;
}

gboolean
xd_remote_terminal_resize (XdRemoteTerminal *self,
                           guint             columns,
                           guint             rows,
                           GError          **error)
{
  g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                       "This machine cannot host terminals.");
  return FALSE;
}

void
xd_remote_terminal_close (XdRemoteTerminal *self)
{
}

static void
xd_remote_terminal_class_init (XdRemoteTerminalClass *klass)
{
  signals[SIGNAL_OUTPUT] =
    g_signal_new ("output", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_BYTES);
  signals[SIGNAL_CLOSED] =
    g_signal_new ("closed", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 0);
}

static void
xd_remote_terminal_init (XdRemoteTerminal *self)
{
}
