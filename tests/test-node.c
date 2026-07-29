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

static void
test_folder_ids_follow_the_chain (void)
{
  g_autoptr (XdNode) root = xd_node_new_folder ("/root", "Root", "root-id");
  g_autoptr (XdNode) child =
    xd_node_new_folder ("/root/child", "Child", "child-id");
  g_autoptr (XdNode) chat = xd_node_new_chat ("chat", "Chat", child);
  g_auto (GStrv) ids = NULL;

  xd_node_set_parent (child, root);
  ids = xd_node_folder_ids (xd_node_get_parent (chat));

  g_assert_cmpstr (ids[0], ==, "root-id");
  g_assert_cmpstr (ids[1], ==, "child-id");
  g_assert_null (ids[2]);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);
  g_test_add_func ("/node/attention-states", test_attention_states);
  g_test_add_func ("/node/folder-id-chain", test_folder_ids_follow_the_chain);

  return g_test_run ();
}
