require "../../spec_helper"
require "../../../src/xd/daemon/execution_host"

describe Xd::Daemon::ExecutionHost do
  it "runs blocking daemon work outside the default scheduler" do
    host = Xd::Daemon::ExecutionHost.new("xd execution host spec")
    started = Channel(Nil).new
    finished = Channel(Nil).new
    release = Atomic(Bool).new(false)

    host.spawn do
      started.send(nil)
      until release.get
      end
      finished.send(nil)
    end

    started.receive
    release.set(true)
    finished.receive
  end
end
