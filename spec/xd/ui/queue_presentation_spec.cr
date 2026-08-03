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

  it "uses queued events as incremental queue snapshots" do
    queue = [JSON::Any.new("next")]
    event = {
      "event" => JSON::Any.new("queued"),
      "queue" => JSON::Any.new(queue),
    }

    Xd::UI::QueuePresentation.event_queue(event).should eq(queue)
    Xd::UI::QueuePresentation.event_queue({
      "event" => JSON::Any.new("queued"),
    }).should be_nil
    Xd::UI::QueuePresentation.event_queue({
      "queue" => JSON::Any.new("invalid"),
    }).should be_nil
  end

  it "reloads the transcript only when a send starts a turn" do
    queued = {"queued" => JSON::Any.new(true)}
    started = {"queued" => JSON::Any.new(false)}

    Xd::UI::QueuePresentation.reload_after_send?(queued).should be_false
    Xd::UI::QueuePresentation.reload_after_send?(started).should be_true
    Xd::UI::QueuePresentation.reload_after_send?(
      {} of String => JSON::Any
    ).should be_true
  end
end
