require "../../spec_helper"
require "../../../src/xd/ui/branch_build"

describe Xd::UI::BranchBuild do
  it "parses pull request forms" do
    short = Xd::UI::BranchBuild.parse("#128").not_nil!
    short.url.should eq("https://github.com/RestartFU/xd.git")
    short.ref.should eq("refs/pull/128/head")
    short.label.should eq("pull request #128")

    plain = Xd::UI::BranchBuild.parse("128").not_nil!
    plain.should eq(short)

    linked = Xd::UI::BranchBuild.parse(
      "https://github.com/example/fork/pull/42/files#diff"
    ).not_nil!
    linked.url.should eq("https://github.com/example/fork.git")
    linked.ref.should eq("refs/pull/42/head")
    linked.label.should eq("pull request #42 in example/fork")
  end

  it "parses branch names and links with slashes" do
    named = Xd::UI::BranchBuild.parse("feature/sidebar").not_nil!
    named.url.should eq("https://github.com/RestartFU/xd.git")
    named.ref.should eq("refs/heads/feature/sidebar")
    named.label.should eq("branch feature/sidebar")

    linked = Xd::UI::BranchBuild.parse(
      "https://github.com/example/xd.git/tree/fix/gtk?tab=readme"
    ).not_nil!
    linked.url.should eq("https://github.com/example/xd.git")
    linked.ref.should eq("refs/heads/fix/gtk")
    linked.label.should eq("branch fix/gtk in example/xd")
  end

  it "rejects ambiguous and shell-active input" do
    invalid = [
      "",
      "https://github.com/RestartFU/xd",
      "main; rm -rf /",
      "-main",
      ".main",
      "/main",
      "main/",
      "main.",
      "main..next",
      "main//next",
      "main.lock",
      "https://github.com/-owner/xd/tree/main",
      "https://github.com/owner/x$d/tree/main",
      "１２８",
    ]
    invalid.each do |value|
      Xd::UI::BranchBuild.parse(value).should be_nil
    end
    Xd::UI::BranchBuild.parse("a" * 201).should be_nil
    Xd::UI::BranchBuild.parse("#1234567890").should be_nil
  end

  it "quotes checkout paths in the build command" do
    target = Xd::UI::BranchBuild.parse("feature/sidebar").not_nil!
    command = Xd::UI::BranchBuild.command(
      target,
      "/tmp/person's cache/source"
    )

    command.should contain(
      "checkout='/tmp/person'\"'\"'s cache/source'"
    )
    command.should contain(
      "fetch --depth 1 --force " \
      "https://github.com/RestartFU/xd.git " \
      "refs/heads/feature/sidebar"
    )
    command.should contain(
      "./scripts/build.sh --build-arg PROFILE=nightly"
    )
    command.should contain("sh scripts/install.sh --from dist")
  end
end
