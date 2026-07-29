#include "util/text-reveal.h"

static void
test_starts_late_then_catches_up (void)
{
  XdTextReveal reveal = { 0 };
  gboolean settled = FALSE;

  xd_text_reveal_note_append (&reveal, 1000);

  g_assert_cmpuint (
    xd_text_reveal_advance (&reveal, "abcdefghij", 50 * 1000, &settled),
    ==, 0);
  g_assert_false (settled);

  g_assert_cmpuint (
    xd_text_reveal_advance (&reveal, "abcdefghij", 90 * 1000, &settled),
    >, 0);
  g_assert_false (settled);

  while (!settled)
    xd_text_reveal_advance (&reveal, "abcdefghij", 150 * 1000, &settled);

  g_assert_cmpuint (reveal.shown, ==, 10);
}

static void
test_holds_live_tail (void)
{
  XdTextReveal reveal = { 0 };
  gboolean settled = FALSE;

  xd_text_reveal_note_append (&reveal, 1000);
  for (guint i = 0; i < 10; i++)
    xd_text_reveal_advance (&reveal, "abcdef", 90 * 1000, &settled);

  g_assert_cmpuint (reveal.shown, ==, 4);
  g_assert_false (settled);

  xd_text_reveal_advance (&reveal, "abcdef", 120 * 1000, &settled);
  g_assert_cmpuint (reveal.shown, ==, 6);
  g_assert_true (settled);
}

static void
test_prefix_keeps_utf8_whole (void)
{
  g_autofree char *prefix = xd_text_reveal_prefix ("a🦀é", 2);

  g_assert_true (g_utf8_validate (prefix, -1, NULL));
  g_assert_cmpstr (prefix, ==, "a🦀");
}

static void
test_more_text_resumes_reveal (void)
{
  XdTextReveal reveal = { 0 };
  gboolean settled = FALSE;

  xd_text_reveal_note_append (&reveal, 1000);
  while (!settled)
    xd_text_reveal_advance (&reveal, "done", 120 * 1000, &settled);

  xd_text_reveal_note_append (&reveal, 130 * 1000);
  xd_text_reveal_advance (&reveal, "done next", 140 * 1000, &settled);

  g_assert_cmpuint (reveal.shown, >, 4);
  g_assert_cmpuint (reveal.shown, <, 9);
  g_assert_false (settled);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/text-reveal/starts-late", test_starts_late_then_catches_up);
  g_test_add_func ("/text-reveal/live-tail", test_holds_live_tail);
  g_test_add_func ("/text-reveal/utf8-prefix", test_prefix_keeps_utf8_whole);
  g_test_add_func ("/text-reveal/resumes", test_more_text_resumes_reveal);

  return g_test_run ();
}
