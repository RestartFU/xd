require "../../spec_helper"
require "../../../src/xd/daemon/repository_monitor"

describe Xd::Daemon::RepositoryMonitor do
  it "publishes settled signature changes and resets its baseline" do
    signatures = {"chat" => "main:a"}
    changed = [] of String
    monitor = Xd::Daemon::RepositoryMonitor.new(
      ->(chat : String) { signatures[chat]? || "" },
      ->(chat : String) {
        changed << chat
        nil
      },
      10.milliseconds
    )

    begin
      monitor.watch("chat")
      sleep 30.milliseconds
      changed.should be_empty

      signatures["chat"] = "main:b"
      deadline = Time.instant + 1.second
      until changed == ["chat"]
        fail "repository change did not arrive" if Time.instant >= deadline
        sleep 5.milliseconds
      end

      monitor.reset("chat")
      signatures["chat"] = "feature:c"
      sleep 30.milliseconds
      changed.should eq(["chat"])
    ensure
      monitor.close
    end
  end
end
