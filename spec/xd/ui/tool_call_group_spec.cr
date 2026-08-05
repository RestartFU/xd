require "../../spec_helper"
require "../../../src/xd/ui/tool_call_group"

describe Xd::UI::ToolCallGroup do
  it "shows a lone tool call rather than collapsing it" do
    # "1 tool call" behind an arrow is less than the command itself, in the
    # same space.
    Xd::UI::ToolCallGroup.collapse?(1).should be_false
  end

  it "collapses a run of tool calls" do
    Xd::UI::ToolCallGroup.collapse?(2).should be_true
    Xd::UI::ToolCallGroup.collapse?(9).should be_true
  end

  it "counts the calls it hides" do
    Xd::UI::ToolCallGroup.collapsed_label(2).should eq("2 tool calls")
    Xd::UI::ToolCallGroup.collapsed_label(12).should eq("12 tool calls")
    # A subagent card can collapse a single call, so this still has to read.
    Xd::UI::ToolCallGroup.collapsed_label(1).should eq("1 tool call")
  end

  it "bounds expanded activity text" do
    summaries = Array.new(60) { |index| "tool-#{index}" }
    rendered = Xd::UI::ToolCallGroup.rendered_label(summaries)

    rendered.should contain("earlier tool calls")
    rendered.should contain("tool-59")
    rendered.size.should be <= Xd::UI::ToolCallGroup::MAX_RENDERED_CHARS + 64
  end

  it "bounds stored activity through long delegated runs" do
    buffer = Xd::UI::ToolCallGroup::ActivityBuffer.new
    10_000.times do |index|
      buffer.append("tool-#{index} #{"x" * 300}")
    end

    buffer.count.should eq(10_000)
    buffer.summaries.size.should be <= Xd::UI::ToolCallGroup::MAX_RENDERED_CALLS
    buffer.characters.should be <= Xd::UI::ToolCallGroup::MAX_RENDERED_CHARS
    buffer.summaries.last.should contain("tool-9999")
    Xd::UI::ToolCallGroup.rendered_label(
      buffer.summaries,
      buffer.count
    ).should contain("earlier tool calls")
  end

  it "merges repeated agent activity without unbounded storage" do
    current = Xd::UI::ToolCallGroup::ActivityBuffer.new
    update = Xd::UI::ToolCallGroup::ActivityBuffer.new
    500.times { |index| current.append("first-#{index}") }
    700.times { |index| update.append("update-#{index}") }

    current.merge(update)

    current.count.should eq(1_200)
    current.summaries.size.should be <= Xd::UI::ToolCallGroup::MAX_RENDERED_CALLS
    current.characters.should be <= Xd::UI::ToolCallGroup::MAX_RENDERED_CHARS
    current.summaries.last.should eq("update-699")
  end
end
