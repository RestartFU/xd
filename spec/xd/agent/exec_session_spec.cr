require "../../spec_helper"
require "../../../src/xd/agent/exec_session"

private def await_finish(
  channel : Channel(Tuple(Bool, String?)),
) : Tuple(Bool, String?)
  select
  when result = channel.receive
    result
  when timeout(5.seconds)
    raise "session did not finish"
  end
end

describe Xd::Agent::ExecSession do
  it "owns a real CLI process and streams its events" do
    events = [] of Xd::Agent::Event
    finished = Channel(Tuple(Bool, String?)).new(1)
    fixture = File.expand_path(
      File.join("tests", "fixtures", "claude-stream.jsonl")
    )
    spec = Xd::Agent::RunSpec.new("unused")
    session = Xd::Agent::ExecSession.new(
      Xd::Agent::Catalog::CLAUDE,
      spec,
      ENV.to_h,
      ->(event : Xd::Agent::Event) { events << event },
      ->(ok : Bool, message : String?) { finished.send({ok, message}) },
      arguments: ["/bin/cat", fixture]
    )

    session.start
    await_finish(finished).should eq({true, nil})
    events.select(&.type.text_delta?)
      .compact_map(&.text)
      .join
      .should eq("hello from hy")
    events.count(&.type.result?).should eq(1)
    session.running?.should be_false
  end

  it "returns bounded stderr when a CLI exits unsuccessfully" do
    finished = Channel(Tuple(Bool, String?)).new(1)
    session = Xd::Agent::ExecSession.new(
      Xd::Agent::Catalog::CLAUDE,
      Xd::Agent::RunSpec.new("unused"),
      ENV.to_h,
      ->(_event : Xd::Agent::Event) { },
      ->(ok : Bool, message : String?) { finished.send({ok, message}) },
      arguments: [
        "/bin/sh",
        "-c",
        "printf 'decisive failure\\n' >&2; exit 7",
      ]
    )

    session.start
    result = await_finish(finished)
    result[0].should be_false
    result[1].should eq("decisive failure")
  end

  it "treats cancellation as a normal finish" do
    finished = Channel(Tuple(Bool, String?)).new(1)
    session = Xd::Agent::ExecSession.new(
      Xd::Agent::Catalog::CLAUDE,
      Xd::Agent::RunSpec.new("unused"),
      ENV.to_h,
      ->(_event : Xd::Agent::Event) { },
      ->(ok : Bool, message : String?) { finished.send({ok, message}) },
      arguments: [
        "/bin/sh",
        "-c",
        "trap 'exit 0' INT; while :; do sleep 1; done",
      ]
    )

    session.start
    session.cancel
    await_finish(finished).should eq({true, nil})
  end
end
