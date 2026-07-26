#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * Creates the private checkout used by a new chat.
 *
 * The checkout starts at the current HEAD and gets its own xd/<chat-id>
 * branch. Its path is returned; an existing checkout from an interrupted
 * first-send attempt is reused.
 */
char *xd_worktree_create (const char  *workdir,
                          const char  *chat_id,
                          GError     **error);

G_END_DECLS
