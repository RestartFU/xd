#include "util/workflow-run.h"

#include <glib/gstdio.h>

static void
remove_tree (const char *path)
{
  g_autoptr (GDir) dir = g_dir_open (path, 0, NULL);
  const char *name;

  while (dir != NULL && (name = g_dir_read_name (dir)) != NULL)
    {
      g_autofree char *child = g_build_filename (path, name, NULL);

      if (g_file_test (child, G_FILE_TEST_IS_DIR) &&
          !g_file_test (child, G_FILE_TEST_IS_SYMLINK))
        remove_tree (child);
      else
        g_remove (child);
    }

  g_rmdir (path);
}

static void
assert_workflow (const char *message,
                 const char *run_id,
                 const char *url)
{
  g_autofree char *actual_id = NULL;
  g_autofree char *actual_url = NULL;

  g_assert_true (
    xd_workflow_run_from_tool (message, &actual_id, &actual_url));
  g_assert_cmpstr (actual_id, ==, run_id);
  g_assert_cmpstr (actual_url, ==, url);
}

static void
test_explicit_repository (void)
{
  g_autofree char *message = xd_workflow_run_capture_tool (
    "$ git push && gh run watch 30230367515 "
    "--repo RestartFU/xd --exit-status --interval 10",
    NULL);

  assert_workflow (
    message,
    "30230367515",
    "https://github.com/RestartFU/xd/actions/runs/30230367515");
}

static void
test_repository_from_workdir (void)
{
  g_autoptr (GError) error = NULL;
  g_autofree char *dir = g_dir_make_tmp ("xd-workflow-run-XXXXXX", &error);
  g_autofree char *git_dir = NULL;
  g_autofree char *head = NULL;
  g_autofree char *config = NULL;
  g_autofree char *message = NULL;

  g_assert_no_error (error);
  git_dir = g_build_filename (dir, ".git", NULL);
  head = g_build_filename (git_dir, "HEAD", NULL);
  config = g_build_filename (git_dir, "config", NULL);

  g_assert_cmpint (g_mkdir (git_dir, 0700), ==, 0);
  g_assert_true (
    g_file_set_contents (head, "ref: refs/heads/master\n", -1, &error));
  g_assert_no_error (error);
  g_assert_true (g_file_set_contents (
    config,
    "[remote \"origin\"]\n"
    "\turl = git@github.com:RestartFU/xd.git\n",
    -1, &error));
  g_assert_no_error (error);

  message = xd_workflow_run_capture_tool (
    "$ gh run watch 99 --exit-status", dir);

  assert_workflow (
    message, "99", "https://github.com/RestartFU/xd/actions/runs/99");

  remove_tree (dir);
}

static void
test_ignores_other_commands (void)
{
  g_autofree char *view =
    xd_workflow_run_capture_tool ("$ gh run view 123 --repo a/b", NULL);
  g_autofree char *unsafe =
    xd_workflow_run_capture_tool (
      "$ gh run watch not-a-number --repo a/b", NULL);

  g_assert_cmpstr (view, ==, "$ gh run view 123 --repo a/b");
  g_assert_cmpstr (unsafe, ==, "$ gh run watch not-a-number --repo a/b");
  g_assert_false (xd_workflow_run_from_tool (view, NULL, NULL));
}

static void
test_live_activity (void)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  g_autoptr (GError) error = NULL;
  g_autofree char *activity = NULL;
  JsonArray *jobs;

  g_assert_true (json_parser_load_from_data (
    parser,
    "["
    " {\"name\":\"Linux\",\"status\":\"completed\"},"
    " {\"name\":\"Windows\",\"status\":\"in_progress\",\"steps\":["
    "   {\"name\":\"Checkout\",\"status\":\"completed\"},"
    "   {\"name\":\"Build MSI\",\"status\":\"in_progress\"}"
    " ]},"
    " {\"name\":\"macOS\",\"status\":\"queued\"},"
    " {\"name\":\"Publish\",\"status\":\"waiting\"}"
    "]",
    -1, &error));
  g_assert_no_error (error);
  jobs = json_node_get_array (json_parser_get_root (parser));

  activity = xd_workflow_run_activity (jobs, 2);

  g_assert_cmpstr (activity, ==,
                   "Windows · Build MSI\n"
                   "macOS · Queued");
}

static void
test_empty_activity (void)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  g_autoptr (GError) error = NULL;
  JsonArray *jobs;

  g_assert_true (json_parser_load_from_data (
    parser, "[{\"name\":\"Linux\",\"status\":\"completed\"}]", -1, &error));
  g_assert_no_error (error);
  jobs = json_node_get_array (json_parser_get_root (parser));

  g_assert_null (xd_workflow_run_activity (jobs, 5));
  g_assert_null (xd_workflow_run_activity (jobs, 0));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/workflow-run/explicit-repository",
                   test_explicit_repository);
  g_test_add_func ("/workflow-run/repository-from-workdir",
                   test_repository_from_workdir);
  g_test_add_func ("/workflow-run/ignores-other-commands",
                   test_ignores_other_commands);
  g_test_add_func ("/workflow-run/live-activity", test_live_activity);
  g_test_add_func ("/workflow-run/empty-activity", test_empty_activity);

  return g_test_run ();
}
