require "../../spec_helper"
require "../../../src/xd/ui/text_reveal"

describe Xd::UI::TextReveal do
  it "starts late and then catches up" do
    reveal = Xd::UI::TextReveal.new
    reveal.note_append(1_000_i64)

    frame = reveal.advance("abcdefghij", 50_000_i64)
    frame.shown.should eq(0)
    frame.settled.should be_false

    frame = reveal.advance("abcdefghij", 90_000_i64)
    frame.shown.should be > 0
    frame.settled.should be_false

    until frame.settled
      frame = reveal.advance("abcdefghij", 150_000_i64)
    end
    reveal.shown.should eq(10)
  end

  it "holds the live tail until input becomes quiet" do
    reveal = Xd::UI::TextReveal.new
    reveal.note_append(1_000_i64)
    frame = reveal.advance("abcdef", 90_000_i64)
    9.times { frame = reveal.advance("abcdef", 90_000_i64) }

    frame.shown.should eq(4)
    frame.settled.should be_false

    frame = reveal.advance("abcdef", 120_000_i64)
    frame.shown.should eq(6)
    frame.settled.should be_true
  end

  it "keeps UTF-8 prefixes whole" do
    Xd::UI::TextReveal.prefix("a🦀é", 2).should eq("a🦀")
  end

  it "resumes when more text arrives" do
    reveal = Xd::UI::TextReveal.new
    reveal.note_append(1_000_i64)
    frame = reveal.advance("done", 120_000_i64)
    until frame.settled
      frame = reveal.advance("done", 120_000_i64)
    end

    reveal.note_append(130_000_i64)
    frame = reveal.advance("done next", 140_000_i64)
    frame.shown.should be > 4
    frame.shown.should be < 9
    frame.settled.should be_false
  end

  it "can synchronize and reset cached live text" do
    reveal = Xd::UI::TextReveal.new
    reveal.sync("a🦀é")
    reveal.shown.should eq(3)
    reveal.reset
    reveal.shown.should eq(0)
  end
end
