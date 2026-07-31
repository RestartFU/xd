require "../../spec_helper"
require "../../../src/xd/ui/queue_presentation"

describe Xd::UI::QueuePresentation do
  it "bounds rows while preserving actionable queue indexes" do
    queue = 250.times.map { |index| JSON::Any.new("message #{index}") }.to_a
    plan = Xd::UI::QueuePresentation.prepare(queue)

    plan.rows.size.should eq(Xd::UI::QueuePresentation::MAX_ROWS)
    plan.rows.first.should eq("message 0")
    plan.rows.last.should eq("message 49")
    plan.hidden.should eq(200)
  end

  it "keeps a short queue complete" do
    plan = Xd::UI::QueuePresentation.prepare([
      JSON::Any.new("one"),
      JSON::Any.new("two"),
    ])

    plan.rows.should eq(["one", "two"])
    plan.hidden.should eq(0)
  end
end
