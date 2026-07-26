#include "app-paths.h"

#include <glib/gstdio.h>

const char *
xd_app_data_dir (void)
{
  static char *dir = NULL;

  if (g_once_init_enter_pointer (&dir))
    {
      char *path = g_build_filename (g_get_user_data_dir (), XD_DATA_NAME, NULL);

      g_mkdir_with_parents (path, 0700);

      g_once_init_leave_pointer (&dir, path);
    }

  return dir;
}

char *
xd_app_database_path (void)
{
  return g_build_filename (xd_app_data_dir (), "chats.db", NULL);
}

char *
xd_app_workspaces_root (void)
{
  char *root = g_build_filename (xd_app_data_dir (), "Workspaces", NULL);
  g_autofree char *legacy = NULL;

  if (g_file_test (root, G_FILE_TEST_EXISTS))
    return root;

  /*
   * A tree from before it moved.
   *
   * Only for the release build: a nightly picking up ~/Workspaces would be
   * taking the release's, and separate storage is the reason it has a name of
   * its own.
   */
  if (g_strcmp0 (XD_DATA_NAME, "xd") != 0)
    return root;

  legacy = g_build_filename (g_get_home_dir (), "Workspaces", NULL);
  if (!g_file_test (legacy, G_FILE_TEST_IS_DIR))
    return root;

  if (g_rename (legacy, root) == 0)
    return root;

  /*
   * Across filesystems a rename cannot work, and copying a tree of
   * repositories is not something to do behind the user's back. The old
   * place goes on being used, which is worse only in tidiness.
   */
  g_message ("keeping the workspaces at %s: they could not be moved to %s",
             legacy, root);

  g_free (root);

  return g_steal_pointer (&legacy);
}
