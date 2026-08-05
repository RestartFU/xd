require "../../spec_helper"

describe "desktop subagent card activity" do
  it "transfers bounded activity data without nesting GTK groups" do
    source = File.read("src/xd/ui/subagent_card.cr")

    source.should contain("@activity.merge(activity.take_activity)")
    source.should_not contain("Gtk::Expander.new")
    source.should_not contain("bind_property")
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
