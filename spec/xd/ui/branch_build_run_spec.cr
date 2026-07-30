require "../../spec_helper"
require "../../../src/xd/ui/branch_build_run"

describe Xd::UI::BranchBuildRun do
  it "keeps a bounded failure tail" do
    target = Xd::UI::BranchBuild.parse("test").not_nil!
    run = Xd::UI::BranchBuildRun.new(
      checkout: "/tmp/unused",
      environment: ENV.to_h,
      command_builder: ->(_target : Xd::UI::BranchBuild::Target, _checkout : String) {
        "i=1; while [ $i -le 700 ]; do " \
        "printf 'line-%04d-xxxxxxxx\\n' \"$i\"; " \
        "i=$((i + 1)); done; exit 7"
      }
    )
    finished = Channel(Bool).new
    run.on_change = ->(installed : Bool) {
      finished.send(installed) unless run.running
    }

    run.start(target).should be_true
    finished.receive.should be_false
    run.trouble.should eq("branch test did not build.")
    run.tail.lines.size.should eq(8)
    run.tail.should contain("line-0700")
  end

  it "reports install completion" do
    target = Xd::UI::BranchBuild.parse("#12").not_nil!
    run = Xd::UI::BranchBuildRun.new(
      checkout: "/tmp/unused",
      environment: ENV.to_h,
      command_builder: ->(_target : Xd::UI::BranchBuild::Target, _checkout : String) { "printf 'done\\n'" }
    )
    installed = Channel(Nil).new
    finished = Channel(Bool).new
    run.on_installed = -> { installed.send(nil) }
    run.on_change = ->(success : Bool) {
      finished.send(success) unless run.running
    }

    run.start(target).should be_true
    installed.receive
    finished.receive.should be_true
    run.trouble.should eq(
      "Installed pull request #12. Restart to run it."
    )
    run.tail.should be_empty
  end

  it "stops a noisy build without blocking the UI scheduler" do
    target = Xd::UI::BranchBuild.parse("test").not_nil!
    run = Xd::UI::BranchBuildRun.new(
      checkout: "/tmp/unused",
      environment: ENV.to_h,
      command_builder: ->(_target : Xd::UI::BranchBuild::Target, _checkout : String) {
        "while :; do printf 'docker build output\\n'; done"
      }
    )
    finished = Channel(Nil).new(1)
    run.on_change = ->(_installed : Bool) {
      finished.send(nil) unless run.running
    }

    run.start(target).should be_true
    heartbeat = Channel(Nil).new(1)
    spawn { heartbeat.send(nil) }
    select
    when heartbeat.receive
    when timeout(250.milliseconds)
      fail("build output blocked the Crystal scheduler")
    end

    run.stop
    select
    when finished.receive
    when timeout(2.seconds)
      fail("stopped build did not finish")
    end
    run.running.should be_false
    run.trouble.should eq("Stopped.")
  ensure
    run.try(&.stop)
  end
end
