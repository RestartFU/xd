#pragma once

#include <gio/gio.h>

#include "storage/storage.h"

G_BEGIN_DECLS

#define XD_TYPE_DAEMON_TURN (xd_daemon_turn_get_type ())
G_DECLARE_FINAL_TYPE (XdDaemonTurn, xd_daemon_turn, XD, DAEMON_TURN, GObject)

/*
 * One turn, run by the daemon on behalf of whoever asked for it.
 *
 * The agent runs where the work is: the working directory, the repository, the
 * CLI and its credentials are all on this machine, and a client thousands of
 * kilometres away is only watching. So this is the same shape as a turn in the
 * window -- the same session object driving the same CLI -- with the messages
 * written to the same database and the text handed out as it arrives instead of
 * being drawn.
 *
 * Signals:
 *   text     (delta)          another piece of the reply
 *   tool     (name)           the agent reached for something
 *   finished (ok, message)    the turn ended; message explains a failure
 *
 * Everything the turn produces is in the database by the time ::finished is
 * emitted, so a client that missed the stream only has to read the chat again.
 */

XdDaemonTurn *xd_daemon_turn_new       (XdStorage    *storage,
                                        const char   *root_path);

/*
 * Starts @prompt as a turn in @chat_id.
 *
 * The user's message is stored first, so it is in the transcript whatever
 * happens next -- including this failing.
 */
gboolean      xd_daemon_turn_start     (XdDaemonTurn  *self,
                                        const char    *chat_id,
                                        const char    *prompt,
                                        GError       **error);

void          xd_daemon_turn_cancel    (XdDaemonTurn *self);

gboolean      xd_daemon_turn_is_running (XdDaemonTurn *self);

/* Whole seconds since this turn started, measured on daemon's monotonic clock. */
gint64        xd_daemon_turn_get_elapsed (XdDaemonTurn *self);

/*
 * Where @chat runs, once the folder chain has had its say.
 *
 * The chain is on this machine, so this is the only side that can answer it --
 * a client showing "runs in" is showing what it was told.
 */
char         *xd_daemon_turn_resolve_workdir (XdDaemonTurn *self,
                                              const XdChat *chat);

/* Who is answering: the model and effort the turn actually started on. */
const char   *xd_daemon_turn_get_label (XdDaemonTurn *self);

/*
 * The turn so far, in the order it happened.
 *
 * A turn is written to the database when it ends, so until then this is the
 * only copy -- and it is what a device joining the chat halfway through has to
 * be given, or it sees the message that started the turn and nothing else.
 *
 * In order and in pieces rather than as one string: an agent says something,
 * reaches for a tool, then says something about what it found, and a device
 * that joined would otherwise show all of that as one paragraph with the tools
 * collected at the end -- the same turn, in an order that never happened.
 *
 * Elements are XdTurnItem*, owned by the turn.
 */
typedef struct
{
  gboolean tool;      /* a tool it reached for, rather than something it said */
  char *text;
} XdTurnItem;

GPtrArray    *xd_daemon_turn_get_items   (XdDaemonTurn *self);

/* What it is saying now, which no item holds yet. */
const char   *xd_daemon_turn_get_segment (XdDaemonTurn *self);

G_END_DECLS
