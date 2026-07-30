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

describe Xd::UI::EventInbox do
  it "uses one scheduled drain and merges adjacent text deltas" do
    inbox = Xd::UI::EventInbox(String).new

    original = ui_event("text", "hel")
    inbox.push("local", original).should be_true
    inbox.push("local", ui_event("text", "lo")).should be_false
    inbox.push("local", ui_event("tool", "done")).should be_false

    batch, more = inbox.drain
    more.should be_false
    batch.size.should eq(2)
    batch[0][1]["text"].as_s.should eq("hello")
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
end
