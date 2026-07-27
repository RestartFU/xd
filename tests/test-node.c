#include "tree/xd-node.h"

static void
test_attention_states (void)
{
  g_autoptr (XdNode) chat = xd_node_new_chat ("chat", "Chat", NULL);

  xd_node_set_state (chat, XD_NODE_DONE);
  g_assert_cmpint (xd_node_get_state (chat), ==, XD_NODE_DONE);

  /* Opening a completed chat acknowledges it. */
  xd_node_set_active (chat, TRUE);
  g_assert_true (xd_node_is_active (chat));
  g_assert_cmpint (xd_node_get_state (chat), ==, XD_NODE_IDLE);

  /* Opening a question does not answer it. */
  xd_node_set_state (chat, XD_NODE_WAITING);
  xd_node_set_active (chat, TRUE);
  g_assert_cmpint (xd_node_get_state (chat), ==, XD_NODE_WAITING);

  xd_node_set_active (chat, FALSE);
  g_assert_false (xd_node_is_active (chat));
  g_assert_cmpint (xd_node_get_state (chat), ==, XD_NODE_WAITING);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);
  g_test_add_func ("/node/attention-states", test_attention_states);

  return g_test_run ();
}
