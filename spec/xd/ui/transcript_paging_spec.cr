require "../../spec_helper"
require "../../../src/xd/ui/transcript_paging"

describe Xd::UI::TranscriptPaging do
  it "requests one boundary row and displays fixed pages" do
    paging = Xd::UI::TranscriptPaging.new

    paging.query_limit.should eq(101)
    paging.start(101).should eq(1)
    paging.displayed(101).should eq(100)
    paging.hidden(245, 101).should eq(145)
    paging.earlier_label(245, 101).should eq(
      "Load 100 earlier messages"
    )

    paging.load_earlier.should eq(200)
    paging.query_limit.should eq(201)
    paging.start(201).should eq(1)
    paging.hidden(245, 201).should eq(45)
    paging.earlier_label(245, 201).should eq(
      "Load 45 earlier messages"
    )
  end

  it "extends a raw page until the assistant turn has its prompt" do
    paging = Xd::UI::TranscriptPaging.new

    paging.extend_to_turn_start(205, 101, "tool").should be_true
    paging.limit.should eq(200)
    paging.extend_to_turn_start(205, 201, "assistant").should be_true
    paging.limit.should eq(300)
    paging.extend_to_turn_start(205, 205, "user").should be_false
  end

  it "keeps a page that already starts on a user message" do
    paging = Xd::UI::TranscriptPaging.new

    paging.extend_to_turn_start(205, 101, "user").should be_false
    paging.limit.should eq(100)
  end

  it "does not extend when all available history is already fetched" do
    paging = Xd::UI::TranscriptPaging.new

    paging.extend_to_turn_start(80, 80, "tool").should be_false
    paging.limit.should eq(100)
  end

  it "uses singular copy and saturates its protocol limit" do
    paging = Xd::UI::TranscriptPaging.new(Int32::MAX - 50)

    paging.earlier_label(2, 1).should eq("Load 1 earlier message")
    paging.load_earlier.should eq(Int32::MAX)
    paging.query_limit.should eq(Int32::MAX)
  end
end

describe Xd::UI::TranscriptBatch do
  it "keeps GTK work bounded while preserving source positions" do
    batch = Xd::UI::TranscriptBatch(Int32).new(
      (0...11).to_a,
      start: 2,
      batch_size: 4
    )

    first = batch.next_batch
    first.map(&.[0]).should eq([2, 3, 4, 5])
    first.map(&.[1]).should eq([2, 3, 4, 5])
    batch.done?.should be_false

    batch.next_batch.map(&.[0]).should eq([6, 7, 8, 9])
    batch.next_batch.map(&.[0]).should eq([10])
    batch.done?.should be_true
    batch.next_batch.should be_empty
  end

  it "validates bounds and clamps its starting position" do
    expect_raises(ArgumentError, "batch size must be positive") do
      Xd::UI::TranscriptBatch(Int32).new([1], batch_size: 0)
    end

    before = Xd::UI::TranscriptBatch(Int32).new([1, 2], start: -5)
    before.next_batch.map(&.[0]).should eq([0, 1])

    after = Xd::UI::TranscriptBatch(Int32).new([1, 2], start: 50)
    after.done?.should be_true
    after.next_batch.should be_empty
  end
end

describe Xd::UI::TranscriptLru do
  it "keeps the four most recently touched chats" do
    lru = Xd::UI::TranscriptLru.new

    %w(one two three four).each do |key|
      lru.touch(key).should be_nil
    end
    lru.touch("one").should be_nil
    lru.touch("five").should eq("two")
    lru.keys.should eq(%w(three four one five))

    lru.delete("four")
    lru.keys.should eq(%w(three one five))
  end
end
