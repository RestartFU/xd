require "../../spec_helper"
require "../../../src/xd/ui/message_row"

describe Xd::UI::MessageKind do
  it "maps the same transcript roles as the C row" do
    Xd::UI::MessageKind.from_role("user")
      .should eq(Xd::UI::MessageKind::User)
    Xd::UI::MessageKind.from_role("assistant")
      .should eq(Xd::UI::MessageKind::Assistant)
    Xd::UI::MessageKind.from_role("tool")
      .should eq(Xd::UI::MessageKind::Tool)
    Xd::UI::MessageKind.from_role("event")
      .should eq(Xd::UI::MessageKind::Tool)
    Xd::UI::MessageKind.from_role("error")
      .should eq(Xd::UI::MessageKind::Error)
    Xd::UI::MessageKind.from_role("unknown")
      .should eq(Xd::UI::MessageKind::User)
  end

  it "only renders user messages as bubbles" do
    Xd::UI::MessageKind::User.bubble?.should be_true
    Xd::UI::MessageKind::Assistant.bubble?.should be_false
    Xd::UI::MessageKind::Tool.bubble?.should be_false
    Xd::UI::MessageKind::Error.bubble?.should be_false
  end
end
