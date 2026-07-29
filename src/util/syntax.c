#include "syntax.h"

#include <string.h>

static const char *const C_KEYWORDS[] = {
  "auto", "break", "case", "char", "const", "continue", "default", "do",
  "double", "else", "enum", "extern", "float", "for", "goto", "if", "inline",
  "int", "long", "register", "restrict", "return", "short", "signed", "sizeof",
  "static", "struct", "switch", "typedef", "union", "unsigned", "void",
  "volatile", "while", "_Alignas", "_Alignof", "_Atomic", "_Bool", "_Generic",
  "_Noreturn", "_Static_assert", "_Thread_local",
  NULL,
};

static const char *const C_TYPES[] = {
  "bool", "size_t", "ssize_t", "ptrdiff_t", "intptr_t", "uintptr_t",
  "int8_t", "int16_t", "int32_t", "int64_t",
  "uint8_t", "uint16_t", "uint32_t", "uint64_t",
  "wchar_t", "va_list", "FILE",
  NULL,
};

static const char *const C_CONSTANTS[] = {
  "NULL", "true", "false", NULL,
};

static const char *const GO_KEYWORDS[] = {
  "break", "case", "chan", "const", "continue", "default", "defer", "else",
  "fallthrough", "for", "func", "go", "goto", "if", "import", "interface",
  "map", "package", "range", "return", "select", "struct", "switch", "type",
  "var",
  NULL,
};

static const char *const GO_TYPES[] = {
  "any", "bool", "byte", "comparable", "complex64", "complex128", "error",
  "float32", "float64", "int", "int8", "int16", "int32", "int64", "rune",
  "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr",
  NULL,
};

/*
 * Go's builtins are ordinary identifiers the compiler happens to predeclare,
 * and reading as a call is exactly what len and make do. They sit with the
 * constants because that is the same shade.
 */
static const char *const GO_CONSTANTS[] = {
  "append", "cap", "clear", "close", "complex", "copy", "delete", "imag",
  "iota", "len", "make", "max", "min", "new", "nil", "panic", "print",
  "println", "real", "recover", "true", "false",
  NULL,
};

static const char *const KOTLIN_KEYWORDS[] = {
  "abstract", "actual", "annotation", "as", "break", "by", "catch", "class",
  "companion", "const", "constructor", "context", "continue", "crossinline",
  "data", "delegate", "do", "dynamic", "else", "enum", "expect", "external",
  "field", "file", "final", "finally", "for", "fun", "get", "if", "import",
  "in", "infix", "init", "inline", "inner", "interface", "internal", "is",
  "lateinit", "noinline", "object", "open", "operator", "out", "override",
  "package", "param", "private", "property", "protected", "public",
  "receiver", "reified", "return", "sealed", "set", "setparam", "super",
  "suspend", "tailrec", "this", "throw", "try", "typealias", "typeof", "val",
  "value", "var", "vararg", "when", "where", "while",
  NULL,
};

static const char *const KOTLIN_TYPES[] = {
  "Any", "Array", "Boolean", "Byte", "Char", "Double", "Float", "Int", "Long",
  "Nothing", "Short", "String", "UByte", "UInt", "ULong", "UShort", "Unit",
  NULL,
};

static const char *const KOTLIN_CONSTANTS[] = {
  "false", "null", "true", NULL,
};

static const char *const DOCKERFILE_KEYWORDS[] = {
  "ADD", "ARG", "AS", "CMD", "COPY", "ENTRYPOINT", "ENV", "EXPOSE", "FROM",
  "HEALTHCHECK", "LABEL", "MAINTAINER", "ONBUILD", "RUN", "SHELL",
  "STOPSIGNAL", "USER", "VOLUME", "WORKDIR",
  NULL,
};

static const char *const NO_WORDS[] = {
  NULL,
};

typedef struct
{
  const char *const *keywords;
  const char *const *types;
  const char *const *constants;
  gboolean raw_strings;         /* Go's backtick string, which spans lines */
  gboolean triple_strings;      /* Kotlin's triple-quoted string */
  gboolean directives;          /* C's # lines */
  gboolean slash_comments;       /* C, Go and Kotlin's // comments */
  gboolean block_comments;       /* C, Go and Kotlin's block comments */
  gboolean hash_comments;        /* Dockerfile's leading # comments */
  gboolean case_insensitive;     /* Dockerfile instructions */
  gboolean capitalized_types;    /* Kotlin's user-defined types */
  gboolean composite_literals;  /* Go's Type{...} */
} Language;

static const Language C_LANGUAGE = {
  .keywords = C_KEYWORDS,
  .types = C_TYPES,
  .constants = C_CONSTANTS,
  .directives = TRUE,
  .slash_comments = TRUE,
  .block_comments = TRUE,
};

static const Language GO_LANGUAGE = {
  .keywords = GO_KEYWORDS,
  .types = GO_TYPES,
  .constants = GO_CONSTANTS,
  .raw_strings = TRUE,
  .slash_comments = TRUE,
  .block_comments = TRUE,
  .composite_literals = TRUE,
};

static const Language KOTLIN_LANGUAGE = {
  .keywords = KOTLIN_KEYWORDS,
  .types = KOTLIN_TYPES,
  .constants = KOTLIN_CONSTANTS,
  .triple_strings = TRUE,
  .slash_comments = TRUE,
  .block_comments = TRUE,
  .capitalized_types = TRUE,
};

static const Language DOCKERFILE_LANGUAGE = {
  .keywords = DOCKERFILE_KEYWORDS,
  .types = NO_WORDS,
  .constants = NO_WORDS,
  .hash_comments = TRUE,
  .case_insensitive = TRUE,
};

static const Language *
language_table (XdSyntaxLanguage language)
{
  if (language == XD_SYNTAX_C)
    return &C_LANGUAGE;
  if (language == XD_SYNTAX_GO)
    return &GO_LANGUAGE;
  if (language == XD_SYNTAX_DOCKERFILE)
    return &DOCKERFILE_LANGUAGE;
  if (language == XD_SYNTAX_KOTLIN)
    return &KOTLIN_LANGUAGE;

  return NULL;
}

XdSyntaxLanguage
xd_syntax_language_for_path (const char *path)
{
  const char *dot;

  if (path == NULL)
    return XD_SYNTAX_NONE;

  /* Only the last component has an extension: a directory called "src.go"
   * says nothing about the file inside it. */
  for (const char *at = path; *at != '\0'; at++)
    if (*at == '/' || *at == '\\')
      path = at + 1;

  if (g_ascii_strcasecmp (path, "Dockerfile") == 0 ||
      g_ascii_strncasecmp (path, "Dockerfile.", 11) == 0 ||
      g_ascii_strcasecmp (path, "Containerfile") == 0 ||
      g_ascii_strncasecmp (path, "Containerfile.", 14) == 0)
    return XD_SYNTAX_DOCKERFILE;

  dot = strrchr (path, '.');
  if (dot == NULL)
    return XD_SYNTAX_NONE;

  if (g_strcmp0 (dot, ".go") == 0)
    return XD_SYNTAX_GO;
  if (g_strcmp0 (dot, ".c") == 0 || g_strcmp0 (dot, ".h") == 0)
    return XD_SYNTAX_C;
  if (g_strcmp0 (dot, ".kt") == 0 || g_strcmp0 (dot, ".kts") == 0)
    return XD_SYNTAX_KOTLIN;
  if (g_ascii_strcasecmp (dot, ".dockerfile") == 0)
    return XD_SYNTAX_DOCKERFILE;

  return XD_SYNTAX_NONE;
}

const char *
xd_syntax_token_colour (XdSyntaxToken token)
{
  switch (token)
    {
    case XD_SYNTAX_TOKEN_KEYWORD:  return "#dc8add";
    case XD_SYNTAX_TOKEN_TYPE:     return "#78aeed";
    case XD_SYNTAX_TOKEN_FUNCTION: return "#99c1f1";
    case XD_SYNTAX_TOKEN_STRING:   return "#f8e45c";
    case XD_SYNTAX_TOKEN_NUMBER:   return "#ffbe6f";
    case XD_SYNTAX_TOKEN_COMMENT:  return "#8b8e8f";
    case XD_SYNTAX_TOKEN_PREPROC:  return "#c061cc";
    default:                       return NULL;
    }
}

/*
 * Unclassified bytes are gathered rather than emitted one at a time.
 *
 * A line of code is mostly punctuation and indentation; handing each byte to
 * the caller separately would mean a Pango span or a buffer tag per character.
 */
typedef struct
{
  XdSyntaxTokenFunc emit;
  gpointer user_data;
  const char *pending;
  gsize pending_length;
} Emitter;

static void
flush_plain (Emitter *emitter)
{
  if (emitter->pending_length == 0)
    return;

  if (emitter->emit != NULL)
    emitter->emit (XD_SYNTAX_TOKEN_TEXT, emitter->pending,
                   emitter->pending_length, emitter->user_data);

  emitter->pending = NULL;
  emitter->pending_length = 0;
}

static void
append_plain (Emitter    *emitter,
              const char *at,
              gsize       length)
{
  if (emitter->pending == NULL)
    emitter->pending = at;

  emitter->pending_length += length;
}

static void
append_token (Emitter      *emitter,
              XdSyntaxToken token,
              const char   *at,
              gsize         length)
{
  if (length == 0)
    return;

  flush_plain (emitter);

  if (emitter->emit != NULL)
    emitter->emit (token, at, length, emitter->user_data);
}

static gboolean
word_listed (const char *const *list,
             const char        *word,
             gsize              length,
             gboolean           case_insensitive)
{
  for (gsize i = 0; list[i] != NULL; i++)
    {
      int compared = case_insensitive
        ? g_ascii_strncasecmp (list[i], word, length)
        : strncmp (list[i], word, length);

      if (compared == 0 && list[i][length] == '\0')
        return TRUE;
    }

  return FALSE;
}

static gboolean
is_word_byte (char byte)
{
  return g_ascii_isalnum (byte) || byte == '_';
}

/* Everything up to the closing quote, or to the end of an unterminated line. */
static const char *
scan_quoted (Emitter    *emitter,
             const char *at,
             char        quote)
{
  const char *scan = at + 1;

  while (*scan != '\0' && *scan != quote)
    scan += (*scan == '\\' && scan[1] != '\0') ? 2 : 1;

  if (*scan == quote)
    scan++;

  append_token (emitter, XD_SYNTAX_TOKEN_STRING, at, (gsize) (scan - at));
  return scan;
}

/*
 * One numeric literal, in whatever spelling either language allows.
 *
 * Bases, exponents, Go's digit separators and C's suffixes all live inside a
 * run of alphanumerics, dots and underscores; the only break in that run is
 * the sign of an exponent.
 */
static const char *
scan_number (Emitter    *emitter,
             const char *at)
{
  const char *scan = at;

  while (*scan != '\0')
    {
      if (is_word_byte (*scan) || *scan == '.')
        {
          scan++;
          continue;
        }

      if ((*scan == '+' || *scan == '-') && scan > at &&
          strchr ("eEpP", scan[-1]) != NULL)
        {
          scan++;
          continue;
        }

      break;
    }

  append_token (emitter, XD_SYNTAX_TOKEN_NUMBER, at, (gsize) (scan - at));
  return scan;
}

static const char *
scan_word (Emitter        *emitter,
           const Language *language,
           const char     *at)
{
  const char *scan = at;
  const char *after;
  gsize length;

  while (is_word_byte (*scan))
    scan++;

  length = (gsize) (scan - at);

  /* This project writes calls as "name (args)", so the space is skipped
   * before deciding whether an unlisted word is being called. */
  for (after = scan; *after == ' ' || *after == '\t'; after++)
    ;

  if (word_listed (language->keywords, at, length,
                   language->case_insensitive))
    append_token (emitter, XD_SYNTAX_TOKEN_KEYWORD, at, length);
  else if (word_listed (language->types, at, length,
                        language->case_insensitive))
    append_token (emitter, XD_SYNTAX_TOKEN_TYPE, at, length);
  else if (word_listed (language->constants, at, length,
                        language->case_insensitive))
    append_token (emitter, XD_SYNTAX_TOKEN_NUMBER, at, length);
  else if (language->capitalized_types && g_ascii_isupper (*at))
    append_token (emitter, XD_SYNTAX_TOKEN_TYPE, at, length);
  /*
   * A composite literal names a type: Vec3{40, 70, 40}. The space is what
   * tells it apart from a block -- gofmt writes the literal tight and "if ok
   * {" loose -- so this one brace is looked for where it is, not past the
   * whitespace the call below skips.
   */
  else if (language->composite_literals && *scan == '{')
    append_token (emitter, XD_SYNTAX_TOKEN_TYPE, at, length);
  else if (*after == '(')
    append_token (emitter, XD_SYNTAX_TOKEN_FUNCTION, at, length);
  else
    append_plain (emitter, at, length);

  return scan;
}

/* A directive only counts in the first column, which is where C puts it. */
static gboolean
starts_the_line (const char *line,
                 const char *at)
{
  for (const char *scan = line; scan < at; scan++)
    if (*scan != ' ' && *scan != '\t')
      return FALSE;

  return TRUE;
}

static const char *
scan_directive (Emitter    *emitter,
                const char *at)
{
  const char *scan = at + 1;
  gsize name_length;

  while (g_ascii_isalpha (*scan))
    scan++;

  name_length = (gsize) (scan - at) - 1;
  append_token (emitter, XD_SYNTAX_TOKEN_PREPROC, at, (gsize) (scan - at));

  /* An included path is quoted or bracketed; both read as one string. */
  if (name_length == 7 && strncmp (at + 1, "include", 7) == 0)
    {
      const char *open = scan;

      while (*open == ' ' || *open == '\t')
        open++;

      if (*open == '<')
        {
          const char *close = strchr (open, '>');

          if (close != NULL)
            {
              append_plain (emitter, scan, (gsize) (open - scan));
              append_token (emitter, XD_SYNTAX_TOKEN_STRING, open,
                            (gsize) (close + 1 - open));
              return close + 1;
            }
        }
    }

  return scan;
}

void
xd_syntax_scan_line (XdSyntaxLanguage   language,
                     const char        *line,
                     XdSyntaxState     *state,
                     XdSyntaxTokenFunc  emit,
                     gpointer           user_data)
{
  const Language *table = language_table (language);
  Emitter emitter = { .emit = emit, .user_data = user_data };
  const char *at = line;

  g_return_if_fail (state != NULL);

  if (line == NULL)
    return;

  if (table == NULL)
    {
      if (emit != NULL && *line != '\0')
        emit (XD_SYNTAX_TOKEN_TEXT, line, strlen (line), user_data);
      return;
    }

  if (state->in_comment)
    {
      const char *close = strstr (at, "*/");

      if (close == NULL)
        {
          append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at, strlen (at));
          return;
        }

      append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at,
                    (gsize) (close + 2 - at));
      at = close + 2;
      state->in_comment = FALSE;
    }
  else if (state->in_raw_string)
    {
      const char *close = strchr (at, '`');

      if (close == NULL)
        {
          append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at, strlen (at));
          return;
        }

      append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at,
                    (gsize) (close + 1 - at));
      at = close + 1;
      state->in_raw_string = FALSE;
    }
  else if (state->in_triple_string)
    {
      const char *close = strstr (at, "\"\"\"");

      if (close == NULL)
        {
          append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at, strlen (at));
          return;
        }

      append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at,
                    (gsize) (close + 3 - at));
      at = close + 3;
      state->in_triple_string = FALSE;
    }

  while (*at != '\0')
    {
      if (table->block_comments && at[0] == '/' && at[1] == '*')
        {
          const char *close = strstr (at + 2, "*/");

          if (close == NULL)
            {
              append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at, strlen (at));
              state->in_comment = TRUE;
              return;
            }

          append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at,
                        (gsize) (close + 2 - at));
          at = close + 2;
        }
      else if (table->slash_comments && at[0] == '/' && at[1] == '/')
        {
          append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at, strlen (at));
          return;
        }
      else if (table->hash_comments && *at == '#' &&
               starts_the_line (line, at))
        {
          append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at, strlen (at));
          return;
        }
      else if (table->triple_strings && strncmp (at, "\"\"\"", 3) == 0)
        {
          const char *close = strstr (at + 3, "\"\"\"");

          if (close == NULL)
            {
              append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at, strlen (at));
              state->in_triple_string = TRUE;
              return;
            }

          append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at,
                        (gsize) (close + 3 - at));
          at = close + 3;
        }
      else if (*at == '"' || *at == '\'')
        {
          at = scan_quoted (&emitter, at, *at);
        }
      else if (table->raw_strings && *at == '`')
        {
          const char *close = strchr (at + 1, '`');

          if (close == NULL)
            {
              append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at, strlen (at));
              state->in_raw_string = TRUE;
              return;
            }

          append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at,
                        (gsize) (close + 1 - at));
          at = close + 1;
        }
      else if (table->directives && *at == '#' && starts_the_line (line, at))
        {
          at = scan_directive (&emitter, at);
        }
      else if (g_ascii_isdigit (*at) ||
               (*at == '.' && g_ascii_isdigit (at[1])))
        {
          at = scan_number (&emitter, at);
        }
      else if (g_ascii_isalpha (*at) || *at == '_')
        {
          at = scan_word (&emitter, table, at);
        }
      else
        {
          append_plain (&emitter, at, 1);
          at++;
        }
    }

  flush_plain (&emitter);
}
