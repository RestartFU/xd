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
