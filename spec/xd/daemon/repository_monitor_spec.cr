require "../../spec_helper"
require "../../../src/xd/daemon/repository_monitor"

describe Xd::Daemon::RepositoryMonitor do
  it "publishes settled signature changes and resets its baseline" do
    signatures = {"chat" => "main:a"}
    sampled = Channel(String).new(8)
    changed = Channel(String).new(4)
    monitor = Xd::Daemon::RepositoryMonitor.new(
      ->(chat : String) {
        signature = signatures[chat]? || ""
        sampled.send(signature)
        signature
      },
      ->(chat : String) {
        changed.send(chat)
        nil
      },
      10.milliseconds
    )

    begin
      monitor.watch("chat")
      sampled.receive.should eq("main:a")
      select
      when chat = changed.receive
        fail "initial repository signature published for #{chat}"
      when timeout(20.milliseconds)
      end

      signatures["chat"] = "main:b"
      select
      when chat = changed.receive
        chat.should eq("chat")
      when timeout(1.second)
        fail "repository change did not arrive"
      end

      monitor.reset("chat")
      signatures["chat"] = "feature:c"
      loop do
        break if sampled.receive == "feature:c"
      end
      select
      when chat = changed.receive
        fail "reset repository signature published for #{chat}"
      when timeout(20.milliseconds)
      end
    ensure
      monitor.close
    end
  end
end
