#include "host-launch.h"

/* Overridden by the launcher, and recorded there as HY_HOST_<name>. */
static const char *rewritten[] = {
  "XDG_DATA_DIRS", "LANG", "LC_ALL",
  "GIO_EXTRA_MODULES", "GTK_IM_MODULE", "GTK_PATH",
};

/* Set by the launcher with no host value to go back to. */
static const char *bundle_only[] = {
  "GSETTINGS_SCHEMA_DIR", "GSETTINGS_BACKEND", "GDK_PIXBUF_MODULE_FILE",
  "GIO_MODULE_DIR", "GSK_RENDERER", "XCURSOR_PATH",
  "FONTCONFIG_FILE", "FONTCONFIG_PATH", "XKB_CONFIG_ROOT", "XLOCALEDIR",
};

GStrv
hy_host_environ (void)
{
  GStrv env = g_get_environ ();

  for (gsize i = 0; i < G_N_ELEMENTS (rewritten); i++)
    {
      g_autofree char *key = g_strconcat ("HY_HOST_", rewritten[i], NULL);
      const char *value = g_environ_getenv (env, key);

      /* An empty recorded value means the host did not set it either. */
      if (value != NULL && *value != '\0')
        env = g_environ_setenv (env, rewritten[i], value, TRUE);
      else
        env = g_environ_unsetenv (env, rewritten[i]);

      env = g_environ_unsetenv (env, key);
    }

  for (gsize i = 0; i < G_N_ELEMENTS (bundle_only); i++)
    env = g_environ_unsetenv (env, bundle_only[i]);

  return env;
}
