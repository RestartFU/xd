#include "util/ask-block.h"

/* The happy path: a question and its options come out, and the markup that
 * carried them never reaches the transcript. */
static void
test_lifts_the_question_out (void)
{
  g_autoptr (HyAsk) ask = NULL;
  g_autofree char *remainder = NULL;

  ask = hy_ask_parse ("Here is what I found.\n\n"
                      "<ask>\n"
                      "Which approach should I take?\n"
                      "- Fix the parser\n"
                      "- Rewrite the parser\n"
                      "</ask>",
                      &remainder);

  g_assert_nonnull (ask);
  g_assert_cmpstr (ask->question, ==, "Which approach should I take?");
  g_assert_cmpuint (g_strv_length (ask->options), ==, 2);
  g_assert_cmpstr (ask->options[0], ==, "Fix the parser");
  g_assert_cmpstr (ask->options[1], ==, "Rewrite the parser");

  g_assert_cmpstr (remainder, ==, "Here is what I found.");
  g_assert_null (strstr (remainder, "<ask>"));
}

/* A block still arriving must not render half a question. */
static void
test_ignores_an_unclosed_block (void)
{
  g_autofree char *remainder = NULL;
  g_autoptr (HyAsk) ask = hy_ask_parse ("<ask>\nWhich one?\n- A\n", &remainder);

  g_assert_null (ask);
  g_assert_null (remainder);
}

/* One option is not a choice; showing a single button would be theatre. */
static void
test_needs_at_least_two_options (void)
{
  g_autoptr (HyAsk) one = hy_ask_parse ("<ask>\nGo ahead?\n- Yes\n</ask>", NULL);
  g_autoptr (HyAsk) none = hy_ask_parse ("<ask>\nJust talking\n</ask>", NULL);

  g_assert_null (one);
  g_assert_null (none);
}

static void
test_plain_text_is_left_alone (void)
{
  g_autofree char *remainder = NULL;
  g_autoptr (HyAsk) ask = hy_ask_parse ("Which one:\n- A\n- B\n", &remainder);

  g_assert_null (ask);
  g_assert_null (remainder);
}

/* Text after the block is kept: the assistant may sign off below it. */
static void
test_keeps_text_on_both_sides (void)
{
  g_autofree char *remainder = NULL;
  g_autoptr (HyAsk) ask = hy_ask_parse ("Before.\n<ask>\nPick\n- A\n- B\n</ask>\nAfter.",
                                        &remainder);

  g_assert_nonnull (ask);
  g_assert_cmpstr (remainder, ==, "Before.\n\nAfter.");
}

/*
 * The block must never be visible, not even mid-stream. The opening tag
 * arrives in fragments, so a tail that could still become one is held back.
 */
static void
test_hides_the_block_while_it_streams (void)
{
  /* Nothing to hide yet. */
  g_assert_cmpuint (hy_ask_visible_length ("All done."), ==, 9);

  /* A partial tag: hold everything from the "<" that might start it. */
  g_assert_cmpuint (hy_ask_visible_length ("Done.<"), ==, 5);
  g_assert_cmpuint (hy_ask_visible_length ("Done.<as"), ==, 5);
  g_assert_cmpuint (hy_ask_visible_length ("Done.<ask"), ==, 5);

  /* The complete tag and everything after it. */
  g_assert_cmpuint (hy_ask_visible_length ("Done.<ask>\nPick\n- A\n- B\n"), ==, 5);

  /* A "<" that was never going to be a tag stays visible. */
  g_assert_cmpuint (hy_ask_visible_length ("a < b"), ==, 5);
  g_assert_cmpuint (hy_ask_visible_length ("<div>"), ==, 5);
  g_assert_cmpuint (hy_ask_visible_length (NULL), ==, 0);
}

int
main (int argc, char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/ask/lifts-question", test_lifts_the_question_out);
  g_test_add_func ("/ask/unclosed", test_ignores_an_unclosed_block);
  g_test_add_func ("/ask/two-options", test_needs_at_least_two_options);
  g_test_add_func ("/ask/plain-text", test_plain_text_is_left_alone);
  g_test_add_func ("/ask/both-sides", test_keeps_text_on_both_sides);
  g_test_add_func ("/ask/hidden-while-streaming", test_hides_the_block_while_it_streams);

  return g_test_run ();
}
