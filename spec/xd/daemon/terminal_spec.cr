require "../../spec_helper"
require "../../../src/xd/daemon/terminal"

describe Xd::Daemon::Terminal do
  it "streams shell output and keeps replay geometry" do
    output = IO::Memory.new
    closed = Channel(Nil).new
    terminal = Xd::Daemon::Terminal.new(
      "chat",
      Dir.tempdir,
      100,
      30,
      ->(_terminal : Xd::Daemon::Terminal, data : Bytes) {
        output.write(data)
      },
      ->(_terminal : Xd::Daemon::Terminal) {
        closed.send(nil)
        nil
      }
    )
    terminal.start

    terminal.write("printf '\\nCRYSTAL_PTY_OK\\n'; exit\n".to_slice)
    select
    when closed.receive
    when timeout(5.seconds)
      fail "terminal did not close; output=#{output.to_s.inspect}"
    end

    output.to_s.should contain("CRYSTAL_PTY_OK")
    replay = terminal.replay_json
    replay.first["columns"].as_i.should eq(100)
    replay.first["rows"].as_i.should eq(30)
    replay.compact_map { |item| item["data"]?.try(&.as_s?) }
      .join
      .should_not be_empty
  ensure
    terminal.try(&.close)
  end

  it "records resize boundaries" do
    terminal = Xd::Daemon::Terminal.new("chat", Dir.tempdir)
    terminal.start
    terminal.resize(120, 40).should eq({120, 40})

    geometry = terminal.replay_json.select(&.["columns"]?)
    geometry.size.should eq(2)
    geometry.last["columns"].as_i.should eq(120)
    geometry.last["rows"].as_i.should eq(40)
  ensure
    terminal.try(&.close)
  end
end
