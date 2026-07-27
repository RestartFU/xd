#include <glib.h>
#include <json-glib/json-glib.h>

#include "integrations/discord-presence.h"

static void
test_activity_payload (void)
{
  g_autofree char *payload =
    xd_discord_presence_build_activity ("Agent working", 1234, 42, 7);
  g_autoptr (JsonParser) parser = json_parser_new ();
  JsonObject *root;
  JsonObject *args;
  JsonObject *activity;
  JsonObject *timestamps;

  g_assert_true (json_parser_load_from_data (parser, payload, -1, NULL));

  root = json_node_get_object (json_parser_get_root (parser));
  g_assert_cmpstr (json_object_get_string_member (root, "cmd"), ==,
                   "SET_ACTIVITY");
  g_assert_cmpstr (json_object_get_string_member (root, "nonce"), ==, "7");

  args = json_object_get_object_member (root, "args");
  g_assert_cmpint (json_object_get_int_member (args, "pid"), ==, 42);

  activity = json_object_get_object_member (args, "activity");
  g_assert_cmpstr (json_object_get_string_member (activity, "details"), ==,
                   "Building with AI");
  g_assert_cmpstr (json_object_get_string_member (activity, "state"), ==,
                   "Agent working");
  g_assert_false (json_object_has_member (activity, "chat"));
  g_assert_false (json_object_has_member (activity, "workspace"));
  g_assert_false (json_object_has_member (activity, "repository"));

  timestamps = json_object_get_object_member (activity, "timestamps");
  g_assert_cmpint (json_object_get_int_member (timestamps, "start"), ==, 1234);
}

int
main (int argc,
      char **argv)
{
  g_test_init (&argc, &argv, NULL);
  g_test_add_func ("/discord-presence/activity-payload", test_activity_payload);

  return g_test_run ();
}
