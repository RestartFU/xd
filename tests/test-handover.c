#include "chat/handover.h"

static void
test_handover_keeps_commands_first (void)
{
  g_autofree char *bare = xd_handover_join (NULL, "/simplify");
  g_autofree char *simple =
    xd_handover_join ("earlier conversation", "/simplify");
  g_autofree char *with_arguments =
    xd_handover_join ("earlier conversation", "/review focus on memory");
  g_autofree char *namespaced =
    xd_handover_join ("earlier conversation", "/plugin:command");

  g_assert_cmpstr (bare, ==, "/simplify");
  g_assert_cmpstr (simple, ==, "/simplify\n\nearlier conversation");
  g_assert_cmpstr (with_arguments, ==,
                   "/review focus on memory\n\nearlier conversation");
  g_assert_cmpstr (namespaced, ==,
                   "/plugin:command\n\nearlier conversation");
}

static void
test_handover_stays_first_for_normal_prompts (void)
{
  g_autofree char *normal =
    xd_handover_join ("earlier conversation", "continue");
  g_autofree char *path =
    xd_handover_join ("earlier conversation", "/tmp/checkout");

  g_assert_cmpstr (normal, ==, "earlier conversation\n\ncontinue");
  g_assert_cmpstr (path, ==, "earlier conversation\n\n/tmp/checkout");
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/handover/commands-first",
                   test_handover_keeps_commands_first);
  g_test_add_func ("/handover/normal-first",
                   test_handover_stays_first_for_normal_prompts);

  return g_test_run ();
}
