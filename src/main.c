#include "xd-app.h"
#include "util/app-paths.h"

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
#endif

static void
print_version (void)
{
  printf ("xd %s\n", XD_VERSION_STRING);
}

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
  g_autofree char *db_path = NULL;
  g_autofree char *root = NULL;
  GMainLoop *loop;
  guint16 port = 4001;
  gboolean pair = FALSE;

  for (int i = 2; i < argc; i++)
    {
      if (g_strcmp0 (argv[i], "--port") == 0 && i + 1 < argc)
        port = (guint16) g_ascii_strtoull (argv[++i], NULL, 10);
      else if (g_strcmp0 (argv[i], "--pair") == 0)
        pair = TRUE;
      else if (g_strcmp0 (argv[i], "--root") == 0 && i + 1 < argc)
        root = g_strdup (argv[++i]);
      else
        {
          fprintf (stderr, "usage: xd serve [--port N] [--pair] [--root DIR]\n");
          return 1;
        }
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

  printf ("xd serve: %s, listening on %u, workspaces at %s\n",
          XD_VERSION_STRING, xd_remote_server_get_port (server), root);

  if (pair)
    {
      g_autofree char *code = xd_remote_server_arm_pairing (server, 300);

      printf ("pairing code (5 minutes, one use): %s\n", code);
    }

  fflush (stdout);

  loop = g_main_loop_new (NULL, FALSE);
  g_main_loop_run (loop);

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
