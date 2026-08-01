require "../../spec_helper"
require "../../../src/xd/ui/diff_text"

describe Xd::UI::DiffText do
  it "keeps label allocation offset in background coordinates" do
    Xd::UI::DiffText.line_y(7.5_f32, 3, 2 * 1024)
      .should eq(12.5_f32)
  end
end
