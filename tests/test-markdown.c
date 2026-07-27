#include <pango/pango.h>

#include "util/markdown.h"

#include <string.h>

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
  g_autofree char *bold = xd_markdown_to_pango ("a **strong** word");
  g_autofree char *italic = xd_markdown_to_pango ("an *emphasised* word");
  g_autofree char *code = xd_markdown_to_pango ("call `g_free()` on it");

  g_assert_cmpstr (bold, ==, "a <b>strong</b> word");
  g_assert_cmpstr (italic, ==, "an <i>emphasised</i> word");
  g_assert_cmpstr (code, ==, "call <tt>g_free()</tt> on it");
}

static void
test_escapes_markup_characters (void)
{
  g_autofree char *result = xd_markdown_to_pango ("compare a < b && c > d");

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
      g_autofree char *result = xd_markdown_to_pango (partials[i]);

      assert_valid_markup (result);
    }
}

/* CommonMark understands underscore emphasis without corrupting identifiers. */
static void
test_underscore_emphasis (void)
{
  g_autofree char *result =
    xd_markdown_to_pango ("call some_long_name and _stress this_");

  g_assert_cmpstr (
    result, ==, "call some_long_name and <i>stress this</i>");
}

static void
test_fenced_block (void)
{
  g_autofree char *result =
    xd_markdown_to_pango ("before\n```c\nint x = 1 < 2;\n```\nafter");

  assert_valid_markup (result);
  g_assert_nonnull (strstr (result, "<tt>"));
  g_assert_nonnull (strstr (result, "</tt>"));
  /* Content inside the fence is code, not markup. */
  g_assert_nonnull (strstr (result, "int x = 1 &lt; 2;"));
}

static void
test_heading (void)
{
  g_autofree char *result = xd_markdown_to_pango ("## Summary");

  /* Sized as well as bold: a heading has to be visible as one among the
   * emphasised words around it. */
  g_assert_nonnull (strstr (result, "size=\"large\""));
  g_assert_nonnull (strstr (result, "<b>Summary</b>"));
  assert_valid_markup (result);
}

static void
test_hash_prefixed_prose_is_not_a_heading (void)
{
  const char *lines[] = {
    "#1 fixed. Moving to #2.",
    "#include <stdio.h>",
    "####### too many hashes",
  };

  for (gsize i = 0; i < G_N_ELEMENTS (lines); i++)
    {
      g_autofree char *result = xd_markdown_to_pango (lines[i]);

      g_assert_null (strstr (result, "size="));
      g_assert_null (strstr (result, "<b>"));
      g_assert_true (g_str_has_prefix (result, "#"));
      assert_valid_markup (result);
    }
}

static void
test_empty_and_null (void)
{
  g_autofree char *empty = xd_markdown_to_pango ("");
  g_autofree char *null_input = xd_markdown_to_pango (NULL);

  g_assert_cmpstr (empty, ==, "");
  g_assert_cmpstr (null_input, ==, "");
}

/* A link renders as one, and the URL is escaped as an attribute. */
static void
test_links (void)
{
  g_autofree char *out =
    xd_markdown_to_pango ("see [PR #54](https://github.com/x/practice/pull/54) now");
  g_autofree char *amp =
    xd_markdown_to_pango ("[q](https://x.dev/a?b=1&c=2)");

  g_assert_nonnull (strstr (out,
    "<a href=\"https://github.com/x/practice/pull/54\">PR #54</a>"));
  g_assert_nonnull (strstr (amp, "b=1&amp;c=2"));
}

static void
test_bare_urls (void)
{
  g_autofree char *out = xd_markdown_to_pango (
    "see https://github.com/RestartFU/xd/issues/5 now");
  g_autofree char *punctuation = xd_markdown_to_pango (
    "(https://example.com/a_(b)). Next.");
  g_autofree char *plain = xd_urls_to_pango (
    "**literal** https://example.com/a?b=1&c=2");

  g_assert_nonnull (strstr (
    out,
    "<a href=\"https://github.com/RestartFU/xd/issues/5\">"
    "https://github.com/RestartFU/xd/issues/5</a>"));
  g_assert_nonnull (strstr (
    punctuation,
    "<a href=\"https://example.com/a_(b)\">https://example.com/a_(b)</a>)"));
  g_assert_nonnull (strstr (plain, "**literal**"));
  g_assert_nonnull (strstr (
    plain,
    "<a href=\"https://example.com/a?b=1&amp;c=2\">"
    "https://example.com/a?b=1&amp;c=2</a>"));
}

/* List markers render as the dots they stand for. */
static void
test_list_bullets (void)
{
  g_autofree char *out = xd_markdown_to_pango ("- first\n- second\n  - nested");

  g_assert_nonnull (strstr (out, "\xe2\x80\xa2 first"));
  g_assert_nonnull (strstr (out, "\xe2\x80\xa2 second"));
  g_assert_nonnull (strstr (out, "  \xe2\x80\xa2 nested"));
  g_assert_null (strstr (out, "- first"));
}

static void
test_commonmark_blocks (void)
{
  g_autofree char *out = xd_markdown_to_pango (
    "> quoted **text**\n>\n> continued\n\n"
    "3. third\n4. fourth\n\n---\n\n"
    "    indented <code>");

  g_assert_nonnull (strstr (out, "\xe2\x94\x82 quoted <b>text</b>"));
  g_assert_nonnull (strstr (out, "3. third\n4. fourth"));
  g_assert_nonnull (strstr (out, "\xe2\x94\x80\xe2\x94\x80"));
  g_assert_nonnull (strstr (
    out, "<tt><span background=\"#181818\">indented &lt;code&gt;"));
  assert_valid_markup (out);
}

static void
test_commonmark_inline_nesting (void)
{
  g_autofree char *out = xd_markdown_to_pango (
    "***bold italic***, ~~literal~~, and \\*literal\\*");

  g_assert_nonnull (strstr (out, "<i><b>bold italic</b></i>"));
  g_assert_nonnull (strstr (out, "~~literal~~"));
  g_assert_nonnull (strstr (out, "*literal*"));
  assert_valid_markup (out);
}

static void
test_tables (void)
{
  g_autofree char *out = xd_markdown_to_pango (
    "| metric | old | new |\n"
    "|---|---|---|\n"
    "| ack_rtt_p50 | 269ms | 41ms |\n"
    "| corrections | 0 | 0 |");

  g_assert_nonnull (strstr (out, "<tt><b>metric"));
  g_assert_nonnull (strstr (out, "ack_rtt_p50"));
  g_assert_nonnull (strstr (out, "269ms"));
  g_assert_nonnull (strstr (out, "\xe2\x94\x82"));
  g_assert_nonnull (strstr (out, "\xe2\x94\xbc"));
  g_assert_null (strstr (out, "|---|"));
  assert_valid_markup (out);
}

static void
test_pipe_prose_is_not_a_table (void)
{
  g_autofree char *out = xd_markdown_to_pango (
    "Run foo | bar.\n"
    "This is still ordinary prose.");

  g_assert_null (strstr (out, "<tt>"));
  g_assert_nonnull (strstr (out, "foo | bar"));
  assert_valid_markup (out);
}

static void
test_images_and_unsafe_links (void)
{
  g_autofree char *image =
    xd_markdown_to_pango ("![diagram](https://example.com/image.png)");
  g_autofree char *unsafe =
    xd_markdown_to_pango ("[do not run](javascript:alert(1))");

  g_assert_nonnull (strstr (
    image,
    "Image: <a href=\"https://example.com/image.png\">diagram</a>"));
  g_assert_cmpstr (unsafe, ==, "do not run");
}

static void
test_raw_html_stays_literal (void)
{
  g_autofree char *out =
    xd_markdown_to_pango ("<span size=\"999999\">small</span>");

  g_assert_nonnull (strstr (
    out, "&lt;span size=&quot;999999&quot;&gt;small&lt;/span&gt;"));
  assert_valid_markup (out);
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/markdown/inline", test_inline_spans);
  g_test_add_func ("/markdown/escaping", test_escapes_markup_characters);
  g_test_add_func ("/markdown/partial", test_partial_input_stays_valid);
  g_test_add_func ("/markdown/underscores", test_underscore_emphasis);
  g_test_add_func ("/markdown/fence", test_fenced_block);
  g_test_add_func ("/markdown/heading", test_heading);
  g_test_add_func ("/markdown/hash-prefixed-prose",
                   test_hash_prefixed_prose_is_not_a_heading);
  g_test_add_func ("/markdown/empty", test_empty_and_null);

  g_test_add_func ("/markdown/links", test_links);
  g_test_add_func ("/markdown/bare-urls", test_bare_urls);
  g_test_add_func ("/markdown/bullets", test_list_bullets);
  g_test_add_func ("/markdown/commonmark-blocks", test_commonmark_blocks);
  g_test_add_func ("/markdown/commonmark-inline", test_commonmark_inline_nesting);
  g_test_add_func ("/markdown/tables", test_tables);
  g_test_add_func ("/markdown/pipe-prose", test_pipe_prose_is_not_a_table);
  g_test_add_func ("/markdown/images-and-unsafe-links",
                   test_images_and_unsafe_links);
  g_test_add_func ("/markdown/raw-html", test_raw_html_stays_literal);

  return g_test_run ();
}
