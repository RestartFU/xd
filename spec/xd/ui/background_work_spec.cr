require "../../spec_helper"
require "../../../src/xd/ui/background_work"

describe Xd::UI::BackgroundWork do
  it "runs jobs inside a scheduler-backed execution context" do
    finished = Channel(String).new(1)

    Xd::UI::BackgroundWork.submit do
      Fiber.yield
      finished.send(Thread.current.execution_context.class.name)
    end.should be_true

    select
    when context = finished.receive
      context.should eq("Fiber::ExecutionContext::Parallel")
    when timeout(2.seconds)
      fail("background UI job had no execution context")
    end
  end
end
