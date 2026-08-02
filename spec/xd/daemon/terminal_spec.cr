require "../../spec_helper"
require "../../../src/xd/daemon/terminal"

private def with_terminal_environment(
  values : Hash(String, String?),
  &block
) : Nil
  saved = values.keys.to_h { |name| {name, ENV[name]?} }
  begin
    values.each do |name, value|
      if value
        ENV[name] = value
      else
        ENV.delete(name)
      end
    end
    yield
  ensure
    saved.each do |name, value|
      if value
        ENV[name] = value
      else
        ENV.delete(name)
      end
    end
  end
end

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

  it "yields while terminal input is backpressured" do
    output = IO::Memory.new
    ready = Channel(Nil).new(1)
    closed = Channel(Nil).new(1)
    ready_sent = false
    terminal = Xd::Daemon::Terminal.new(
      "chat",
      Dir.tempdir,
      on_output: ->(_terminal : Xd::Daemon::Terminal, data : Bytes) {
        output.write(data)
        if !ready_sent && output.to_s.includes?("INPUT_READY")
          ready_sent = true
          ready.send(nil)
        end
      },
      on_closed: ->(_terminal : Xd::Daemon::Terminal) {
        closed.send(nil)
      }
    )
    terminal.start
    terminal.write(
      "stty -echo; printf '\\nINPUT_READY\\n'; sleep 1; exit\n".to_slice
    )
    select
    when ready.receive
    when timeout(2.seconds)
      fail "terminal input did not execute; output=#{output.to_s.inspect}"
    end

    terminal.write(("x" * (512 * 1024) + "\n").to_slice)
    heartbeat = Channel(Time::Instant).new(1)
    started = Time.instant
    spawn do
      sleep 50.milliseconds
      heartbeat.send(Time.instant)
    end
    select
    when tick = heartbeat.receive
      (tick - started).should be < 250.milliseconds
    when timeout(250.milliseconds)
      fail "backpressured terminal input blocked the scheduler"
    end

    terminal.close
    select
    when closed.receive
    when timeout(3.seconds)
      fail "backpressured terminal did not close"
    end
  ensure
    terminal.try(&.close)
  end

  it "restores the host environment inside the shell" do
    output = IO::Memory.new
    closed = Channel(Nil).new
    terminal : Xd::Daemon::Terminal? = nil

    with_terminal_environment({
      "SHELL"                        => "/bin/sh",
      "GSETTINGS_SCHEMA_DIR"         => "/bundle/schemas",
      "XD_HOST_GSETTINGS_SCHEMA_DIR" => "/host/schemas",
      "OPENSSL_MODULES"              => "/bundle/modules",
      "XD_HOST_OPENSSL_MODULES"      => "",
    }) do
      terminal = Xd::Daemon::Terminal.new(
        "chat",
        Dir.tempdir,
        on_output: ->(_terminal : Xd::Daemon::Terminal, data : Bytes) {
          output.write(data)
        },
        on_closed: ->(_terminal : Xd::Daemon::Terminal) {
          closed.send(nil)
          nil
        }
      )
      terminal.not_nil!.start
      terminal.not_nil!.write(
        "printf '\\nSCHEMA=%s MODULES=%s\\n' " \
        "\"$GSETTINGS_SCHEMA_DIR\" \"${OPENSSL_MODULES-unset}\"; exit\n"
          .to_slice
      )

      select
      when closed.receive
      when timeout(5.seconds)
        fail "terminal did not close; output=#{output.to_s.inspect}"
      end
    end

    output.to_s.should contain("SCHEMA=/host/schemas MODULES=unset")
  ensure
    terminal.try(&.close)
  end
end
