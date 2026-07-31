require "../../spec_helper"
require "../../../src/xd/ui/turn_timing"

describe Xd::UI::TurnTiming do
  it "matches C elapsed labels at every boundary" do
    timing = Xd::UI::TurnTiming

    timing.format("Working", 0_i64).should eq("Working for 0s")
    timing.format("Worked", 59_i64).should eq("Worked for 59s")
    timing.format("Worked", 60_i64).should eq("Worked for 1m 00s")
    timing.format("Worked", 3599_i64).should eq("Worked for 59m 59s")
    timing.format("Worked", 3600_i64).should eq("Worked for 1h 00m")
    timing.format("Worked", 7380_i64).should eq("Worked for 2h 03m")
  end
end
