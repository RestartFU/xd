#pragma once

#include <gio/gio.h>

#include "backend/backend.h"

G_BEGIN_DECLS

#define XD_TYPE_CHAT_SESSION (xd_chat_session_get_type ())
G_DECLARE_FINAL_TYPE (XdChatSession, xd_chat_session, XD, CHAT_SESSION, GObject)

/*
 * One turn of a conversation, running as a child process.
 *
 * Everything happens on the main loop: stdout is read a line at a time with
 * the async stream API, so a long reply never blocks the interface and no
 * threads are involved.
 *
 * Signals:
 *   session-started (id)          the backend reported a resumable session
 *   text-delta      (text)        another piece of the reply
 *   tool-use        (name)        the agent used a tool
 *   finished        (ok, message) the turn ended; message explains a failure
 */
XdChatSession *xd_chat_session_new         (const AiBackend  *backend);

gboolean       xd_chat_session_start       (XdChatSession    *self,
                                            const AiRunSpec  *spec,
                                            GError          **error);

/* Asks the child to stop, then insists if it does not. */
void           xd_chat_session_cancel      (XdChatSession    *self);

gboolean       xd_chat_session_is_running  (XdChatSession    *self);

G_END_DECLS
