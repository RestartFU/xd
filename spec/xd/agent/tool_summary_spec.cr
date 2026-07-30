require "../../spec_helper"
require "../../../src/xd/agent/tool_summary"

private def summary_input(
  values : Hash(String, String),
) : Hash(String, JSON::Any)
  values.transform_values { |value| JSON::Any.new(value) }
end

describe Xd::Agent::ToolSummary do
  it "names commands, files, and shell-wrapped work" do
    Xd::Agent::ToolSummary.build(
      "Bash",
      summary_input({
        "command"   => "git status",
        "file_path" => "ignored",
      })
    ).should eq("$ git status")

    Xd::Agent::ToolSummary.build(
      "command_execution",
      summary_input({
        "command" =>
          %(/run/current-system/sw/bin/bash -lc "rg -n 'x|y' src/"),
      })
    ).should eq("$ rg -n 'x|y' src/")

    input = summary_input({"file_path" => "src/main.c"})
    Xd::Agent::ToolSummary.build("Edit", input)
      .should eq("file_change  src/main.c")
    Xd::Agent::ToolSummary.build("write", input)
      .should eq("file_change  src/main.c")
    Xd::Agent::ToolSummary.build("Think", nil).should eq("Think")
  end

  it "round trips compact subagent records" do
    message = Xd::Agent::ToolSummary.build(
      "Agent",
      summary_input({
        "subagent_type" => "Explore\nagent",
        "description"   => "Inspect   parser\ncarefully",
      })
    )
    Xd::Agent::SubagentTool.parse(message).should eq({
      "Explore agent",
      "Inspect parser carefully",
    })

    collab = Xd::Agent::ToolSummary.build(
      "collab_tool_call",
      summary_input({
        "tool"   => "spawnAgent",
        "model"  => "gpt-5",
        "prompt" => "Review diff",
      })
    )
    Xd::Agent::SubagentTool.parse(collab).should eq({
      "gpt-5",
      "Review diff",
    })
  end

  it "builds Codex file diffs without a Git repository" do
    input = JSON.parse({
      "changes" => [
        {
          "path" => "src/new.cr",
          "kind" => "add",
          "diff" => "puts :hello\n",
        },
        {
          "path" => "src/old.cr",
          "kind" => "update",
          "diff" => "@@ -1 +1 @@\n-old\n+new\n",
        },
      ],
    }.to_json).as_h

    summary = Xd::Agent::ToolSummary.build("file_change", input)
    summary.should start_with("file_change\ndiff --git ")
    summary.should contain("+++ b/src/new.cr")
    summary.should contain("+puts :hello")
    summary.should contain("@@ -1 +1 @@")
  end

  it "builds Claude edit and write diffs from tool arguments" do
    edit = JSON.parse({
      "file_path" => "README.md",
      "old_string" => "before\n",
      "new_string" => "after\n",
    }.to_json).as_h
    edit_summary = Xd::Agent::ToolSummary.build("Edit", edit)
    edit_summary.should start_with("file_change\ndiff --git ")
    edit_summary.should contain("-before")
    edit_summary.should contain("+after")

    write = JSON.parse({
      "file_path" => "new.txt",
      "content" => "created\n",
    }.to_json).as_h
    write_summary = Xd::Agent::ToolSummary.build("Write", write)
    write_summary.should contain("new file mode 100644")
    write_summary.should contain("+created")
  end
end
