#include "util/workspace-block.h"

static void
test_extracts_and_hides_workspace (void)
{
  g_autofree char *remainder = NULL;
  g_autofree char *path = xd_workspace_block_parse (
    "Moved the work.\n\n"
    "<workspace>/tmp/project worktree</workspace>\n",
    &remainder);

  g_assert_cmpstr (path, ==, "/tmp/project worktree");
  g_assert_cmpstr (remainder, ==, "Moved the work.");
}

static void
test_last_workspace_wins (void)
{
  g_autofree char *remainder = NULL;
  g_autofree char *path = xd_workspace_block_parse (
    "<workspace>/tmp/first</workspace>\n"
    "Changed again.\n"
    "<workspace>/tmp/second</workspace>",
    &remainder);

  g_assert_cmpstr (path, ==, "/tmp/second");
  g_assert_cmpstr (remainder, ==, "Changed again.");
}

static void
test_tag_mentioned_in_prose_stays (void)
{
  const char *text =
    "Use <workspace>/tmp/repo</workspace> to report it.";
  g_autofree char *remainder = NULL;
  g_autofree char *path = xd_workspace_block_parse (text, &remainder);

  g_assert_null (path);
  g_assert_null (remainder);
}

static void
test_malformed_blocks_stay (void)
{
  const char *multiline =
    "<workspace>\n/tmp/repo\n</workspace>";
  const char *unclosed = "<workspace>/tmp/repo";

  g_assert_null (xd_workspace_block_parse (multiline, NULL));
  g_assert_null (xd_workspace_block_parse (unclosed, NULL));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/workspace-block/extract",
                   test_extracts_and_hides_workspace);
  g_test_add_func ("/workspace-block/last-wins",
                   test_last_workspace_wins);
  g_test_add_func ("/workspace-block/tag-in-prose",
                   test_tag_mentioned_in_prose_stays);
  g_test_add_func ("/workspace-block/malformed",
                   test_malformed_blocks_stay);

  return g_test_run ();
}
