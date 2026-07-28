#include "util/update-channel.h"

#include <string.h>

/*
 * These run against whichever profile was built -- the suite is part of every
 * one of them -- so nothing here may assume which channel is current. What can
 * be checked is that the current one is the channel this build says it is, and
 * everything else takes its channel as an argument: all three are then covered
 * from a single build, and the point of these tests is that a channel cannot
 * end up checking one release and installing another.
 */
static void
test_current_channel_is_this_build (void)
{
  XdUpdateChannel current = xd_update_channel_current ();

  if (g_strcmp0 (XD_CHANNEL, "nightly") == 0)
    g_assert_cmpint (current, ==, XD_UPDATE_CHANNEL_NIGHTLY);
  else if (g_strcmp0 (XD_CHANNEL, "dev") == 0)
    g_assert_cmpint (current, ==, XD_UPDATE_CHANNEL_DEV);
  else
    g_assert_cmpint (current, ==, XD_UPDATE_CHANNEL_RELEASE);

  g_assert_null (xd_update_channel_tag (XD_UPDATE_CHANNEL_RELEASE));
  g_assert_cmpstr (xd_update_channel_tag (XD_UPDATE_CHANNEL_NIGHTLY), ==,
                   "nightly");
  g_assert_cmpstr (xd_update_channel_tag (XD_UPDATE_CHANNEL_DEV), ==, "dev");
}

/*
 * A release asks for the newest release, a nightly for its own tag -- and a dev
 * build for the commit written beside its bundle, because the API allows sixty
 * unauthenticated requests an hour and a dev build looks every twenty-five
 * seconds from two processes at once.
 */
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
  g_assert_true (g_str_has_suffix (dev, "/releases/download/dev/commit.txt"));
  g_assert_null (strstr (dev, "api.github.com"));
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

/*
 * A dev build is watched while it is being pushed to, so it looks often. The
 * window and the daemon read this same answer: one of them polling on its own
 * schedule would mean a machine where the client offers a build the daemon has
 * not noticed, or the reverse.
 */
static void
test_a_dev_build_looks_often (void)
{
  g_assert_cmpuint (xd_update_channel_poll_seconds (XD_UPDATE_CHANNEL_DEV),
                    ==, 25);
  g_assert_cmpuint (xd_update_channel_poll_seconds (XD_UPDATE_CHANNEL_NIGHTLY),
                    ==, 60 * 5);
  g_assert_cmpuint (xd_update_channel_poll_seconds (XD_UPDATE_CHANNEL_RELEASE),
                    ==, 60 * 5);
}

static void
test_rolling_releases_are_named_by_commit (void)
{
  static const char *reply =
    "{\"tag_name\":\"dev\","
    " \"target_commitish\":\"1234567890abcdef1234567890abcdef12345678\"}";
  g_autofree char *nightly =
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_NIGHTLY, reply);
  g_autofree char *dev =
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_DEV, reply);
  g_autofree char *release =
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_RELEASE, reply);

  g_assert_cmpstr (nightly, ==, "1234567890abcdef1234567890abcdef12345678");
  g_assert_cmpstr (release, ==, "dev");

  /* A dev build is answered by the commit itself, on its own line. */
  g_assert_null (dev);

  {
    g_autofree char *plain = xd_update_channel_latest_from_reply (
      XD_UPDATE_CHANNEL_DEV, "1234567890abcdef1234567890abcdef12345678\n");

    g_assert_cmpstr (plain, ==, "1234567890abcdef1234567890abcdef12345678");
  }

  /* Nothing usable is not the same as something newer: an error page, a
   * truncated file or an empty one must not read as a new commit. */
  g_assert_null (
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_DEV, "not a commit"));
  g_assert_null (
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_DEV, "<html>404"));
  g_assert_null (
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_DEV, "abc123"));
  g_assert_null (
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_DEV, "  \n"));
  g_assert_null (
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_DEV, NULL));
  g_assert_null (
    xd_update_channel_latest_from_reply (XD_UPDATE_CHANNEL_NIGHTLY, "not json"));
}

static void
test_newer_compares_what_identifies_the_build (void)
{
  g_assert_false (xd_update_channel_is_newer (XD_UPDATE_CHANNEL_DEV, NULL));
  g_assert_false (xd_update_channel_is_newer (XD_UPDATE_CHANNEL_DEV, ""));

  if (XD_COMMIT[0] == '\0')
    {
      /* Built outside a checkout: a rolling channel has nothing to compare
       * against, and must not offer an update it cannot reason about. */
      g_assert_false (xd_update_channel_is_newer (
        XD_UPDATE_CHANNEL_DEV, "1234567890abcdef1234567890abcdef12345678"));
    }
  else
    {
      /* The API gives the whole commit; this build knows its own short one. */
      g_autofree char *same =
        g_strconcat (XD_COMMIT, "0123456789abcdef0123456789abcdef", NULL);
      g_autofree char *other = g_strdup (same);

      other[0] = other[0] == 'a' ? 'b' : 'a';

      g_assert_false (xd_update_channel_is_newer (XD_UPDATE_CHANNEL_DEV, same));
      g_assert_true (xd_update_channel_is_newer (XD_UPDATE_CHANNEL_DEV, other));
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
  g_test_add_func ("/update-channel/dev-looks-often",
                   test_a_dev_build_looks_often);
  g_test_add_func ("/update-channel/rolling-named-by-commit",
                   test_rolling_releases_are_named_by_commit);
  g_test_add_func ("/update-channel/newer-compares-identity",
                   test_newer_compares_what_identifies_the_build);

  return g_test_run ();
}
