#include "util/update-channel.h"

#include <string.h>

/*
 * These run against whichever profile was built -- the suite is part of every
 * one of them -- so nothing here may assume which channel is current. What can
 * be checked is that the current one is the channel this build says it is, and
 * everything else takes its channel as an argument: both are then covered from
 * a single build, and the point of these tests is that a channel cannot end up
 * checking one release and installing another.
 */
static void
test_current_channel_is_this_build (void)
{
  XdUpdateChannel current = xd_update_channel_current ();

  if (g_strcmp0 (XD_CHANNEL, "nightly") == 0)
    g_assert_cmpint (current, ==, XD_UPDATE_CHANNEL_NIGHTLY);
  else
    g_assert_cmpint (current, ==, XD_UPDATE_CHANNEL_RELEASE);

  g_assert_null (xd_update_channel_tag (XD_UPDATE_CHANNEL_RELEASE));
  g_assert_cmpstr (xd_update_channel_tag (XD_UPDATE_CHANNEL_NIGHTLY), ==,
                   "nightly");
}

/* A release asks for the newest release, a nightly for its own rolling tag. */
static void
test_each_channel_checks_its_own_release (void)
{
  g_autofree char *release =
    xd_update_channel_check_url (XD_UPDATE_CHANNEL_RELEASE);
  g_autofree char *nightly =
    xd_update_channel_check_url (XD_UPDATE_CHANNEL_NIGHTLY);

  g_assert_true (g_str_has_suffix (release, "/releases/latest"));
  g_assert_true (g_str_has_suffix (nightly, "/releases/tags/nightly"));
}

/* And installs from the release it just checked. */
static void
test_each_channel_installs_what_it_checked (void)
{
  g_autofree char *release =
    xd_update_channel_install_command (XD_UPDATE_CHANNEL_RELEASE);
  g_autofree char *nightly =
    xd_update_channel_install_command (XD_UPDATE_CHANNEL_NIGHTLY);

  g_assert_nonnull (strstr (release, "/releases/latest/download/install.sh"));
  g_assert_nonnull (strstr (release, "--release"));

  g_assert_nonnull (strstr (nightly, "/releases/download/nightly/install.sh"));
  g_assert_null (strstr (nightly, "--"));
}

static void
test_rolling_releases_are_named_by_commit (void)
{
  static const char *reply =
    "{\"tag_name\":\"nightly\","
    " \"target_commitish\":\"1234567890abcdef1234567890abcdef12345678\"}";
  g_autofree char *nightly =
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_NIGHTLY, reply);
  g_autofree char *release =
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_RELEASE, reply);

  g_assert_cmpstr (nightly, ==, "1234567890abcdef1234567890abcdef12345678");
  g_assert_cmpstr (release, ==, "nightly");

  /* Nothing usable is not the same as something newer: an error page or a
   * truncated reply must not read as a new build. */
  g_assert_null (
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_NIGHTLY, "not json"));
  g_assert_null (
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_NIGHTLY, "<html>404"));
  g_assert_null (
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_NIGHTLY, NULL));
}

static void
test_newer_compares_what_identifies_the_build (void)
{
  g_assert_false (xd_update_channel_is_newer (XD_UPDATE_CHANNEL_NIGHTLY, NULL));
  g_assert_false (xd_update_channel_is_newer (XD_UPDATE_CHANNEL_NIGHTLY, ""));

  if (XD_COMMIT[0] == '\0')
    {
      /* Built outside a checkout: the rolling channel has nothing to compare
       * against, and must not offer an update it cannot reason about. */
      g_assert_false (xd_update_channel_is_newer (
        XD_UPDATE_CHANNEL_NIGHTLY, "1234567890abcdef1234567890abcdef12345678"));
    }
  else
    {
      /* The API gives the whole commit; this build knows its own short one. */
      g_autofree char *same =
        g_strconcat (XD_COMMIT, "0123456789abcdef0123456789abcdef", NULL);
      g_autofree char *other = g_strdup (same);

      other[0] = other[0] == 'a' ? 'b' : 'a';

      g_assert_false (
        xd_update_channel_is_newer (XD_UPDATE_CHANNEL_NIGHTLY, same));
      g_assert_true (
        xd_update_channel_is_newer (XD_UPDATE_CHANNEL_NIGHTLY, other));
    }

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
