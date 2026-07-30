require "../../spec_helper"
require "../../../src/xd/ui/diff_file_sections"

describe Xd::UI::DiffFileSections do
  it "caps each expanded file before GTK widget creation" do
    plan = Xd::UI::DiffFileSections.render_plan(1, 10_001)

    plan.finish.should eq(4_001)
    plan.omitted.should eq(6_000)
  end

  it "keeps small file sections complete" do
    plan = Xd::UI::DiffFileSections.render_plan(3, 103)

    plan.finish.should eq(103)
    plan.omitted.should eq(0)
  end
end
