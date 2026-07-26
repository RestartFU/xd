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

G_END_DECLS
