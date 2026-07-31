require "../../spec_helper"
require "../../../src/xd/ui/idle_queue"

describe Xd::UI::IdleQueue do
  it "bounds each drain while preserving dynamically queued work" do
    queue = Xd::UI::IdleQueue(Int32).new
    queue << 1 << 2 << 3
    seen = [] of Int32

    count = queue.drain(2) do |item|
      seen << item
      queue << 4 if item == 1
    end

    count.should eq(2)
    seen.should eq([1, 2])
    queue.size.should eq(2)
    queue.drain(2) { |item| seen << item }.should eq(2)
    seen.should eq([1, 2, 3, 4])
    queue.empty?.should be_true
  end

  it "rejects an unbounded drain size" do
    queue = Xd::UI::IdleQueue(Int32).new
    expect_raises(ArgumentError, "limit must be positive") do
      queue.drain(0) { }
    end
  end
end
