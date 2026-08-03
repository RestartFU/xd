require "../../spec_helper"
require "../../../src/xd/agent/tool_summary"
require "../../../src/xd/unified_diff"

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
        "command" => %(/run/current-system/sw/bin/bash -lc "rg -n 'x|y' src/"),
      })
    ).should eq("$ rg -n 'x|y' src/")

    input = summary_input({"file_path" => "src/main.c"})
    Xd::Agent::ToolSummary.build("Edit", input)
      .should eq("file_change  src/main.c")
    Xd::Agent::ToolSummary.build("write", input)
      .should eq("file_change  src/main.c")
    Xd::Agent::ToolSummary.build("Think", nil).should eq("Think")
  end

  it "summarizes task tool calls" do
    Xd::Agent::ToolSummary.build(
      "TaskCreate",
      summary_input({
        "subject"     => "Identify every stacked PR",
        "description" => "The longer task description",
      })
    ).should eq("TaskCreate  Identify every stacked PR")

    Xd::Agent::ToolSummary.build(
      "TaskUpdate",
      summary_input({
        "taskId" => "7",
        "status" => "completed",
      })
    ).should eq("TaskUpdate  #7 → completed")
    Xd::Agent::ToolSummary.build(
      "TaskGet",
      summary_input({"taskId" => "7"})
    ).should eq("TaskGet  #7")
    Xd::Agent::ToolSummary.build("TaskList", nil).should eq("TaskList")
    Xd::Agent::ToolSummary.build(
      "TaskStop",
      summary_input({"task_id" => "background-7"})
    ).should eq("TaskStop  background-7")
  end

  it "builds useful Claude subagent cards" do
    message = Xd::Agent::ToolSummary.build(
      "Agent",
      summary_input({
        "subagent_type" => "Explore\nagent",
        "description"   => "Inspect   parser\ncarefully",
        "prompt"        => "Trace every parser path",
        "model"         => "haiku",
      })
    )
    Xd::Agent::SubagentTool.parse(message).should eq({
      "Claude · Explore agent · haiku",
      "Inspect parser carefully · Trace every parser path",
    })
  end

  it "builds useful Codex subagent cards from app-server state" do
    input = JSON.parse({
      "tool"              => "spawnAgent",
      "status"            => "completed",
      "receiverThreadIds" => ["thread-agent-123456789"],
      "prompt"            => "Review diff",
      "model"             => "gpt-5.6-sol",
      "reasoningEffort"   => "high",
      "agentsStates"      => {
        "thread-agent-123456789" => {
          "status"  => "running",
          "message" => "Checking native builds",
        },
      },
    }.to_json).as_h
    collab = Xd::Agent::ToolSummary.build(
      "collab_agent_tool_call",
      input
    )
    Xd::Agent::SubagentTool.parse(collab).should eq({
      "Codex · gpt-5.6-sol · high",
      "Running · Review diff · Agent thread-agent… · Checking native builds",
    })
  end

  it "keeps old subagent records readable and bounds new ones" do
    old = "subagent\nExplore\nInspect parser"
    Xd::Agent::SubagentTool.parse(old).should eq({
      "Explore",
      "Inspect parser",
    })

    message = Xd::Agent::ToolSummary.build(
      "Task",
      summary_input({
        "description" => "d" * 400,
        "prompt"      => "p" * 400,
      })
    )
    parsed = Xd::Agent::SubagentTool.parse(message).not_nil!
    parsed[0].size.should be <= 81
    parsed[1].size.should be <= 321
  end

  it "builds Codex file diffs without a Git repository" do
    input = JSON.parse({
      "changes" => [
        {
          "path" => "src/new.cr",
          "kind" => {"type" => "add"},
          "diff" => "puts :hello\n",
        },
        {
          "path" => "src/old.cr",
          "kind" => {"type" => "update", "move_path" => nil},
          "diff" => "@@ -1 +1 @@\n-old\n+new\n",
        },
      ],
    }.to_json).as_h

    summary = Xd::Agent::ToolSummary.build("file_change", input)
    summary.should start_with("file_change\ndiff --git ")
    summary.should contain("+++ b/src/new.cr")
    summary.should contain("+puts :hello")
    summary.should contain("@@ -1 +1 @@")

    patch = summary.byte_slice(Xd::Agent::ToolDiff::PREFIX.bytesize)
    parsed = Xd::UnifiedDiff.parse(patch)
    parsed.additions.should eq(2)
    parsed.deletions.should eq(1)
    parsed.lines.select(&.kind.added?).map(&.text)
      .should eq(["puts :hello", "new"])
  end

  it "builds Claude edit and write diffs from tool arguments" do
    edit = JSON.parse({
      "file_path"  => "README.md",
      "old_string" => "before\n",
      "new_string" => "after\n",
    }.to_json).as_h
    edit_summary = Xd::Agent::ToolSummary.build("Edit", edit)
    edit_summary.should start_with("file_change\ndiff --git ")
    edit_summary.should contain("-before")
    edit_summary.should contain("+after")

    write = JSON.parse({
      "file_path" => "new.txt",
      "content"   => "created\n",
    }.to_json).as_h
    write_summary = Xd::Agent::ToolSummary.build("Write", write)
    write_summary.should contain("new file mode 100644")
    write_summary.should contain("+created")
  end

  it "bounds generated-file diffs while building them" do
    content = ("é" * (2 * 1024 * 1024)) + "\nnever reached\n"
    write = summary_input({
      "file_path" => "generated.txt",
      "content"   => content,
    })

    summary = Xd::Agent::ToolDiff.build("Write", write).not_nil!
    summary.bytesize.should be < Xd::Agent::ToolDiff::LIMIT + 128
    summary.valid_encoding?.should be_true
    summary.should contain("… diff truncated …")
    summary.should_not contain("never reached")
  end

  it "shares one output budget across multi-edit patches" do
    edits = (0...32).map do |index|
      JSON::Any.new({
        "old_string" => JSON::Any.new("old #{index}\n"),
        "new_string" => JSON::Any.new(("x" * (512 * 1024)) + "\n"),
      })
    end
    input = {
      "file_path" => JSON::Any.new("generated.txt"),
      "edits"     => JSON::Any.new(edits),
    }

    summary = Xd::Agent::ToolDiff.build("MultiEdit", input).not_nil!
    summary.bytesize.should be < Xd::Agent::ToolDiff::LIMIT + 128
    summary.should contain("… diff truncated …")
    summary.should contain("old 0")
    summary.should_not contain("old 31")
  end

  it "builds apply-patch diffs without reading a repository" do
    input = summary_input({
      "patch" => <<-PATCH,
        *** Begin Patch
        *** Update File: src/old.cr
        *** Move to: src/new.cr
        @@
        -puts :old
        +puts :new
        *** Add File: notes.txt
        +hello
        *** Delete File: gone.txt
        *** End Patch
        PATCH
    })

    summary = Xd::Agent::ToolSummary.build("apply_patch", input)
    summary.should start_with("file_change\ndiff --git ")
    summary.should contain("rename from src/old.cr")
    summary.should contain("rename to src/new.cr")
    summary.should contain("-puts :old")
    summary.should contain("+puts :new")
    summary.should contain("new file mode 100644")
    summary.should contain("deleted file mode 100644")
  end

  it "bounds apply-patch parsing before splitting lines" do
    body = (0...200_000).map { |index| "+generated #{index}" }.join('\n')
    input = summary_input({
      "patch" => "*** Begin Patch\n*** Add File: generated.txt\n" \
                 "#{body}\n*** End Patch",
    })

    summary = Xd::Agent::ToolDiff.build("apply_patch", input).not_nil!
    summary.bytesize.should be < Xd::Agent::ToolDiff::LIMIT + 128
    summary.should contain("new file mode 100644")
    summary.should contain("… diff truncated …")
    summary.should_not contain("generated 199999")
  end

  it "builds notebook edits from source arguments" do
    input = summary_input({
      "notebook_path" => "analysis.ipynb",
      "old_source"    => "before()",
      "new_source"    => "after()",
    })

    summary = Xd::Agent::ToolSummary.build("NotebookEdit", input)
    summary.should start_with("file_change\ndiff --git ")
    summary.should contain("--- a/analysis.ipynb")
    summary.should contain("-before()")
    summary.should contain("+after()")
  end

  it "rejects malformed apply-patch bodies" do
    input = summary_input({
      "patch" => "*** Begin Patch\nnot a file\n*** End Patch",
    })
    Xd::Agent::ToolDiff.build("apply_patch", input).should be_nil
  end

  it "wraps bounded native unified diffs" do
    patch = "diff --git a/new.txt b/new.txt\n+created\n"
    Xd::Agent::ToolDiff.wrap_unified(patch)
      .should eq("file_change\n#{patch.rstrip}")
    Xd::Agent::ToolDiff.wrap_unified("not a diff").should be_nil
  end
end
