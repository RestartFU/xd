#include "xd-app.h"
#include "util/app-paths.h"
#include "util/host-launch.h"

#include <errno.h>
#include <stdio.h>

#ifdef __APPLE__
#include <mach-o/dyld.h>
#endif

#ifdef G_OS_WIN32
#include <glib/gstdio.h>
#include <glib/gwin32.h>
#endif

#if XD_HAS_SERVER
#include "remote/server.h"
#include <json-glib/json-glib.h>
#include <unistd.h>
#endif

#ifdef G_OS_WIN32
/*
 * Dynamic GTK modules are not linked into the executable. Point their caches
 * at the MSI payload before GIO or GTK first asks for one.
 */
static void
prepare_windows_runtime (void)
{
  g_autofree char *prefix =
    g_win32_get_package_installation_directory_of_module (NULL);
  g_autofree char *gio_modules = NULL;
  g_autofree char *cache_template = NULL;
  g_autofree char *cache_dir = NULL;
  g_autofree char *cache_path = NULL;
  g_autofree char *template_text = NULL;
  g_autoptr (GString) cache = NULL;

  if (prefix == NULL)
    return;

  gio_modules = g_build_filename (prefix, "lib", "gio", "modules", NULL);
  g_setenv ("GIO_MODULE_DIR", gio_modules, TRUE);

  cache_template =
    g_build_filename (prefix, "etc", "gdk-pixbuf-loaders.cache.in", NULL);
  if (!g_file_get_contents (cache_template, &template_text, NULL, NULL))
    return;

  /* Cache syntax accepts forward slashes and then needs no escaping for a
   * normal Windows installation path. */
  g_strdelimit (prefix, "\\", '/');
  cache = g_string_new (template_text);
  g_string_replace (cache, "@BUNDLE@", prefix, 0);

  cache_dir = g_build_filename (g_get_user_cache_dir (), XD_DATA_NAME, NULL);
  cache_path = g_build_filename (cache_dir, "gdk-pixbuf-loaders.cache", NULL);
  if (g_mkdir_with_parents (cache_dir, 0700) == 0 &&
      g_file_set_contents (cache_path, cache->str, cache->len, NULL))
    g_setenv ("GDK_PIXBUF_MODULE_FILE", cache_path, TRUE);
}
#endif

#ifdef __APPLE__
static void
remember_host_value (const char *name)
{
  g_autofree char *key = g_strconcat ("XD_HOST_", name, NULL);
  const char *value = g_getenv (name);

  g_setenv (key, value != NULL ? value : "", TRUE);
}

static char *
expand_macos_template (const char *resources,
                       const char *template_name,
                       const char *output_name)
{
  g_autofree char *template_path =
    g_build_filename (resources, "etc", template_name, NULL);
  g_autofree char *template_text = NULL;
  g_autofree char *cache_dir = NULL;
  g_autofree char *output_path = NULL;
  g_autoptr (GString) output = NULL;

  if (!g_file_get_contents (template_path, &template_text, NULL, NULL))
    return NULL;

  output = g_string_new (template_text);
  g_string_replace (output, "@BUNDLE@", resources, 0);

  cache_dir = g_build_filename (g_get_user_cache_dir (), XD_DATA_NAME, NULL);
  output_path = g_build_filename (cache_dir, output_name, NULL);
  if (g_mkdir_with_parents (cache_dir, 0700) != 0 ||
      !g_file_set_contents (output_path, output->str, output->len, NULL))
    return NULL;

  return g_steal_pointer (&output_path);
}

/*
 * A .app moves as one directory, so paths compiled into Homebrew libraries
 * cannot name runtime data. Resolve everything from Contents/MacOS/xd before
 * GIO or GTK starts, while preserving the user's environment for child CLIs.
 */
static void
prepare_macos_runtime (void)
{
  static const char *host_names[] = {
    "XDG_DATA_DIRS", "LANG", "LC_ALL", "LOCPATH", "LOCALE_ARCHIVE",
    "GIO_EXTRA_MODULES",
    "GTK_IM_MODULE", "GTK_PATH",
  };
  uint32_t size = 0;
  g_autofree char *executable = NULL;
  g_autofree char *canonical = NULL;
  g_autofree char *macos = NULL;
  g_autofree char *contents = NULL;
  g_autofree char *resources = NULL;
  g_autofree char *share = NULL;
  g_autofree char *gio_modules = NULL;
  g_autofree char *schemas = NULL;
  g_autofree char *pixbuf_cache = NULL;
  g_autofree char *fontconfig_file = NULL;
  g_autofree char *fontconfig_path = NULL;

  if (_NSGetExecutablePath (NULL, &size) == 0 || size == 0)
    return;

  executable = g_malloc (size);
  if (_NSGetExecutablePath (executable, &size) != 0)
    return;

  canonical = g_canonicalize_filename (executable, NULL);
  macos = g_path_get_dirname (canonical);
  contents = g_path_get_dirname (macos);
  resources = g_build_filename (contents, "Resources", NULL);
  if (!g_file_test (resources, G_FILE_TEST_IS_DIR))
    return;

  for (gsize i = 0; i < G_N_ELEMENTS (host_names); i++)
    remember_host_value (host_names[i]);

  share = g_build_filename (resources, "share", NULL);
  gio_modules = g_build_filename (resources, "lib", "gio", "modules", NULL);
  schemas = g_build_filename (share, "glib-2.0", "schemas", NULL);
  fontconfig_path = g_build_filename (resources, "etc", "fonts", NULL);
  pixbuf_cache = g_build_filename (
    resources, "lib", "gdk-pixbuf-2.0", "2.10.0", "loaders.cache", NULL);
  fontconfig_file = expand_macos_template (
    resources, "fonts.conf.in", "fonts.conf");

  g_setenv ("XDG_DATA_DIRS", share, TRUE);
  g_setenv ("GIO_EXTRA_MODULES", gio_modules, TRUE);
  g_setenv ("GSETTINGS_SCHEMA_DIR", schemas, TRUE);
  g_setenv ("GSETTINGS_BACKEND", "keyfile", FALSE);
  g_setenv ("GTK_DATA_PREFIX", resources, TRUE);
  g_setenv ("GTK_EXE_PREFIX", resources, TRUE);
  g_setenv ("GTK_PATH", resources, TRUE);
  g_setenv ("GTK_IM_MODULE", "gtk-im-context-simple", TRUE);

  if (g_file_test (pixbuf_cache, G_FILE_TEST_IS_REGULAR))
    g_setenv ("GDK_PIXBUF_MODULE_FILE", pixbuf_cache, TRUE);
  if (fontconfig_file != NULL)
    {
      g_setenv ("FONTCONFIG_FILE", fontconfig_file, TRUE);
      g_setenv ("FONTCONFIG_PATH", fontconfig_path, TRUE);
    }
}
#endif

/*
 * Loads the daemon's certificate, minting one the first time.
 *
 * GLib can read certificates but not create them, so creation is one spawn
 * of openssl -- present nearly everywhere, and the error says so plainly
 * where it is not. Self-signed on purpose: the client pins this exact
 * certificate at pairing time, the way SSH pins a host key, so a CA would
 * add nothing but a bill.
 */
#if XD_HAS_SERVER
static GTlsCertificate *
ensure_certificate (GError **error)
{
  const char *dir = xd_app_data_dir ();
  g_autofree char *cert_path = g_build_filename (dir, "server-cert.pem", NULL);
  g_autofree char *key_path = g_build_filename (dir, "server-key.pem", NULL);

  if (!g_file_test (cert_path, G_FILE_TEST_EXISTS))
    {
      const char *argv[] = {
        "openssl", "req", "-x509", "-newkey", "rsa:2048",
        "-keyout", key_path, "-out", cert_path,
        "-days", "3650", "-nodes", "-subj", "/CN=xd",
        NULL,
      };
      g_autoptr (GSubprocess) process = NULL;

      process = g_subprocess_newv ((const char * const *) argv,
                                   G_SUBPROCESS_FLAGS_STDERR_SILENCE, error);
      if (process == NULL || !g_subprocess_wait_check (process, NULL, error))
        {
          g_prefix_error (error, "Cannot create the server certificate "
                                 "(is openssl installed?): ");
          return NULL;
        }
    }

  return g_tls_certificate_new_from_files (cert_path, key_path, error);
}

#define DAEMON_UPDATE_FIRST_SECONDS 8
#define DAEMON_UPDATE_EVERY_SECONDS (60 * 5)

typedef struct
{
  XdRemoteServer *server;      /* unowned; run_serve owns it */
  GMainLoop *loop;             /* unowned */
  GCancellable *cancellable;
  char *install_dir;
  char *launcher;
  GStrv restart_argv;
  guint first_check_id;
  guint repeating_check_id;
  gboolean busy;
  gboolean restart_ready;
} DaemonUpdater;

static char *
daemon_install_dir (void)
{
  g_autofree char *self = g_file_read_link ("/proc/self/exe", NULL);
  g_autofree char *bin = NULL;
  g_autofree char *dir = NULL;
  g_autofree char *launcher = NULL;
  g_autofree char *expected = NULL;

  if (self == NULL)
    return NULL;

  bin = g_path_get_dirname (self);
  dir = g_path_get_dirname (bin);
  launcher = g_build_filename (dir, "xd.sh", NULL);
  expected = g_build_filename (g_get_home_dir (), ".local", "opt",
                               XD_DATA_NAME, NULL);

  if (!g_file_test (launcher, G_FILE_TEST_IS_EXECUTABLE) ||
      g_strcmp0 (dir, expected) != 0)
    return NULL;

  return g_steal_pointer (&dir);
}

static char *
daemon_latest_from_json (const char *json)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  JsonObject *release;

  if (!json_parser_load_from_data (parser, json, -1, NULL) ||
      !JSON_NODE_HOLDS_OBJECT (json_parser_get_root (parser)))
    return NULL;

  release = json_node_get_object (json_parser_get_root (parser));
  return g_strdup (json_object_get_string_member_with_default (
    release, g_strcmp0 (XD_CHANNEL, "nightly") == 0
      ? "target_commitish" : "tag_name", NULL));
}

static gboolean
daemon_update_is_newer (const char *latest)
{
  if (latest == NULL || *latest == '\0')
    return FALSE;

  if (g_strcmp0 (XD_CHANNEL, "nightly") == 0)
    return XD_COMMIT[0] != '\0' && !g_str_has_prefix (latest, XD_COMMIT);

  if (latest[0] == 'v')
    latest++;

  return g_strcmp0 (latest, XD_VERSION) != 0;
}

static GSubprocess *
daemon_spawn_host (GSubprocessFlags   flags,
                   const char *const *argv,
                   GError           **error)
{
  g_autoptr (GSubprocessLauncher) launcher =
    g_subprocess_launcher_new (flags);
  g_auto (GStrv) environment = xd_host_environ ();

  if (environment != NULL)
    g_subprocess_launcher_set_environ (launcher, environment);

  return g_subprocess_launcher_spawnv (launcher, argv, error);
}

static void
daemon_update_resume_after_failure (DaemonUpdater *updater)
{
  g_autoptr (GError) error = NULL;

  if (!xd_remote_server_resume_interrupted (updater->server, &error))
    g_warning ("cannot resume chats after failed update: %s", error->message);

  updater->busy = FALSE;
}

static void
on_daemon_update_installed (GObject      *source,
                            GAsyncResult *result,
                            gpointer      user_data)
{
  DaemonUpdater *updater = user_data;
  g_autoptr (GError) error = NULL;

  if (!g_subprocess_wait_check_finish (G_SUBPROCESS (source), result, &error))
    {
      g_warning ("daemon update failed: %s", error->message);
      daemon_update_resume_after_failure (updater);
      return;
    }

  updater->restart_ready = TRUE;
  g_main_loop_quit (updater->loop);
}

static void
on_daemon_quiesced (GObject      *source,
                    GAsyncResult *result,
                    gpointer      user_data)
{
  DaemonUpdater *updater = user_data;
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *line = NULL;

  if (!xd_remote_server_quiesce_finish (
        XD_REMOTE_SERVER (source), result, &error))
    {
      g_warning ("cannot prepare daemon update: %s", error->message);
      updater->busy = FALSE;
      return;
    }

  line = g_strcmp0 (XD_CHANNEL, "nightly") == 0
    ? g_strdup ("curl -fsSL https://github.com/" XD_REPO
                "/releases/download/nightly/install.sh | sh")
    : g_strdup ("curl -fsSL https://github.com/" XD_REPO
                "/releases/latest/download/install.sh | sh -s -- --release");

  {
    const char *argv[] = { "sh", "-c", line, NULL };

    process = daemon_spawn_host (G_SUBPROCESS_FLAGS_NONE, argv, &error);
  }

  if (process == NULL)
    {
      g_warning ("cannot start daemon update: %s", error->message);
      daemon_update_resume_after_failure (updater);
      return;
    }

  g_subprocess_wait_check_async (process, updater->cancellable,
                                 on_daemon_update_installed, updater);
}

static void
on_daemon_update_checked (GObject      *source,
                          GAsyncResult *result,
                          gpointer      user_data)
{
  DaemonUpdater *updater = user_data;
  g_autoptr (GBytes) out = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *latest = NULL;
  const char *json;
  gsize length;

  if (!g_subprocess_communicate_finish (
        G_SUBPROCESS (source), result, &out, NULL, &error))
    {
      g_debug ("daemon update check failed: %s", error->message);
      updater->busy = FALSE;
      return;
    }

  json = g_bytes_get_data (out, &length);
  latest = json != NULL && length > 0 ? daemon_latest_from_json (json) : NULL;
  if (!daemon_update_is_newer (latest))
    {
      updater->busy = FALSE;
      return;
    }

  printf ("xd serve: update available; stopping active turns safely\n");
  fflush (stdout);
  xd_remote_server_quiesce_async (
    updater->server, updater->cancellable, on_daemon_quiesced, updater);
}

static void
daemon_update_check (DaemonUpdater *updater)
{
  g_autoptr (GSubprocess) process = NULL;
  g_autoptr (GError) error = NULL;
  g_autofree char *url = NULL;

  if (updater->busy)
    return;

  updater->busy = TRUE;
  url = g_strcmp0 (XD_CHANNEL, "nightly") == 0
    ? g_strdup ("https://api.github.com/repos/" XD_REPO
                "/releases/tags/nightly")
    : g_strdup ("https://api.github.com/repos/" XD_REPO "/releases/latest");

  {
    const char *argv[] = {
      "curl", "-fsSL", "--max-time", "20",
      "-H", "Accept: application/vnd.github+json", url, NULL,
    };

    process = daemon_spawn_host (G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                 G_SUBPROCESS_FLAGS_STDERR_SILENCE,
                                 argv, &error);
  }

  if (process == NULL)
    {
      g_debug ("cannot check daemon update: %s", error->message);
      updater->busy = FALSE;
      return;
    }

  g_subprocess_communicate_async (
    process, NULL, updater->cancellable, on_daemon_update_checked, updater);
}

static gboolean
daemon_update_first_check (gpointer user_data)
{
  DaemonUpdater *updater = user_data;

  updater->first_check_id = 0;
  daemon_update_check (updater);
  return G_SOURCE_REMOVE;
}

static gboolean
daemon_update_repeating_check (gpointer user_data)
{
  daemon_update_check (user_data);
  return G_SOURCE_CONTINUE;
}

static GStrv
daemon_restart_argv (int          argc,
                     char        *argv[],
                     const char  *launcher)
{
  GPtrArray *args = g_ptr_array_new_with_free_func (g_free);

  g_ptr_array_add (args, g_strdup (launcher));
  for (int i = 1; i < argc; i++)
    {
      /* Pairing is an explicit one-use action, not daemon configuration. */
      if (g_strcmp0 (argv[i], "--pair") != 0)
        g_ptr_array_add (args, g_strdup (argv[i]));
    }
  g_ptr_array_add (args, NULL);

  return (GStrv) g_ptr_array_free (args, FALSE);
}

static void
daemon_updater_init (DaemonUpdater *updater,
                     XdRemoteServer *server,
                     GMainLoop      *loop,
                     int             argc,
                     char           *argv[],
                     char           *install_dir)
{
  updater->server = server;
  updater->loop = loop;
  updater->cancellable = g_cancellable_new ();
  updater->install_dir = g_strdup (install_dir);
  updater->launcher = g_build_filename (install_dir, "xd.sh", NULL);
  updater->restart_argv =
    daemon_restart_argv (argc, argv, updater->launcher);
  updater->first_check_id = g_timeout_add_seconds (
    DAEMON_UPDATE_FIRST_SECONDS, daemon_update_first_check, updater);
  updater->repeating_check_id = g_timeout_add_seconds (
    DAEMON_UPDATE_EVERY_SECONDS, daemon_update_repeating_check, updater);
}

static void
daemon_updater_clear (DaemonUpdater *updater)
{
  g_clear_handle_id (&updater->first_check_id, g_source_remove);
  g_clear_handle_id (&updater->repeating_check_id, g_source_remove);
  if (updater->cancellable != NULL)
    g_cancellable_cancel (updater->cancellable);
  g_clear_object (&updater->cancellable);
  g_clear_pointer (&updater->install_dir, g_free);
  g_clear_pointer (&updater->launcher, g_free);
  g_clear_pointer (&updater->restart_argv, g_strfreev);
}
#endif

static void
print_version (void)
{
  printf ("xd %s\n", XD_VERSION_STRING);
}

#if XD_HAS_SERVER
static gboolean
repair_daemon_cwd (GError **error)
{
  g_autofree char *cwd = getcwd (NULL, 0);
  int saved_errno;

  if (cwd != NULL)
    return TRUE;

  /*
   * A long-running shell may still refer to a directory that was deleted.
   * The daemon uses absolute data paths, but updater shells and the replacement
   * launcher inherit its cwd and otherwise repeat getcwd errors indefinitely.
   */
  if (chdir (g_get_home_dir ()) == 0 || chdir ("/") == 0)
    return TRUE;

  saved_errno = errno;
  g_set_error (error, G_FILE_ERROR, g_file_error_from_errno (saved_errno),
               "Cannot select a working directory for the daemon: %s",
               g_strerror (saved_errno));
  return FALSE;
}
#endif

static int
run_serve (int argc, char *argv[])
{
#if !XD_HAS_SERVER
  fprintf (stderr, "xd serve is not available in this build yet.\n");
  return 1;
#else
  g_autoptr (GError) error = NULL;
  g_autoptr (XdStorage) storage = NULL;
  g_autoptr (GTlsCertificate) certificate = NULL;
  g_autoptr (XdRemoteServer) server = NULL;
  g_autoptr (GMainLoop) loop = NULL;
  g_autofree char *db_path = NULL;
  g_autofree char *root = NULL;
  g_autofree char *install_dir = NULL;
  g_autofree char *restart_launcher = NULL;
  g_auto (GStrv) restart_argv = NULL;
  DaemonUpdater updater = { 0 };
  guint16 port = 4001;
  gboolean pair = FALSE;
  gboolean auto_update = FALSE;

  if (!repair_daemon_cwd (&error))
    {
      fprintf (stderr, "xd serve: %s\n", error->message);
      return 1;
    }

  for (int i = 2; i < argc; i++)
    {
      if (g_strcmp0 (argv[i], "--port") == 0 && i + 1 < argc)
        port = (guint16) g_ascii_strtoull (argv[++i], NULL, 10);
      else if (g_strcmp0 (argv[i], "--pair") == 0)
        pair = TRUE;
      else if (g_strcmp0 (argv[i], "--root") == 0 && i + 1 < argc)
        root = g_strdup (argv[++i]);
      else if (g_strcmp0 (argv[i], "--auto-update") == 0)
        auto_update = TRUE;
      else
        {
          fprintf (stderr, "usage: xd serve [--port N] [--pair] [--root DIR]"
                           " [--auto-update]\n");
          return 1;
        }
    }

  if (auto_update && (install_dir = daemon_install_dir ()) == NULL)
    {
      fprintf (stderr, "xd serve: --auto-update requires an installed bundle.\n");
      return 1;
    }

  if (root == NULL)
    root = xd_app_workspaces_root ();

  db_path = xd_app_database_path ();
  storage = xd_storage_new (db_path, &error);
  if (storage == NULL)
    {
      fprintf (stderr, "xd serve: %s\n", error->message);
      return 1;
    }

  certificate = ensure_certificate (&error);
  if (certificate == NULL)
    {
      fprintf (stderr, "xd serve: %s\n", error->message);
      return 1;
    }

  server = xd_remote_server_new (storage, root, port, certificate, &error);
  if (server == NULL)
    {
      fprintf (stderr, "xd serve: %s\n", error->message);
      return 1;
    }

  if (!xd_remote_server_resume_interrupted (server, &error))
    {
      fprintf (stderr, "xd serve: cannot resume interrupted chats: %s\n",
               error->message);
      g_clear_error (&error);
    }

  printf ("xd serve: %s, listening on %u, workspaces at %s\n",
          XD_VERSION_STRING, xd_remote_server_get_port (server), root);

  if (pair)
    {
      g_autofree char *code = xd_remote_server_arm_pairing (server, 300);

      printf ("pairing code (5 minutes, one use): %s\n", code);
    }

  fflush (stdout);

  loop = g_main_loop_new (NULL, FALSE);
  if (auto_update)
    {
      printf ("xd serve: automatic updates enabled\n");
      fflush (stdout);
      daemon_updater_init (&updater, server, loop, argc, argv, install_dir);
    }

  g_main_loop_run (loop);

  if (updater.restart_ready)
    {
      restart_launcher = g_strdup (updater.launcher);
      restart_argv = g_strdupv (updater.restart_argv);
    }
  daemon_updater_clear (&updater);

  if (restart_launcher != NULL)
    {
      g_clear_object (&server);
      g_clear_object (&certificate);
      g_clear_object (&storage);
      g_clear_pointer (&loop, g_main_loop_unref);

      execv (restart_launcher, restart_argv);
      fprintf (stderr, "xd serve: cannot restart updated daemon: %s\n",
               g_strerror (errno));
      return 1;
    }

  return 0;
#endif
}

int
main (int argc, char *argv[])
{
#ifdef G_OS_WIN32
  prepare_windows_runtime ();
#endif
#ifdef __APPLE__
  prepare_macos_runtime ();
#endif

  /*
   * Before GTK sees argv: the daemon must run without a display at all, and
   * neither must saying which build this is -- the usual reason to ask is that
   * something is not working.
   */
  if (argc > 1 && (g_strcmp0 (argv[1], "--version") == 0 ||
                   g_strcmp0 (argv[1], "-v") == 0))
    {
      print_version ();
      return 0;
    }

  if (argc > 1 && g_strcmp0 (argv[1], "serve") == 0)
    return run_serve (argc, argv);

  {
    g_autoptr (XdApplication) app = xd_application_new ();

    return g_application_run (G_APPLICATION (app), argc, argv);
  }
}
