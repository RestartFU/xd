require "../../spec_helper"
require "../../../src/xd/ui/tool_panel"

private def panel_event(
  name : String,
  text : String? = nil,
  success : Bool? = nil,
) : Hash(String, JSON::Any)
  event = {"event" => JSON::Any.new(name)}
  event["text"] = JSON::Any.new(text) if text
  event["success"] = JSON::Any.new(success) unless success.nil?
  event
end

describe Xd::UI::ToolPanel do
  it "refreshes repository panes for file and turn changes" do
    Xd::UI::ToolPanel.repository_changed?(
      panel_event("tool", "file_change\ndiff --git a/a b/a")
    ).should be_true
    Xd::UI::ToolPanel.repository_changed?(
      panel_event("turn-finished")
    ).should be_true
    Xd::UI::ToolPanel.repository_changed?(
      panel_event("repository-changed")
    ).should be_true
    Xd::UI::ToolPanel.repository_changed?(
      panel_event("git-action-finished", success: true)
    ).should be_true
  end

  it "ignores unrelated and failed tool events" do
    Xd::UI::ToolPanel.repository_changed?(
      panel_event("tool", "$ pwd")
    ).should be_false
    Xd::UI::ToolPanel.repository_changed?(
      panel_event("git-action-finished", success: false)
    ).should be_false
  end
end
