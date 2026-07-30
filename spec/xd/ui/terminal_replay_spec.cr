require "../../spec_helper"
require "../../../src/xd/ui/terminal_replay"

private def replay_data(encoded : String) : JSON::Any
  JSON::Any.new({"data" => JSON::Any.new(encoded)})
end

describe Xd::UI::TerminalReplay do
  it "decodes history in bounded batches" do
    payload = Bytes.new(8 * 1024, 65_u8)
    encoded = Base64.strict_encode(payload)
    replay = Xd::UI::TerminalReplay.new(
      Array.new(40) { replay_data(encoded) },
      80_i64,
      24_i64
    )

    first = replay.next_batch

    first.sum { |action| action.as(Bytes).size }
      .should be <= Xd::UI::TerminalReplay::BATCH_DECODED_BYTES
    replay.done?.should be_false
  end

  it "keeps live output and geometry behind existing replay" do
    replay = Xd::UI::TerminalReplay.new(
      [replay_data(Base64.strict_encode("history"))],
      80_i64,
      24_i64
    )
    replay.append_data(Base64.strict_encode("live"))
    replay.append_geometry(100_i64, 30_i64)

    actions = replay.next_batch

    String.new(actions[0].as(Bytes)).should eq("history")
    String.new(actions[1].as(Bytes)).should eq("live")
    actions[2].should eq(
      Xd::UI::TerminalReplayGeometry.new(100_i64, 30_i64)
    )
    replay.done?.should be_true
  end

  it "turns malformed and oversized data into a bounded notice" do
    oversized = "A" * (Xd::UI::TerminalReplay::MAX_ENCODED_ITEM + 1)
    replay = Xd::UI::TerminalReplay.new(
      [replay_data("%%%"), replay_data(oversized)],
      80_i64,
      24_i64
    )

    actions = replay.next_batch

    actions.size.should eq(2)
    actions.each do |action|
      String.new(action.as(Bytes)).should eq(
        Xd::UI::TerminalReplay::INVALID_REPLAY_NOTICE
      )
    end
  end
end
