require "../../spec_helper"
require "../../../src/xd/ui/dots"

describe Xd::UI::Dots do
  it "lights one more fixed dot on each frame" do
    Xd::UI::Dots.opacity(0, 0).should eq(0.3)
    Xd::UI::Dots.opacity(1, 0).should eq(1.0)
    Xd::UI::Dots.opacity(1, 1).should eq(0.3)
    Xd::UI::Dots.opacity(3, 2).should eq(1.0)
  end
end
