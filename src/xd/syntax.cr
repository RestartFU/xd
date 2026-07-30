require "set"

module Xd
  enum SyntaxLanguage
    None
    C
    Go
    Dockerfile
    Kotlin
    Makefile
    Rust
    JSON
    YAML
    TOML
    V
    Odin
    Ruby
    Crystal
    CSharp
  end

  enum SyntaxToken
    Text
    Keyword
    Type
    Function
    String
    Number
    Comment
    Preprocessor

    def colour : ::String?
      case self
      when Keyword      then "#dc8add"
      when Type         then "#78aeed"
      when Function     then "#99c1f1"
      when String       then "#f8e45c"
      when Number       then "#ffbe6f"
      when Comment      then "#8b8e8f"
      when Preprocessor then "#c061cc"
      else                   nil
      end
    end
  end

  record SyntaxPiece,
    token : SyntaxToken,
    text : String

  class SyntaxState
    property in_comment = 0
    property in_raw_string = false
    property in_triple_string = false
    property triple_quote = '\0'
    property in_rust_raw_string = false
    property rust_raw_hashes = 0
    property in_heredoc = false
    property heredoc_indent = false
    property heredoc_delimiter = ""
    property crystal_macro_close = '\0'
    property in_csharp_verbatim_string = false
    property csharp_raw_quotes = 0
  end

  module Syntax
    extend self

    C_KEYWORDS = Set{
      "auto", "break", "case", "char", "const", "continue", "default",
      "do", "double", "else", "enum", "extern", "float", "for", "goto",
      "if", "inline", "int", "long", "register", "restrict", "return",
      "short", "signed", "sizeof", "static", "struct", "switch",
      "typedef", "union", "unsigned", "void", "volatile", "while",
      "_Alignas", "_Alignof", "_Atomic", "_Bool", "_Generic",
      "_Noreturn", "_Static_assert", "_Thread_local",
    }
    C_TYPES = Set{
      "bool", "size_t", "ssize_t", "ptrdiff_t", "intptr_t", "uintptr_t",
      "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t",
      "uint32_t", "uint64_t", "wchar_t", "va_list", "FILE",
    }
    C_CONSTANTS = Set{"NULL", "true", "false"}

    CSHARP_KEYWORDS = Set{
      "abstract", "add", "alias", "and", "as", "ascending", "async",
      "await", "base", "break", "by", "case", "catch", "checked", "class",
      "const", "continue", "default", "delegate", "descending", "do",
      "else", "enum", "equals", "event", "explicit", "extern", "file",
      "finally", "fixed", "for", "foreach", "from", "get", "global",
      "goto", "group", "if", "implicit", "in", "init", "interface",
      "internal", "into", "is", "join", "let", "lock", "managed", "nameof",
      "namespace", "new", "not", "notnull", "on", "operator", "or",
      "orderby", "out", "override", "params", "partial", "private",
      "protected", "public", "readonly", "record", "ref", "remove",
      "required", "return", "scoped", "sealed", "select", "set", "sizeof",
      "stackalloc", "static", "struct", "switch", "this", "throw", "try",
      "typeof", "unchecked", "unmanaged", "unsafe", "using", "value", "var",
      "virtual", "volatile", "when", "where", "while", "with", "yield",
    }
    CSHARP_TYPES = Set{
      "bool", "byte", "char", "decimal", "double", "dynamic", "float",
      "int", "long", "nint", "nuint", "object", "sbyte", "short", "string",
      "uint", "ulong", "ushort", "void",
    }
    CSHARP_CONSTANTS     = Set{"false", "null", "true"}
    CSHARP_TYPE_CONTEXTS = Set{
      "class", "enum", "interface", "new", "record", "struct",
    }

    GO_KEYWORDS = Set{
      "break", "case", "chan", "const", "continue", "default", "defer",
      "else", "fallthrough", "for", "func", "go", "goto", "if", "import",
      "interface", "map", "package", "range", "return", "select", "struct",
      "switch", "type", "var",
    }
    GO_TYPES = Set{
      "any", "bool", "byte", "comparable", "complex64", "complex128",
      "error", "float32", "float64", "int", "int8", "int16", "int32",
      "int64", "rune", "string", "uint", "uint8", "uint16", "uint32",
      "uint64", "uintptr",
    }
    GO_CONSTANTS = Set{
      "append", "cap", "clear", "close", "complex", "copy", "delete",
      "imag", "iota", "len", "make", "max", "min", "new", "nil", "panic",
      "print", "println", "real", "recover", "true", "false",
    }

    KOTLIN_KEYWORDS = Set{
      "abstract", "actual", "annotation", "as", "break", "by", "catch",
      "class", "companion", "const", "constructor", "context", "continue",
      "crossinline", "data", "delegate", "do", "dynamic", "else", "enum",
      "expect", "external", "field", "file", "final", "finally", "for",
      "fun", "get", "if", "import", "in", "infix", "init", "inline",
      "inner", "interface", "internal", "is", "lateinit", "noinline",
      "object", "open", "operator", "out", "override", "package", "param",
      "private", "property", "protected", "public", "receiver", "reified",
      "return", "sealed", "set", "setparam", "super", "suspend", "tailrec",
      "this", "throw", "try", "typealias", "typeof", "val", "value", "var",
      "vararg", "when", "where", "while",
    }
    KOTLIN_TYPES = Set{
      "Any", "Array", "Boolean", "Byte", "Char", "Double", "Float", "Int",
      "Long", "Nothing", "Short", "String", "UByte", "UInt", "ULong",
      "UShort", "Unit",
    }
    KOTLIN_CONSTANTS = Set{"false", "null", "true"}

    DOCKERFILE_KEYWORDS = Set{
      "ADD", "ARG", "AS", "CMD", "COPY", "ENTRYPOINT", "ENV", "EXPOSE",
      "FROM", "HEALTHCHECK", "LABEL", "MAINTAINER", "ONBUILD", "RUN",
      "SHELL", "STOPSIGNAL", "USER", "VOLUME", "WORKDIR",
    }
    MAKEFILE_KEYWORDS = Set{
      "define", "else", "endef", "endif", "export", "ifdef", "ifeq",
      "ifndef", "ifneq", "include", "override", "private", "sinclude",
      "undefine", "unexport", "vpath",
    }

    RUST_KEYWORDS = Set{
      "abstract", "as", "async", "await", "become", "box", "break",
      "const", "continue", "crate", "do", "dyn", "else", "enum", "extern",
      "final", "fn", "for", "gen", "if", "impl", "in", "let", "loop",
      "macro", "macro_rules", "match", "mod", "move", "mut", "override",
      "priv", "pub", "ref", "return", "self", "static", "struct", "super",
      "trait", "try", "type", "typeof", "unsafe", "unsized", "use",
      "virtual", "where", "while", "yield",
    }
    RUST_TYPES = Set{
      "bool", "char", "str", "i8", "i16", "i32", "i64", "i128", "isize",
      "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "Self",
      "String", "Vec", "Option", "Result", "Box",
    }
    RUST_CONSTANTS = Set{"false", "None", "true"}

    JSON_CONSTANTS = Set{"false", "null", "true"}
    YAML_CONSTANTS = Set{"false", "no", "null", "off", "on", "true", "yes"}
    TOML_CONSTANTS = Set{"false", "true"}

    V_KEYWORDS = Set{
      "as", "asm", "assert", "atomic", "break", "const", "continue",
      "defer", "else", "enum", "fn", "for", "go", "goto", "if",
      "implements", "import", "in", "interface", "is", "isreftype", "lock",
      "match", "module", "mut", "or", "pub", "return", "rlock", "select",
      "shared", "sizeof", "spawn", "static", "struct", "type", "typeof",
      "union", "unsafe", "volatile", "__global", "__offsetof",
    }
    V_TYPES = Set{
      "any", "bool", "byte", "byteptr", "char", "charptr", "f32", "f64",
      "i8", "i16", "int", "i64", "i128", "isize", "map", "rune", "string",
      "u8", "u16", "u32", "u64", "u128", "usize", "voidptr",
    }
    V_CONSTANTS = Set{"false", "none", "true"}

    ODIN_KEYWORDS = Set{
      "asm", "auto_cast", "bit_field", "bit_set", "break", "case", "cast",
      "context", "continue", "defer", "distinct", "do", "dynamic", "else",
      "enum", "fallthrough", "for", "foreign", "if", "import", "in",
      "inline", "map", "matrix", "no_inline", "not_in", "or_break",
      "or_continue", "or_else", "or_return", "package", "proc", "return",
      "struct", "switch", "transmute", "typeid", "union", "using", "when",
      "where",
    }
    ODIN_TYPES = Set{
      "any", "bool", "byte", "b8", "b16", "b32", "b64", "int", "i8",
      "i16", "i32", "i64", "i128", "uint", "u8", "u16", "u32", "u64",
      "u128", "uintptr", "f16", "f32", "f64", "complex32", "complex64",
      "complex128", "quaternion64", "quaternion128", "quaternion256", "rune",
      "string", "cstring", "rawptr",
    }
    ODIN_CONSTANTS = Set{"false", "nil", "true"}

    RUBY_KEYWORDS = Set{
      "BEGIN", "END", "__ENCODING__", "__FILE__", "__LINE__", "alias",
      "and", "begin", "break", "case", "class", "def", "defined", "do",
      "else", "elsif", "end", "ensure", "for", "if", "in", "module",
      "next", "not", "or", "redo", "rescue", "retry", "return", "self",
      "super", "then", "undef", "unless", "until", "when", "while", "yield",
    }
    RUBY_CONSTANTS = Set{"false", "nil", "true"}
    RUBY_FUNCTIONS = Set{
      "abort", "at_exit", "autoload", "binding", "block_given", "caller",
      "catch", "eval", "exec", "exit", "fail", "fork", "format", "gets",
      "lambda", "load", "loop", "open", "p", "print", "printf", "proc",
      "putc", "puts", "raise", "readline", "require", "require_relative",
      "select", "sleep", "sprintf", "system", "throw", "trap", "warn",
    }
    RUBY_DEFINITION_KEYWORDS = Set{"def"}

    CRYSTAL_KEYWORDS = Set{
      "__DIR__", "__END_LINE__", "__FILE__", "__LINE__", "abstract",
      "alias", "alignof", "annotation", "as", "asm", "begin", "break",
      "case", "class", "def", "do", "else", "elsif", "end", "ensure",
      "enum", "extend", "for", "fun", "if", "in", "include",
      "instance_alignof", "instance_sizeof", "is_a", "lib", "macro",
      "module", "next", "of", "offsetof", "out", "pointerof",
      "previous_def", "private", "protected", "require", "rescue",
      "responds_to", "return", "select", "self", "sizeof", "struct",
      "super", "then", "type", "typeof", "union", "uninitialized", "unless",
      "until", "verbatim", "when", "while", "with", "yield",
    }
    CRYSTAL_TYPES = Set{
      "Array", "Bool", "Bytes", "Char", "Class", "Enum", "Exception",
      "Fiber", "Float32", "Float64", "Hash", "IO", "Int8", "Int16",
      "Int32", "Int64", "Int128", "Iterator", "NamedTuple", "Nil", "Number",
      "Object", "Pointer", "Proc", "Range", "Reference", "Regex", "Set",
      "Slice", "String", "Struct", "Symbol", "Tuple", "UInt8", "UInt16",
      "UInt32", "UInt64", "UInt128", "Value",
    }
    CRYSTAL_CONSTANTS = Set{"false", "nil", "true"}
    CRYSTAL_FUNCTIONS = Set{
      "abort", "at_exit", "delegate", "exit", "getter", "p", "pp", "print",
      "printf", "property", "puts", "raise", "record", "setter", "sleep",
      "spawn",
    }
    CRYSTAL_DEFINITION_KEYWORDS = Set{"def", "fun", "macro"}
    NO_WORDS                    = Set(String).new

    def language_for_path(path : String?) : SyntaxLanguage
      return SyntaxLanguage::None unless path

      name = path.split(/[\/\\]/).last
      downcase = name.downcase
      if downcase == "dockerfile" ||
         downcase.starts_with?("dockerfile.") ||
         downcase == "containerfile" ||
         downcase.starts_with?("containerfile.")
        return SyntaxLanguage::Dockerfile
      end
      if downcase == "makefile" ||
         downcase.starts_with?("makefile.") ||
         downcase == "gnumakefile" ||
         downcase == "bsdmakefile"
        return SyntaxLanguage::Makefile
      end
      if {"Gemfile", "Rakefile", "Vagrantfile"}.includes?(name)
        return SyntaxLanguage::Ruby
      end

      dot = name.rindex('.')
      return SyntaxLanguage::None unless dot

      extension = name.byte_slice(dot, name.bytesize - dot)
      case extension
      when ".go"         then SyntaxLanguage::Go
      when ".c", ".h"    then SyntaxLanguage::C
      when ".kt", ".kts" then SyntaxLanguage::Kotlin
      when ".mk", ".mak",
           ".make" then SyntaxLanguage::Makefile
      when ".rs"           then SyntaxLanguage::Rust
      when ".json"         then SyntaxLanguage::JSON
      when ".yaml", ".yml" then SyntaxLanguage::YAML
      when ".toml"         then SyntaxLanguage::TOML
      when ".v", ".vsh"    then SyntaxLanguage::V
      when ".odin"         then SyntaxLanguage::Odin
      when ".rb", ".rake",
           ".gemspec" then SyntaxLanguage::Ruby
      when ".cr"         then SyntaxLanguage::Crystal
      when ".cs", ".csx" then SyntaxLanguage::CSharp
      else
        extension.downcase == ".dockerfile" ? SyntaxLanguage::Dockerfile : SyntaxLanguage::None
      end
    end

    def scan_line(
      language : SyntaxLanguage,
      line : String,
      state : SyntaxState,
    ) : Array(SyntaxPiece)
      return [] of SyntaxPiece if line.empty?
      return [SyntaxPiece.new(SyntaxToken::Text, line)] if language.none?

      pieces = [] of SyntaxPiece
      at = 0

      if state.in_comment > 0
        at = scan_comment_continuation(pieces, language, line, state)
        return pieces if state.in_comment > 0
      elsif state.in_raw_string
        close = line.index('`')
        unless close
          emit(pieces, SyntaxToken::String, line, 0, line.bytesize)
          return pieces
        end

        at = close + 1
        emit(pieces, SyntaxToken::String, line, 0, at)
        state.in_raw_string = false
      elsif state.in_triple_string
        marker = state.triple_quote.to_s * 3
        close = line.index(marker)
        unless close
          emit(pieces, SyntaxToken::String, line, 0, line.bytesize)
          return pieces
        end

        at = close + 3
        emit(pieces, SyntaxToken::String, line, 0, at)
        state.in_triple_string = false
        state.triple_quote = '\0'
      elsif state.in_rust_raw_string
        at = scan_rust_raw_string(pieces, line, 0, state, false) || 0
        return pieces if state.in_rust_raw_string
      end

      while at < line.bytesize
        if block_comments?(language) && bytes_at?(line, at, "/*")
          at = scan_block_comment(pieces, language, line, at, state)
          return pieces if state.in_comment > 0
        elsif slash_comments?(language) && bytes_at?(line, at, "//")
          emit(pieces, SyntaxToken::Comment, line, at, line.bytesize)
          return pieces
        elsif shebangs?(language) && at == 0 && bytes_at?(line, at, "#!")
          emit(pieces, SyntaxToken::Comment, line, at, line.bytesize)
          return pieces
        elsif hash_comments?(language) &&
              byte(line, at) == byte_of('#') &&
              starts_line?(line, at)
          emit(pieces, SyntaxToken::Comment, line, at, line.bytesize)
          return pieces
        elsif inline_hash_comments?(language) &&
              byte(line, at) == byte_of('#') &&
              !escaped?(line, at)
          emit(pieces, SyntaxToken::Comment, line, at, line.bytesize)
          return pieces
        elsif spaced_hash_comments?(language) &&
              byte(line, at) == byte_of('#') &&
              (at == 0 || byte(line, at - 1) == byte_of(' ') ||
              byte(line, at - 1) == byte_of('\t'))
          emit(pieces, SyntaxToken::Comment, line, at, line.bytesize)
          return pieces
        elsif table_headers?(language) &&
              byte(line, at) == byte_of('[') &&
              starts_line?(line, at)
          at = scan_table_header(pieces, line, at)
        elsif byte(line, at) == byte_of('@') &&
              ((at_attributes?(language) &&
              byte(line, at + 1) == byte_of('[')) ||
              (paren_attributes?(language) &&
              byte(line, at + 1) == byte_of('(')))
          at = scan_at_attribute(pieces, line, at)
        elsif triple_strings?(language) &&
              (bytes_at?(line, at, "\"\"\"") ||
              (single_triple_strings?(language) &&
              bytes_at?(line, at, "'''")))
          quote = byte(line, at)
          marker = byte_slice(line, at, at + 3)
          close = line.index(marker, at + 3)
          unless close
            emit(pieces, SyntaxToken::String, line, at, line.bytesize)
            state.in_triple_string = true
            state.triple_quote = quote.chr
            return pieces
          end

          finish = close + 3
          emit(pieces, SyntaxToken::String, line, at, finish)
          at = finish
        elsif rust_strings?(language) &&
              (byte(line, at) == byte_of('r') ||
              ((byte(line, at) == byte_of('b') ||
              byte(line, at) == byte_of('c')) &&
              byte(line, at + 1) == byte_of('r')))
          after = scan_rust_raw_string(pieces, line, at, state, true)
          if after
            at = after
            return pieces if state.in_rust_raw_string
          else
            at = scan_word(pieces, language, line, at)
          end
        elsif rust_lifetimes?(language) &&
              byte(line, at) == byte_of('\'') &&
              (ascii_alpha?(byte(line, at + 1)) ||
              byte(line, at + 1) == byte_of('_'))
          after = scan_rust_lifetime(pieces, line, at)
          if after
            at = after
          else
            at = scan_quoted(pieces, line, at, byte_of('\''))
          end
        elsif prefixed_raw_strings?(language) &&
              byte(line, at) == byte_of('r') &&
              (byte(line, at + 1) == byte_of('\'') ||
              byte(line, at + 1) == byte_of('"'))
          at = scan_prefixed_raw_string(pieces, line, at)
        elsif quoted_keys?(language) &&
              (byte(line, at) == byte_of('"') ||
              byte(line, at) == byte_of('\'')) &&
              quoted_key?(line, at, byte(line, at), key_delimiter(language))
          at = scan_quoted_key(pieces, line, at, byte(line, at))
        elsif byte(line, at) == byte_of('"') ||
              byte(line, at) == byte_of('\'')
          at = scan_quoted(pieces, line, at, byte(line, at))
        elsif backtick_literals?(language) &&
              byte(line, at) == byte_of('`')
          at = scan_quoted(pieces, line, at, byte_of('`'))
        elsif make_variables?(language) && byte(line, at) == byte_of('$')
          at = scan_make_variable(pieces, line, at)
        elsif raw_strings?(language) && byte(line, at) == byte_of('`')
          close = line.index('`', at + 1)
          unless close
            emit(pieces, SyntaxToken::String, line, at, line.bytesize)
            state.in_raw_string = true
            return pieces
          end

          finish = close + 1
          emit(pieces, SyntaxToken::String, line, at, finish)
          at = finish
        elsif directives?(language) &&
              byte(line, at) == byte_of('#') &&
              starts_line?(line, at)
          at = scan_directive(pieces, line, at)
        elsif hash_word_directives?(language) &&
              byte(line, at) == byte_of('#')
          after = scan_hash_word_directive(pieces, line, at)
          if after
            at = after
          else
            emit(pieces, SyntaxToken::Text, line, at, at + 1)
            at += 1
          end
        elsif dollar_directives?(language) &&
              byte(line, at) == byte_of('$')
          after = scan_dollar_directive(pieces, line, at)
          if after
            at = after
          else
            emit(pieces, SyntaxToken::Text, line, at, at + 1)
            at += 1
          end
        elsif yaml_references?(language) &&
              (byte(line, at) == byte_of('&') ||
              byte(line, at) == byte_of('*') ||
              byte(line, at) == byte_of('!'))
          after = scan_yaml_reference(pieces, line, at)
          if after
            at = after
          else
            emit(pieces, SyntaxToken::Text, line, at, at + 1)
            at += 1
          end
        elsif tilde_constant?(language) && byte(line, at) == byte_of('~')
          emit(pieces, SyntaxToken::Number, line, at, at + 1)
          at += 1
        elsif undefined_constant?(language) &&
              bytes_at?(line, at, "---")
          emit(pieces, SyntaxToken::Number, line, at, at + 3)
          at += 3
        elsif bare_keys?(language) &&
              (word_byte?(byte(line, at)) ||
              byte(line, at) == byte_of('-') ||
              byte(line, at) == byte_of('.'))
          after = scan_bare_key(
            pieces,
            line,
            at,
            key_delimiter(language)
          )
          if after
            at = after
          elsif ascii_digit?(byte(line, at)) ||
                (byte(line, at) == byte_of('.') &&
                ascii_digit?(byte(line, at + 1)))
            at = scan_number(pieces, line, at)
          elsif ascii_alpha?(byte(line, at)) ||
                byte(line, at) == byte_of('_')
            at = scan_word(pieces, language, line, at)
          else
            emit(pieces, SyntaxToken::Text, line, at, at + 1)
            at += 1
          end
        elsif ascii_digit?(byte(line, at)) ||
              (byte(line, at) == byte_of('.') &&
              ascii_digit?(byte(line, at + 1)))
          at = scan_number(pieces, line, at)
        elsif ascii_alpha?(byte(line, at)) ||
              byte(line, at) == byte_of('_')
          at = scan_word(pieces, language, line, at)
        else
          emit(pieces, SyntaxToken::Text, line, at, at + 1)
          at += 1
        end
      end

      pieces
    end

    private def scan_comment_continuation(
      pieces : Array(SyntaxPiece),
      language : SyntaxLanguage,
      line : String,
      state : SyntaxState,
    ) : Int32
      if nested_block_comments?(language)
        return scan_nested_comment(pieces, line, 0, state, false)
      end

      close = line.index("*/")
      unless close
        emit(pieces, SyntaxToken::Comment, line, 0, line.bytesize)
        return line.bytesize
      end

      finish = close + 2
      emit(pieces, SyntaxToken::Comment, line, 0, finish)
      state.in_comment = 0
      finish
    end

    private def scan_block_comment(
      pieces : Array(SyntaxPiece),
      language : SyntaxLanguage,
      line : String,
      at : Int32,
      state : SyntaxState,
    ) : Int32
      if nested_block_comments?(language)
        return scan_nested_comment(pieces, line, at, state, true)
      end

      close = line.index("*/", at + 2)
      unless close
        emit(pieces, SyntaxToken::Comment, line, at, line.bytesize)
        state.in_comment = 1
        return line.bytesize
      end

      finish = close + 2
      emit(pieces, SyntaxToken::Comment, line, at, finish)
      finish
    end

    private def scan_nested_comment(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
      state : SyntaxState,
      opening : Bool,
    ) : Int32
      scan = at
      if opening
        state.in_comment = 1
        scan += 2
      end

      while scan < line.bytesize
        if bytes_at?(line, scan, "/*")
          state.in_comment += 1 if state.in_comment < UInt8::MAX
          scan += 2
        elsif bytes_at?(line, scan, "*/")
          state.in_comment -= 1
          scan += 2
          break if state.in_comment == 0
        else
          scan += 1
        end
      end

      emit(pieces, SyntaxToken::Comment, line, at, scan)
      scan
    end

    private def scan_at_attribute(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32
      scan = at + 2
      open = byte(line, at + 1)
      close = open == byte_of('[') ? byte_of(']') : byte_of(')')
      depth = 1

      while scan < line.bytesize && depth > 0
        current = byte(line, scan)
        if current == byte_of('\'') || current == byte_of('"')
          quote = current
          scan += 1
          while scan < line.bytesize && byte(line, scan) != quote
            scan += byte(line, scan) == byte_of('\\') &&
                    scan + 1 < line.bytesize ? 2 : 1
          end
          scan += 1 if scan < line.bytesize
        elsif current == open
          depth += 1
          scan += 1
        elsif current == close
          depth -= 1
          scan += 1
        else
          scan += 1
        end
      end

      emit(pieces, SyntaxToken::Preprocessor, line, at, scan)
      scan
    end

    private def scan_prefixed_raw_string(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32
      quote = byte(line, at + 1)
      scan = at + 2
      while scan < line.bytesize && byte(line, scan) != quote
        scan += 1
      end
      scan += 1 if scan < line.bytesize
      emit(pieces, SyntaxToken::String, line, at, scan)
      scan
    end

    private def scan_hash_word_directive(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32?
      scan = at + 1
      scan += 1 if byte(line, scan) == byte_of('+')
      while word_byte?(byte(line, scan)) ||
            byte(line, scan) == byte_of('-')
        scan += 1
      end
      return nil if scan == at + 1

      emit(pieces, SyntaxToken::Preprocessor, line, at, scan)
      scan
    end

    private def scan_dollar_directive(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32?
      scan = at + 1
      while word_byte?(byte(line, scan))
        scan += 1
      end
      return nil if scan == at + 1

      emit(pieces, SyntaxToken::Preprocessor, line, at, scan)
      scan
    end

    private def quoted_key?(
      line : String,
      at : Int32,
      quote : UInt8,
      delimiter : UInt8,
    ) : Bool
      scan = at + 1
      while scan < line.bytesize && byte(line, scan) != quote
        scan += byte(line, scan) == byte_of('\\') &&
                scan + 1 < line.bytesize ? 2 : 1
      end
      return false if scan >= line.bytesize

      scan = skip_space(line, scan + 1)
      byte(line, scan) == delimiter
    end

    private def scan_quoted_key(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
      quote : UInt8,
    ) : Int32
      scan = at + 1
      while scan < line.bytesize && byte(line, scan) != quote
        scan += byte(line, scan) == byte_of('\\') &&
                scan + 1 < line.bytesize ? 2 : 1
      end
      scan += 1 if scan < line.bytesize
      emit(pieces, SyntaxToken::Type, line, at, scan)
      scan
    end

    private def scan_make_variable(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32
      scan = at + 1
      current = byte(line, scan)
      if current == byte_of('(') || current == byte_of('{')
        open = current
        close = open == byte_of('(') ? byte_of(')') : byte_of('}')
        depth = 1
        scan += 1
        while scan < line.bytesize && depth > 0
          if byte(line, scan) == byte_of('$') &&
             byte(line, scan + 1) == open
            depth += 1
            scan += 2
          elsif byte(line, scan) == close
            depth -= 1
            scan += 1
          else
            scan += 1
          end
        end
      elsif scan < line.bytesize
        scan += 1
      end

      emit(pieces, SyntaxToken::Preprocessor, line, at, scan)
      scan
    end

    private def scan_table_header(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32
      close = line.index(']', at + 1)
      finish =
        if close
          close += 1
          close += 1 if byte(line, close) == byte_of(']')
          close
        else
          line.bytesize
        end
      emit(pieces, SyntaxToken::Preprocessor, line, at, finish)
      finish
    end

    private def scan_yaml_reference(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32?
      scan = at + 1
      scan += 1 if byte(line, scan) == byte_of('!')
      while word_byte?(byte(line, scan)) ||
            byte(line, scan) == byte_of('-') ||
            byte(line, scan) == byte_of('.') ||
            byte(line, scan) == byte_of('/') ||
            byte(line, scan) == byte_of(':')
        scan += 1
      end
      return nil if scan == at + 1

      emit(pieces, SyntaxToken::Preprocessor, line, at, scan)
      scan
    end

    private def scan_bare_key(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
      delimiter : UInt8,
    ) : Int32?
      return nil unless starts_bare_key?(line, at, delimiter)

      scan = at
      while scan < line.bytesize &&
            byte(line, scan) != delimiter &&
            byte(line, scan) != byte_of('#')
        scan += 1
      end
      return nil unless byte(line, scan) == delimiter
      if delimiter == byte_of(':') &&
         byte(line, scan + 1) != 0_u8 &&
         byte(line, scan + 1) != byte_of(' ') &&
         byte(line, scan + 1) != byte_of('\t') &&
         byte(line, scan + 1) != byte_of('[') &&
         byte(line, scan + 1) != byte_of('{')
        return nil
      end

      finish = scan
      while finish > at &&
            (byte(line, finish - 1) == byte_of(' ') ||
            byte(line, finish - 1) == byte_of('\t'))
        finish -= 1
      end
      return nil if finish == at

      emit(pieces, SyntaxToken::Type, line, at, finish)
      emit(pieces, SyntaxToken::Text, line, finish, scan)
      scan
    end

    private def starts_bare_key?(
      line : String,
      at : Int32,
      delimiter : UInt8,
    ) : Bool
      scan = skip_space(line, 0)
      if delimiter == byte_of(':') &&
         byte(line, scan) == byte_of('-') &&
         (byte(line, scan + 1) == byte_of(' ') ||
         byte(line, scan + 1) == byte_of('\t'))
        scan = skip_space(line, scan + 1)
      end
      scan == at
    end

    private def escaped?(line : String, at : Int32) : Bool
      scan = at
      backslashes = 0
      while scan > 0 && byte(line, scan - 1) == byte_of('\\')
        backslashes += 1
        scan -= 1
      end
      backslashes.odd?
    end

    private def scan_quoted(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
      quote : UInt8,
    ) : Int32
      scan = at + 1
      while scan < line.bytesize && byte(line, scan) != quote
        scan += byte(line, scan) == byte_of('\\') &&
                scan + 1 < line.bytesize ? 2 : 1
      end
      scan += 1 if scan < line.bytesize
      emit(pieces, SyntaxToken::String, line, at, scan)
      scan
    end

    private def scan_number(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32
      scan = at
      while scan < line.bytesize
        current = byte(line, scan)
        if word_byte?(current) || current == byte_of('.')
          scan += 1
        elsif (current == byte_of('+') || current == byte_of('-')) &&
              scan > at && exponent_byte?(byte(line, scan - 1))
          scan += 1
        else
          break
        end
      end

      emit(pieces, SyntaxToken::Number, line, at, scan)
      scan
    end

    private def scan_word(
      pieces : Array(SyntaxPiece),
      language : SyntaxLanguage,
      line : String,
      at : Int32,
    ) : Int32
      scan = at
      while scan < line.bytesize && word_byte?(byte(line, scan))
        scan += 1
      end

      word = byte_slice(line, at, scan)
      after = skip_space(line, scan)
      called = byte(line, after) == byte_of('(') ||
               (bang_functions?(language) &&
                byte(line, after) == byte_of('!')) ||
               (generic_functions?(language) &&
                followed_by_generic_call?(line, after)) ||
               (square_generic_functions?(language) &&
                followed_by_square_generic_call?(line, after)) ||
               (odin_procedures?(language) &&
                followed_by_odin_procedure?(line, after))

      token =
        if listed?(keywords(language), word, case_insensitive?(language))
          SyntaxToken::Keyword
        elsif listed?(types(language), word, case_insensitive?(language))
          SyntaxToken::Type
        elsif listed?(constants(language), word, case_insensitive?(language))
          SyntaxToken::Number
        elsif capitalized_types?(language) &&
              ascii_upper?(byte(line, at))
          SyntaxToken::Type
        elsif composite_literals?(language) &&
              byte(line, scan) == byte_of('{')
          SyntaxToken::Type
        elsif called
          SyntaxToken::Function
        else
          SyntaxToken::Text
        end

      emit(pieces, token, line, at, scan)
      scan
    end

    private def scan_directive(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32
      scan = at + 1
      while scan < line.bytesize && ascii_alpha?(byte(line, scan))
        scan += 1
      end
      name = byte_slice(line, at + 1, scan)
      emit(pieces, SyntaxToken::Preprocessor, line, at, scan)

      if name == "include"
        open = skip_space(line, scan)
        if byte(line, open) == byte_of('<')
          close = line.index('>', open)
          if close
            emit(pieces, SyntaxToken::Text, line, scan, open)
            emit(pieces, SyntaxToken::String, line, open, close + 1)
            return close + 1
          end
        end
      end

      scan
    end

    private def scan_rust_lifetime(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
    ) : Int32?
      scan = at + 2
      while scan < line.bytesize && word_byte?(byte(line, scan))
        scan += 1
      end
      return nil if byte(line, scan) == byte_of('\'')

      emit(pieces, SyntaxToken::Preprocessor, line, at, scan)
      scan
    end

    private def scan_rust_raw_string(
      pieces : Array(SyntaxPiece),
      line : String,
      at : Int32,
      state : SyntaxState,
      opening : Bool,
    ) : Int32?
      contents = at
      hashes = state.rust_raw_hashes

      if opening
        parsed = rust_raw_opening(line, at)
        return nil unless parsed
        contents, hashes = parsed
      end

      close = rust_raw_close(line, contents, hashes)
      unless close
        emit(pieces, SyntaxToken::String, line, at, line.bytesize)
        state.in_rust_raw_string = true
        state.rust_raw_hashes = hashes
        return line.bytesize
      end

      emit(pieces, SyntaxToken::String, line, at, close)
      state.in_rust_raw_string = false
      state.rust_raw_hashes = 0
      close
    end

    private def rust_raw_opening(
      line : String,
      at : Int32,
    ) : Tuple(Int32, Int32)?
      scan = at
      if byte(line, scan) == byte_of('r')
        scan += 1
      elsif (byte(line, scan) == byte_of('b') ||
            byte(line, scan) == byte_of('c')) &&
            byte(line, scan + 1) == byte_of('r')
        scan += 2
      else
        return nil
      end

      hashes = 0
      while byte(line, scan) == byte_of('#')
        return nil if hashes == UInt8::MAX
        hashes += 1
        scan += 1
      end
      return nil unless byte(line, scan) == byte_of('"')

      {scan + 1, hashes}
    end

    private def rust_raw_close(
      line : String,
      at : Int32,
      hashes : Int32,
    ) : Int32?
      scan = at
      while quote = line.index('"', scan)
        valid = true
        hashes.times do |offset|
          unless byte(line, quote + 1 + offset) == byte_of('#')
            valid = false
            break
          end
        end
        return quote + 1 + hashes if valid
        scan = quote + 1
      end
      nil
    end

    private def followed_by_generic_call?(line : String, at : Int32) : Bool
      return false unless byte(line, at) == byte_of('<')

      scan = at
      depth = 0
      loop do
        current = byte(line, scan)
        if current == byte_of('<')
          depth += 1
        elsif current == byte_of('>')
          depth -= 1
        end
        scan += 1
        break if scan >= line.bytesize || depth == 0
      end

      scan = skip_space(line, scan)
      depth == 0 && byte(line, scan) == byte_of('(')
    end

    private def followed_by_square_generic_call?(
      line : String,
      at : Int32,
    ) : Bool
      return false unless byte(line, at) == byte_of('[')

      scan = at
      depth = 0
      loop do
        current = byte(line, scan)
        if current == byte_of('[')
          depth += 1
        elsif current == byte_of(']')
          depth -= 1
        end
        scan += 1
        break if scan >= line.bytesize || depth == 0
      end

      scan = skip_space(line, scan)
      depth == 0 && byte(line, scan) == byte_of('(')
    end

    private def followed_by_odin_procedure?(
      line : String,
      at : Int32,
    ) : Bool
      return false unless bytes_at?(line, at, "::")

      scan = skip_space(line, at + 2)
      while byte(line, scan) == byte_of('#')
        scan += 1
        while word_byte?(byte(line, scan))
          scan += 1
        end
        scan = skip_space(line, scan)
      end
      bytes_at?(line, scan, "proc") &&
        !word_byte?(byte(line, scan + 4))
    end

    private def keywords(language : SyntaxLanguage) : Set(String)
      case language
      when .c?          then C_KEYWORDS
      when .c_sharp?    then CSHARP_KEYWORDS
      when .go?         then GO_KEYWORDS
      when .kotlin?     then KOTLIN_KEYWORDS
      when .dockerfile? then DOCKERFILE_KEYWORDS
      when .makefile?   then MAKEFILE_KEYWORDS
      when .rust?       then RUST_KEYWORDS
      when .v?          then V_KEYWORDS
      when .odin?       then ODIN_KEYWORDS
      when .ruby?       then RUBY_KEYWORDS
      when .crystal?    then CRYSTAL_KEYWORDS
      else                   NO_WORDS
      end
    end

    private def types(language : SyntaxLanguage) : Set(String)
      case language
      when .c?       then C_TYPES
      when .c_sharp? then CSHARP_TYPES
      when .go?      then GO_TYPES
      when .kotlin?  then KOTLIN_TYPES
      when .rust?    then RUST_TYPES
      when .v?       then V_TYPES
      when .odin?    then ODIN_TYPES
      when .crystal? then CRYSTAL_TYPES
      else                NO_WORDS
      end
    end

    private def constants(language : SyntaxLanguage) : Set(String)
      case language
      when .c?       then C_CONSTANTS
      when .c_sharp? then CSHARP_CONSTANTS
      when .go?      then GO_CONSTANTS
      when .kotlin?  then KOTLIN_CONSTANTS
      when .rust?    then RUST_CONSTANTS
      when .json?    then JSON_CONSTANTS
      when .yaml?    then YAML_CONSTANTS
      when .toml?    then TOML_CONSTANTS
      when .v?       then V_CONSTANTS
      when .odin?    then ODIN_CONSTANTS
      when .ruby?    then RUBY_CONSTANTS
      when .crystal? then CRYSTAL_CONSTANTS
      else                NO_WORDS
      end
    end

    private def listed?(
      words : Set(String),
      word : String,
      case_insensitive : Bool,
    ) : Bool
      case_insensitive ? words.any? { |entry| entry.compare(word, case_insensitive: true) == 0 } : words.includes?(word)
    end

    private def emit(
      pieces : Array(SyntaxPiece),
      token : SyntaxToken,
      line : String,
      from : Int32,
      to : Int32,
    ) : Nil
      return if to <= from

      text = byte_slice(line, from, to)
      if token == SyntaxToken::Text &&
         (last = pieces.last?) &&
         last.token == SyntaxToken::Text
        pieces[-1] = SyntaxPiece.new(SyntaxToken::Text, last.text + text)
      else
        pieces << SyntaxPiece.new(token, text)
      end
    end

    private def byte_slice(line : String, from : Int32, to : Int32) : String
      line.byte_slice(from, to - from)
    end

    private def byte(line : String, at : Int32) : UInt8
      return 0_u8 if at < 0 || at >= line.bytesize
      line.byte_at(at)
    end

    private def byte_of(char : Char) : UInt8
      char.ord.to_u8
    end

    private def bytes_at?(line : String, at : Int32, bytes : String) : Bool
      return false if at < 0 || at + bytes.bytesize > line.bytesize
      line.byte_slice(at, bytes.bytesize) == bytes
    end

    private def ascii_alpha?(byte : UInt8) : Bool
      ascii_lower?(byte) || ascii_upper?(byte)
    end

    private def ascii_lower?(byte : UInt8) : Bool
      byte >= byte_of('a') && byte <= byte_of('z')
    end

    private def ascii_upper?(byte : UInt8) : Bool
      byte >= byte_of('A') && byte <= byte_of('Z')
    end

    private def ascii_digit?(byte : UInt8) : Bool
      byte >= byte_of('0') && byte <= byte_of('9')
    end

    private def word_byte?(byte : UInt8) : Bool
      ascii_alpha?(byte) || ascii_digit?(byte) || byte == byte_of('_')
    end

    private def exponent_byte?(byte : UInt8) : Bool
      byte == byte_of('e') || byte == byte_of('E') ||
        byte == byte_of('p') || byte == byte_of('P')
    end

    private def skip_space(line : String, at : Int32) : Int32
      scan = at
      while byte(line, scan) == byte_of(' ') ||
            byte(line, scan) == byte_of('\t')
        scan += 1
      end
      scan
    end

    private def starts_line?(line : String, at : Int32) : Bool
      (0...at).all? do |scan|
        byte(line, scan) == byte_of(' ') ||
          byte(line, scan) == byte_of('\t')
      end
    end

    private def directives?(language : SyntaxLanguage) : Bool
      language.c? || language.c_sharp? || language.v?
    end

    private def shebangs?(language : SyntaxLanguage) : Bool
      language.v? || language.ruby? || language.crystal?
    end

    private def hash_comments?(language : SyntaxLanguage) : Bool
      language.dockerfile?
    end

    private def inline_hash_comments?(language : SyntaxLanguage) : Bool
      language.makefile? || language.toml? ||
        language.ruby? || language.crystal?
    end

    private def spaced_hash_comments?(language : SyntaxLanguage) : Bool
      language.yaml?
    end

    private def slash_comments?(language : SyntaxLanguage) : Bool
      language.c? || language.c_sharp? || language.go? ||
        language.kotlin? || language.rust? || language.v? ||
        language.odin?
    end

    private def block_comments?(language : SyntaxLanguage) : Bool
      slash_comments?(language)
    end

    private def nested_block_comments?(language : SyntaxLanguage) : Bool
      language.rust? || language.v? || language.odin?
    end

    private def raw_strings?(language : SyntaxLanguage) : Bool
      language.go? || language.odin?
    end

    private def make_variables?(language : SyntaxLanguage) : Bool
      language.makefile?
    end

    private def prefixed_raw_strings?(language : SyntaxLanguage) : Bool
      language.v?
    end

    private def backtick_literals?(language : SyntaxLanguage) : Bool
      language.v? || language.crystal?
    end

    private def at_attributes?(language : SyntaxLanguage) : Bool
      language.v? || language.crystal?
    end

    private def paren_attributes?(language : SyntaxLanguage) : Bool
      language.odin?
    end

    private def hash_word_directives?(language : SyntaxLanguage) : Bool
      language.odin?
    end

    private def dollar_directives?(language : SyntaxLanguage) : Bool
      language.v? || language.odin?
    end

    private def undefined_constant?(language : SyntaxLanguage) : Bool
      language.odin?
    end

    private def triple_strings?(language : SyntaxLanguage) : Bool
      language.kotlin? || language.toml?
    end

    private def single_triple_strings?(language : SyntaxLanguage) : Bool
      language.toml?
    end

    private def capitalized_types?(language : SyntaxLanguage) : Bool
      language.c_sharp? || language.kotlin? || language.rust? ||
        language.v? || language.odin? || language.ruby? ||
        language.crystal?
    end

    private def composite_literals?(language : SyntaxLanguage) : Bool
      language.go? || language.v? || language.odin?
    end

    private def bang_functions?(language : SyntaxLanguage) : Bool
      language.rust?
    end

    private def generic_functions?(language : SyntaxLanguage) : Bool
      language.rust? || language.c_sharp?
    end

    private def square_generic_functions?(language : SyntaxLanguage) : Bool
      language.v?
    end

    private def odin_procedures?(language : SyntaxLanguage) : Bool
      language.odin?
    end

    private def rust_lifetimes?(language : SyntaxLanguage) : Bool
      language.rust?
    end

    private def rust_strings?(language : SyntaxLanguage) : Bool
      language.rust?
    end

    private def case_insensitive?(language : SyntaxLanguage) : Bool
      language.dockerfile? || language.yaml?
    end

    private def quoted_keys?(language : SyntaxLanguage) : Bool
      language.json? || language.yaml? || language.toml?
    end

    private def bare_keys?(language : SyntaxLanguage) : Bool
      language.yaml? || language.toml?
    end

    private def table_headers?(language : SyntaxLanguage) : Bool
      language.toml?
    end

    private def yaml_references?(language : SyntaxLanguage) : Bool
      language.yaml?
    end

    private def tilde_constant?(language : SyntaxLanguage) : Bool
      language.yaml?
    end

    private def key_delimiter(language : SyntaxLanguage) : UInt8
      language.toml? ? byte_of('=') : byte_of(':')
    end
  end
end
