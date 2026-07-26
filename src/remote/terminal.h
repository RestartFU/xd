#pragma once

#include <gio/gio.h>

G_BEGIN_DECLS

#define XD_TYPE_REMOTE_TERMINAL (xd_remote_terminal_get_type ())
G_DECLARE_FINAL_TYPE (XdRemoteTerminal, xd_remote_terminal,
                      XD, REMOTE_TERMINAL, GObject)

/*
 * One shell owned by the daemon.
 *
 * Bytes from the pty are retained so a device attaching later can reconstruct
 * the same screen before it starts consuming live ::output signals.
 */
XdRemoteTerminal *xd_remote_terminal_new       (const char  *chat_id,
                                                const char  *workdir,
                                                guint        columns,
                                                guint        rows,
                                                GError     **error);

const char       *xd_remote_terminal_get_id      (XdRemoteTerminal *self);
const char       *xd_remote_terminal_get_chat_id (XdRemoteTerminal *self);
const char       *xd_remote_terminal_get_title   (XdRemoteTerminal *self);
guint             xd_remote_terminal_get_columns (XdRemoteTerminal *self);
guint             xd_remote_terminal_get_rows    (XdRemoteTerminal *self);
gboolean          xd_remote_terminal_is_closing  (XdRemoteTerminal *self);

/*
 * Ordered replay state, borrowed from the terminal.
 *
 * A data item has non-NULL @data. A geometry item has NULL @data and carries
 * columns/rows. Replaying both kinds in order reconstructs the screen against
 * the dimensions that produced each piece of output.
 */
typedef struct
{
  GBytes *data;
  guint columns;
  guint rows;
} XdTerminalReplayItem;

GPtrArray        *xd_remote_terminal_get_replay  (XdRemoteTerminal *self);

gboolean          xd_remote_terminal_write       (XdRemoteTerminal *self,
                                                  const guint8     *data,
                                                  gsize             length,
                                                  GError          **error);
gboolean          xd_remote_terminal_resize      (XdRemoteTerminal *self,
                                                  guint             columns,
                                                  guint             rows,
                                                  GError          **error);
void              xd_remote_terminal_close       (XdRemoteTerminal *self);

G_END_DECLS
