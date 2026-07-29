#include "util/branch-build.h"

#include <string.h>

/*
 * What is typed in becomes a git ref and then a shell command, so these are
 * the two things worth pinning: that every way of writing down the same pull
 * request reaches the same ref, and that nothing which is not one of the
 * shapes below is accepted at all. The second is the important one -- what
 * comes out of the parser is run.
 */

typedef struct
{
  const char *text;
  const char *url;
  const char *ref;
  const char *label;
} Understood;

static void
test_understood_forms (void)
{
  static const Understood cases[] = {
    /* A pull request, however it was copied. */
    { "https://github.com/" XD_REPO "/pull/128",
      "https://github.com/" XD_REPO ".git", "refs/pull/128/head",
      "pull request #128" },
    { "https://github.com/" XD_REPO "/pull/128/files",
      "https://github.com/" XD_REPO ".git", "refs/pull/128/head",
      "pull request #128" },
    { "https://github.com/" XD_REPO "/pull/128#issuecomment-42",
      "https://github.com/" XD_REPO ".git", "refs/pull/128/head",
      "pull request #128" },
    { "  #128  ", "https://github.com/" XD_REPO ".git", "refs/pull/128/head",
      "pull request #128" },
    { "128", "https://github.com/" XD_REPO ".git", "refs/pull/128/head",
      "pull request #128" },

    /* A branch, by link or by name, with the slashes branch names have. */
    { "https://github.com/" XD_REPO "/tree/persistent-agent-session",
      "https://github.com/" XD_REPO ".git",
      "refs/heads/persistent-agent-session",
      "branch persistent-agent-session" },
    { "https://github.com/" XD_REPO "/tree/feature/nested/name",
      "https://github.com/" XD_REPO ".git",
      "refs/heads/feature/nested/name",
      "branch feature/nested/name" },
    { "feature/nested/name", "https://github.com/" XD_REPO ".git",
      "refs/heads/feature/nested/name", "branch feature/nested/name" },

    /* Somewhere else: fetched from there, and said so. A fork's pull request
     * is still fetched from the repository the pull request is on. */
    { "https://github.com/someone/xd/tree/their-work",
      "https://github.com/someone/xd.git", "refs/heads/their-work",
      "branch their-work in someone/xd" },
    { "https://github.com/someone/xd.git/pull/7",
      "https://github.com/someone/xd.git", "refs/pull/7/head",
      "pull request #7 in someone/xd" },
  };

  for (gsize i = 0; i < G_N_ELEMENTS (cases); i++)
    {
      g_autofree char *url = NULL;
      g_autofree char *ref = NULL;
      g_autofree char *label = NULL;

      g_assert_true (xd_branch_build_parse (cases[i].text, &url, &ref, &label));
      g_assert_cmpstr (url, ==, cases[i].url);
      g_assert_cmpstr (ref, ==, cases[i].ref);
      g_assert_cmpstr (label, ==, cases[i].label);
    }
}

static void
test_refused_forms (void)
{
  static const char *cases[] = {
    NULL,
    "",
    "   ",
    /* A sentence is not a branch name, and neither is a shell fragment. */
    "build the fix branch please",
    "branch; rm -rf ~",
    "$(whoami)",
    "`id`",
    "main'",
    "main\"",
    "main\nmaster",
    /* Names git itself refuses. */
    "-delete",
    "/leading",
    "trailing/",
    ".hidden",
    "two..dots",
    "double//slash",
    "main.lock",
    /* Links that name no particular code. */
    "https://github.com/" XD_REPO,
    "https://github.com/" XD_REPO "/",
    "https://github.com/" XD_REPO "/issues/12",
    "https://github.com/" XD_REPO "/pull/",
    "https://github.com/" XD_REPO "/pull/notanumber",
    "https://github.com/" XD_REPO "/tree/",
    /* And a URL somewhere else entirely. */
    "https://example.com/" XD_REPO "/pull/1",
  };

  for (gsize i = 0; i < G_N_ELEMENTS (cases); i++)
    {
      char *url = NULL;
      char *ref = NULL;
      char *label = NULL;

      if (xd_branch_build_parse (cases[i], &url, &ref, &label))
        g_error ("accepted \"%s\" as %s", cases[i], ref);

      /* Nothing is handed back for something that was not understood. */
      g_assert_null (url);
      g_assert_null (ref);
      g_assert_null (label);
    }
}

/*
 * The command is what actually replaces the running copy, so it has to fetch
 * the ref it was given, build the same bundle the nightly is, and install
 * through the checkout's own installer rather than a copy of its steps.
 */
static void
test_command_builds_and_installs (void)
{
  g_autofree char *command = xd_branch_build_command (
    "https://github.com/" XD_REPO ".git", "refs/pull/128/head",
    "/home/someone/.cache/xd-nightly/source");

  g_assert_nonnull (strstr (command, "set -eu"));
  g_assert_nonnull (strstr (command, "'/home/someone/.cache/xd-nightly/source'"));
  g_assert_nonnull (strstr (command, "fetch --depth 1 --force"));
  g_assert_nonnull (strstr (command, "'refs/pull/128/head'"));
  g_assert_nonnull (strstr (command, "FETCH_HEAD"));
  g_assert_nonnull (strstr (command, "scripts/build.sh --build-arg PROFILE=nightly"));
  g_assert_nonnull (strstr (command, "scripts/install.sh --from dist"));

  /* A branch whose installer cannot install a local bundle would download the
   * published nightly and report success; it is checked before the build
   * rather than discovered after it. */
  g_assert_nonnull (strstr (command, "grep -q -- '--from)' scripts/install.sh"));
  g_assert_nonnull (strstr (command, "exit 1"));
}

/* A path is not a ref: it comes from the environment, and a home directory
 * with a quote in it must not end the quoting around it. */
static void
test_command_quotes_the_path (void)
{
  g_autofree char *command = xd_branch_build_command (
    "https://github.com/" XD_REPO ".git", "refs/heads/main",
    "/home/o'brien/.cache/xd-nightly/source");

  g_assert_null (strstr (command, "'/home/o'brien"));
  g_assert_nonnull (strstr (command, "'\\''"));
}

static void
test_checkout_is_under_the_cache (void)
{
  g_autofree char *checkout = xd_branch_build_checkout_dir ();

  g_assert_nonnull (strstr (checkout, XD_DATA_NAME));
  g_assert_true (g_str_has_suffix (checkout, "source"));
  g_assert_true (g_path_is_absolute (checkout));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/branch-build/understood-forms", test_understood_forms);
  g_test_add_func ("/branch-build/refused-forms", test_refused_forms);
  g_test_add_func ("/branch-build/command-builds-and-installs",
                   test_command_builds_and_installs);
  g_test_add_func ("/branch-build/command-quotes-the-path",
                   test_command_quotes_the_path);
  g_test_add_func ("/branch-build/checkout-under-the-cache",
                   test_checkout_is_under_the_cache);

  return g_test_run ();
}
