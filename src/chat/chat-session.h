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
 *   commands        (names)       installed slash commands, without leading /
 *   text-delta      (text)        another piece of the reply
 *   tool-use        (name)        the agent used a tool
 *   finished        (ok, message) the turn ended; message explains a failure
 */
XdChatSession *xd_chat_session_new         (const AiBackend  *backend);

gboolean       xd_chat_session_start       (XdChatSession    *self,
                                            const AiRunSpec  *spec,
                                            GError          **error);

/*
 * Runs another turn on the process that is already up.
 *
 * A backend whose CLI takes its prompt in argv ends when the turn does, so
 * there is nothing to continue and this refuses. Where it succeeds, the
 * process was never restarted -- which is the point: anything the agent left
 * running is still running, and there is no start-up cost between messages.
 *
 * Refuses a turn the running process cannot serve, notably one that changed
 * model, effort, access or working directory, all of which are fixed in argv.
 * Ask first with can_continue, and start a new session when it says no.
 */
/*
 * @backend is passed rather than assumed: a session belongs to the CLI it was
 * created for, and a caller keeping one per chat has to notice when the chat
 * has been switched to another. Handing a claude turn to codex would otherwise
 * look exactly like continuing.
 */
gboolean       xd_chat_session_can_continue (XdChatSession   *self,
                                             const AiBackend *backend,
                                             const AiRunSpec *spec);
gboolean       xd_chat_session_continue    (XdChatSession    *self,
                                            const AiRunSpec  *spec,
                                            GError          **error);

/* Asks the child to stop, then insists if it does not. */
void           xd_chat_session_cancel      (XdChatSession    *self);

gboolean       xd_chat_session_is_running  (XdChatSession    *self);

G_END_DECLS
