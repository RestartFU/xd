#include <glib/gstdio.h>

#include "settings/folder-settings.h"
#include "settings/pane-state.h"
#include "settings/settings-resolver.h"

typedef struct
{
  char *root;
} Fixture;

static void
fixture_set_up (Fixture       *fixture,
                gconstpointer  user_data)
{
  g_autoptr (GError) error = NULL;

  fixture->root = g_dir_make_tmp ("xd-settings-XXXXXX", &error);
  g_assert_no_error (error);
}

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
fixture_tear_down (Fixture       *fixture,
                   gconstpointer  user_data)
{
  if (fixture->root != NULL)
    {
      remove_tree (fixture->root);
      g_clear_pointer (&fixture->root, g_free);
    }
}

/* Creates a directory with a .xd.json and returns the node for it. */
static XdNode *
make_folder (XdNode      *parent,
             const char  *parent_path,
             const char  *name,
             const char  *backend,
             const char  *model,
             const char  *workdir,
             const char  *instructions)
{
  g_autoptr (XdFolderSettings) settings = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *path = g_build_filename (parent_path, name, NULL);
  XdNode *node;

  g_assert_cmpint (g_mkdir_with_parents (path, 0700), ==, 0);

  settings = xd_folder_settings_ensure (path, &error);
  g_assert_no_error (error);

  settings->backend = g_strdup (backend);
  settings->model = g_strdup (model);
  settings->workdir = g_strdup (workdir);
  settings->instructions = g_strdup (instructions);
  g_assert_true (xd_folder_settings_save (settings, path, &error));
  g_assert_no_error (error);

  node = xd_node_new_folder (path, name, settings->id);
  xd_node_set_parent (node, parent);

  return node;
}

static void
test_child_overrides_parent (Fixture       *fixture,
                             gconstpointer  user_data)
{
  g_autoptr (XdNode) workspace = NULL;
  g_autoptr (XdNode) child = NULL;
  g_autoptr (XdEffectiveSettings) resolved = NULL;

  workspace = make_folder (NULL, fixture->root, "Lunar", "claude",
                           "claude-opus-5", NULL, NULL);
  child = make_folder (workspace, xd_node_get_path (workspace), "Proxy",
                       "codex", NULL, NULL, NULL);

  resolved = xd_settings_resolve (child, "claude");

  /* The child names a backend, so it wins. */
  g_assert_cmpstr (resolved->backend, ==, "codex");
  g_assert_null (resolved->backend_from);

  /* It says nothing about the model, so the parent's applies and is labelled. */
  g_assert_cmpstr (resolved->model, ==, "claude-opus-5");
  g_assert_cmpstr (resolved->model_from, ==, "Lunar");
}

/*
 * Instructions are the one thing that accumulates instead of overriding: a
 * workspace-wide rule and a folder-specific one are both meant to apply, in
 * that order.
 */
static void
test_instructions_accumulate (Fixture       *fixture,
                              gconstpointer  user_data)
{
  g_autoptr (XdNode) workspace = NULL;
  g_autoptr (XdNode) child = NULL;
  g_autoptr (XdNode) grandchild = NULL;
  g_autoptr (XdEffectiveSettings) resolved = NULL;

  workspace = make_folder (NULL, fixture->root, "Lunar", NULL, NULL, NULL,
                           "Always answer in French.");
  child = make_folder (workspace, xd_node_get_path (workspace), "Proxy",
                       NULL, NULL, NULL, "This is a Go codebase.");
  grandchild = make_folder (child, xd_node_get_path (child), "Bugs",
                            NULL, NULL, NULL, NULL);

  resolved = xd_settings_resolve (grandchild, "claude");

  g_assert_cmpstr (resolved->instructions, ==,
                   "Always answer in French.\n\nThis is a Go codebase.");
}

static void
test_falls_back_to_default_backend (Fixture       *fixture,
                                    gconstpointer  user_data)
{
  g_autoptr (XdNode) workspace = NULL;
  g_autoptr (XdEffectiveSettings) resolved = NULL;

  workspace = make_folder (NULL, fixture->root, "Personal", NULL, NULL, NULL, NULL);

  resolved = xd_settings_resolve (workspace, "codex");

  g_assert_cmpstr (resolved->backend, ==, "codex");
  g_assert_null (resolved->model);
}

/* A folder that names no working directory runs where it lives, so a chat
 * always points at something real. */
static void
test_workdir_defaults_to_the_folder (Fixture       *fixture,
                                     gconstpointer  user_data)
{
  g_autoptr (XdNode) workspace = NULL;
  g_autoptr (XdEffectiveSettings) resolved = NULL;

  workspace = make_folder (NULL, fixture->root, "Personal", NULL, NULL, NULL, NULL);

  resolved = xd_settings_resolve (workspace, "claude");

  g_assert_cmpstr (resolved->workdir, ==, xd_node_get_path (workspace));
}

/* Repositories live outside the workspace tree, so a folder can point at one
 * anywhere and its children follow. */
static void
test_workdir_is_inherited (Fixture       *fixture,
                           gconstpointer  user_data)
{
  g_autoptr (XdNode) workspace = NULL;
  g_autoptr (XdNode) child = NULL;
  g_autoptr (XdEffectiveSettings) resolved = NULL;

  workspace = make_folder (NULL, fixture->root, "Lunar", NULL, NULL,
                           "/home/someone/code/proxy", NULL);
  child = make_folder (workspace, xd_node_get_path (workspace), "Bugs",
                       NULL, NULL, NULL, NULL);

  resolved = xd_settings_resolve (child, "claude");

  g_assert_cmpstr (resolved->workdir, ==, "/home/someone/code/proxy");
  g_assert_cmpstr (resolved->workdir_from, ==, "Lunar");
}

static void
test_pane_state_keeps_schema_type (Fixture       *fixture,
                                   gconstpointer  user_data)
{
  GVariantBuilder builder;
  g_autoptr (GVariant) states = NULL;
  g_autoptr (GVariant) updated = NULL;
  guint state = 0;

  g_variant_builder_init (&builder, G_VARIANT_TYPE ("a{su}"));
  g_variant_builder_add (&builder, "{su}", "local/one", 1);
  g_variant_builder_add (
    &builder, "{su}", "remote/host:4001/two", 4);
  states = g_variant_ref_sink (g_variant_builder_end (&builder));
  updated = xd_pane_state_update (states, "remote/host:4001/two", 3);

  g_assert_true (
    g_variant_is_of_type (updated, G_VARIANT_TYPE ("a{su}")));
  g_assert_true (
    g_variant_lookup (updated, "local/one", "u", &state));
  g_assert_cmpuint (state, ==, 1);
  g_assert_true (
    g_variant_lookup (updated, "remote/host:4001/two", "u", &state));
  g_assert_cmpuint (state, ==, 3);
  g_assert_cmpuint (g_variant_n_children (updated), ==, 2);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

#define ADD(path, func) \
  g_test_add (path, Fixture, NULL, fixture_set_up, func, fixture_tear_down)

  ADD ("/settings/child-overrides-parent", test_child_overrides_parent);
  ADD ("/settings/instructions-accumulate", test_instructions_accumulate);
  ADD ("/settings/default-backend", test_falls_back_to_default_backend);
  ADD ("/settings/workdir-default", test_workdir_defaults_to_the_folder);
  ADD ("/settings/workdir-inherited", test_workdir_is_inherited);
  ADD ("/settings/pane-state-schema", test_pane_state_keeps_schema_type);

#undef ADD

  return g_test_run ();
}
