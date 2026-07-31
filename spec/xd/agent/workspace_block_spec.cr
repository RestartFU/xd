require "../../spec_helper"
require "../../../src/xd/agent/workspace_block"

describe Xd::Agent::WorkspaceBlock do
  it "removes standalone reports and uses the last path" do
    parsed = Xd::Agent::WorkspaceBlock.parse(<<-TEXT).not_nil!
      Started here.
      <workspace>/code/first</workspace>
      Continued.
      <workspace> /code/final </workspace>
      Done.
      TEXT

    parsed.path.should eq("/code/final")
    parsed.remainder.should eq("Started here.\nContinued.\nDone.")
  end

  it "keeps inline, empty, and multiline tags as prose" do
    [
      "Use <workspace>/code</workspace> here.",
      "<workspace></workspace>",
      "<workspace>/one\n/two</workspace>",
    ].each do |text|
      Xd::Agent::WorkspaceBlock.parse(text).should be_nil
    end
  end

  it "holds only line-leading partial markers while streaming" do
    Xd::Agent::WorkspaceBlock.visible_bytes("Done\n<work").should eq(5)
    Xd::Agent::WorkspaceBlock.visible_bytes(
      "Done\n<workspace>/code</workspace>"
    ).should eq(5)
    inline = "Mention <workspace>"
    Xd::Agent::WorkspaceBlock.visible_bytes(inline).should eq(inline.bytesize)
  end
end
