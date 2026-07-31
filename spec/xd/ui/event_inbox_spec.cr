require "../../spec_helper"
require "../../../src/xd/ui/event_inbox"

private def ui_event(
  name : String,
  text : String? = nil,
  chat : String = "chat-1",
) : Hash(String, JSON::Any)
  event = {
    "event" => JSON::Any.new(name),
    "chat"  => JSON::Any.new(chat),
  }
  event["text"] = JSON::Any.new(text) if text
  event
end

private def voice_progress(
  request : String,
  progress : Int,
) : Hash(String, JSON::Any)
  {
    "event"    => JSON::Any.new("voice"),
    "request"  => JSON::Any.new(request),
    "state"    => JSON::Any.new("downloading"),
    "progress" => JSON::Any.new(progress.to_i64),
  }
end

describe Xd::UI::EventInbox do
  it "uses one scheduled drain and merges adjacent text deltas" do
    inbox = Xd::UI::EventInbox(String).new

    original = ui_event("text", "hel")
    original["turn_id"] = JSON::Any.new(7_i64)
    original["turn_sequence"] = JSON::Any.new(1_i64)
    continuation = ui_event("text", "lo")
    continuation["turn_id"] = JSON::Any.new(7_i64)
    continuation["turn_sequence"] = JSON::Any.new(2_i64)
    inbox.push("local", original).should be_true
    inbox.push("local", continuation).should be_false
    inbox.push("local", ui_event("tool", "done")).should be_false

    batch, more = inbox.drain
    more.should be_false
    batch.size.should eq(2)
    batch[0][1]["text"].as_s.should eq("hello")
    batch[0][1]["turn_id"].as_i64.should eq(7_i64)
    batch[0][1]["turn_sequence"].as_i64.should eq(2_i64)
    batch[0][1]["turn_parts"].as_a.map do |part|
      {part["sequence"].as_i64, part["text"].as_s}
    end.should eq([{1_i64, "hel"}, {2_i64, "lo"}])
    original["text"].as_s.should eq("hel")
    batch[1][1]["event"].as_s.should eq("tool")
  end

  it "preserves target, chat, and non-text ordering" do
    inbox = Xd::UI::EventInbox(String).new

    inbox.push("local", ui_event("text", "a"))
    inbox.push("remote", ui_event("text", "b"))
    inbox.push("remote", ui_event("text", "c", "chat-2"))
    inbox.push("remote", ui_event("text", "d", "chat-2"))

    batch, _more = inbox.drain
    batch.map(&.[0]).should eq(%w(local remote remote))
    batch.map { |item| item[1]["text"].as_s }
      .should eq(%w(a b cd))
  end

  it "caps text coalescing to avoid quadratic giant-string copies" do
    inbox = Xd::UI::EventInbox(String).new
    chunk = "x" * (16 * 1024 // 2 + 1)

    inbox.push("local", ui_event("text", chunk)).should be_true
    inbox.push("local", ui_event("text", chunk)).should be_false
    batch, more = inbox.drain

    more.should be_false
    batch.size.should eq(2)
    batch.each { |item| item[1]["text"].as_s.should eq(chunk) }
  end

  it "bounds each GTK batch and reschedules after becoming empty" do
    inbox = Xd::UI::EventInbox(String).new
    40.times do |index|
      inbox.push("local", ui_event("tool", index.to_s))
    end

    first, more = inbox.drain
    first.size.should eq(32)
    more.should be_true
    inbox.push("local", ui_event("tool", "after")).should be_false

    second, more = inbox.drain
    second.size.should eq(9)
    more.should be_false
    inbox.push("local", ui_event("tool", "new")).should be_true
  end

  it "keeps only the latest adjacent speech download progress" do
    inbox = Xd::UI::EventInbox(String).new

    inbox.push("local", voice_progress("voice-1", 1)).should be_true
    98.times do |offset|
      inbox.push("local", voice_progress("voice-1", offset + 2))
        .should be_false
    end
    inbox.push("local", voice_progress("voice-2", 20)).should be_false
    inbox.push("remote", voice_progress("voice-2", 30)).should be_false

    batch, more = inbox.drain
    more.should be_false
    batch.size.should eq(3)
    batch[0][1]["progress"].as_i.should eq(99)
    batch[1][1]["progress"].as_i.should eq(20)
    batch[2][1]["progress"].as_i.should eq(30)
  end
end
