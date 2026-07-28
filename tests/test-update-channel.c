#include "util/update-channel.h"

#include <string.h>

/*
 * The tests link the default profile, so the running channel is the release
 * one. Everything else takes the channel as an argument, which is what lets all
 * three be checked from one build -- and the point of these tests is that a
 * channel cannot end up checking one release and installing another.
 */
static void
test_current_channel_is_this_build (void)
{
  g_assert_cmpint (xd_update_channel_current (), ==,
                   XD_UPDATE_CHANNEL_RELEASE);
  g_assert_null (xd_update_channel_tag (XD_UPDATE_CHANNEL_RELEASE));
  g_assert_cmpstr (xd_update_channel_tag (XD_UPDATE_CHANNEL_NIGHTLY), ==,
                   "nightly");
  g_assert_cmpstr (xd_update_channel_tag (XD_UPDATE_CHANNEL_DEV), ==, "dev");
}

/* A rolling channel asks for its own tag; a release asks for the newest one. */
static void
test_each_channel_checks_its_own_release (void)
{
  g_autofree char *release =
    xd_update_channel_check_url (XD_UPDATE_CHANNEL_RELEASE);
  g_autofree char *nightly =
    xd_update_channel_check_url (XD_UPDATE_CHANNEL_NIGHTLY);
  g_autofree char *dev = xd_update_channel_check_url (XD_UPDATE_CHANNEL_DEV);

  g_assert_true (g_str_has_suffix (release, "/releases/latest"));
  g_assert_true (g_str_has_suffix (nightly, "/releases/tags/nightly"));
  g_assert_true (g_str_has_suffix (dev, "/releases/tags/dev"));
}

/*
 * And installs from the release it just checked. A dev build has to say --dev:
 * it installs to the nightly's paths, so the script cannot tell from the paths
 * alone which of the two rolling releases was meant.
 */
static void
test_each_channel_installs_what_it_checked (void)
{
  g_autofree char *release =
    xd_update_channel_install_command (XD_UPDATE_CHANNEL_RELEASE);
  g_autofree char *nightly =
    xd_update_channel_install_command (XD_UPDATE_CHANNEL_NIGHTLY);
  g_autofree char *dev =
    xd_update_channel_install_command (XD_UPDATE_CHANNEL_DEV);

  g_assert_nonnull (strstr (release, "/releases/latest/download/install.sh"));
  g_assert_nonnull (strstr (release, "--release"));

  g_assert_nonnull (strstr (nightly, "/releases/download/nightly/install.sh"));
  g_assert_null (strstr (nightly, "--"));

  g_assert_nonnull (strstr (dev, "/releases/download/dev/install.sh"));
  g_assert_nonnull (strstr (dev, "--dev"));
}

static void
test_rolling_releases_are_named_by_commit (void)
{
  static const char *reply =
    "{\"tag_name\":\"dev\","
    " \"target_commitish\":\"1234567890abcdef1234567890abcdef12345678\"}";
  g_autofree char *nightly =
    xd_update_channel_latest_from_json (XD_UPDATE_CHANNEL_NIGHTLY, reply);
  g_autofree char *dev =
    xd_update_channel_latest_from_json (XD_UPDATE_CHANNEL_DEV, reply);
  g_autofree char *release =
    xd_update_channel_latest_from_json (XD_UPDATE_CHANNEL_RELEASE, reply);

  g_assert_cmpstr (nightly, ==, "1234567890abcdef1234567890abcdef12345678");
  g_assert_cmpstr (dev, ==, nightly);
  g_assert_cmpstr (release, ==, "dev");

  /* Nothing usable is not the same as something newer. */
  g_assert_null (
    xd_update_channel_latest_from_json (XD_UPDATE_CHANNEL_DEV, "not json"));
  g_assert_null (
    xd_update_channel_latest_from_json (XD_UPDATE_CHANNEL_DEV, NULL));
}

static void
test_newer_compares_what_identifies_the_build (void)
{
  /* This build carries no commit, so a rolling channel has nothing to compare
   * and must not offer an update it cannot reason about. */
  g_assert_false (xd_update_channel_is_newer (
    XD_UPDATE_CHANNEL_DEV, "1234567890abcdef1234567890abcdef12345678"));

  g_assert_false (xd_update_channel_is_newer (XD_UPDATE_CHANNEL_DEV, NULL));
  g_assert_false (xd_update_channel_is_newer (XD_UPDATE_CHANNEL_DEV, ""));

  /* A release compares tags against the version, with the v the tags carry. */
  g_assert_false (
    xd_update_channel_is_newer (XD_UPDATE_CHANNEL_RELEASE, "v" XD_VERSION));
  g_assert_false (
    xd_update_channel_is_newer (XD_UPDATE_CHANNEL_RELEASE, XD_VERSION));
  g_assert_true (
    xd_update_channel_is_newer (XD_UPDATE_CHANNEL_RELEASE, "v99.0.0"));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/update-channel/current-channel-is-this-build",
                   test_current_channel_is_this_build);
  g_test_add_func ("/update-channel/checks-its-own-release",
                   test_each_channel_checks_its_own_release);
  g_test_add_func ("/update-channel/installs-what-it-checked",
                   test_each_channel_installs_what_it_checked);
  g_test_add_func ("/update-channel/rolling-named-by-commit",
                   test_rolling_releases_are_named_by_commit);
  g_test_add_func ("/update-channel/newer-compares-identity",
                   test_newer_compares_what_identifies_the_build);

  return g_test_run ();
}
