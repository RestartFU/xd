#pragma once

#include <glib.h>

G_BEGIN_DECLS

/* What an unnamed chat is called before anything has been asked in it. */
#define XD_CHAT_UNTITLED "New Chat"

/*
 * A name for a chat, taken from the first thing asked in it.
 *
 * The first line only, shortened: a pasted stack trace should not become the
 * title. Deriving it costs nothing, where asking the model for one would cost
 * a whole round trip before the answer even starts.
 *
 * NULL when there is nothing worth using.
 */
char *xd_chat_title_from_prompt (const char *prompt);

G_END_DECLS
