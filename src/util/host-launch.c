#include "host-launch.h"

/*
 * Terminals that take the directory as an argument, in the order to try.
 *
 * A terminal spawned with the right working directory usually inherits it,
 * but several are clients of an already-running server and start wherever
 * that server did, so the flag is passed where one exists.
 */
static const struct
{
  const char *program;
  const char *flag;     /* NULL: it just needs the working directory set */
} terminals[] = {
  { "ptyxis",         "--working-directory" },
  { "kgx",            "--working-directory" },
  { "gnome-terminal", "--working-directory" },
  { "foot",           "--working-directory" },
  { "alacritty",      "--working-directory" },
  { "kitty",          "--directory" },
  { "wezterm",        "--cwd" },
  { "konsole",        "--workdir" },
  { "xterm",          NULL },
};

/*
 * The environment as it was before the bundle's launcher rewrote it.
 *
 * Without this a GTK terminal reads hy's bundled settings schemas and pixbuf
 * loaders, which belong to a different build of GTK than the one it links.
 */
static void
restore_host_environment (GSubprocessLauncher *launcher)
{
  static const char *restored[] = {
    "XDG_DATA_DIRS", "LANG", "LC_ALL",
    "GIO_EXTRA_MODULES", "GTK_IM_MODULE", "GTK_PATH",
  };
  static const char *dropped[] = {
    "GSETTINGS_SCHEMA_DIR", "GSETTINGS_BACKEND", "GDK_PIXBUF_MODULE_FILE",
    "GIO_MODULE_DIR", "GSK_RENDERER", "XCURSOR_PATH",
    "FONTCONFIG_FILE", "FONTCONFIG_PATH", "XKB_CONFIG_ROOT", "XLOCALEDIR",
  };

  for (gsize i = 0; i < G_N_ELEMENTS (restored); i++)
    {
      g_autofree char *key = g_strconcat ("HY_HOST_", restored[i], NULL);
      const char *value = g_getenv (key);

      if (value != NULL && *value != '\0')
        g_subprocess_launcher_setenv (launcher, restored[i], value, TRUE);
      else
        g_subprocess_launcher_unsetenv (launcher, restored[i]);

      g_subprocess_launcher_unsetenv (launcher, key);
    }

  for (gsize i = 0; i < G_N_ELEMENTS (dropped); i++)
    g_subprocess_launcher_unsetenv (launcher, dropped[i]);
}

gboolean
hy_open_terminal (const char  *workdir,
                  GError     **error)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  const char *preferred = g_getenv ("TERMINAL");

  g_return_val_if_fail (workdir != NULL, FALSE);

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_NONE);
  g_subprocess_launcher_set_cwd (launcher, workdir);
  restore_host_environment (launcher);

  /* $TERMINAL is the user saying which one they want, so it wins -- and hy
   * cannot know its flags, so it only gets the working directory. */
  if (preferred != NULL && *preferred != '\0')
    {
      g_autoptr (GSubprocess) process =
        g_subprocess_launcher_spawn (launcher, NULL, preferred, NULL);

      if (process != NULL)
        return TRUE;
    }

  for (gsize i = 0; i < G_N_ELEMENTS (terminals); i++)
    {
      g_autoptr (GSubprocess) process = NULL;
      g_autofree char *path = g_find_program_in_path (terminals[i].program);

      if (path == NULL)
        continue;

      if (terminals[i].flag != NULL)
        process = g_subprocess_launcher_spawn (launcher, NULL, path,
                                               terminals[i].flag, workdir, NULL);
      else
        process = g_subprocess_launcher_spawn (launcher, NULL, path, NULL);

      if (process != NULL)
        return TRUE;
    }

  g_set_error (error, G_SPAWN_ERROR, G_SPAWN_ERROR_NOENT,
               "No terminal found. Set $TERMINAL to the one you use.");

  return FALSE;
}
