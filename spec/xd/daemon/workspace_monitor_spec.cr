require "../../spec_helper"
require "../../../src/xd/daemon/workspace_monitor"

describe Xd::Daemon::WorkspaceMonitor do
  it "publishes external changes and adopts acknowledged mutations" do
    signature = "initial"
    changed = Channel(Nil).new(2)
    monitor = Xd::Daemon::WorkspaceMonitor.new(
      -> { signature },
      -> {
        changed.send(nil)
        nil
      },
      10.milliseconds
    )

    begin
      monitor.acknowledge
      signature = "internal"
      monitor.acknowledge
      select
      when changed.receive
        fail "acknowledged workspace mutation was published again"
      when timeout(30.milliseconds)
      end

      signature = "external"
      select
      when changed.receive
      when timeout(1.second)
        fail "external workspace change was not published"
      end
    ensure
      monitor.close
    end
  end
end
