#include "util/subagent-tool.h"

static void
test_round_trip (void)
{
  g_autofree char *message =
    xd_subagent_tool_new ("Explore\nagent", "Inspect   the parser\ncarefully");
  g_autofree char *identity = NULL;
  g_autofree char *task = NULL;

  g_assert_true (
    xd_subagent_tool_from_tool (message, &identity, &task));
  g_assert_cmpstr (identity, ==, "Explore agent");
  g_assert_cmpstr (task, ==, "Inspect the parser carefully");
}

static void
test_rejects_other_tools (void)
{
  g_assert_false (
    xd_subagent_tool_from_tool ("$ git status", NULL, NULL));
  g_assert_false (
    xd_subagent_tool_from_tool ("subagent\nmissing-task", NULL, NULL));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/subagent-tool/round-trip", test_round_trip);
  g_test_add_func ("/subagent-tool/rejects-other-tools",
                   test_rejects_other_tools);

  return g_test_run ();
}
