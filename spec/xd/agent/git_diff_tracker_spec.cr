require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/git_diff_tracker"

private def tracker_git(workdir : String, *arguments : String) : String
  output = IO::Memory.new
  status = Process.run(
    "git",
    arguments,
    chdir: workdir,
    output: output,
    error: Process::Redirect::Close
  )
  status.success?.should be_true
  output.to_s
end

private class TestGitDiffTracker < Xd::Agent::GitDiffTracker
  def self.run_git(
    workdir : String,
    executable : String,
    timeout : Time::Span,
  ) : String?
    git(
      workdir,
      [] of String,
      timeout: timeout,
      executable: executable
    )
  end
end

describe Xd::Agent::GitDiffTracker do
  it "captures only changes since the previous tool event" do
    directory = File.join(
      Dir.tempdir,
      "xd-diff-tracker-#{Random::Secure.hex(12)}"
    )
    Dir.mkdir_p(directory)
    tracker_git(directory, "init", "-q", "-b", "main")
    tracker_git(directory, "config", "user.email", "test@example.com")
    tracker_git(directory, "config", "user.name", "Test")
    tracked = File.join(directory, "tracked.txt")
    staged = File.join(directory, "staged.txt")
    File.write(tracked, "initial\n")
    File.write(staged, "initial\n")
    tracker_git(directory, "add", ".")
    tracker_git(directory, "commit", "-q", "-m", "initial")

    begin
      File.write(tracked, "before\n")
      File.write(staged, "staged\n")
      tracker_git(directory, "add", "staged.txt")
      cached_before = tracker_git(directory, "diff", "--cached")
      tracker = Xd::Agent::GitDiffTracker.open(directory).not_nil!

      File.write(tracked, "after\n")
      File.write(File.join(directory, "new.txt"), "new\n")
      captured = tracker.capture("file_change  tracked.txt")
      patch = Xd::Agent::GitDiffTracker.patch(captured).not_nil!

      patch.should contain("-before")
      patch.should contain("+after")
      patch.should contain("new.txt")
      patch.should_not contain("-initial")
      tracker_git(directory, "diff", "--cached").should eq(cached_before)
      tracker.capture("file_change").should eq("file_change")
      tracker.capture("$ git status").should eq("$ git status")
    ensure
      FileUtils.rm_r(directory)
    end
  end

  it "bounds snapshot subprocess time and output" do
    directory = File.join(
      Dir.tempdir,
      "xd-diff-command-#{Random::Secure.hex(12)}"
    )
    Dir.mkdir_p(directory)
    slow = File.join(directory, "slow")
    noisy = File.join(directory, "noisy")
    File.write(slow, "#!/bin/sh\nsleep 5\n")
    File.write(
      noisy,
      "#!/bin/sh\nhead -c 1048576 /dev/zero | tr '\\0' x\n"
    )
    File.chmod(slow, 0o700)
    File.chmod(noisy, 0o700)

    begin
      started = Time.instant
      TestGitDiffTracker.run_git(
        directory,
        slow,
        50.milliseconds
      ).should be_nil
      (Time.instant - started).should be < 2.seconds

      output = TestGitDiffTracker.run_git(
        directory,
        noisy,
        2.seconds
      ).not_nil!
      output.bytesize.should eq(Xd::Agent::GitDiffTracker::OUTPUT_LIMIT)
    ensure
      FileUtils.rm_r(directory)
    end
  end
end
