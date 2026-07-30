require "../spec_helper"
require "../../src/xd/syntax"

private def syntax_has?(
  pieces : Array(Xd::SyntaxPiece),
  token : Xd::SyntaxToken,
  text : String,
) : Bool
  pieces.includes?(Xd::SyntaxPiece.new(token, text))
end

describe Xd::Syntax do
  it "detects every language supported by the C renderer" do
    paths = {
      "src/util/syntax.c"         => Xd::SyntaxLanguage::C,
      "a/b.h"                     => Xd::SyntaxLanguage::C,
      "main.go"                   => Xd::SyntaxLanguage::Go,
      "Main.kt"                   => Xd::SyntaxLanguage::Kotlin,
      "build.gradle.kts"          => Xd::SyntaxLanguage::Kotlin,
      "Dockerfile"                => Xd::SyntaxLanguage::Dockerfile,
      "images/Dockerfile.release" => Xd::SyntaxLanguage::Dockerfile,
      "Containerfile"             => Xd::SyntaxLanguage::Dockerfile,
      "image.dockerfile"          => Xd::SyntaxLanguage::Dockerfile,
      "Makefile"                  => Xd::SyntaxLanguage::Makefile,
      "build/Makefile.release"    => Xd::SyntaxLanguage::Makefile,
      "GNUmakefile"               => Xd::SyntaxLanguage::Makefile,
      "rules.mk"                  => Xd::SyntaxLanguage::Makefile,
      "src/main.rs"               => Xd::SyntaxLanguage::Rust,
      "package.json"              => Xd::SyntaxLanguage::JSON,
      "compose.yaml"              => Xd::SyntaxLanguage::YAML,
      "workflow.yml"              => Xd::SyntaxLanguage::YAML,
      "Cargo.toml"                => Xd::SyntaxLanguage::TOML,
      "main.v"                    => Xd::SyntaxLanguage::V,
      "deploy.vsh"                => Xd::SyntaxLanguage::V,
      "game/main.odin"            => Xd::SyntaxLanguage::Odin,
      "lib/report.rb"             => Xd::SyntaxLanguage::Ruby,
      "tasks/release.rake"        => Xd::SyntaxLanguage::Ruby,
      "xd.gemspec"                => Xd::SyntaxLanguage::Ruby,
      "Gemfile"                   => Xd::SyntaxLanguage::Ruby,
      "Rakefile"                  => Xd::SyntaxLanguage::Ruby,
      "Vagrantfile"               => Xd::SyntaxLanguage::Ruby,
      "src/server.cr"             => Xd::SyntaxLanguage::Crystal,
      "src/Program.cs"            => Xd::SyntaxLanguage::CSharp,
      "scripts/setup.csx"         => Xd::SyntaxLanguage::CSharp,
    }

    paths.each do |path, language|
      Xd::Syntax.language_for_path(path).should eq(language)
    end
    Xd::Syntax.language_for_path("README.md")
      .should eq(Xd::SyntaxLanguage::None)
    Xd::Syntax.language_for_path("vendor.go/LICENSE")
      .should eq(Xd::SyntaxLanguage::None)
    Xd::Syntax.language_for_path(nil)
      .should eq(Xd::SyntaxLanguage::None)
  end

  it "uses the exact C token palette" do
    {
      Xd::SyntaxToken::Text         => nil,
      Xd::SyntaxToken::Keyword      => "#dc8add",
      Xd::SyntaxToken::Type         => "#78aeed",
      Xd::SyntaxToken::Function     => "#99c1f1",
      Xd::SyntaxToken::String       => "#f8e45c",
      Xd::SyntaxToken::Number       => "#ffbe6f",
      Xd::SyntaxToken::Comment      => "#8b8e8f",
      Xd::SyntaxToken::Preprocessor => "#c061cc",
    }.each do |token, colour|
      token.colour.should eq(colour)
    end
  end

  it "returns unknown source losslessly" do
    line = "func main() { // go"
    pieces = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::None,
      line,
      Xd::SyntaxState.new
    )

    pieces.map(&.text).join.should eq(line)
    pieces.should eq([
      Xd::SyntaxPiece.new(Xd::SyntaxToken::Text, line),
    ])
  end

  it "classifies C without losing source bytes" do
    line = "  static int count = 0x1f; g_free (value); // seen"
    pieces = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::C,
      line,
      Xd::SyntaxState.new
    )

    pieces.map(&.text).join.should eq(line)
    syntax_has?(pieces, Xd::SyntaxToken::Keyword, "static").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Keyword, "int").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Number, "0x1f").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Function, "g_free").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Comment, "// seen").should be_true

    include_line = "#include <glib.h>"
    included = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::C,
      include_line,
      Xd::SyntaxState.new
    )
    included.map(&.text).join.should eq(include_line)
    syntax_has?(included, Xd::SyntaxToken::Preprocessor, "#include")
      .should be_true
    syntax_has?(included, Xd::SyntaxToken::String, "<glib.h>")
      .should be_true

    unterminated = "char *s = \"open"
    open_string = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::C,
      unterminated,
      Xd::SyntaxState.new
    )
    open_string.map(&.text).join.should eq(unterminated)
    syntax_has?(open_string, Xd::SyntaxToken::String, "\"open")
      .should be_true
  end

  it "carries C block comments between lines" do
    state = Xd::SyntaxState.new
    opened = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::C,
      "int a; /* why",
      state
    )
    state.in_comment.should eq(1)
    syntax_has?(opened, Xd::SyntaxToken::Comment, "/* why").should be_true

    inside = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::C,
      " * still int",
      state
    )
    state.in_comment.should eq(1)
    syntax_has?(inside, Xd::SyntaxToken::Keyword, "int").should be_false

    closed = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::C,
      " */ int b;",
      state
    )
    state.in_comment.should eq(0)
    syntax_has?(closed, Xd::SyntaxToken::Comment, " */").should be_true
    syntax_has?(closed, Xd::SyntaxToken::Keyword, "int").should be_true
  end

  it "classifies Go calls, types, composites, and raw string state" do
    line = "func read(p []byte) (int, error) { return len(p), nil }"
    pieces = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Go,
      line,
      Xd::SyntaxState.new
    )
    syntax_has?(pieces, Xd::SyntaxToken::Keyword, "func").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Function, "read").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Type, "byte").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Type, "error").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Number, "nil").should be_true

    literal = "\tpk := &packet.PlayerAuthInput{Position: mgl32.Vec3{40}}"
    composites = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Go,
      literal,
      Xd::SyntaxState.new
    )
    syntax_has?(composites, Xd::SyntaxToken::Type, "PlayerAuthInput")
      .should be_true
    syntax_has?(composites, Xd::SyntaxToken::Type, "Vec3").should be_true
    syntax_has?(composites, Xd::SyntaxToken::Type, "packet").should be_false

    state = Xd::SyntaxState.new
    opened = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Go,
      "const q = `select func",
      state
    )
    state.in_raw_string.should be_true
    syntax_has?(opened, Xd::SyntaxToken::Keyword, "func").should be_false
    closed = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Go,
      "from t` + x",
      state
    )
    state.in_raw_string.should be_false
    syntax_has?(closed, Xd::SyntaxToken::String, "from t`").should be_true
  end

  it "classifies Kotlin and carries triple strings" do
    declaration = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Kotlin,
      "data class User(val name: String, val age: Int = 42)",
      Xd::SyntaxState.new
    )
    syntax_has?(declaration, Xd::SyntaxToken::Keyword, "data").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Type, "User").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Type, "String").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Number, "42").should be_true

    state = Xd::SyntaxState.new
    opened = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Kotlin,
      "val query = \"\"\"select fun",
      state
    )
    state.in_triple_string.should be_true
    syntax_has?(opened, Xd::SyntaxToken::Keyword, "fun").should be_false
    inside = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Kotlin,
      "from users // literal",
      state
    )
    syntax_has?(inside, Xd::SyntaxToken::Comment, "// literal")
      .should be_false
    closed = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Kotlin,
      "where active\"\"\".trimIndent ()",
      state
    )
    state.in_triple_string.should be_false
    syntax_has?(closed, Xd::SyntaxToken::String, "where active\"\"\"")
      .should be_true
    syntax_has?(closed, Xd::SyntaxToken::Function, "trimIndent")
      .should be_true
  end

  it "classifies Rust and carries raw strings and nested comments" do
    declaration = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Rust,
      "pub async fn load<'a>(path: &'a str) -> Result<String, Error> {",
      Xd::SyntaxState.new
    )
    syntax_has?(declaration, Xd::SyntaxToken::Keyword, "pub").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Function, "load").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Preprocessor, "'a")
      .should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Type, "Result").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Type, "Error").should be_true

    body = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Rust,
      "println!(r#\"value // {}\"#, 42); let letter = 'x';",
      Xd::SyntaxState.new
    )
    syntax_has?(body, Xd::SyntaxToken::Function, "println").should be_true
    syntax_has?(body, Xd::SyntaxToken::String, "r#\"value // {}\"#")
      .should be_true
    syntax_has?(body, Xd::SyntaxToken::String, "'x'").should be_true

    raw_state = Xd::SyntaxState.new
    Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Rust,
      "let json = r##\"{ \"kind\":",
      raw_state
    )
    raw_state.in_rust_raw_string.should be_true
    Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Rust,
      "// still string",
      raw_state
    )
    raw_state.in_rust_raw_string.should be_true
    raw_close = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Rust,
      "}\"##; fn done() {}",
      raw_state
    )
    raw_state.in_rust_raw_string.should be_false
    syntax_has?(raw_close, Xd::SyntaxToken::Function, "done").should be_true

    comment_state = Xd::SyntaxState.new
    Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Rust,
      "/* outer /* inner */ still",
      comment_state
    )
    comment_state.in_comment.should eq(1)
    comment_close = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Rust,
      "comment */ fn main() {}",
      comment_state
    )
    comment_state.in_comment.should eq(0)
    syntax_has?(comment_close, Xd::SyntaxToken::Function, "main")
      .should be_true
  end
end
