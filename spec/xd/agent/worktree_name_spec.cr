require "../../spec_helper"
require "../../../src/xd/agent/worktree_name"

describe Xd::Agent::WorktreeNames do
  it "parses a concise name from the assistant response" do
    Xd::Agent::WorktreeNames.parse(
      "Here is the name: {\"name\":\"autoform module\"}"
    ).should eq("autoform module")
  end

  it "accepts a title field for commit-agent compatibility" do
    Xd::Agent::WorktreeNames.parse(
      %({"title":"review autofarm PR","body":"ignored"})
    ).should eq("review autofarm PR")
  end

  it "normalizes whitespace and rejects empty names" do
    Xd::Agent::WorktreeNames.parse(
      %({"name":"  fix   parser  "})
    ).should eq("fix parser")

    expect_raises(Xd::Agent::WorktreeNames::Error) do
      Xd::Agent::WorktreeNames.parse(%({"name":"   "}))
    end
  end
end
