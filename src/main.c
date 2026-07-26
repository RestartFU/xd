#include "hy-app.h"
#include "remote/server.h"

#include <stdio.h>

/*
 * Loads the daemon's certificate, minting one the first time.
 *
 * GLib can read certificates but not create them, so creation is one spawn
 * of openssl -- present nearly everywhere, and the error says so plainly
 * where it is not. Self-signed on purpose: the client pins this exact
 * certificate at pairing time, the way SSH pins a host key, so a CA would
 * add nothing but a bill.
 */
static GTlsCertificate *
ensure_certificate (GError **error)
{
  g_autofree char *dir = g_build_filename (g_get_user_data_dir (), "hy", NULL);
  g_autofree char *cert_path = g_build_filename (dir, "server-cert.pem", NULL);
  g_autofree char *key_path = g_build_filename (dir, "server-key.pem", NULL);

  g_mkdir_with_parents (dir, 0700);

  if (!g_file_test (cert_path, G_FILE_TEST_EXISTS))
    {
      const char *argv[] = {
        "openssl", "req", "-x509", "-newkey", "rsa:2048",
        "-keyout", key_path, "-out", cert_path,
        "-days", "3650", "-nodes", "-subj", "/CN=hy",
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

static int
run_serve (int argc, char *argv[])
{
  g_autoptr (GError) error = NULL;
  g_autoptr (HyStorage) storage = NULL;
  g_autoptr (GTlsCertificate) certificate = NULL;
  g_autoptr (HyRemoteServer) server = NULL;
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
          fprintf (stderr, "usage: hy serve [--port N] [--pair] [--root DIR]\n");
          return 1;
        }
    }

  if (root == NULL)
    root = g_build_filename (g_get_home_dir (), "Workspaces", NULL);

  db_path = g_build_filename (g_get_user_data_dir (), "hy", "chats.db", NULL);
  storage = hy_storage_new (db_path, &error);
  if (storage == NULL)
    {
      fprintf (stderr, "hy serve: %s\n", error->message);
      return 1;
    }

  certificate = ensure_certificate (&error);
  if (certificate == NULL)
    {
      fprintf (stderr, "hy serve: %s\n", error->message);
      return 1;
    }

  server = hy_remote_server_new (storage, root, port, certificate, &error);
  if (server == NULL)
    {
      fprintf (stderr, "hy serve: %s\n", error->message);
      return 1;
    }

  printf ("hy serve: listening on %u, workspaces at %s\n",
          hy_remote_server_get_port (server), root);

  if (pair)
    {
      g_autofree char *code = hy_remote_server_arm_pairing (server, 300);

      printf ("pairing code (5 minutes, one use): %s\n", code);
    }

  fflush (stdout);

  loop = g_main_loop_new (NULL, FALSE);
  g_main_loop_run (loop);

  return 0;
}

int
main (int argc, char *argv[])
{
  /* Before GTK sees argv: the daemon must run without a display at all. */
  if (argc > 1 && g_strcmp0 (argv[1], "serve") == 0)
    return run_serve (argc, argv);

  {
    g_autoptr (HyApplication) app = hy_application_new ();

    return g_application_run (G_APPLICATION (app), argc, argv);
  }
}
