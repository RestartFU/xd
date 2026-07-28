#include "util/syntax.h"

#include <string.h>

typedef struct
{
  GString *text;      /* every byte handed back, in order */
  GString *record;    /* "token:text" per classified piece */
} Capture;

static const char *
token_name (XdSyntaxToken token)
{
  switch (token)
    {
    case XD_SYNTAX_TOKEN_KEYWORD:  return "keyword";
    case XD_SYNTAX_TOKEN_TYPE:     return "type";
    case XD_SYNTAX_TOKEN_FUNCTION: return "function";
    case XD_SYNTAX_TOKEN_STRING:   return "string";
    case XD_SYNTAX_TOKEN_NUMBER:   return "number";
    case XD_SYNTAX_TOKEN_COMMENT:  return "comment";
    case XD_SYNTAX_TOKEN_PREPROC:  return "preproc";
    default:                       return "text";
    }
}

static void
capture_token (XdSyntaxToken token,
               const char   *text,
               gsize         length,
               gpointer      user_data)
{
  Capture *capture = user_data;

  g_string_append_len (capture->text, text, (gssize) length);
  g_string_append_printf (capture->record, "%s:%.*s\n",
                          token_name (token), (int) length, text);
}

static char *
scan (XdSyntaxLanguage  language,
      const char       *line,
      XdSyntaxState    *state,
      char            **whole)
{
  Capture capture = { g_string_new (NULL), g_string_new (NULL) };

  xd_syntax_scan_line (language, line, state, capture_token, &capture);

  if (whole != NULL)
    *whole = g_string_free (capture.text, FALSE);
  else
    g_string_free (capture.text, TRUE);

  return g_string_free (capture.record, FALSE);
}

static void
test_reads_the_path (void)
{
  g_assert_cmpint (xd_syntax_language_for_path ("src/util/syntax.c"),
                   ==, XD_SYNTAX_C);
  g_assert_cmpint (xd_syntax_language_for_path ("a/b.h"), ==, XD_SYNTAX_C);
  g_assert_cmpint (xd_syntax_language_for_path ("main.go"), ==, XD_SYNTAX_GO);
  g_assert_cmpint (xd_syntax_language_for_path ("README.md"),
                   ==, XD_SYNTAX_NONE);
  g_assert_cmpint (xd_syntax_language_for_path ("Makefile"),
                   ==, XD_SYNTAX_NONE);
  g_assert_cmpint (xd_syntax_language_for_path (NULL), ==, XD_SYNTAX_NONE);

  /* An extension on a directory says nothing about the file inside it. */
  g_assert_cmpint (xd_syntax_language_for_path ("vendor.go/LICENSE"),
                   ==, XD_SYNTAX_NONE);
}

/* Concatenating the pieces has to give the line back, or text would go
 * missing from a diff row the moment it was coloured. */
static void
test_hands_back_every_byte (void)
{
  static const char *line =
    "  if (x < 3 && s == \"a & b\") /* why */ return foo (1);";
  XdSyntaxState state = { 0 };
  g_autofree char *whole = NULL;
  g_autofree char *record = scan (XD_SYNTAX_C, line, &state, &whole);

  g_assert_cmpstr (whole, ==, line);
  g_assert_nonnull (record);
}

static void
test_classifies_c (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *record = scan (
    XD_SYNTAX_C, "  static int count = 0x1f;  // seen", &state, NULL);

  g_assert_nonnull (strstr (record, "keyword:static\n"));
  g_assert_nonnull (strstr (record, "keyword:int\n"));
  g_assert_nonnull (strstr (record, "number:0x1f\n"));
  g_assert_nonnull (strstr (record, "comment:// seen\n"));
}

static void
test_classifies_go (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *record = scan (
    XD_SYNTAX_GO, "func read(p []byte) (int, error) { return len(p), nil }",
    &state, NULL);

  g_assert_nonnull (strstr (record, "keyword:func\n"));
  g_assert_nonnull (strstr (record, "function:read\n"));
  g_assert_nonnull (strstr (record, "type:byte\n"));
  g_assert_nonnull (strstr (record, "type:error\n"));
  g_assert_nonnull (strstr (record, "number:nil\n"));
}

/* This project writes calls with a space before the bracket. */
static void
test_sees_calls_across_a_space (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *record = scan (
    XD_SYNTAX_C, "  g_free (self->path);", &state, NULL);

  g_assert_nonnull (strstr (record, "function:g_free\n"));
}

static void
test_colours_the_included_path (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *record = scan (
    XD_SYNTAX_C, "#include <glib.h>", &state, NULL);

  g_assert_nonnull (strstr (record, "preproc:#include\n"));
  g_assert_nonnull (strstr (record, "string:<glib.h>\n"));
}

/* A comment that outlives its line, and a line that ends it. */
static void
test_carries_a_block_comment (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *opened = scan (XD_SYNTAX_C, "int a; /* why", &state, NULL);
  g_autofree char *inside = NULL;
  g_autofree char *closed = NULL;

  g_assert_nonnull (strstr (opened, "comment:/* why\n"));
  g_assert_true (state.in_comment);

  inside = scan (XD_SYNTAX_C, " * still int", &state, NULL);
  g_assert_nonnull (strstr (inside, "comment: * still int\n"));
  g_assert_null (strstr (inside, "keyword:int\n"));
  g_assert_true (state.in_comment);

  closed = scan (XD_SYNTAX_C, " */ int b;", &state, NULL);
  g_assert_false (state.in_comment);
  g_assert_nonnull (strstr (closed, "comment: */\n"));
  g_assert_nonnull (strstr (closed, "keyword:int\n"));
}

static void
test_carries_a_go_raw_string (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *opened = scan (
    XD_SYNTAX_GO, "const q = `select func", &state, NULL);
  g_autofree char *closed = NULL;

  g_assert_true (state.in_raw_string);
  g_assert_null (strstr (opened, "keyword:func\n"));

  closed = scan (XD_SYNTAX_GO, "from t` + x", &state, NULL);
  g_assert_false (state.in_raw_string);
  g_assert_nonnull (strstr (closed, "string:from t`\n"));
}

/* A language this cannot read is handed straight back, uncoloured. */
static void
test_leaves_unknown_languages_alone (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *whole = NULL;
  g_autofree char *record =
    scan (XD_SYNTAX_NONE, "func main() { // go", &state, &whole);

  g_assert_cmpstr (whole, ==, "func main() { // go");
  g_assert_cmpstr (record, ==, "text:func main() { // go\n");
}

static void
test_survives_unterminated_text (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *whole = NULL;
  g_autofree char *record =
    scan (XD_SYNTAX_C, "char *s = \"open", &state, &whole);

  g_assert_cmpstr (whole, ==, "char *s = \"open");
  g_assert_nonnull (strstr (record, "string:\"open\n"));

  g_clear_pointer (&whole, g_free);
  g_clear_pointer (&record, g_free);
  record = scan (XD_SYNTAX_C, "", &state, &whole);
  g_assert_cmpstr (whole, ==, "");
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/syntax/reads-the-path", test_reads_the_path);
  g_test_add_func ("/syntax/hands-back-every-byte",
                   test_hands_back_every_byte);
  g_test_add_func ("/syntax/classifies-c", test_classifies_c);
  g_test_add_func ("/syntax/classifies-go", test_classifies_go);
  g_test_add_func ("/syntax/sees-calls-across-a-space",
                   test_sees_calls_across_a_space);
  g_test_add_func ("/syntax/colours-the-included-path",
                   test_colours_the_included_path);
  g_test_add_func ("/syntax/carries-a-block-comment",
                   test_carries_a_block_comment);
  g_test_add_func ("/syntax/carries-a-go-raw-string",
                   test_carries_a_go_raw_string);
  g_test_add_func ("/syntax/leaves-unknown-languages-alone",
                   test_leaves_unknown_languages_alone);
  g_test_add_func ("/syntax/survives-unterminated-text",
                   test_survives_unterminated_text);

  return g_test_run ();
}
