#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * Just enough of a lexer to colour code the way an editor does.
 *
 * No highlighting library is linked for this. The two consumers want opposite
 * things -- the diff wants Pango markup for a label, the file preview wants
 * tags applied to a GtkTextBuffer -- and every library worth having (GtkSource
 * and its language definitions) arrives as a widget with its own buffer, which
 * is the one part neither consumer can use: the diff draws its own row
 * backgrounds behind a single shared layout, and would have to be rewritten
 * around a foreign text view to gain nothing. What is left is a token scanner,
 * and a token scanner for this small language set is compact enough to keep
 * here, where it also costs the bundled builds nothing.
 *
 * Scanning is per line and stateful, because that is what a diff can offer:
 * hunks are fragments, and the old and new sides of one hunk are two different
 * texts that have to be scanned separately.
 */

typedef enum
{
  XD_SYNTAX_NONE = 0,
  XD_SYNTAX_C,
  XD_SYNTAX_GO,
  XD_SYNTAX_DOCKERFILE,
  XD_SYNTAX_KOTLIN,
  XD_SYNTAX_MAKEFILE,
  XD_SYNTAX_RUST,
  XD_SYNTAX_JSON,
  XD_SYNTAX_YAML,
  XD_SYNTAX_TOML,
  XD_SYNTAX_V,
  XD_SYNTAX_ODIN,
  XD_SYNTAX_RUBY,
  XD_SYNTAX_CRYSTAL,
  XD_SYNTAX_CSHARP,
} XdSyntaxLanguage;

typedef enum
{
  XD_SYNTAX_TOKEN_TEXT,      /* identifiers, punctuation, whitespace */
  XD_SYNTAX_TOKEN_KEYWORD,
  XD_SYNTAX_TOKEN_TYPE,
  XD_SYNTAX_TOKEN_FUNCTION,
  XD_SYNTAX_TOKEN_STRING,
  XD_SYNTAX_TOKEN_NUMBER,    /* also the literal constants: NULL, nil, true */
  XD_SYNTAX_TOKEN_COMMENT,
  XD_SYNTAX_TOKEN_PREPROC,

  XD_SYNTAX_TOKEN_COUNT,
} XdSyntaxToken;

/*
 * What a line leaves open for the next one.
 *
 * Zeroed means "start of a text". Callers scanning a diff keep one of these
 * per side of the hunk, so a block comment removed on the old side does not
 * grey out the added lines beside it.
 */
typedef struct
{
  guint8 in_comment;         /* block-comment depth */
  guint8 in_raw_string;      /* inside a backtick raw string */
  guint8 in_triple_string;   /* inside a triple-quoted string */
  guint8 triple_quote;       /* delimiter used by that string */
  guint8 in_rust_raw_string; /* inside Rust's raw string */
  guint8 rust_raw_hashes;    /* delimiter width of that raw string */
  guint8 in_heredoc;         /* inside a Ruby or Crystal heredoc */
  guint8 heredoc_indent;     /* its terminator may be indented */
  guint8 crystal_macro_close; /* '%' or '}' while inside macro delimiters */
  guint8 in_csharp_verbatim_string; /* inside C#'s multiline @"" string */
  guint8 csharp_raw_quotes;  /* delimiter width of C#'s raw string */
  char heredoc_delimiter[32];
} XdSyntaxState;

/* XD_SYNTAX_NONE for a path in a language this does not know. */
XdSyntaxLanguage xd_syntax_language_for_path (const char *path);

/* The colour a token is drawn in, or NULL for one that keeps the text's own. */
const char      *xd_syntax_token_colour      (XdSyntaxToken token);

typedef void (*XdSyntaxTokenFunc) (XdSyntaxToken token,
                                   const char   *text,
                                   gsize         length,
                                   gpointer      user_data);

/*
 * Splits one line into tokens, advancing @state past it.
 *
 * @line is the line's text without its newline. @emit is called for every
 * byte of it in order, so concatenating the pieces gives the line back; NULL
 * only advances @state, which is how the side of a diff that is not being
 * drawn keeps up with the side that is.
 */
void             xd_syntax_scan_line         (XdSyntaxLanguage   language,
                                              const char        *line,
                                              XdSyntaxState     *state,
                                              XdSyntaxTokenFunc  emit,
                                              gpointer           user_data);

G_END_DECLS
