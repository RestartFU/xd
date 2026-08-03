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
