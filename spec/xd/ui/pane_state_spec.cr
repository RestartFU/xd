require "../../spec_helper"
require "../../../src/xd/ui/pane_state"

describe Xd::UI::PaneState do
  it "updates one a{su} entry without changing other chats" do
    states = GLib::Variant.parse(
      "@a{su} {'local/first': 1, 'remote/host:4001/second': 4}"
    )

    updated = Xd::UI::PaneState.update(
      states,
      "local/first",
      Xd::UI::PaneState::Files
    )

    updated.type_string.should eq("a{su}")
    Xd::UI::PaneState.fetch(
      updated,
      "local/first"
    ).should eq(Xd::UI::PaneState::Files)
    Xd::UI::PaneState.fetch(
      updated,
      "remote/host:4001/second"
    ).should eq(Xd::UI::PaneState::Diff)
  end

  it "adds new entries and returns a fallback for missing chats" do
    states = GLib::Variant.parse("@a{su} {}")
    combined = Xd::UI::PaneState::Terminal |
               Xd::UI::PaneState::Diff
    updated = Xd::UI::PaneState.update(states, "local/chat", combined)

    Xd::UI::PaneState.fetch(updated, "local/chat").should eq(combined)
    Xd::UI::PaneState.fetch(
      updated,
      "missing",
      Xd::UI::PaneState::Files
    ).should eq(Xd::UI::PaneState::Files)
  end
end
