package com.restartfu.xd.syntax

// Generated from src/xd/syntax.cr. The desktop highlighter is the source of
// truth for these; transcribing them by hand would only invite drift.

internal object Words {
    val cKeywords: Set<String> = setOf(
        "auto", "break", "case", "char", "const", "continue", "default", "do", "double",
        "else", "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long",
        "register", "restrict", "return", "short", "signed", "sizeof", "static", "struct",
        "switch", "typedef", "union", "unsigned", "void", "volatile", "while", "_Alignas",
        "_Alignof", "_Atomic", "_Bool", "_Generic", "_Noreturn", "_Static_assert",
        "_Thread_local",
    )

    val cTypes: Set<String> = setOf(
        "bool", "size_t", "ssize_t", "ptrdiff_t", "intptr_t", "uintptr_t", "int8_t", "int16_t",
        "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "wchar_t",
        "va_list", "FILE",
    )

    val cConstants: Set<String> = setOf(
        "NULL", "true", "false",
    )

    val csharpKeywords: Set<String> = setOf(
        "abstract", "add", "alias", "and", "as", "ascending", "async", "await", "base",
        "break", "by", "case", "catch", "checked", "class", "const", "continue", "default",
        "delegate", "descending", "do", "else", "enum", "equals", "event", "explicit",
        "extern", "file", "finally", "fixed", "for", "foreach", "from", "get", "global",
        "goto", "group", "if", "implicit", "in", "init", "interface", "internal", "into", "is",
        "join", "let", "lock", "managed", "nameof", "namespace", "new", "not", "notnull", "on",
        "operator", "or", "orderby", "out", "override", "params", "partial", "private",
        "protected", "public", "readonly", "record", "ref", "remove", "required", "return",
        "scoped", "sealed", "select", "set", "sizeof", "stackalloc", "static", "struct",
        "switch", "this", "throw", "try", "typeof", "unchecked", "unmanaged", "unsafe",
        "using", "value", "var", "virtual", "volatile", "when", "where", "while", "with",
        "yield",
    )

    val csharpTypes: Set<String> = setOf(
        "bool", "byte", "char", "decimal", "double", "dynamic", "float", "int", "long", "nint",
        "nuint", "object", "sbyte", "short", "string", "uint", "ulong", "ushort", "void",
    )

    val csharpConstants: Set<String> = setOf(
        "false", "null", "true",
    )

    val csharpTypeContexts: Set<String> = setOf(
        "class", "enum", "interface", "new", "record", "struct",
    )

    val goKeywords: Set<String> = setOf(
        "break", "case", "chan", "const", "continue", "default", "defer", "else",
        "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map",
        "package", "range", "return", "select", "struct", "switch", "type", "var",
    )

    val goTypes: Set<String> = setOf(
        "any", "bool", "byte", "comparable", "complex64", "complex128", "error", "float32",
        "float64", "int", "int8", "int16", "int32", "int64", "rune", "string", "uint", "uint8",
        "uint16", "uint32", "uint64", "uintptr",
    )

    val goConstants: Set<String> = setOf(
        "append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "iota", "len",
        "make", "max", "min", "new", "nil", "panic", "print", "println", "real", "recover",
        "true", "false",
    )

    val kotlinKeywords: Set<String> = setOf(
        "abstract", "actual", "annotation", "as", "break", "by", "catch", "class", "companion",
        "const", "constructor", "context", "continue", "crossinline", "data", "delegate", "do",
        "dynamic", "else", "enum", "expect", "external", "field", "file", "final", "finally",
        "for", "fun", "get", "if", "import", "in", "infix", "init", "inline", "inner",
        "interface", "internal", "is", "lateinit", "noinline", "object", "open", "operator",
        "out", "override", "package", "param", "private", "property", "protected", "public",
        "receiver", "reified", "return", "sealed", "set", "setparam", "super", "suspend",
        "tailrec", "this", "throw", "try", "typealias", "typeof", "val", "value", "var",
        "vararg", "when", "where", "while",
    )

    val kotlinTypes: Set<String> = setOf(
        "Any", "Array", "Boolean", "Byte", "Char", "Double", "Float", "Int", "Long", "Nothing",
        "Short", "String", "UByte", "UInt", "ULong", "UShort", "Unit",
    )

    val kotlinConstants: Set<String> = setOf(
        "false", "null", "true",
    )

    val dockerfileKeywords: Set<String> = setOf(
        "ADD", "ARG", "AS", "CMD", "COPY", "ENTRYPOINT", "ENV", "EXPOSE", "FROM",
        "HEALTHCHECK", "LABEL", "MAINTAINER", "ONBUILD", "RUN", "SHELL", "STOPSIGNAL", "USER",
        "VOLUME", "WORKDIR",
    )

    val makefileKeywords: Set<String> = setOf(
        "define", "else", "endef", "endif", "export", "ifdef", "ifeq", "ifndef", "ifneq",
        "include", "override", "private", "sinclude", "undefine", "unexport", "vpath",
    )

    val rustKeywords: Set<String> = setOf(
        "abstract", "as", "async", "await", "become", "box", "break", "const", "continue",
        "crate", "do", "dyn", "else", "enum", "extern", "final", "fn", "for", "gen", "if",
        "impl", "in", "let", "loop", "macro", "macro_rules", "match", "mod", "move", "mut",
        "override", "priv", "pub", "ref", "return", "self", "static", "struct", "super",
        "trait", "try", "type", "typeof", "unsafe", "unsized", "use", "virtual", "where",
        "while", "yield",
    )

    val rustTypes: Set<String> = setOf(
        "bool", "char", "str", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32",
        "u64", "u128", "usize", "f32", "f64", "Self", "String", "Vec", "Option", "Result",
        "Box",
    )

    val rustConstants: Set<String> = setOf(
        "false", "None", "true",
    )

    val jsonConstants: Set<String> = setOf(
        "false", "null", "true",
    )

    val yamlConstants: Set<String> = setOf(
        "false", "no", "null", "off", "on", "true", "yes",
    )

    val tomlConstants: Set<String> = setOf(
        "false", "true",
    )

    val vKeywords: Set<String> = setOf(
        "as", "asm", "assert", "atomic", "break", "const", "continue", "defer", "else", "enum",
        "fn", "for", "go", "goto", "if", "implements", "import", "in", "interface", "is",
        "isreftype", "lock", "match", "module", "mut", "or", "pub", "return", "rlock",
        "select", "shared", "sizeof", "spawn", "static", "struct", "type", "typeof", "union",
        "unsafe", "volatile", "__global", "__offsetof",
    )

    val vTypes: Set<String> = setOf(
        "any", "bool", "byte", "byteptr", "char", "charptr", "f32", "f64", "i8", "i16", "int",
        "i64", "i128", "isize", "map", "rune", "string", "u8", "u16", "u32", "u64", "u128",
        "usize", "voidptr",
    )

    val vConstants: Set<String> = setOf(
        "false", "none", "true",
    )

    val odinKeywords: Set<String> = setOf(
        "asm", "auto_cast", "bit_field", "bit_set", "break", "case", "cast", "context",
        "continue", "defer", "distinct", "do", "dynamic", "else", "enum", "fallthrough", "for",
        "foreign", "if", "import", "in", "inline", "map", "matrix", "no_inline", "not_in",
        "or_break", "or_continue", "or_else", "or_return", "package", "proc", "return",
        "struct", "switch", "transmute", "typeid", "union", "using", "when", "where",
    )

    val odinTypes: Set<String> = setOf(
        "any", "bool", "byte", "b8", "b16", "b32", "b64", "int", "i8", "i16", "i32", "i64",
        "i128", "uint", "u8", "u16", "u32", "u64", "u128", "uintptr", "f16", "f32", "f64",
        "complex32", "complex64", "complex128", "quaternion64", "quaternion128",
        "quaternion256", "rune", "string", "cstring", "rawptr",
    )

    val odinConstants: Set<String> = setOf(
        "false", "nil", "true",
    )

    val rubyKeywords: Set<String> = setOf(
        "BEGIN", "END", "__ENCODING__", "__FILE__", "__LINE__", "alias", "and", "begin",
        "break", "case", "class", "def", "defined", "do", "else", "elsif", "end", "ensure",
        "for", "if", "in", "module", "next", "not", "or", "redo", "rescue", "retry", "return",
        "self", "super", "then", "undef", "unless", "until", "when", "while", "yield",
    )

    val rubyConstants: Set<String> = setOf(
        "false", "nil", "true",
    )

    val rubyFunctions: Set<String> = setOf(
        "abort", "at_exit", "autoload", "binding", "block_given", "caller", "catch", "eval",
        "exec", "exit", "fail", "fork", "format", "gets", "lambda", "load", "loop", "open",
        "p", "print", "printf", "proc", "putc", "puts", "raise", "readline", "require",
        "require_relative", "select", "sleep", "sprintf", "system", "throw", "trap", "warn",
    )

    val rubyDefinitionKeywords: Set<String> = setOf(
        "def",
    )

    val crystalKeywords: Set<String> = setOf(
        "__DIR__", "__END_LINE__", "__FILE__", "__LINE__", "abstract", "alias", "alignof",
        "annotation", "as", "asm", "begin", "break", "case", "class", "def", "do", "else",
        "elsif", "end", "ensure", "enum", "extend", "for", "fun", "if", "in", "include",
        "instance_alignof", "instance_sizeof", "is_a", "lib", "macro", "module", "next", "of",
        "offsetof", "out", "pointerof", "previous_def", "private", "protected", "require",
        "rescue", "responds_to", "return", "select", "self", "sizeof", "struct", "super",
        "then", "type", "typeof", "union", "uninitialized", "unless", "until", "verbatim",
        "when", "while", "with", "yield",
    )

    val crystalTypes: Set<String> = setOf(
        "Array", "Bool", "Bytes", "Char", "Class", "Enum", "Exception", "Fiber", "Float32",
        "Float64", "Hash", "IO", "Int8", "Int16", "Int32", "Int64", "Int128", "Iterator",
        "NamedTuple", "Nil", "Number", "Object", "Pointer", "Proc", "Range", "Reference",
        "Regex", "Set", "Slice", "String", "Struct", "Symbol", "Tuple", "UInt8", "UInt16",
        "UInt32", "UInt64", "UInt128", "Value",
    )

    val crystalConstants: Set<String> = setOf(
        "false", "nil", "true",
    )

    val crystalFunctions: Set<String> = setOf(
        "abort", "at_exit", "delegate", "exit", "getter", "p", "pp", "print", "printf",
        "property", "puts", "raise", "record", "setter", "sleep", "spawn",
    )

    val crystalDefinitionKeywords: Set<String> = setOf(
        "def", "fun", "macro",
    )

    val bashKeywords: Set<String> = setOf(
        "break", "case", "continue", "coproc", "do", "done", "elif", "else", "esac", "fi",
        "for", "function", "if", "in", "return", "select", "then", "time", "until", "while",
    )

    val bashFunctions: Set<String> = setOf(
        "alias", "bg", "bind", "builtin", "caller", "cd", "command", "compgen", "complete",
        "compopt", "declare", "dirs", "disown", "echo", "enable", "eval", "exec", "exit",
        "export", "fc", "fg", "getopts", "hash", "help", "history", "jobs", "kill", "let",
        "local", "logout", "mapfile", "popd", "printf", "pushd", "pwd", "read", "readarray",
        "readonly", "set", "shift", "shopt", "source", "suspend", "test", "times", "trap",
        "type", "typeset", "ulimit", "umask", "unalias", "unset", "wait",
    )

    val bashDefinitionKeywords: Set<String> = setOf(
        "function",
    )

    val bashCommandContexts: Set<String> = setOf(
        "do", "elif", "else", "if", "then", "until", "while",
    )

}
