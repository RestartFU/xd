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

  it "reads the same without a verb, and never counts below zero" do
    timing = Xd::UI::TurnTiming

    timing.duration(0_i64).should eq("0s")
    timing.duration(59_i64).should eq("59s")
    timing.duration(60_i64).should eq("1m 00s")
    timing.duration(3599_i64).should eq("59m 59s")
    timing.duration(3600_i64).should eq("1h 00m")
    timing.duration(-3_i64).should eq("0s")
    timing.format("Working", -3_i64).should eq("Working for 0s")
  end
end
