#include "util/unified-diff.h"

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

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/unified-diff/parses-display-rows",
                   test_parses_display_rows);
  g_test_add_func ("/unified-diff/keeps-meaningful-metadata",
                   test_keeps_meaningful_metadata);

  return g_test_run ();
}
