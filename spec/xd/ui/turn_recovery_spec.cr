require "../../spec_helper"
require "../../../src/xd/ui/turn_recovery"

private def recovery_event(
  name : String,
  turn : Int64? = nil,
  sequence : Int64? = nil,
) : Hash(String, JSON::Any)
  event = {"event" => JSON::Any.new(name)}
  event["turn_id"] = JSON::Any.new(turn) if turn
  event["turn_sequence"] = JSON::Any.new(sequence) if sequence
  event
end

describe Xd::UI::TurnRecovery do
  it "drops same-turn events already represented by the snapshot" do
    Xd::UI::TurnRecovery.replay?(
      recovery_event("turn-started", 7, 0),
      7,
      4
    ).should be_false
    Xd::UI::TurnRecovery.replay?(
      recovery_event("text", 7, 4),
      7,
      4
    ).should be_false
    Xd::UI::TurnRecovery.replay?(
      recovery_event("tool", 7, 3),
      7,
      4
    ).should be_false
  end

  it "replays later, finishing, next-turn, and legacy events" do
    Xd::UI::TurnRecovery.replay?(
      recovery_event("text", 7, 5),
      7,
      4
    ).should be_true
    Xd::UI::TurnRecovery.replay?(
      recovery_event("turn-finished", 7, 4),
      7,
      4
    ).should be_true
    Xd::UI::TurnRecovery.replay?(
      recovery_event("turn-started", 8, 0),
      7,
      4
    ).should be_true
    Xd::UI::TurnRecovery.replay?(
      recovery_event("text"),
      7,
      4
    ).should be_true
  end

  it "trims the covered prefix from coalesced text deltas" do
    event = recovery_event("text", 7, 6)
    event["text"] = JSON::Any.new("abcdef")
    event["turn_parts"] = JSON::Any.new([
      JSON::Any.new({
        "sequence" => JSON::Any.new(4_i64),
        "text"     => JSON::Any.new("ab"),
      }),
      JSON::Any.new({
        "sequence" => JSON::Any.new(5_i64),
        "text"     => JSON::Any.new("cd"),
      }),
      JSON::Any.new({
        "sequence" => JSON::Any.new(6_i64),
        "text"     => JSON::Any.new("ef"),
      }),
    ])

    replay = Xd::UI::TurnRecovery.replay(event, 7, 4).not_nil!
    replay["text"].as_s.should eq("cdef")
    Xd::UI::TurnRecovery.replay(event, 7, 6).should be_nil
    event["text"].as_s.should eq("abcdef")
  end
end
