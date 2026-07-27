#pragma once

#include <glib.h>

G_BEGIN_DECLS

typedef enum
{
  XD_DIFF_LINE_FILE,
  XD_DIFF_LINE_CONTEXT,
  XD_DIFF_LINE_ADDED,
  XD_DIFF_LINE_REMOVED,
  XD_DIFF_LINE_HUNK,
  XD_DIFF_LINE_META,
} XdDiffLineKind;

typedef struct
{
  XdDiffLineKind kind;
  char *text;
  guint old_line;  /* zero means the old-side gutter is blank */
  guint new_line;  /* zero means the new-side gutter is blank */
} XdDiffLine;

void       xd_diff_line_free       (XdDiffLine *line);

/*
 * Turns Git's unified output into display rows.
 *
 * File boundaries are retained for multi-file inline patches. Other plumbing
 * headers are omitted; meaningful metadata, hunks and changed lines remain.
 */
GPtrArray *xd_unified_diff_parse   (const char *patch,
                                    guint      *additions,
                                    guint      *deletions);

/* Number of rows shown after optional file headers are filtered. */
guint      xd_unified_diff_display_rows (GPtrArray *lines,
                                         gboolean   show_file_headers);

/*
 * Formats parsed rows as one selectable monospace Pango layout.
 *
 * A single layout keeps inline diffs cheap for the transcript scroller;
 * @limit zero means all rows. The returned markup is always valid.
 */
char      *xd_unified_diff_markup  (GPtrArray *lines,
                                    gboolean   show_file_headers,
                                    guint      limit);

/*
 * Formats one raw parsed-line range without a truncation footer.
 *
 * Used by the virtualized Git pane: only ranges represented by visible list
 * items become Pango layouts.
 */
char      *xd_unified_diff_markup_slice (GPtrArray *lines,
                                         gboolean   show_file_headers,
                                         guint      start,
                                         guint      end);

G_END_DECLS
