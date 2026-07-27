#include "util/host-launch.h"

static void
test_restores_host_locale_paths (void)
{
  g_auto (GStrv) env = NULL;

  g_setenv ("GTK_PATH", "/host/gtk", TRUE);
  g_unsetenv ("XD_HOST_GTK_PATH");
  g_setenv ("LOCPATH", "/bundle/share/locale-data", TRUE);
  g_setenv ("LOCALE_ARCHIVE", "", TRUE);
  g_setenv ("XD_HOST_LOCPATH", "", TRUE);
  g_setenv ("XD_HOST_LOCALE_ARCHIVE",
            "/run/current-system/sw/lib/locale/locale-archive", TRUE);

  env = xd_host_environ ();

  g_assert_null (g_environ_getenv (env, "LOCPATH"));
  g_assert_cmpstr (
    g_environ_getenv (env, "LOCALE_ARCHIVE"), ==,
    "/run/current-system/sw/lib/locale/locale-archive");
  g_assert_null (g_environ_getenv (env, "XD_HOST_LOCPATH"));
  g_assert_null (g_environ_getenv (env, "XD_HOST_LOCALE_ARCHIVE"));
  g_assert_cmpstr (g_environ_getenv (env, "GTK_PATH"), ==, "/host/gtk");
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/host-launch/restores-host-locale-paths",
                   test_restores_host_locale_paths);

  return g_test_run ();
}
