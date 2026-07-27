#pragma once

#include "storage/storage.h"

G_BEGIN_DECLS

/*
 * Retells whatever a backend has not been told.
 *
 * Resuming a session restores only what *that* assistant was sent, so anything
 * said to the other one in between is missing from it. Replaying those
 * messages is what keeps one conversation coherent across two CLIs -- and it
 * matters on every turn, not only the first after a switch.
 *
 * @last_seen is the last message id that backend has been brought up to. The
 * message being sent right now is already stored, so the last entry is left
 * out: it travels as the prompt. NULL when there is nothing to catch up on.
 */
char *xd_handover_build (XdStorage  *storage,
                         const char *chat_id,
                         gint64      last_seen);

/*
 * Adds unseen conversation to a prompt without hiding a slash command behind
 * it. Agent CLIs only resolve commands when the command is first.
 */
char *xd_handover_join  (const char *handover,
                         const char *prompt);

G_END_DECLS
