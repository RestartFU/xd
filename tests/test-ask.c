#include "util/ask-block.h"

#include <string.h>

/* The happy path: a question and its options come out, and the markup that
 * carried them never reaches the transcript. */
static void
test_lifts_the_question_out (void)
{
  g_autoptr (XdAsk) ask = NULL;
  g_autofree char *remainder = NULL;

  ask = xd_ask_parse ("Here is what I found.\n\n"
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
  g_autoptr (XdAsk) ask = xd_ask_parse ("<ask>\nWhich one?\n- A\n", &remainder);

  g_assert_null (ask);
  g_assert_null (remainder);
}

/* One option is not a choice; showing a single button would be theatre. */
static void
test_needs_at_least_two_options (void)
{
  g_autoptr (XdAsk) one = xd_ask_parse ("<ask>\nGo ahead?\n- Yes\n</ask>", NULL);
  g_autoptr (XdAsk) none = xd_ask_parse ("<ask>\nJust talking\n</ask>", NULL);

  g_assert_null (one);
  g_assert_null (none);
}

static void
test_accepts_specific_input (void)
{
  g_autofree char *remainder = NULL;
  g_autoptr (XdAsk) ask = xd_ask_parse (
    "Before.\n\n"
    "<ask>\n"
    "What branch name should I use?\n"
    "<input>\n"
    "</ask>",
    &remainder);

  g_assert_nonnull (ask);
  g_assert_true (ask->accepts_input);
  g_assert_cmpstr (ask->question, ==, "What branch name should I use?");
  g_assert_cmpuint (g_strv_length (ask->options), ==, 0);
  g_assert_cmpstr (remainder, ==, "Before.");
}

static void
test_combines_options_and_input (void)
{
  g_autoptr (XdAsk) ask = xd_ask_parse (
    "<ask>\n"
    "Where should I deploy?\n"
    "- Production\n"
    "- Staging\n"
    "<input>\n"
    "</ask>",
    NULL);

  g_assert_nonnull (ask);
  g_assert_true (ask->accepts_input);
  g_assert_cmpuint (g_strv_length (ask->options), ==, 2);
}

static void
test_plain_text_is_left_alone (void)
{
  g_autofree char *remainder = NULL;
  g_autoptr (XdAsk) ask = xd_ask_parse ("Which one:\n- A\n- B\n", &remainder);

  g_assert_null (ask);
  g_assert_null (remainder);
}

/* Text after the block is kept: the assistant may sign off below it. */
static void
test_keeps_text_on_both_sides (void)
{
  g_autofree char *remainder = NULL;
  g_autoptr (XdAsk) ask = xd_ask_parse ("Before.\n<ask>\nPick\n- A\n- B\n</ask>\nAfter.",
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
  g_assert_cmpuint (xd_ask_visible_length ("All done."), ==, 9);

  /* A partial tag: hold everything from the "<" that might start it. */
  g_assert_cmpuint (xd_ask_visible_length ("Done.<"), ==, 5);
  g_assert_cmpuint (xd_ask_visible_length ("Done.<as"), ==, 5);
  g_assert_cmpuint (xd_ask_visible_length ("Done.<ask"), ==, 5);

  /* The complete tag and everything after it. */
  g_assert_cmpuint (xd_ask_visible_length ("Done.<ask>\nPick\n- A\n- B\n"), ==, 5);

  /* A "<" that was never going to be a tag stays visible. */
  g_assert_cmpuint (xd_ask_visible_length ("a < b"), ==, 5);
  g_assert_cmpuint (xd_ask_visible_length ("<div>"), ==, 5);
  g_assert_cmpuint (xd_ask_visible_length (NULL), ==, 0);
}

/*
 * A reply that talks about the format before using it.
 *
 * Codex answered "the prior response should have been wrapped in the required
 * <ask>...</ask> format", then asked properly underneath. Reading the first
 * "<ask>" found the sentence, which has no options, and the real question was
 * shown as raw tags.
 */
static void
test_ignores_the_tag_mentioned_in_prose (void)
{
  g_autofree char *remainder = NULL;
  g_autoptr (XdAsk) ask = NULL;
  const char *text =
    "You're right -- the prior response should have been wrapped in the "
    "required <ask>...</ask> format and I missed that.\n\n"
    "<ask>\n"
    "Which approach should I take?\n"
    "- Audit the repo first\n"
    "- Clean up one parser file\n"
    "</ask>";

  ask = xd_ask_parse (text, &remainder);

  g_assert_nonnull (ask);
  g_assert_cmpstr (ask->question, ==, "Which approach should I take?");
  g_assert_cmpuint (g_strv_length (ask->options), ==, 2);
  g_assert_cmpstr (ask->options[0], ==, "Audit the repo first");

  /* The sentence that mentions the tag is prose, and stays. */
  g_assert_nonnull (strstr (remainder, "required <ask>...</ask> format"));

  /* It also stays visible while the reply is still arriving. */
  g_assert_cmpuint (xd_ask_visible_length (text), ==,
                    (gsize) (strstr (text, "\n\n<ask>") + 2 - text));
}

static void
test_instructions_require_reporting_links (void)
{
  const char *instructions = xd_ask_instructions ();

  g_assert_nonnull (
    strstr (instructions,
            "[abc1234](https://github.com/owner/repo/commit/abc1234)"));
  g_assert_nonnull (strstr (instructions, "Do not report a bare hash"));
  g_assert_nonnull (
    strstr (instructions, "[#35](https://github.com/owner/repo/issues/35)"));
  g_assert_nonnull (
    strstr (instructions, "[PR #12](https://github.com/owner/repo/pull/12)"));
  g_assert_nonnull (strstr (instructions,
                            "Do not leave a resolvable #number as bare text"));
  g_assert_nonnull (strstr (instructions, "<input>"));
  g_assert_nonnull (strstr (instructions, "This shows a text field"));
}

int
main (int argc, char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/ask/lifts-question", test_lifts_the_question_out);
  g_test_add_func ("/ask/unclosed", test_ignores_an_unclosed_block);
  g_test_add_func ("/ask/two-options", test_needs_at_least_two_options);
  g_test_add_func ("/ask/specific-input", test_accepts_specific_input);
  g_test_add_func ("/ask/options-and-input", test_combines_options_and_input);
  g_test_add_func ("/ask/plain-text", test_plain_text_is_left_alone);
  g_test_add_func ("/ask/both-sides", test_keeps_text_on_both_sides);
  g_test_add_func ("/ask/hidden-while-streaming", test_hides_the_block_while_it_streams);
  g_test_add_func ("/ask/tag-in-prose", test_ignores_the_tag_mentioned_in_prose);
  g_test_add_func ("/ask/instructions-require-reporting-links",
                   test_instructions_require_reporting_links);

  return g_test_run ();
}
