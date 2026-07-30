require "../spec_helper"
require "../../src/xd/syntax"

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
end
