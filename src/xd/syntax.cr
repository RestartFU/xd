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
