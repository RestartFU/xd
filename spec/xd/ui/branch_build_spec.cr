require "../../spec_helper"
require "../../../src/xd/ui/branch_build"

describe Xd::UI::BranchBuild do
  it "parses branches, pull requests, and commits" do
    Xd::UI::BranchBuild.parse("feature/sidebar").not_nil!.ref
      .should eq("refs/heads/feature/sidebar")
    Xd::UI::BranchBuild.parse("#128").not_nil!.ref
      .should eq("refs/pull/128/head")
    commit = "abcdef1234567890abcdef1234567890abcdef12"
    Xd::UI::BranchBuild.parse(commit).not_nil!.ref.should eq(commit)
    Xd::UI::BranchBuild.parse(
      "https://github.com/example/xd/commit/#{commit}"
    ).not_nil!.url.should eq("https://github.com/example/xd.git")
  end

  it "rejects shell-active and ambiguous input" do
    ["", "main; rm -rf /", "-main", "main..next", "main.lock"].each do |value|
      Xd::UI::BranchBuild.parse(value).should be_nil
    end
  end

  it "quotes checkout paths and uses bundled Git" do
    target = Xd::UI::BranchBuild.parse("main").not_nil!
    command = Xd::UI::BranchBuild.command(
      target,
      "/tmp/person's cache/source"
    )
    command.should contain("checkout='/tmp/person'\"'\"'s cache/source'")
    command.should contain("refs/heads/main")
    command.should contain("PROFILE=nightly")
    command.should contain("XD_ALLOW_RUNNING_INSTALL=1")
  end
end
