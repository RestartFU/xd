#include "host-launch.h"

#include <gio/gio.h>

/* Overridden by the launcher, and recorded there as XD_HOST_<name>. */
static const char *rewritten[] = {
  "XDG_DATA_DIRS", "LANG", "LC_ALL", "LOCPATH", "LOCALE_ARCHIVE",
  "GIO_EXTRA_MODULES", "GTK_IM_MODULE", "GTK_PATH", "GTK_THEME",
};

/* Set by the launcher with no host value to go back to. */
static const char *bundle_only[] = {
  "GSETTINGS_SCHEMA_DIR", "GSETTINGS_BACKEND", "GDK_PIXBUF_MODULE_FILE",
  "GIO_MODULE_DIR", "GSK_RENDERER", "XCURSOR_PATH",
  "FONTCONFIG_FILE", "FONTCONFIG_PATH", "XKB_CONFIG_ROOT", "XLOCALEDIR",
  "GTK_DATA_PREFIX", "GTK_EXE_PREFIX", "XD_AGENT_SECRETS_FILE",
};

GStrv
xd_host_environ (void)
{
  GStrv env = g_get_environ ();

  for (gsize i = 0; i < G_N_ELEMENTS (rewritten); i++)
    {
      g_autofree char *key = g_strconcat ("XD_HOST_", rewritten[i], NULL);
      const char *value = g_environ_getenv (env, key);

      /* A development build has no launcher markers and already runs in the
       * host environment. Leave those values alone. */
      if (value == NULL)
        continue;

      /* An empty recorded value means the host did not set it either. */
      if (*value != '\0')
        env = g_environ_setenv (env, rewritten[i], value, TRUE);
      else
        env = g_environ_unsetenv (env, rewritten[i]);

      env = g_environ_unsetenv (env, key);
    }

  for (gsize i = 0; i < G_N_ELEMENTS (bundle_only); i++)
    env = g_environ_unsetenv (env, bundle_only[i]);

  return env;
}

void
xd_host_open_uri (const char *uri)
{
  g_return_if_fail (uri != NULL);

#if defined(G_OS_WIN32) || defined(__APPLE__)
  g_app_info_launch_default_for_uri (uri, NULL, NULL);
#else
  g_auto (GStrv) env = xd_host_environ ();
  const char *argv[] = { "xdg-open", uri, NULL };

  g_spawn_async (NULL, (char **) argv, env, G_SPAWN_SEARCH_PATH,
                 NULL, NULL, NULL, NULL);
#endif
}
