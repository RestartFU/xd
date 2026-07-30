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

  it "classifies Dockerfile instructions without treating URLs as comments" do
    instruction = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Dockerfile,
      "from \"debian:bookworm\" AS build",
      Xd::SyntaxState.new
    )
    syntax_has?(instruction, Xd::SyntaxToken::Keyword, "from")
      .should be_true
    syntax_has?(instruction, Xd::SyntaxToken::String, "\"debian:bookworm\"")
      .should be_true
    syntax_has?(instruction, Xd::SyntaxToken::Keyword, "AS").should be_true

    comment = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Dockerfile,
      "  # syntax=docker/dockerfile:1",
      Xd::SyntaxState.new
    )
    syntax_has?(
      comment,
      Xd::SyntaxToken::Comment,
      "# syntax=docker/dockerfile:1"
    ).should be_true

    url = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Dockerfile,
      "RUN curl https://example.test/archive",
      Xd::SyntaxState.new
    )
    url.none? { |piece| piece.token == Xd::SyntaxToken::Comment }
      .should be_true
  end

  it "classifies Make variables and escaped comments" do
    recipe = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Makefile,
      "\t$(CC) -o \"$@\" $(call output,$(objects)) # link",
      Xd::SyntaxState.new
    )
    syntax_has?(recipe, Xd::SyntaxToken::Preprocessor, "$(CC)")
      .should be_true
    syntax_has?(
      recipe,
      Xd::SyntaxToken::Preprocessor,
      "$(call output,$(objects))"
    ).should be_true
    syntax_has?(recipe, Xd::SyntaxToken::String, "\"$@\"").should be_true
    syntax_has?(recipe, Xd::SyntaxToken::Comment, "# link").should be_true

    escaped = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Makefile,
      "HASH := \\#literal",
      Xd::SyntaxState.new
    )
    escaped.none? { |piece| piece.token == Xd::SyntaxToken::Comment }
      .should be_true
  end

  it "classifies JSON quoted keys" do
    pieces = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::JSON,
      "{\"name\": \"xd\", \"enabled\": true, \"retries\": 3, \"empty\": null}",
      Xd::SyntaxState.new
    )
    syntax_has?(pieces, Xd::SyntaxToken::Type, "\"name\"").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::String, "\"xd\"").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Number, "true").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Number, "3").should be_true
    syntax_has?(pieces, Xd::SyntaxToken::Number, "null").should be_true
  end

  it "classifies YAML keys, references, constants, and spaced comments" do
    setting = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::YAML,
      "service-name: true # enabled",
      Xd::SyntaxState.new
    )
    syntax_has?(setting, Xd::SyntaxToken::Type, "service-name")
      .should be_true
    syntax_has?(setting, Xd::SyntaxToken::Number, "true").should be_true
    syntax_has?(setting, Xd::SyntaxToken::Comment, "# enabled")
      .should be_true

    url = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::YAML,
      "- endpoint: https://example.test/a#fragment",
      Xd::SyntaxState.new
    )
    syntax_has?(url, Xd::SyntaxToken::Type, "endpoint").should be_true
    url.none? { |piece| piece.token == Xd::SyntaxToken::Comment }
      .should be_true

    anchor = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::YAML,
      "defaults: &base",
      Xd::SyntaxState.new
    )
    syntax_has?(anchor, Xd::SyntaxToken::Preprocessor, "&base")
      .should be_true
  end

  it "classifies TOML keys, tables, comments, and multiline strings" do
    table = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::TOML,
      "[server.database]",
      Xd::SyntaxState.new
    )
    syntax_has?(table, Xd::SyntaxToken::Preprocessor, "[server.database]")
      .should be_true

    setting = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::TOML,
      "listen-address = \"127.0.0.1\" # local",
      Xd::SyntaxState.new
    )
    syntax_has?(setting, Xd::SyntaxToken::Type, "listen-address")
      .should be_true
    syntax_has?(setting, Xd::SyntaxToken::String, "\"127.0.0.1\"")
      .should be_true
    syntax_has?(setting, Xd::SyntaxToken::Comment, "# local").should be_true

    state = Xd::SyntaxState.new
    Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::TOML,
      "description = '''first",
      state
    )
    state.in_triple_string.should be_true
    state.triple_quote.should eq('\'')
    inside = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::TOML,
      "# still string",
      state
    )
    inside.none? { |piece| piece.token == Xd::SyntaxToken::Comment }
      .should be_true
    closed = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::TOML,
      "last''' # comment",
      state
    )
    state.in_triple_string.should be_false
    syntax_has?(closed, Xd::SyntaxToken::Comment, "# comment").should be_true
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

  it "classifies V attributes, generics, literals, and directives" do
    attribute = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::V,
      "@[json: 'userName']",
      Xd::SyntaxState.new
    )
    syntax_has?(
      attribute,
      Xd::SyntaxToken::Preprocessor,
      "@[json: 'userName']"
    ).should be_true

    declaration = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::V,
      "pub fn decode[T](name string) ?User {",
      Xd::SyntaxState.new
    )
    syntax_has?(declaration, Xd::SyntaxToken::Function, "decode")
      .should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Type, "T").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Type, "string").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Type, "User").should be_true

    body = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::V,
      "user := User{name: r'Ada\\n'}; println(user); letter := `V` // done",
      Xd::SyntaxState.new
    )
    syntax_has?(body, Xd::SyntaxToken::String, "r'Ada\\n'").should be_true
    syntax_has?(body, Xd::SyntaxToken::Function, "println").should be_true
    syntax_has?(body, Xd::SyntaxToken::String, "`V`").should be_true
    syntax_has?(body, Xd::SyntaxToken::Comment, "// done").should be_true

    compile_time = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::V,
      "$if linux { assert true }",
      Xd::SyntaxState.new
    )
    syntax_has?(compile_time, Xd::SyntaxToken::Preprocessor, "$if")
      .should be_true
    syntax_has?(compile_time, Xd::SyntaxToken::Number, "true").should be_true

    shebang = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::V,
      "#!/usr/bin/env -S v",
      Xd::SyntaxState.new
    )
    syntax_has?(
      shebang,
      Xd::SyntaxToken::Comment,
      "#!/usr/bin/env -S v"
    ).should be_true
  end

  it "classifies Odin procedures, attributes, directives, and state" do
    attribute = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Odin,
      "@(private)",
      Xd::SyntaxState.new
    )
    syntax_has?(attribute, Xd::SyntaxToken::Preprocessor, "@(private)")
      .should be_true

    declaration = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Odin,
      "fibonacci :: #force_inline proc($T: typeid, n: int) -> int {",
      Xd::SyntaxState.new
    )
    syntax_has?(declaration, Xd::SyntaxToken::Function, "fibonacci")
      .should be_true
    syntax_has?(
      declaration,
      Xd::SyntaxToken::Preprocessor,
      "#force_inline"
    ).should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Keyword, "proc").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Preprocessor, "$T")
      .should be_true

    body = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Odin,
      "user := User{name = \"Ada\"}; fmt.println(user, nil, 42)",
      Xd::SyntaxState.new
    )
    syntax_has?(body, Xd::SyntaxToken::Type, "User").should be_true
    syntax_has?(body, Xd::SyntaxToken::Function, "println").should be_true
    syntax_has?(body, Xd::SyntaxToken::Number, "nil").should be_true

    tags = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Odin,
      "#+test value: int = ---",
      Xd::SyntaxState.new
    )
    syntax_has?(tags, Xd::SyntaxToken::Preprocessor, "#+test").should be_true
    syntax_has?(tags, Xd::SyntaxToken::Number, "---").should be_true

    state = Xd::SyntaxState.new
    Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Odin,
      "data := `line // still raw",
      state
    )
    state.in_raw_string.should be_true
    raw_close = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Odin,
      "last`; #load(\"data.bin\")",
      state
    )
    state.in_raw_string.should be_false
    syntax_has?(raw_close, Xd::SyntaxToken::Preprocessor, "#load")
      .should be_true

    Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Odin,
      "/* outer /* inner */ still",
      state
    )
    state.in_comment.should eq(1)
    close = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Odin,
      "comment */ main :: proc() {}",
      state
    )
    state.in_comment.should eq(0)
    syntax_has?(close, Xd::SyntaxToken::Function, "main").should be_true
  end

  it "classifies Ruby definitions, literals, sigils, and comments" do
    declaration = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Ruby,
      "class Greeter; def self.greet name",
      Xd::SyntaxState.new
    )
    syntax_has?(declaration, Xd::SyntaxToken::Keyword, "class")
      .should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Type, "Greeter").should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Function, "greet")
      .should be_true

    body = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Ruby,
      "puts %Q(Hello \#{name}) if @enabled && name != :world # welcome",
      Xd::SyntaxState.new
    )
    syntax_has?(body, Xd::SyntaxToken::Function, "puts").should be_true
    syntax_has?(body, Xd::SyntaxToken::String, "%Q(Hello \#{name})")
      .should be_true
    syntax_has?(body, Xd::SyntaxToken::Preprocessor, "@enabled")
      .should be_true
    syntax_has?(body, Xd::SyntaxToken::String, ":world").should be_true
    syntax_has?(body, Xd::SyntaxToken::Comment, "# welcome").should be_true

    pattern = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Ruby,
      "pattern = /foo\\/[a-z]+/im; ratio = total / count; value %= 2",
      Xd::SyntaxState.new
    )
    syntax_has?(
      pattern,
      Xd::SyntaxToken::String,
      "/foo\\/[a-z]+/im"
    ).should be_true
    syntax_has?(pattern, Xd::SyntaxToken::String, "/ count")
      .should be_false
    syntax_has?(pattern, Xd::SyntaxToken::String, "%=").should be_false
  end

  it "carries Ruby heredocs and block comments" do
    state = Xd::SyntaxState.new
    opened = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Ruby,
      "message = <<~TEXT",
      state
    )
    state.in_heredoc.should be_true
    syntax_has?(opened, Xd::SyntaxToken::String, "<<~TEXT").should be_true
    body = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Ruby,
      "  \#{name} # still a string",
      state
    )
    body.should eq([
      Xd::SyntaxPiece.new(
        Xd::SyntaxToken::String,
        "  \#{name} # still a string"
      ),
    ])
    Xd::Syntax.scan_line(Xd::SyntaxLanguage::Ruby, "  TEXT", state)
    state.in_heredoc.should be_false

    opened_comment = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Ruby,
      "=begin docs",
      state
    )
    state.in_comment.should eq(1)
    syntax_has?(
      opened_comment,
      Xd::SyntaxToken::Comment,
      "=begin docs"
    ).should be_true
    body_comment = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Ruby,
      "def not_code",
      state
    )
    body_comment.first.token.should eq(Xd::SyntaxToken::Comment)
    Xd::Syntax.scan_line(Xd::SyntaxLanguage::Ruby, "=end", state)
    state.in_comment.should eq(0)
  end

  it "classifies Crystal annotations, macros, and heredocs" do
    annotation_pieces = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Crystal,
      "@[JSON::Field(key: \"name\")]",
      Xd::SyntaxState.new
    )
    syntax_has?(
      annotation_pieces,
      Xd::SyntaxToken::Preprocessor,
      "@[JSON::Field(key: \"name\")]"
    ).should be_true

    declaration = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Crystal,
      "class Greeter; def greet name : String",
      Xd::SyntaxState.new
    )
    syntax_has?(declaration, Xd::SyntaxToken::Function, "greet")
      .should be_true
    syntax_has?(declaration, Xd::SyntaxToken::Type, "String").should be_true

    state = Xd::SyntaxState.new
    macro_pieces = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Crystal,
      "{% if flag?(:linux) %}",
      state
    )
    syntax_has?(macro_pieces, Xd::SyntaxToken::Preprocessor, "{%")
      .should be_true
    syntax_has?(macro_pieces, Xd::SyntaxToken::String, ":linux")
      .should be_true
    syntax_has?(macro_pieces, Xd::SyntaxToken::Preprocessor, "%}")
      .should be_true
    state.crystal_macro_close.should eq('\0')

    literals = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Crystal,
      "pattern = /foo\\/[a-z]+/im; command = `uname -a`; getter = :[]?",
      state
    )
    syntax_has?(
      literals,
      Xd::SyntaxToken::String,
      "/foo\\/[a-z]+/im"
    ).should be_true
    syntax_has?(literals, Xd::SyntaxToken::String, "`uname -a`")
      .should be_true
    syntax_has?(literals, Xd::SyntaxToken::String, ":[]?").should be_true

    opened = Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Crystal,
      "message = <<-TEXT.upcase",
      state
    )
    state.in_heredoc.should be_true
    syntax_has?(opened, Xd::SyntaxToken::String, "<<-TEXT").should be_true
    Xd::Syntax.scan_line(
      Xd::SyntaxLanguage::Crystal,
      "  \#{name} # still a string",
      state
    )
    Xd::Syntax.scan_line(Xd::SyntaxLanguage::Crystal, "  TEXT", state)
    state.in_heredoc.should be_false
  end
end
