require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/session"

private def with_session_engine(
  & : Xd::Daemon::Engine, Xd::Storage::Store ->
) : Nil
  path = File.join(
    Dir.tempdir,
    "xd-session-#{Random::Secure.hex(12)}",
    "chats.db"
  )
  store = Xd::Storage::Store.new(path)
  engine = Xd::Daemon::Engine.new(store)

  begin
    yield engine, store
  ensure
    engine.close
    store.close
    FileUtils.rm_r(Path[path].dirname)
  end
end

describe Xd::Daemon::Session do
  it "runs every local line through one persistent connection" do
    with_session_engine do |engine, _store|
      input = IO::Memory.new(%({"op":"ping"}) + "\n\n" + %({"op":"ping"}) + "\n")
      output = IO::Memory.new

      Xd::Daemon::Session.new(engine).run(
        input,
        output,
        Xd::Daemon::Transport::Local
      )

      output.to_s.lines.map { |line| JSON.parse(line) }.should eq([
        JSON.parse(%({"ok":true})),
        JSON.parse(%({"ok":true})),
      ])
    end
  end

  it "keeps remote authentication for later lines" do
    with_session_engine do |engine, _store|
      code = engine.arm_pairing(1.minute)
      input = IO::Memory.new(
        {
          "op"   => "pair",
          "code" => code,
          "name" => "test",
        }.to_json + "\n" + %({"op":"ping"}) + "\n"
      )
      output = IO::Memory.new

      Xd::Daemon::Session.new(engine).run(
        input,
        output,
        Xd::Daemon::Transport::Remote
      )

      responses = output.to_s.lines.map { |line| JSON.parse(line) }
      responses.size.should eq(2)
      responses.each(&.["ok"].as_bool.should(be_true))
    end
  end

  it "bounds remote frames before authentication" do
    with_session_engine do |engine, _store|
      oversized = "x" * (Xd::Protocol::AUTH_FRAME_LIMIT + 1)
      input = IO::Memory.new(oversized)
      output = IO::Memory.new

      Xd::Daemon::Session.new(engine).run(
        input,
        output,
        Xd::Daemon::Transport::Remote
      )

      input.pos.should eq(Xd::Protocol::AUTH_FRAME_LIMIT + 1)
      output.to_s.should be_empty
    end
  end
end
