#pragma once

#include <glib.h>

G_BEGIN_DECLS

typedef struct _XdGitDiffTracker XdGitDiffTracker;

/* True for the stable event emitted when Codex finishes editing files. */
gboolean    xd_tool_is_file_change       (const char *message);

/*
 * Captures the working tree at turn start, then advances it after each
 * file_change event. The resulting patch contains only what happened since
 * the previous event, even when the repository was already dirty.
 *
 * A temporary index is used to write Git trees without reading or changing the
 * user's index. NULL is valid for capture and simply retains the tool event,
 * which is how non-Git working directories degrade.
 */
XdGitDiffTracker *xd_git_diff_tracker_new     (const char       *workdir);
void              xd_git_diff_tracker_free    (XdGitDiffTracker *self);
char             *xd_git_diff_tracker_capture (XdGitDiffTracker *self,
                                                const char       *message);

/* The captured patch, or NULL when @message is an ordinary tool record. */
const char *xd_git_diff_from_tool        (const char *message);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (XdGitDiffTracker, xd_git_diff_tracker_free)

G_END_DECLS
