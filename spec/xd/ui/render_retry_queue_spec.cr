require "../../spec_helper"
require "../../../src/xd/ui/render_retry_queue"

describe Xd::UI::RenderRetryQueue do
  it "retries one saturated item without running later work" do
    retries = Xd::UI::RenderRetryQueue.new
    available = false
    calls = [] of String
    retries.push do
      calls << "first"
      available
    end
    retries.push do
      calls << "second"
      true
    end

    retries.drain.should be_true
    retries.size.should eq(2)
    calls.should eq(["first"])

    available = true
    retries.drain.should be_false
    retries.size.should eq(0)
    calls.should eq(["first", "first", "second"])
  end
end

describe Xd::UI::FrameRenderQueue do
  it "limits one drain and rotates unfinished rows" do
    renders = Xd::UI::FrameRenderQueue.new
    calls = [] of String
    first_steps = 0
    renders.push do
      calls << "first"
      first_steps += 1
      first_steps < 4
    end
    renders.push do
      calls << "second"
      false
    end

    renders.drain(2).should be_true
    calls.should eq(["first", "second"])
    renders.size.should eq(1)

    renders.drain(2).should be_true
    calls.should eq(["first", "second", "first", "first"])
    renders.drain(1).should be_false
    renders.size.should eq(0)
  end

  it "rejects a frame with no work budget" do
    renders = Xd::UI::FrameRenderQueue.new
    expect_raises(ArgumentError, "limit must be positive") do
      renders.drain(0)
    end
  end
end
