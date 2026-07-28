#pragma once

#include <gtk/gtk.h>

#include "util/unified-diff.h"

G_BEGIN_DECLS

#define XD_TYPE_DIFF_TEXT (xd_diff_text_get_type ())

G_DECLARE_FINAL_TYPE (XdDiffText, xd_diff_text, XD, DIFF_TEXT, GtkWidget)

/*
 * One block of diff text, with the changed lines coloured as whole rows.
 *
 * The text is a single selectable Pango layout, which is what keeps a long
 * patch cheap to scroll. Its row colours cannot come from the markup: Pango
 * paints a background attribute over the glyphs it covers and nothing more, so
 * the line spacing between rows and everything past the end of a short line
 * stayed dark -- a block of changed lines read as striped and ragged rather
 * than as a block. The rows are drawn here instead, behind the layout and
 * across the full width.
 */
GtkWidget *xd_diff_text_new       (void);

/* @row_kinds holds one XdDiffLineKind per line of @markup, as guint8. */
void       xd_diff_text_set_rows  (XdDiffText *self,
                                   const char *markup,
                                   GArray     *row_kinds);

/* The label carrying the layout, for callers that style it further. */
GtkLabel  *xd_diff_text_get_label (XdDiffText *self);

G_END_DECLS
