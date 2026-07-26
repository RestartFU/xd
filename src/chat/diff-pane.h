#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define XD_TYPE_DIFF_PANE (xd_diff_pane_get_type ())

G_DECLARE_FINAL_TYPE (XdDiffPane, xd_diff_pane, XD, DIFF_PANE, AdwBin)

XdDiffPane *xd_diff_pane_new         (void);

/* The repository to read. NULL, or a directory outside one, shows nothing. */
void        xd_diff_pane_set_workdir (XdDiffPane *self,
                                      const char *workdir);

/*
 * Re-reads the working tree.
 *
 * Called when the pane is opened and when an agent finishes a turn, since
 * that is when the files it changed stop moving. Does nothing while hidden.
 */
void        xd_diff_pane_refresh     (XdDiffPane *self);

G_END_DECLS
