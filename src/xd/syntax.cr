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

      # Language scanners land in later parity slices. Returning every byte as
      # text keeps this foundation lossless until classification is connected.
      [SyntaxPiece.new(SyntaxToken::Text, line)]
    end
  end
end
