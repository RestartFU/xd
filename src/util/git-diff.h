#pragma once

#include <glib.h>

G_BEGIN_DECLS

/* True for the stable event emitted when Codex finishes editing files. */
gboolean    xd_tool_is_file_change       (const char *message);

/*
 * Replaces a file_change event with the diff that existed at that point.
 *
 * The result remains a tool record, so it is persisted and sent to remote
 * viewers but stays out of assistant handover. Non-file tools are copied
 * unchanged. If Git cannot provide a diff, the original event is retained.
 */
char       *xd_git_diff_capture_tool     (const char *message,
                                          const char *workdir);

/* The captured patch, or NULL when @message is an ordinary tool record. */
const char *xd_git_diff_from_tool        (const char *message);

G_END_DECLS
