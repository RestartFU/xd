require "../../spec_helper"
require "../../../src/xd/ui/sidebar_state"

describe Xd::UI::SidebarState do
  it "tracks daemon work from tree snapshots" do
    state = Xd::UI::SidebarState::Idle.reconcile_tree(
      working: true,
      active: false,
      remote: false
    )
    state.should eq(Xd::UI::SidebarState::Working)

    state.reconcile_tree(
      working: false,
      active: false,
      remote: false
    ).should eq(Xd::UI::SidebarState::Done)

    state.reconcile_tree(
      working: false,
      active: false,
      remote: true
    ).should eq(Xd::UI::SidebarState::Idle)
  end

  it "marks inactive replies and keeps questions until answered" do
    Xd::UI::SidebarState::Working
      .finish(waiting: false, active: false)
      .should eq(Xd::UI::SidebarState::Done)
    Xd::UI::SidebarState::Working
      .finish(waiting: false, active: true)
      .should eq(Xd::UI::SidebarState::Idle)

    waiting = Xd::UI::SidebarState::Working.finish(
      waiting: true,
      active: true
    )
    waiting.opened.should eq(Xd::UI::SidebarState::Waiting)
    waiting.answered.should eq(Xd::UI::SidebarState::Idle)
  end

  it "acknowledges completed replies when opened" do
    Xd::UI::SidebarState::Done.opened
      .should eq(Xd::UI::SidebarState::Idle)
    Xd::UI::SidebarState::Working.opened
      .should eq(Xd::UI::SidebarState::Working)
  end
end
