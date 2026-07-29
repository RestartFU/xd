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
  g_assert_cmpint (xd_syntax_language_for_path ("Main.kt"),
                   ==, XD_SYNTAX_KOTLIN);
  g_assert_cmpint (xd_syntax_language_for_path ("build.gradle.kts"),
                   ==, XD_SYNTAX_KOTLIN);
  g_assert_cmpint (xd_syntax_language_for_path ("Dockerfile"),
                   ==, XD_SYNTAX_DOCKERFILE);
  g_assert_cmpint (xd_syntax_language_for_path ("images/Dockerfile.release"),
                   ==, XD_SYNTAX_DOCKERFILE);
  g_assert_cmpint (xd_syntax_language_for_path ("Containerfile"),
                   ==, XD_SYNTAX_DOCKERFILE);
  g_assert_cmpint (xd_syntax_language_for_path ("image.dockerfile"),
                   ==, XD_SYNTAX_DOCKERFILE);
  g_assert_cmpint (xd_syntax_language_for_path ("Makefile"),
                   ==, XD_SYNTAX_MAKEFILE);
  g_assert_cmpint (xd_syntax_language_for_path ("build/Makefile.release"),
                   ==, XD_SYNTAX_MAKEFILE);
  g_assert_cmpint (xd_syntax_language_for_path ("GNUmakefile"),
                   ==, XD_SYNTAX_MAKEFILE);
  g_assert_cmpint (xd_syntax_language_for_path ("rules.mk"),
                   ==, XD_SYNTAX_MAKEFILE);
  g_assert_cmpint (xd_syntax_language_for_path ("src/main.rs"),
                   ==, XD_SYNTAX_RUST);
  g_assert_cmpint (xd_syntax_language_for_path ("README.md"),
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

static void
test_classifies_dockerfile (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *instruction = scan (
    XD_SYNTAX_DOCKERFILE,
    "from \"debian:bookworm\" AS build", &state, NULL);
  g_autofree char *port = scan (
    XD_SYNTAX_DOCKERFILE, "EXPOSE 8080", &state, NULL);
  g_autofree char *comment = scan (
    XD_SYNTAX_DOCKERFILE, "  # syntax=docker/dockerfile:1", &state, NULL);
  g_autofree char *url = scan (
    XD_SYNTAX_DOCKERFILE,
    "RUN curl https://example.test/archive", &state, NULL);

  g_assert_nonnull (strstr (instruction, "keyword:from\n"));
  g_assert_nonnull (strstr (instruction, "string:\"debian:bookworm\"\n"));
  g_assert_nonnull (strstr (instruction, "keyword:AS\n"));
  g_assert_nonnull (strstr (port, "keyword:EXPOSE\n"));
  g_assert_nonnull (strstr (port, "number:8080\n"));
  g_assert_nonnull (
    strstr (comment, "comment:# syntax=docker/dockerfile:1\n"));
  g_assert_null (strstr (url, "comment://example.test/archive\n"));
}

static void
test_classifies_kotlin (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *declaration = scan (
    XD_SYNTAX_KOTLIN,
    "data class User(val name: String, val age: Int = 42)", &state, NULL);
  g_autofree char *function = scan (
    XD_SYNTAX_KOTLIN,
    "fun greet(name: String) = \"Hello, $name\" // welcome", &state, NULL);

  g_assert_nonnull (strstr (declaration, "keyword:data\n"));
  g_assert_nonnull (strstr (declaration, "keyword:class\n"));
  g_assert_nonnull (strstr (declaration, "type:User\n"));
  g_assert_nonnull (strstr (declaration, "keyword:val\n"));
  g_assert_nonnull (strstr (declaration, "type:String\n"));
  g_assert_nonnull (strstr (declaration, "type:Int\n"));
  g_assert_nonnull (strstr (declaration, "number:42\n"));
  g_assert_nonnull (strstr (function, "keyword:fun\n"));
  g_assert_nonnull (strstr (function, "function:greet\n"));
  g_assert_nonnull (strstr (function, "string:\"Hello, $name\"\n"));
  g_assert_nonnull (strstr (function, "comment:// welcome\n"));
}

static void
test_classifies_makefile (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *directive = scan (
    XD_SYNTAX_MAKEFILE, "include config.mk", &state, NULL);
  g_autofree char *recipe = scan (
    XD_SYNTAX_MAKEFILE,
    "\t$(CC) -o \"$@\" $(call output,$(objects)) # link", &state, NULL);
  g_autofree char *jobs = scan (
    XD_SYNTAX_MAKEFILE, "JOBS ?= 8", &state, NULL);
  g_autofree char *escaped = scan (
    XD_SYNTAX_MAKEFILE, "HASH := \\#literal", &state, NULL);

  g_assert_nonnull (strstr (directive, "keyword:include\n"));
  g_assert_nonnull (strstr (recipe, "preproc:$(CC)\n"));
  g_assert_nonnull (
    strstr (recipe, "preproc:$(call output,$(objects))\n"));
  g_assert_nonnull (strstr (recipe, "string:\"$@\"\n"));
  g_assert_nonnull (strstr (recipe, "comment:# link\n"));
  g_assert_nonnull (strstr (jobs, "number:8\n"));
  g_assert_null (strstr (escaped, "comment:#literal\n"));
}

static void
test_classifies_rust (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *declaration = scan (
    XD_SYNTAX_RUST,
    "pub async fn load<'a>(path: &'a str) -> Result<String, Error> {",
    &state, NULL);
  g_autofree char *body = scan (
    XD_SYNTAX_RUST,
    "println!(r#\"value // {}\"#, 42); let letter = 'x';", &state, NULL);

  g_assert_nonnull (strstr (declaration, "keyword:pub\n"));
  g_assert_nonnull (strstr (declaration, "keyword:async\n"));
  g_assert_nonnull (strstr (declaration, "keyword:fn\n"));
  g_assert_nonnull (strstr (declaration, "function:load\n"));
  g_assert_nonnull (strstr (declaration, "preproc:'a\n"));
  g_assert_nonnull (strstr (declaration, "type:str\n"));
  g_assert_nonnull (strstr (declaration, "type:Result\n"));
  g_assert_nonnull (strstr (declaration, "type:String\n"));
  g_assert_nonnull (strstr (declaration, "type:Error\n"));
  g_assert_nonnull (strstr (body, "function:println\n"));
  g_assert_nonnull (strstr (body, "string:r#\"value // {}\"#\n"));
  g_assert_nonnull (strstr (body, "number:42\n"));
  g_assert_nonnull (strstr (body, "string:'x'\n"));
}

/*
 * The type of a composite literal is a type, and the space is what says so:
 * gofmt writes "Vec3{40}" tight and "if ok {" loose.
 */
static void
test_names_a_composite_literal_type (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *literal = scan (
    XD_SYNTAX_GO,
    "\tpk := &packet.PlayerAuthInput{Position: mgl32.Vec3{40, 70, 40}}",
    &state, NULL);
  g_autofree char *block = NULL;

  g_assert_nonnull (strstr (literal, "type:PlayerAuthInput\n"));
  g_assert_nonnull (strstr (literal, "type:Vec3\n"));

  /* The package qualifier stays plain, as does anything before a loose one. */
  g_assert_null (strstr (literal, "type:packet\n"));
  g_assert_null (strstr (literal, "type:mgl32\n"));

  block = scan (XD_SYNTAX_GO, "\tif ok {", &state, NULL);
  g_assert_null (strstr (block, "type:ok\n"));
}

/* C's braces follow a keyword or an equals sign, never a bare type name. */
static void
test_leaves_c_braces_alone (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *record = scan (
    XD_SYNTAX_C, "  while (running) { total = 1; }", &state, NULL);

  g_assert_null (strstr (record, "type:running\n"));
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

static void
test_carries_a_kotlin_triple_string (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *opened = scan (
    XD_SYNTAX_KOTLIN, "val query = \"\"\"select fun", &state, NULL);
  g_autofree char *inside = NULL;
  g_autofree char *closed = NULL;

  g_assert_true (state.in_triple_string);
  g_assert_null (strstr (opened, "keyword:fun\n"));

  inside = scan (XD_SYNTAX_KOTLIN, "from users // literal", &state, NULL);
  g_assert_true (state.in_triple_string);
  g_assert_null (strstr (inside, "comment:// literal\n"));

  closed = scan (XD_SYNTAX_KOTLIN, "where active\"\"\".trimIndent ()",
                 &state, NULL);
  g_assert_false (state.in_triple_string);
  g_assert_nonnull (strstr (closed, "string:where active\"\"\"\n"));
  g_assert_nonnull (strstr (closed, "function:trimIndent\n"));
}

static void
test_carries_a_rust_raw_string (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *opened = scan (
    XD_SYNTAX_RUST, "let json = r##\"{ \"kind\":", &state, NULL);
  g_autofree char *inside = NULL;
  g_autofree char *closed = NULL;

  g_assert_true (state.in_rust_raw_string);
  g_assert_null (strstr (opened, "string:\"kind\"\n"));

  inside = scan (XD_SYNTAX_RUST, "// still string", &state, NULL);
  g_assert_true (state.in_rust_raw_string);
  g_assert_null (strstr (inside, "comment:// still string\n"));

  closed = scan (XD_SYNTAX_RUST, "}\"##; fn done() {}", &state, NULL);
  g_assert_false (state.in_rust_raw_string);
  g_assert_nonnull (strstr (closed, "string:}\"##\n"));
  g_assert_nonnull (strstr (closed, "keyword:fn\n"));
  g_assert_nonnull (strstr (closed, "function:done\n"));
}

static void
test_carries_a_nested_rust_comment (void)
{
  XdSyntaxState state = { 0 };
  g_autofree char *opened = scan (
    XD_SYNTAX_RUST, "/* outer /* inner */ still", &state, NULL);
  g_autofree char *closed = NULL;

  g_assert_cmpuint (state.in_comment, ==, 1);
  g_assert_nonnull (
    strstr (opened, "comment:/* outer /* inner */ still\n"));

  closed = scan (XD_SYNTAX_RUST, "comment */ fn main() {}", &state, NULL);
  g_assert_false (state.in_comment);
  g_assert_nonnull (strstr (closed, "comment:comment */\n"));
  g_assert_nonnull (strstr (closed, "keyword:fn\n"));
  g_assert_nonnull (strstr (closed, "function:main\n"));
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
  g_test_add_func ("/syntax/classifies-dockerfile",
                   test_classifies_dockerfile);
  g_test_add_func ("/syntax/classifies-kotlin", test_classifies_kotlin);
  g_test_add_func ("/syntax/classifies-makefile", test_classifies_makefile);
  g_test_add_func ("/syntax/classifies-rust", test_classifies_rust);
  g_test_add_func ("/syntax/names-a-composite-literal-type",
                   test_names_a_composite_literal_type);
  g_test_add_func ("/syntax/leaves-c-braces-alone",
                   test_leaves_c_braces_alone);
  g_test_add_func ("/syntax/sees-calls-across-a-space",
                   test_sees_calls_across_a_space);
  g_test_add_func ("/syntax/colours-the-included-path",
                   test_colours_the_included_path);
  g_test_add_func ("/syntax/carries-a-block-comment",
                   test_carries_a_block_comment);
  g_test_add_func ("/syntax/carries-a-go-raw-string",
                   test_carries_a_go_raw_string);
  g_test_add_func ("/syntax/carries-a-kotlin-triple-string",
                   test_carries_a_kotlin_triple_string);
  g_test_add_func ("/syntax/carries-a-rust-raw-string",
                   test_carries_a_rust_raw_string);
  g_test_add_func ("/syntax/carries-a-nested-rust-comment",
                   test_carries_a_nested_rust_comment);
  g_test_add_func ("/syntax/leaves-unknown-languages-alone",
                   test_leaves_unknown_languages_alone);
  g_test_add_func ("/syntax/survives-unterminated-text",
                   test_survives_unterminated_text);

  return g_test_run ();
}
