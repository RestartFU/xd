require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/codex_app_server"

private def await_codex_finish(
  channel : Channel(Tuple(Bool, String?)),
) : Tuple(Bool, String?)
  select
  when result = channel.receive
    result
  when timeout(5.seconds)
    raise "Codex app-server did not finish"
  end
end

private def fake_codex_script(
  directory : String,
  body : String,
) : String
  path = File.join(directory, "codex")
  File.write(path, "#!/bin/sh\nset -eu\n#{body}")
  File.chmod(path, 0o700)
  path
end

describe Xd::Agent::CodexAppServer do
  it "owns a persistent process and forwards one real turn" do
    directory = File.join(
      Dir.tempdir,
      "xd-codex-server-#{Random::Secure.hex(12)}"
    )
    Dir.mkdir_p(directory)
    script = fake_codex_script(directory, <<-SH)
      IFS= read -r initialize
      printf '%s\\n' '{"id":1,"result":{}}'
      IFS= read -r initialized
      IFS= read -r open_thread
      printf '%s\\n' '{"id":2,"result":{"thread":{"id":"thread-1"}}}'
      IFS= read -r start_turn
      printf '%s\\n' '{"id":3,"result":{"turn":{"id":"turn-1"}}}'
      printf '%s\\n' '{"method":"item/agentMessage/delta","params":{"threadId":"thread-1","itemId":"message-1","delta":"from codex"}}'
      printf '%s\\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"thread-1","tokenUsage":{"last":{"totalTokens":99},"modelContextWindow":272000}}}'
      printf '%s\\n' '{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"status":"completed"}}}'
      SH
    events = [] of Xd::Agent::Event
    finished = Channel(Tuple(Bool, String?)).new(1)
    pool = Xd::Agent::CodexPool.new(version: "spec")
    on_event = ->(event : Xd::Agent::Event) { events << event }
    on_finished = ->(ok : Bool, message : String?) do
      finished.send({ok, message})
    end

    begin
      pool.start(
        Xd::Agent::RunSpec.new("hello"),
        ENV.to_h,
        [] of String,
        on_event,
        on_finished,
        arguments: [script]
      )

      await_codex_finish(finished).should eq({true, nil})
      events.select(&.type.text_delta?)
        .compact_map(&.text)
        .join
        .should eq("from codex")
      usage = events.find(&.type.usage?).not_nil!
      usage.context_used.should eq(99_u64)
      usage.context_window.should eq(272_000_u64)
      events.count(&.type.result?).should eq(1)
    ensure
      pool.close
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "reports bounded process stderr to every waiting turn" do
    directory = File.join(
      Dir.tempdir,
      "xd-codex-failure-#{Random::Secure.hex(12)}"
    )
    Dir.mkdir_p(directory)
    script = fake_codex_script(directory, <<-SH)
      IFS= read -r initialize
      printf '%s\\n' 'decisive app-server failure' >&2
      exit 9
      SH
    finished = Channel(Tuple(Bool, String?)).new(1)
    pool = Xd::Agent::CodexPool.new(version: "spec")
    on_event = ->(_event : Xd::Agent::Event) { }
    on_finished = ->(ok : Bool, message : String?) do
      finished.send({ok, message})
    end

    begin
      pool.start(
        Xd::Agent::RunSpec.new("hello"),
        ENV.to_h,
        [] of String,
        on_event,
        on_finished,
        arguments: [script]
      )

      result = await_codex_finish(finished)
      result[0].should be_false
      result[1].should eq("decisive app-server failure")
    ensure
      pool.close
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
