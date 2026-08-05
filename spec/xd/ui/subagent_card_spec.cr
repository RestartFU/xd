require "../../spec_helper"
require "../../../src/xd/ui/subagent_card"

describe "desktop subagent card activity" do
  it "transfers bounded activity data without nesting GTK groups" do
    source = File.read("src/xd/ui/subagent_card.cr")

    source.should contain("RunCard.new")
    source.should contain("@activity.merge(activity.take_activity)")
    source.should_not contain("Gtk::Expander.new")
    source.should_not contain("bind_property")
  end

  it "uses the workflow card state treatment" do
    source = File.read("src/xd/ui/subagent_card.cr")
    workflow = File.read("src/xd/ui/workflow_card.cr")

    source.should contain("@card.apply_status_class")
    workflow.should contain("@card.apply_status_class")
    source.should_not contain("xd-subagent")
  end

  it "maps agent reports onto shared running and terminal states" do
    running = Xd::UI::SubagentCard.presentation(
      "Running · Review the parser · Checking tests"
    )
    running.detail.should eq("Review the parser · Checking tests")
    running.status.should eq("Running")
    running.spinning.should be_true
    running.css_class.should eq("xd-workflow-running")

    completed = Xd::UI::SubagentCard.presentation("Completed · Done")
    completed.detail.should eq("Done")
    completed.spinning.should be_false
    completed.css_class.should eq("xd-workflow-success")

    legacy = Xd::UI::SubagentCard.presentation("Inspect the parser")
    legacy.detail.should eq("Inspect the parser")
    legacy.status.should eq("Delegated")
  end

  it "keeps tool groups detached until they are committed" do
    source = File.read("src/xd/ui/window.cr")
    append = source
      .split("      private def append_tool_line(summary : String)", 2)[1]
      .split("      private def take_tool_group", 2)[0]

    append.should contain("group.append(summary)")
    append.should_not contain("@transcript.append(group.widget)")
  end
end
