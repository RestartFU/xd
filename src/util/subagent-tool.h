#pragma once

#include <glib.h>

G_BEGIN_DECLS

/* Builds the durable tool record used for delegated agent work. */
char     *xd_subagent_tool_new       (const char *identity,
                                      const char *task);

/* Reads a subagent record. Both requested outputs are newly allocated. */
gboolean  xd_subagent_tool_from_tool (const char *message,
                                      char      **identity,
                                      char      **task);

G_END_DECLS
