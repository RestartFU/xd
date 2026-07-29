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

static const char *const MAKEFILE_KEYWORDS[] = {
  "define", "else", "endef", "endif", "export", "ifdef", "ifeq", "ifndef",
  "ifneq", "include", "override", "private", "sinclude", "undefine",
  "unexport", "vpath",
  NULL,
};

static const char *const RUST_KEYWORDS[] = {
  "abstract", "as", "async", "await", "become", "box", "break", "const",
  "continue", "crate", "do", "dyn", "else", "enum", "extern", "final", "fn",
  "for", "gen", "if", "impl", "in", "let", "loop", "macro", "macro_rules",
  "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
  "self", "static", "struct", "super", "trait", "try", "type", "typeof",
  "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
  NULL,
};

static const char *const RUST_TYPES[] = {
  "bool", "char", "str",
  "i8", "i16", "i32", "i64", "i128", "isize",
  "u8", "u16", "u32", "u64", "u128", "usize",
  "f32", "f64",
  "Self", "String", "Vec", "Option", "Result", "Box",
  NULL,
};

static const char *const RUST_CONSTANTS[] = {
  "false", "None", "true", NULL,
};

static const char *const JSON_CONSTANTS[] = {
  "false", "null", "true", NULL,
};

static const char *const YAML_CONSTANTS[] = {
  "false", "no", "null", "off", "on", "true", "yes",
  NULL,
};

static const char *const TOML_CONSTANTS[] = {
  "false", "true", NULL,
};

static const char *const V_KEYWORDS[] = {
  "as", "asm", "assert", "atomic", "break", "const", "continue", "defer",
  "else", "enum", "fn", "for", "go", "goto", "if", "implements", "import",
  "in", "interface", "is", "isreftype", "lock", "match", "module", "mut",
  "or", "pub", "return", "rlock", "select", "shared", "sizeof", "spawn",
  "static", "struct", "type", "typeof", "union", "unsafe", "volatile",
  "__global", "__offsetof",
  NULL,
};

static const char *const V_TYPES[] = {
  "any", "bool", "byte", "byteptr", "char", "charptr", "f32", "f64",
  "i8", "i16", "int", "i64", "i128", "isize", "map", "rune", "string",
  "u8", "u16", "u32", "u64", "u128", "usize", "voidptr",
  NULL,
};

static const char *const V_CONSTANTS[] = {
  "false", "none", "true", NULL,
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
  gboolean triple_strings;      /* multiline triple-quoted strings */
  gboolean single_triple_strings; /* triple apostrophes too */
  gboolean directives;          /* C's # lines */
  gboolean slash_comments;       /* C-like // comments */
  gboolean block_comments;       /* C-like block comments */
  gboolean hash_comments;        /* Dockerfile's leading # comments */
  gboolean inline_hash_comments; /* Make's unescaped # comments */
  gboolean spaced_hash_comments; /* YAML's whitespace-separated # comments */
  gboolean make_variables;       /* Make's $(...), ${...} and $x */
  gboolean case_insensitive;     /* Dockerfile instructions */
  gboolean capitalized_types;    /* user-defined types */
  gboolean composite_literals;  /* Go's Type{...} */
  gboolean bang_functions;       /* Rust's macro_name! */
  gboolean generic_functions;    /* Rust's function_name<T>(...) */
  gboolean square_generic_functions; /* V's function_name[T](...) */
  gboolean rust_lifetimes;       /* Rust's 'name */
  gboolean rust_strings;         /* Rust's r#"..."# strings */
  gboolean nested_block_comments; /* languages with nested block comments */
  gboolean bare_keys;            /* YAML and TOML unquoted keys */
  gboolean quoted_keys;          /* JSON, YAML and TOML quoted keys */
  gboolean table_headers;        /* TOML's [table] and [[array]] */
  gboolean yaml_references;      /* YAML anchors, aliases and tags */
  gboolean tilde_constant;       /* YAML's null shorthand */
  gboolean prefixed_raw_strings; /* V's r'...' and r"..." strings */
  gboolean backtick_literals;    /* V's rune literals */
  gboolean at_attributes;        /* V's @[attribute] */
  gboolean dollar_directives;    /* V's $if and compile-time calls */
  gboolean shebangs;             /* executable scripts' #! line */
  char key_delimiter;            /* ':' for mappings, '=' for assignments */
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

static const Language MAKEFILE_LANGUAGE = {
  .keywords = MAKEFILE_KEYWORDS,
  .types = NO_WORDS,
  .constants = NO_WORDS,
  .inline_hash_comments = TRUE,
  .make_variables = TRUE,
};

static const Language RUST_LANGUAGE = {
  .keywords = RUST_KEYWORDS,
  .types = RUST_TYPES,
  .constants = RUST_CONSTANTS,
  .slash_comments = TRUE,
  .block_comments = TRUE,
  .capitalized_types = TRUE,
  .bang_functions = TRUE,
  .generic_functions = TRUE,
  .rust_lifetimes = TRUE,
  .rust_strings = TRUE,
  .nested_block_comments = TRUE,
};

static const Language JSON_LANGUAGE = {
  .keywords = NO_WORDS,
  .types = NO_WORDS,
  .constants = JSON_CONSTANTS,
  .quoted_keys = TRUE,
  .key_delimiter = ':',
};

static const Language YAML_LANGUAGE = {
  .keywords = NO_WORDS,
  .types = NO_WORDS,
  .constants = YAML_CONSTANTS,
  .spaced_hash_comments = TRUE,
  .case_insensitive = TRUE,
  .bare_keys = TRUE,
  .quoted_keys = TRUE,
  .yaml_references = TRUE,
  .tilde_constant = TRUE,
  .key_delimiter = ':',
};

static const Language TOML_LANGUAGE = {
  .keywords = NO_WORDS,
  .types = NO_WORDS,
  .constants = TOML_CONSTANTS,
  .triple_strings = TRUE,
  .single_triple_strings = TRUE,
  .inline_hash_comments = TRUE,
  .bare_keys = TRUE,
  .quoted_keys = TRUE,
  .table_headers = TRUE,
  .key_delimiter = '=',
};

static const Language V_LANGUAGE = {
  .keywords = V_KEYWORDS,
  .types = V_TYPES,
  .constants = V_CONSTANTS,
  .directives = TRUE,
  .slash_comments = TRUE,
  .block_comments = TRUE,
  .capitalized_types = TRUE,
  .composite_literals = TRUE,
  .square_generic_functions = TRUE,
  .nested_block_comments = TRUE,
  .prefixed_raw_strings = TRUE,
  .backtick_literals = TRUE,
  .at_attributes = TRUE,
  .dollar_directives = TRUE,
  .shebangs = TRUE,
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
  if (language == XD_SYNTAX_MAKEFILE)
    return &MAKEFILE_LANGUAGE;
  if (language == XD_SYNTAX_RUST)
    return &RUST_LANGUAGE;
  if (language == XD_SYNTAX_JSON)
    return &JSON_LANGUAGE;
  if (language == XD_SYNTAX_YAML)
    return &YAML_LANGUAGE;
  if (language == XD_SYNTAX_TOML)
    return &TOML_LANGUAGE;
  if (language == XD_SYNTAX_V)
    return &V_LANGUAGE;

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

  if (g_ascii_strcasecmp (path, "Makefile") == 0 ||
      g_ascii_strncasecmp (path, "Makefile.", 9) == 0 ||
      g_ascii_strcasecmp (path, "GNUmakefile") == 0 ||
      g_ascii_strcasecmp (path, "BSDmakefile") == 0)
    return XD_SYNTAX_MAKEFILE;

  dot = strrchr (path, '.');
  if (dot == NULL)
    return XD_SYNTAX_NONE;

  if (g_strcmp0 (dot, ".go") == 0)
    return XD_SYNTAX_GO;
  if (g_strcmp0 (dot, ".c") == 0 || g_strcmp0 (dot, ".h") == 0)
    return XD_SYNTAX_C;
  if (g_strcmp0 (dot, ".kt") == 0 || g_strcmp0 (dot, ".kts") == 0)
    return XD_SYNTAX_KOTLIN;
  if (g_strcmp0 (dot, ".mk") == 0 || g_strcmp0 (dot, ".mak") == 0 ||
      g_strcmp0 (dot, ".make") == 0)
    return XD_SYNTAX_MAKEFILE;
  if (g_strcmp0 (dot, ".rs") == 0)
    return XD_SYNTAX_RUST;
  if (g_strcmp0 (dot, ".json") == 0)
    return XD_SYNTAX_JSON;
  if (g_strcmp0 (dot, ".yaml") == 0 || g_strcmp0 (dot, ".yml") == 0)
    return XD_SYNTAX_YAML;
  if (g_strcmp0 (dot, ".toml") == 0)
    return XD_SYNTAX_TOML;
  if (g_strcmp0 (dot, ".v") == 0 || g_strcmp0 (dot, ".vsh") == 0)
    return XD_SYNTAX_V;
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

static gboolean
quoted_key (const char *at,
            char        quote,
            char        delimiter)
{
  const char *scan = at + 1;

  while (*scan != '\0' && *scan != quote)
    scan += (*scan == '\\' && scan[1] != '\0') ? 2 : 1;

  if (*scan != quote)
    return FALSE;

  for (scan++; *scan == ' ' || *scan == '\t'; scan++)
    ;

  return *scan == delimiter;
}

static const char *
scan_quoted_key (Emitter    *emitter,
                 const char *at,
                 char        quote)
{
  const char *scan = at + 1;

  while (*scan != '\0' && *scan != quote)
    scan += (*scan == '\\' && scan[1] != '\0') ? 2 : 1;

  if (*scan == quote)
    scan++;

  append_token (emitter, XD_SYNTAX_TOKEN_TYPE, at, (gsize) (scan - at));
  return scan;
}

static const char *
scan_prefixed_raw_string (Emitter    *emitter,
                          const char *at)
{
  const char *scan = at + 2;
  char quote = at[1];

  while (*scan != '\0' && *scan != quote)
    scan++;

  if (*scan == quote)
    scan++;

  append_token (emitter, XD_SYNTAX_TOKEN_STRING, at, (gsize) (scan - at));
  return scan;
}

/*
 * A Make expansion is one token. Nested expansions using the same brackets
 * advance the depth; ordinary parentheses inside shell text do not.
 */
static const char *
scan_make_variable (Emitter    *emitter,
                    const char *at)
{
  const char *scan = at + 1;

  if (*scan == '(' || *scan == '{')
    {
      char open = *scan;
      char close = open == '(' ? ')' : '}';
      guint depth = 1;

      scan++;
      while (*scan != '\0' && depth > 0)
        {
          if (scan[0] == '$' && scan[1] == open)
            {
              depth++;
              scan += 2;
            }
          else if (*scan == close)
            {
              depth--;
              scan++;
            }
          else
            {
              scan++;
            }
        }
    }
  else if (*scan != '\0')
    {
      scan++;
    }

  append_token (emitter, XD_SYNTAX_TOKEN_PREPROC, at,
                (gsize) (scan - at));
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

static gboolean
followed_by_generic_call (const char *at)
{
  guint depth = 0;

  if (*at != '<')
    return FALSE;

  do
    {
      if (*at == '<')
        depth++;
      else if (*at == '>')
        depth--;
      at++;
    }
  while (*at != '\0' && depth > 0);

  while (*at == ' ' || *at == '\t')
    at++;

  return depth == 0 && *at == '(';
}

static gboolean
followed_by_square_generic_call (const char *at)
{
  guint depth = 0;

  if (*at != '[')
    return FALSE;

  do
    {
      if (*at == '[')
        depth++;
      else if (*at == ']')
        depth--;
      at++;
    }
  while (*at != '\0' && depth > 0);

  while (*at == ' ' || *at == '\t')
    at++;

  return depth == 0 && *at == '(';
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
  else if (*after == '(' ||
           (language->bang_functions && *after == '!') ||
           (language->generic_functions && followed_by_generic_call (after)) ||
           (language->square_generic_functions &&
            followed_by_square_generic_call (after)))
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

static gboolean
starts_a_bare_key (const char *line,
                   const char *at,
                   char        delimiter)
{
  const char *scan = line;

  while (*scan == ' ' || *scan == '\t')
    scan++;

  if (delimiter == ':' && *scan == '-' &&
      (scan[1] == ' ' || scan[1] == '\t'))
    {
      scan++;
      while (*scan == ' ' || *scan == '\t')
        scan++;
    }

  return scan == at;
}

static const char *
scan_bare_key (Emitter    *emitter,
               const char *line,
               const char *at,
               char        delimiter)
{
  const char *scan = at;
  const char *end;

  if (!starts_a_bare_key (line, at, delimiter))
    return NULL;

  while (*scan != '\0' && *scan != delimiter && *scan != '#')
    scan++;

  if (*scan != delimiter)
    return NULL;

  /* A colon within an unquoted scalar, such as https://, is not a YAML key. */
  if (delimiter == ':' && scan[1] != '\0' &&
      scan[1] != ' ' && scan[1] != '\t' &&
      scan[1] != '[' && scan[1] != '{')
    return NULL;

  end = scan;
  while (end > at && (end[-1] == ' ' || end[-1] == '\t'))
    end--;

  if (end == at)
    return NULL;

  append_token (emitter, XD_SYNTAX_TOKEN_TYPE, at, (gsize) (end - at));
  append_plain (emitter, end, (gsize) (scan - end));
  return scan;
}

static const char *
scan_table_header (Emitter    *emitter,
                   const char *at)
{
  const char *close = strchr (at + 1, ']');

  if (close == NULL)
    close = at + strlen (at);
  else
    {
      close++;
      if (*close == ']')
        close++;
    }

  append_token (emitter, XD_SYNTAX_TOKEN_PREPROC, at,
                (gsize) (close - at));
  return close;
}

static const char *
scan_yaml_reference (Emitter    *emitter,
                     const char *at)
{
  const char *scan = at + 1;

  if (*scan == '!')
    scan++;

  while (is_word_byte (*scan) || *scan == '-' || *scan == '.' ||
         *scan == '/' || *scan == ':')
    scan++;

  if (scan == at + 1)
    return NULL;

  append_token (emitter, XD_SYNTAX_TOKEN_PREPROC, at,
                (gsize) (scan - at));
  return scan;
}

static const char *
scan_at_attribute (Emitter    *emitter,
                   const char *at)
{
  const char *scan = at + 2;
  guint depth = 1;

  while (*scan != '\0' && depth > 0)
    {
      if (*scan == '\'' || *scan == '"')
        {
          char quote = *scan++;

          while (*scan != '\0' && *scan != quote)
            scan += (*scan == '\\' && scan[1] != '\0') ? 2 : 1;
          if (*scan == quote)
            scan++;
        }
      else if (*scan == '[')
        {
          depth++;
          scan++;
        }
      else if (*scan == ']')
        {
          depth--;
          scan++;
        }
      else
        {
          scan++;
        }
    }

  append_token (emitter, XD_SYNTAX_TOKEN_PREPROC, at,
                (gsize) (scan - at));
  return scan;
}

static const char *
scan_dollar_directive (Emitter    *emitter,
                       const char *at)
{
  const char *scan = at + 1;

  while (is_word_byte (*scan))
    scan++;

  if (scan == at + 1)
    return NULL;

  append_token (emitter, XD_SYNTAX_TOKEN_PREPROC, at,
                (gsize) (scan - at));
  return scan;
}

static gboolean
is_escaped (const char *line,
            const char *at)
{
  guint backslashes = 0;

  while (at > line && at[-1] == '\\')
    {
      backslashes++;
      at--;
    }

  return backslashes % 2 != 0;
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

static const char *
scan_nested_comment (Emitter    *emitter,
                     const char *at,
                     guint8     *depth,
                     gboolean    opening)
{
  const char *scan = at;

  if (opening)
    {
      *depth = 1;
      scan += 2;
    }

  while (*scan != '\0')
    {
      if (scan[0] == '/' && scan[1] == '*')
        {
          if (*depth < G_MAXUINT8)
            (*depth)++;
          scan += 2;
        }
      else if (scan[0] == '*' && scan[1] == '/')
        {
          (*depth)--;
          scan += 2;
          if (*depth == 0)
            break;
        }
      else
        {
          scan++;
        }
    }

  append_token (emitter, XD_SYNTAX_TOKEN_COMMENT, at,
                (gsize) (scan - at));
  return scan;
}

static gboolean
rust_raw_string_opening (const char  *at,
                         const char **contents,
                         guint8      *hashes)
{
  const char *scan = at;
  guint count = 0;

  if (scan[0] == 'r')
    scan++;
  else if ((scan[0] == 'b' || scan[0] == 'c') && scan[1] == 'r')
    scan += 2;
  else
    return FALSE;

  while (*scan == '#')
    {
      if (count == G_MAXUINT8)
        return FALSE;
      count++;
      scan++;
    }

  if (*scan != '"')
    return FALSE;

  *contents = scan + 1;
  *hashes = (guint8) count;
  return TRUE;
}

static const char *
rust_raw_string_close (const char *at,
                       guint8      hashes)
{
  const char *quote = at;

  while ((quote = strchr (quote, '"')) != NULL)
    {
      guint i;

      for (i = 0; i < hashes && quote[i + 1] == '#'; i++)
        ;

      if (i == hashes)
        return quote + 1 + hashes;

      quote++;
    }

  return NULL;
}

static const char *
scan_rust_raw_string (Emitter       *emitter,
                      const char    *at,
                      XdSyntaxState *state,
                      gboolean       opening)
{
  const char *contents = at;
  const char *close;
  guint8 hashes = state->rust_raw_hashes;

  if (opening && !rust_raw_string_opening (at, &contents, &hashes))
    return NULL;

  close = rust_raw_string_close (contents, hashes);
  if (close == NULL)
    {
      append_token (emitter, XD_SYNTAX_TOKEN_STRING, at, strlen (at));
      state->in_rust_raw_string = TRUE;
      state->rust_raw_hashes = hashes;
      return at + strlen (at);
    }

  append_token (emitter, XD_SYNTAX_TOKEN_STRING, at,
                (gsize) (close - at));
  state->in_rust_raw_string = FALSE;
  state->rust_raw_hashes = 0;
  return close;
}

static const char *
scan_rust_lifetime (Emitter    *emitter,
                    const char *at)
{
  const char *scan = at + 2;

  while (is_word_byte (*scan))
    scan++;

  /* A closing apostrophe makes this an ordinary character literal. */
  if (*scan == '\'')
    return NULL;

  append_token (emitter, XD_SYNTAX_TOKEN_PREPROC, at,
                (gsize) (scan - at));
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
      if (table->nested_block_comments)
        {
          at = scan_nested_comment (&emitter, at, &state->in_comment, FALSE);
          if (state->in_comment)
            return;
        }
      else
        {
          const char *close = strstr (at, "*/");

          if (close == NULL)
            {
              append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at,
                            strlen (at));
              return;
            }

          append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at,
                        (gsize) (close + 2 - at));
          at = close + 2;
          state->in_comment = FALSE;
        }
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
      char marker[4] = {
        (char) state->triple_quote,
        (char) state->triple_quote,
        (char) state->triple_quote,
        '\0',
      };
      const char *close = strstr (at, marker);

      if (close == NULL)
        {
          append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at, strlen (at));
          return;
        }

      append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at,
                    (gsize) (close + 3 - at));
      at = close + 3;
      state->in_triple_string = FALSE;
      state->triple_quote = 0;
    }
  else if (state->in_rust_raw_string)
    {
      at = scan_rust_raw_string (&emitter, at, state, FALSE);
      if (state->in_rust_raw_string)
        return;
    }

  while (*at != '\0')
    {
      if (table->block_comments && at[0] == '/' && at[1] == '*')
        {
          if (table->nested_block_comments)
            {
              at = scan_nested_comment (&emitter, at, &state->in_comment,
                                        TRUE);
              if (state->in_comment)
                return;
              continue;
            }

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
      else if (table->shebangs && at == line &&
               at[0] == '#' && at[1] == '!')
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
      else if (table->inline_hash_comments && *at == '#' &&
               !is_escaped (line, at))
        {
          append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at, strlen (at));
          return;
        }
      else if (table->spaced_hash_comments && *at == '#' &&
               (at == line || at[-1] == ' ' || at[-1] == '\t'))
        {
          append_token (&emitter, XD_SYNTAX_TOKEN_COMMENT, at, strlen (at));
          return;
        }
      else if (table->table_headers && *at == '[' &&
               starts_the_line (line, at))
        {
          at = scan_table_header (&emitter, at);
        }
      else if (table->at_attributes && at[0] == '@' && at[1] == '[')
        {
          at = scan_at_attribute (&emitter, at);
        }
      else if (table->triple_strings &&
               (strncmp (at, "\"\"\"", 3) == 0 ||
                (table->single_triple_strings &&
                 strncmp (at, "'''", 3) == 0)))
        {
          char marker[4] = { *at, *at, *at, '\0' };
          const char *close = strstr (at + 3, marker);

          if (close == NULL)
            {
              append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at, strlen (at));
              state->in_triple_string = TRUE;
              state->triple_quote = (guint8) *at;
              return;
            }

          append_token (&emitter, XD_SYNTAX_TOKEN_STRING, at,
                        (gsize) (close + 3 - at));
          at = close + 3;
        }
      else if (table->rust_strings &&
               (at[0] == 'r' ||
                ((at[0] == 'b' || at[0] == 'c') && at[1] == 'r')))
        {
          const char *after = scan_rust_raw_string (
            &emitter, at, state, TRUE);

          if (after == NULL)
            {
              at = scan_word (&emitter, table, at);
            }
          else
            {
              at = after;
              if (state->in_rust_raw_string)
                return;
            }
        }
      else if (table->prefixed_raw_strings && at[0] == 'r' &&
               (at[1] == '\'' || at[1] == '"'))
        {
          at = scan_prefixed_raw_string (&emitter, at);
        }
      else if (table->rust_lifetimes && *at == '\'' &&
               (g_ascii_isalpha (at[1]) || at[1] == '_'))
        {
          const char *after = scan_rust_lifetime (&emitter, at);

          if (after == NULL)
            at = scan_quoted (&emitter, at, *at);
          else
            at = after;
        }
      else if (table->quoted_keys && (*at == '"' || *at == '\'') &&
               quoted_key (at, *at, table->key_delimiter))
        {
          at = scan_quoted_key (&emitter, at, *at);
        }
      else if (*at == '"' || *at == '\'')
        {
          at = scan_quoted (&emitter, at, *at);
        }
      else if (table->backtick_literals && *at == '`')
        {
          at = scan_quoted (&emitter, at, *at);
        }
      else if (table->make_variables && *at == '$')
        {
          at = scan_make_variable (&emitter, at);
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
      else if (table->dollar_directives && *at == '$')
        {
          const char *after = scan_dollar_directive (&emitter, at);

          if (after == NULL)
            {
              append_plain (&emitter, at, 1);
              at++;
            }
          else
            {
              at = after;
            }
        }
      else if (table->yaml_references &&
               (*at == '&' || *at == '*' || *at == '!'))
        {
          const char *after = scan_yaml_reference (&emitter, at);

          if (after == NULL)
            {
              append_plain (&emitter, at, 1);
              at++;
            }
          else
            {
              at = after;
            }
        }
      else if (table->tilde_constant && *at == '~')
        {
          append_token (&emitter, XD_SYNTAX_TOKEN_NUMBER, at, 1);
          at++;
        }
      else if (table->bare_keys &&
               (g_ascii_isalnum (*at) || strchr ("_-.", *at) != NULL))
        {
          const char *after = scan_bare_key (
            &emitter, line, at, table->key_delimiter);

          if (after == NULL)
            {
              if (g_ascii_isdigit (*at) ||
                  (*at == '.' && g_ascii_isdigit (at[1])))
                at = scan_number (&emitter, at);
              else if (g_ascii_isalpha (*at) || *at == '_')
                at = scan_word (&emitter, table, at);
              else
                {
                  append_plain (&emitter, at, 1);
                  at++;
                }
            }
          else
            {
              at = after;
            }
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
