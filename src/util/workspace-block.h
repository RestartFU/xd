#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * Lifts workspace control blocks out of assistant text.
 *
 * A block is only control markup when it starts on its own line, ends on that
 * line, and names one non-empty path:
 *
 *   <workspace>/path/to/checkout</workspace>
 *
 * All valid blocks are removed from @text and the last reported path is
 * returned. Prose that merely mentions the tag remains prose.
 */
char *xd_workspace_block_parse (const char  *text,
                                char       **remainder);

G_END_DECLS
