#include <pango/pango.h>

#include "util/markdown.h"

static void
assert_valid_markup (const char *markup)
{
  g_autoptr (GError) error = NULL;

  pango_parse_markup (markup, -1, 0, NULL, NULL, NULL, &error);
  g_assert_no_error (error);
}

static void
test_inline_spans (void)
{
  g_autofree char *bold = hy_markdown_to_pango ("a **strong** word");
  g_autofree char *italic = hy_markdown_to_pango ("an *emphasised* word");
  g_autofree char *code = hy_markdown_to_pango ("call `g_free()` on it");

  g_assert_cmpstr (bold, ==, "a <b>strong</b> word");
  g_assert_cmpstr (italic, ==, "an <i>emphasised</i> word");
  g_assert_cmpstr (code, ==, "call <tt>g_free()</tt> on it");
}

static void
test_escapes_markup_characters (void)
{
  g_autofree char *result = hy_markdown_to_pango ("compare a < b && c > d");

  g_assert_cmpstr (result, ==, "compare a &lt; b &amp;&amp; c &gt; d");
  assert_valid_markup (result);
}

/*
 * Replies arrive a token at a time, so the converter is constantly handed a
 * span that has been opened but not closed. Emitting an unbalanced tag would
 * make Pango reject the whole message and show nothing.
 */
static void
test_partial_input_stays_valid (void)
{
  const char *partials[] = {
    "**",
    "**bol",
    "here is `some cod",
    "```\nint main (void)\n{",
    "*",
    "# Headi",
  };

  for (gsize i = 0; i < G_N_ELEMENTS (partials); i++)
    {
      g_autofree char *result = hy_markdown_to_pango (partials[i]);

      assert_valid_markup (result);
    }
}

/* Underscores are left alone: some_function_name is far more common in a chat
 * about code than italics are. */
static void
test_underscores_are_literal (void)
{
  g_autofree char *result = hy_markdown_to_pango ("call some_long_name here");

  g_assert_cmpstr (result, ==, "call some_long_name here");
}

static void
test_fenced_block (void)
{
  g_autofree char *result =
    hy_markdown_to_pango ("before\n```c\nint x = 1 < 2;\n```\nafter");

  assert_valid_markup (result);
  g_assert_nonnull (strstr (result, "<tt>"));
  g_assert_nonnull (strstr (result, "</tt>"));
  /* Content inside the fence is code, not markup. */
  g_assert_nonnull (strstr (result, "int x = 1 &lt; 2;"));
}

static void
test_heading (void)
{
  g_autofree char *result = hy_markdown_to_pango ("## Summary");

  g_assert_cmpstr (result, ==, "<b>Summary</b>");
}

static void
test_empty_and_null (void)
{
  g_autofree char *empty = hy_markdown_to_pango ("");
  g_autofree char *null_input = hy_markdown_to_pango (NULL);

  g_assert_cmpstr (empty, ==, "");
  g_assert_cmpstr (null_input, ==, "");
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/markdown/inline", test_inline_spans);
  g_test_add_func ("/markdown/escaping", test_escapes_markup_characters);
  g_test_add_func ("/markdown/partial", test_partial_input_stays_valid);
  g_test_add_func ("/markdown/underscores", test_underscores_are_literal);
  g_test_add_func ("/markdown/fence", test_fenced_block);
  g_test_add_func ("/markdown/heading", test_heading);
  g_test_add_func ("/markdown/empty", test_empty_and_null);

  return g_test_run ();
}
