#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define HY_TYPE_DIFF_PANE (hy_diff_pane_get_type ())

G_DECLARE_FINAL_TYPE (HyDiffPane, hy_diff_pane, HY, DIFF_PANE, AdwBin)

HyDiffPane *hy_diff_pane_new         (void);

/* The repository to read. NULL, or a directory outside one, shows nothing. */
void        hy_diff_pane_set_workdir (HyDiffPane *self,
                                      const char *workdir);

/*
 * Re-reads the working tree.
 *
 * Called when the pane is opened and when an agent finishes a turn, since
 * that is when the files it changed stop moving. Does nothing while hidden.
 */
void        hy_diff_pane_refresh     (HyDiffPane *self);

G_END_DECLS
