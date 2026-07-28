#include "util/unified-diff.h"

#include <pango/pango.h>
#include <string.h>

static void
test_parses_display_rows (void)
{
  static const char *patch =
    "diff --git a/src/a.c b/src/a.c\n"
    "index 1111111..2222222 100644\n"
    "--- a/src/a.c\n"
    "+++ b/src/a.c\n"
    "@@ -10,2 +10,3 @@ function\n"
    " unchanged\n"
    "-gone\n"
    "+new\n"
    "+extra\n"
    "\\ No newline at end of file\n";
  g_autoptr (GPtrArray) lines = NULL;
  XdDiffLine *line;
  guint additions = 0;
  guint deletions = 0;

  lines = xd_unified_diff_parse (patch, &additions, &deletions);

  g_assert_cmpuint (lines->len, ==, 7);
  g_assert_cmpuint (additions, ==, 2);
  g_assert_cmpuint (deletions, ==, 1);

  line = g_ptr_array_index (lines, 0);
  g_assert_cmpint (line->kind, ==, XD_DIFF_LINE_FILE);
  g_assert_cmpstr (line->text, ==, "src/a.c");

  line = g_ptr_array_index (lines, 1);
  g_assert_cmpint (line->kind, ==, XD_DIFF_LINE_HUNK);
  g_assert_cmpuint (line->old_line, ==, 10);
  g_assert_cmpuint (line->new_line, ==, 10);

  line = g_ptr_array_index (lines, 2);
  g_assert_cmpint (line->kind, ==, XD_DIFF_LINE_CONTEXT);
  g_assert_cmpuint (line->old_line, ==, 10);
  g_assert_cmpuint (line->new_line, ==, 10);
  g_assert_cmpstr (line->text, ==, "unchanged");

  line = g_ptr_array_index (lines, 3);
  g_assert_cmpint (line->kind, ==, XD_DIFF_LINE_REMOVED);
  g_assert_cmpuint (line->old_line, ==, 11);
  g_assert_cmpuint (line->new_line, ==, 0);

  line = g_ptr_array_index (lines, 4);
  g_assert_cmpint (line->kind, ==, XD_DIFF_LINE_ADDED);
  g_assert_cmpuint (line->old_line, ==, 0);
  g_assert_cmpuint (line->new_line, ==, 11);

  line = g_ptr_array_index (lines, 6);
  g_assert_cmpint (line->kind, ==, XD_DIFF_LINE_META);
}

static void
test_keeps_meaningful_metadata (void)
{
  static const char *patch =
    "diff --git a/image.png b/image.png\n"
    "new file mode 100644\n"
    "Binary files /dev/null and b/image.png differ\n";
  g_autoptr (GPtrArray) lines = xd_unified_diff_parse (patch, NULL, NULL);
  XdDiffLine *first = g_ptr_array_index (lines, 1);
  XdDiffLine *second = g_ptr_array_index (lines, 2);

  g_assert_cmpuint (lines->len, ==, 3);
  g_assert_cmpstr (first->text, ==, "new file mode 100644");
  g_assert_cmpstr (second->text, ==,
                   "Binary files /dev/null and b/image.png differ");
}

static void
test_formats_one_safe_layout (void)
{
  static const char *patch =
    "diff --git a/src/a.c b/src/a.c\n"
    "@@ -1,2 +1,2 @@\n"
    "-old <value>\n"
    "+new & value\n"
    " same\n";
  g_autoptr (GPtrArray) lines =
    xd_unified_diff_parse (patch, NULL, NULL);
  g_autofree char *markup =
    xd_unified_diff_markup (lines, TRUE, 3, NULL);
  g_autofree char *plain = NULL;
  g_autoptr (GError) error = NULL;

  g_assert_cmpuint (xd_unified_diff_display_rows (lines, TRUE), ==, 5);
  g_assert_true (
    pango_parse_markup (markup, -1, 0, NULL, &plain, NULL, &error));
  g_assert_no_error (error);
  g_assert_nonnull (strstr (plain, "src/a.c  +1  −1"));
  g_assert_nonnull (strstr (plain, "old <value>"));
  g_assert_nonnull (strstr (plain, "Showing first 3 of 5 rows"));
  g_assert_nonnull (strstr (
    markup, "foreground=\"#ffbe6f\" weight=\"bold\">src/a.c</span>"));
  g_assert_nonnull (strstr (markup, "foreground=\"#f66151\""));
}

static void
test_colours_complete_changed_lines (void)
{
  static const char *patch =
    "@@ -1 +1 @@\n"
    "-removed line\n"
    "+added line\n";
  g_autoptr (GPtrArray) lines =
    xd_unified_diff_parse (patch, NULL, NULL);
  g_autofree char *markup =
    xd_unified_diff_markup (lines, FALSE, 0, NULL);
  g_autoptr (GError) error = NULL;

  g_assert_true (pango_parse_markup (
    markup, -1, 0, NULL, NULL, NULL, &error));
  g_assert_no_error (error);
  g_assert_nonnull (strstr (markup, "foreground=\"#f66151\">removed line"));
  g_assert_nonnull (strstr (markup, "foreground=\"#57e389\">added line"));
}

/*
 * Row colours belong to the rows, not to the markup: a background attribute
 * covers the glyphs and leaves the line spacing and short lines bare.
 */
static void
test_reports_row_kinds_without_backgrounds (void)
{
  static const char *patch =
    "diff --git a/src/a.c b/src/a.c\n"
    "@@ -1,3 +1,3 @@\n"
    " same\n"
    "-old one\n"
    "+new one\n"
    "+\n";
  g_autoptr (GPtrArray) lines =
    xd_unified_diff_parse (patch, NULL, NULL);
  g_autoptr (GArray) kinds = NULL;
  g_autofree char *markup =
    xd_unified_diff_markup (lines, TRUE, 0, &kinds);
  g_autoptr (GError) error = NULL;

  g_assert_true (pango_parse_markup (
    markup, -1, 0, NULL, NULL, NULL, &error));
  g_assert_no_error (error);
  g_assert_null (strstr (markup, "background="));

  g_assert_nonnull (kinds);
  g_assert_cmpuint (kinds->len, ==, 6);
  g_assert_cmpint (g_array_index (kinds, guint8, 0), ==, XD_DIFF_LINE_FILE);
  g_assert_cmpint (g_array_index (kinds, guint8, 1), ==, XD_DIFF_LINE_HUNK);
  g_assert_cmpint (g_array_index (kinds, guint8, 2), ==, XD_DIFF_LINE_CONTEXT);
  g_assert_cmpint (g_array_index (kinds, guint8, 3), ==, XD_DIFF_LINE_REMOVED);
  g_assert_cmpint (g_array_index (kinds, guint8, 4), ==, XD_DIFF_LINE_ADDED);

  /* An added line with no text of its own still colours its whole row. */
  g_assert_cmpint (g_array_index (kinds, guint8, 5), ==, XD_DIFF_LINE_ADDED);

  g_assert_cmpstr (xd_diff_line_background (XD_DIFF_LINE_FILE), ==, "#000000");
  g_assert_cmpstr (xd_diff_line_background (XD_DIFF_LINE_ADDED), ==, "#183522");
  g_assert_cmpstr (xd_diff_line_background (XD_DIFF_LINE_REMOVED), ==, "#3a1d1b");
  g_assert_null (xd_diff_line_background (XD_DIFF_LINE_CONTEXT));
  g_assert_null (xd_diff_line_background (XD_DIFF_LINE_HUNK));
}

/* The truncation footer is a row too, or every colour below it would shift. */
static void
test_counts_every_rendered_row (void)
{
  static const char *patch =
    "@@ -1,4 +1,4 @@\n"
    "-old one\n"
    "-old two\n"
    "+new one\n"
    "+new two\n";
  g_autoptr (GPtrArray) lines =
    xd_unified_diff_parse (patch, NULL, NULL);
  g_autoptr (GArray) kinds = NULL;
  g_autofree char *markup =
    xd_unified_diff_markup (lines, FALSE, 2, &kinds);

  g_assert_nonnull (strstr (markup, "Showing first 2 of 5 rows"));
  g_assert_cmpuint (kinds->len, ==, 3);
  g_assert_cmpint (g_array_index (kinds, guint8, 2), ==, XD_DIFF_LINE_META);
}

static void
test_formats_independent_slices (void)
{
  static const char *patch =
    "diff --git a/src/a.c b/src/a.c\n"
    "@@ -1,2 +1,2 @@\n"
    "-old\n"
    "+new\n"
    " same\n";
  g_autoptr (GPtrArray) lines =
    xd_unified_diff_parse (patch, NULL, NULL);
  g_autoptr (GArray) second_kinds = NULL;
  g_autofree char *first =
    xd_unified_diff_markup_slice (lines, TRUE, 0, 2, NULL);
  g_autofree char *second =
    xd_unified_diff_markup_slice (lines, TRUE, 2, 5, &second_kinds);
  g_autofree char *plain = NULL;
  g_autoptr (GError) error = NULL;

  /* One kind per rendered row of the slice, so a chunk colours its own rows. */
  g_assert_cmpuint (second_kinds->len, ==, 3);
  g_assert_cmpint (
    g_array_index (second_kinds, guint8, 0), ==, XD_DIFF_LINE_REMOVED);

  g_assert_true (
    pango_parse_markup (first, -1, 0, NULL, &plain, NULL, &error));
  g_assert_no_error (error);
  g_assert_nonnull (strstr (plain, "src/a.c  +1  −1"));
  g_assert_null (strstr (plain, "old"));

  g_clear_pointer (&plain, g_free);
  g_assert_true (
    pango_parse_markup (second, -1, 0, NULL, &plain, NULL, &error));
  g_assert_no_error (error);
  g_assert_nonnull (strstr (plain, "old"));
  g_assert_nonnull (strstr (plain, "new"));
  g_assert_null (strstr (plain, "Showing first"));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/unified-diff/parses-display-rows",
                   test_parses_display_rows);
  g_test_add_func ("/unified-diff/keeps-meaningful-metadata",
                   test_keeps_meaningful_metadata);
  g_test_add_func ("/unified-diff/formats-one-safe-layout",
                   test_formats_one_safe_layout);
  g_test_add_func ("/unified-diff/colours-complete-changed-lines",
                   test_colours_complete_changed_lines);
  g_test_add_func ("/unified-diff/reports-row-kinds-without-backgrounds",
                   test_reports_row_kinds_without_backgrounds);
  g_test_add_func ("/unified-diff/counts-every-rendered-row",
                   test_counts_every_rendered_row);
  g_test_add_func ("/unified-diff/formats-independent-slices",
                   test_formats_independent_slices);

  return g_test_run ();
}
