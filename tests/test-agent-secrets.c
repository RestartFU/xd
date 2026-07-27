#include "settings/agent-secrets.h"

#include <glib/gstdio.h>

typedef struct
{
  char *directory;
  char *path;
} SecretFixture;

static void
secret_fixture_set_up (SecretFixture *fixture,
                       gconstpointer  user_data)
{
  fixture->directory = g_dir_make_tmp ("xd-agent-secrets-XXXXXX", NULL);
  g_assert_nonnull (fixture->directory);
  fixture->path =
    g_build_filename (fixture->directory, "agent-secrets.json", NULL);
}

static void
secret_fixture_tear_down (SecretFixture *fixture,
                          gconstpointer  user_data)
{
  g_remove (fixture->path);
  g_rmdir (fixture->directory);
  g_free (fixture->path);
  g_free (fixture->directory);
}

static void
test_round_trip (SecretFixture *fixture,
                 gconstpointer  user_data)
{
  g_autoptr (XdAgentSecrets) secrets = NULL;
  g_autoptr (XdAgentSecrets) loaded = NULL;
  g_autoptr (GError) error = NULL;
  g_auto (GStrv) names = NULL;
  g_auto (GStrv) environment = NULL;
  g_autofree char *prompt = NULL;

  secrets = xd_agent_secrets_load (fixture->path, &error);
  g_assert_no_error (error);
  g_assert_nonnull (secrets);

  g_assert_true (
    xd_agent_secrets_set (secrets, "ZEBRA_TOKEN", "hidden-zebra", &error));
  g_assert_true (
    xd_agent_secrets_set (secrets, "ALPHA_KEY", "hidden-alpha", &error));
  g_assert_no_error (error);
  g_assert_true (xd_agent_secrets_save (secrets, &error));
  g_assert_no_error (error);

#ifndef G_OS_WIN32
  {
    GStatBuf stat_buffer;

    g_assert_cmpint (g_stat (fixture->path, &stat_buffer), ==, 0);
    g_assert_cmpuint (stat_buffer.st_mode & 0777, ==, 0600);
  }
#endif

  loaded = xd_agent_secrets_load (fixture->path, &error);
  g_assert_no_error (error);
  g_assert_nonnull (loaded);
  names = xd_agent_secrets_names (loaded);
  g_assert_cmpstr (names[0], ==, "ALPHA_KEY");
  g_assert_cmpstr (names[1], ==, "ZEBRA_TOKEN");
  g_assert_null (names[2]);

  environment = g_new0 (char *, 1);
  environment = xd_agent_secrets_apply_environment (loaded, environment);
  g_assert_cmpstr (g_environ_getenv (environment, "ALPHA_KEY"),
                   ==, "hidden-alpha");
  g_assert_cmpstr (g_environ_getenv (environment, "ZEBRA_TOKEN"),
                   ==, "hidden-zebra");

  prompt = xd_agent_secrets_prompt (loaded);
  g_assert_nonnull (strstr (prompt, "ALPHA_KEY"));
  g_assert_nonnull (strstr (prompt, "ZEBRA_TOKEN"));
  g_assert_null (strstr (prompt, "hidden-alpha"));
  g_assert_null (strstr (prompt, "hidden-zebra"));

  xd_agent_secrets_remove (loaded, "ALPHA_KEY");
  g_assert_false (xd_agent_secrets_contains (loaded, "ALPHA_KEY"));
}

static void
test_validation (SecretFixture *fixture,
                 gconstpointer  user_data)
{
  g_autoptr (XdAgentSecrets) secrets = NULL;
  g_autoptr (GError) error = NULL;

  secrets = xd_agent_secrets_load (fixture->path, &error);
  g_assert_no_error (error);

  g_assert_true (xd_agent_secret_name_is_valid ("CLOUDFLARE_API_TOKEN"));
  g_assert_true (xd_agent_secret_name_is_valid ("_PRIVATE"));
  g_assert_false (xd_agent_secret_name_is_valid ("9TOKEN"));
  g_assert_false (xd_agent_secret_name_is_valid ("HAS-DASH"));
  g_assert_false (xd_agent_secret_name_is_valid (""));

  g_assert_false (
    xd_agent_secrets_set (secrets, "HAS-DASH", "value", &error));
  g_assert_error (error, g_quark_from_static_string ("xd-agent-secrets-error"), 1);
  g_clear_error (&error);

  g_assert_false (xd_agent_secrets_set (secrets, "TOKEN", "", &error));
  g_assert_error (error, g_quark_from_static_string ("xd-agent-secrets-error"), 1);
}

static void
test_rejects_malformed_store (SecretFixture *fixture,
                              gconstpointer  user_data)
{
  g_autoptr (XdAgentSecrets) secrets = NULL;
  g_autoptr (GError) error = NULL;

  g_assert_true (
    g_file_set_contents (fixture->path,
                         "{\"secrets\":{\"TOKEN\":\"\"}}", -1, &error));
  g_assert_no_error (error);

  secrets = xd_agent_secrets_load (fixture->path, &error);
  g_assert_null (secrets);
  g_assert_nonnull (error);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add ("/agent-secrets/round-trip", SecretFixture, NULL,
              secret_fixture_set_up, test_round_trip,
              secret_fixture_tear_down);
  g_test_add ("/agent-secrets/validation", SecretFixture, NULL,
              secret_fixture_set_up, test_validation,
              secret_fixture_tear_down);
  g_test_add ("/agent-secrets/malformed-store", SecretFixture, NULL,
              secret_fixture_set_up, test_rejects_malformed_store,
              secret_fixture_tear_down);

  return g_test_run ();
}
