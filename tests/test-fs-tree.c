#include <glib/gstdio.h>

#include "settings/folder-settings.h"
#include "tree/fs-tree.h"

typedef struct
{
  char *root;
  XdFsTree *tree;
} Fixture;

static void
remove_tree (const char *path)
{
  g_autoptr (GDir) dir = g_dir_open (path, 0, NULL);
  const char *name;

  while (dir != NULL && (name = g_dir_read_name (dir)) != NULL)
    {
      g_autofree char *child = g_build_filename (path, name, NULL);

      if (g_file_test (child, G_FILE_TEST_IS_DIR))
        remove_tree (child);
      else
        g_remove (child);
    }

  g_rmdir (path);
}

static void
fixture_set_up (Fixture       *fixture,
                gconstpointer  user_data)
{
  fixture->root = g_dir_make_tmp ("xd-fs-tree-XXXXXX", NULL);
  g_assert_nonnull (fixture->root);
}

static void
fixture_tear_down (Fixture       *fixture,
                   gconstpointer  user_data)
{
  g_clear_object (&fixture->tree);

  if (fixture->root != NULL)
    {
      remove_tree (fixture->root);
      g_clear_pointer (&fixture->root, g_free);
    }
}

static gboolean
has_n_children (gpointer user_data)
{
  gpointer *values = user_data;
  XdNode *node = values[0];
  guint expected = GPOINTER_TO_UINT (values[1]);

  return g_list_model_get_n_items (
    G_LIST_MODEL (xd_node_get_children (node))) == expected;
}

static void
wait_for_children (XdNode *node,
                   guint   expected)
{
  gpointer values[] = { node, GUINT_TO_POINTER (expected) };
  gint64 deadline = g_get_monotonic_time () + G_TIME_SPAN_SECOND * 3;

  while (!has_n_children (values) && g_get_monotonic_time () < deadline)
    g_main_context_iteration (NULL, TRUE);

  g_assert_true (has_n_children (values));
}

static XdNode *
child_at (XdNode *node,
          guint   position)
{
  return g_list_model_get_item (
    G_LIST_MODEL (xd_node_get_children (node)), position);
}

static void
test_only_managed_nested_folders_appear (Fixture       *fixture,
                                         gconstpointer  user_data)
{
  g_autofree char *workspace = g_build_filename (fixture->root, "Workspace", NULL);
  g_autofree char *topic = g_build_filename (workspace, "Topic", NULL);
  g_autofree char *source = g_build_filename (workspace, "src", NULL);
  g_autofree char *source_settings =
    g_build_filename (source, XD_FOLDER_SETTINGS_FILE, NULL);
  g_autoptr (XdFolderSettings) settings = NULL;
  g_autoptr (XdNode) workspace_node = NULL;
  g_autoptr (XdNode) topic_node = NULL;

  g_assert_cmpint (g_mkdir_with_parents (topic, 0700), ==, 0);
  g_assert_cmpint (g_mkdir_with_parents (source, 0700), ==, 0);

  settings = xd_folder_settings_ensure (topic, NULL);
  g_assert_nonnull (settings);

  fixture->tree = xd_fs_tree_new (fixture->root, NULL);
  wait_for_children (xd_fs_tree_get_root (fixture->tree), 1);

  workspace_node = child_at (xd_fs_tree_get_root (fixture->tree), 0);
  wait_for_children (workspace_node, 1);
  topic_node = child_at (workspace_node, 0);

  g_assert_cmpstr (xd_node_get_name (topic_node), ==, "Topic");
  g_assert_false (g_file_test (source_settings, G_FILE_TEST_EXISTS));
}

static void
test_repository_is_a_leaf (Fixture       *fixture,
                           gconstpointer  user_data)
{
  g_autofree char *repo = g_build_filename (fixture->root, "Repo", NULL);
  g_autofree char *git = g_build_filename (repo, ".git", NULL);
  g_autofree char *source = g_build_filename (repo, "src", NULL);
  g_autofree char *source_settings =
    g_build_filename (source, XD_FOLDER_SETTINGS_FILE, NULL);
  g_autoptr (XdNode) repo_node = NULL;

  g_assert_cmpint (g_mkdir_with_parents (git, 0700), ==, 0);
  g_assert_cmpint (g_mkdir_with_parents (source, 0700), ==, 0);

  fixture->tree = xd_fs_tree_new (fixture->root, NULL);
  wait_for_children (xd_fs_tree_get_root (fixture->tree), 1);
  repo_node = child_at (xd_fs_tree_get_root (fixture->tree), 0);

  /* Let any mistakenly scheduled recursive scan finish. */
  {
    gint64 deadline = g_get_monotonic_time () + 200 * G_TIME_SPAN_MILLISECOND;

    while (g_get_monotonic_time () < deadline)
      g_main_context_iteration (NULL, FALSE);
  }

  g_assert_cmpuint (
    g_list_model_get_n_items (
      G_LIST_MODEL (xd_node_get_children (repo_node))), ==, 0);
  g_assert_false (g_file_test (source_settings, G_FILE_TEST_EXISTS));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

#define ADD(path, func) \
  g_test_add (path, Fixture, NULL, fixture_set_up, func, fixture_tear_down)

  ADD ("/fs-tree/only-managed-nested-folders",
       test_only_managed_nested_folders_appear);
  ADD ("/fs-tree/repository-is-leaf", test_repository_is_a_leaf);

#undef ADD

  return g_test_run ();
}
