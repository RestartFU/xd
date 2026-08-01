require "../../spec_helper"
require "../../../src/xd/ui/branch_build_run"

private def wait_for_build(run : Xd::UI::BranchBuildRun) : Nil
  20_000.times do
    return unless run.running
    Fiber.yield
  end
  raise "source build did not finish"
end

describe Xd::UI::BranchBuildRun do
  it "keeps only a bounded failure tail" do
    command = ->(_target : Xd::UI::BranchBuild::Target, _checkout : String) do
      "i=0; while [ $i -lt 700 ]; do echo line-$i; i=$((i + 1)); done; exit 1"
    end
    run = Xd::UI::BranchBuildRun.new(command_builder: command)
    target = Xd::UI::BranchBuild.parse("main").not_nil!

    run.start(target).should be_true
    wait_for_build(run)

    run.trouble.should eq("branch main did not build.")
    run.tail.bytesize.should be <= Xd::UI::BranchBuildRun::OUTPUT_LIMIT
    run.tail.should contain("line-699")
    run.tail.should_not contain("line-0\n")
  end

  it "reports a successful install" do
    installed = false
    command = ->(_target : Xd::UI::BranchBuild::Target, _checkout : String) { "echo installed" }
    run = Xd::UI::BranchBuildRun.new(command_builder: command)
    run.on_installed = -> { installed = true }

    run.start(Xd::UI::BranchBuild.parse("main").not_nil!).should be_true
    wait_for_build(run)

    installed.should be_true
    run.trouble.should eq("Installed branch main. Restart XD to run it.")
  end
end
