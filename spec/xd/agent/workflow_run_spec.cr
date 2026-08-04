require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/workflow_run"

describe Xd::Agent::WorkflowRun do
  it "captures quoted and chained gh run commands" do
    message = "$ git push && gh run watch '123456' --repo=RestartFU/xd"
    stored = Xd::Agent::WorkflowRun.capture(message, "/missing")
    run = Xd::Agent::WorkflowRun.parse(stored).not_nil!
    run.id.should eq("123456")
    run.repository.should eq("RestartFU/xd")
    run.url.should eq(
      "https://github.com/RestartFU/xd/actions/runs/123456"
    )
  end

  it "discovers GitHub repository from the workdir remote" do
    directory = File.join(
      Dir.tempdir,
      "xd-workflow-run-#{Random::Secure.hex(12)}"
    )
    Dir.mkdir_p(directory)
    Process.run("git", ["init", "-q"], chdir: directory)
    Process.run(
      "git",
      ["remote", "add", "origin", "git@github.com:owner/repository.git"],
      chdir: directory
    )

    begin
      stored = Xd::Agent::WorkflowRun.capture(
        "$ gh run view 98765",
        directory
      )
      Xd::Agent::WorkflowRun.parse(stored).not_nil!.repository
        .should eq("owner/repository")
    ensure
      FileUtils.rm_r(directory)
    end
  end

  it "rejects unsafe repositories, run ids, and malformed records" do
    Xd::Agent::WorkflowRun.capture(
      "$ gh run watch nope -R owner/repository",
      "/missing"
    ).should eq("$ gh run watch nope -R owner/repository")
    Xd::Agent::WorkflowRun.capture(
      "$ gh run watch 12 -R owner/repo/extra",
      "/missing"
    ).should eq("$ gh run watch 12 -R owner/repo/extra")
    Xd::Agent::WorkflowRun.parse(
      "workflow_run\n12\nhttps://example.com/owner/repo/actions/runs/12"
    ).should be_nil
  end

  it "turns live GitHub replies into display-ready status" do
    running = Xd::Agent::WorkflowRun.parse_status(
      %({"name":"nightly","status":"in_progress","conclusion":null})
    ).not_nil!
    running.label.should eq("nightly")
    running.terminal?.should be_false
    running.css_class.should eq("xd-workflow-running")

    passed = Xd::Agent::WorkflowRun.parse_status(
      %({"name":"nightly","status":"completed","conclusion":"success"})
    ).not_nil!
    passed.label.should eq("nightly · Passed")
    passed.terminal?.should be_true
    passed.css_class.should eq("xd-workflow-success")
  end

  it "parses individual workflow jobs" do
    jobs = Xd::Agent::WorkflowRun.parse_jobs(
      %({"jobs":[
        {"id":101,"name":"linux","status":"in_progress","conclusion":null,
         "steps":[{"name":"Build Linux","status":"in_progress","conclusion":null}]},
        {"id":102,"name":"macos","status":"completed","conclusion":"success",
         "steps":[{"name":"Publish","status":"completed","conclusion":"success"}]}
      ]})
    ).not_nil!
    jobs.size.should eq(2)
    jobs[0].name.should eq("linux")
    jobs[0].log.should eq("Build Linux")
    jobs[0].terminal?.should be_false
    jobs[1].log.should eq("Publish")
    jobs[1].label.should eq("Passed")
    jobs[1].css_class.should eq("xd-workflow-success")
  end

  it "caches gh credentials and injects them into every CLI fallback" do
    directory = File.join(
      Dir.tempdir,
      "xd-workflow-gh-#{Random::Secure.hex(12)}"
    )
    executable = File.join(directory, "gh")
    arguments = File.join(directory, "arguments")
    auth_calls = File.join(directory, "auth-calls")
    Dir.mkdir_p(directory)
    File.write(executable, <<-'SH')
      #!/bin/sh
      set -eu
      if [ "$1" = auth ] && [ "$2" = token ]; then
        printf 'call\n' >> "$XD_GH_AUTH_CALLS"
        printf '%s\n' 'workflow-test-token'
        exit 0
      fi
      test "${GH_TOKEN:-}" = 'workflow-test-token'
      printf '%s\n' "$@" >> "$XD_GH_ARGUMENTS"
      printf '%s\n' '{"name":"nightly","status":"in_progress","conclusion":null,"startedAt":"2026-08-03T10:00:00Z","updatedAt":"2026-08-03T10:03:00Z","jobs":[{"databaseId":101,"name":"linux","status":"in_progress","conclusion":null,"startedAt":"2026-08-03T10:00:05Z","completedAt":"0001-01-01T00:00:00Z","steps":[{"name":"Publish","status":"in_progress","conclusion":null}]}]}'
      SH
    File.chmod(executable, 0o700)

    old_executable = ENV["XD_GH_EXECUTABLE"]?
    old_arguments = ENV["XD_GH_ARGUMENTS"]?
    old_auth_calls = ENV["XD_GH_AUTH_CALLS"]?
    old_gh_token = ENV["GH_TOKEN"]?
    old_github_token = ENV["GITHUB_TOKEN"]?
    begin
      ENV["XD_GH_EXECUTABLE"] = executable
      ENV["XD_GH_ARGUMENTS"] = arguments
      ENV["XD_GH_AUTH_CALLS"] = auth_calls
      ENV.delete("GH_TOKEN")
      ENV.delete("GITHUB_TOKEN")
      run = Xd::Agent::WorkflowRun::Run.new(
        "123%",
        "owner/repo%",
        ""
      )

      cache = Xd::Agent::WorkflowRun::StatusCache.new(active_ttl: 0.seconds)
      status = cache.fetch(run)
      status.label.should eq("nightly")
      status.jobs.first.id.should eq("101")
      status.jobs.first.log.should eq("Publish")
      status.terminal?.should be_false
      status.jobs.first.terminal?.should be_false
      cache.fetch(run).should eq(status)
      File.read(auth_calls).lines.size.should eq(1)
      File.read(arguments).lines.map(&.chomp).should eq([
        "run",
        "view",
        "123%",
        "--repo",
        "owner/repo%",
        "--json",
        "name,status,conclusion,startedAt,updatedAt,jobs",
        "run",
        "view",
        "123%",
        "--repo",
        "owner/repo%",
        "--json",
        "name,status,conclusion,startedAt,updatedAt,jobs",
      ])
      File.read(arguments).should_not contain("workflow-test-token")
    ensure
      if old_executable
        ENV["XD_GH_EXECUTABLE"] = old_executable
      else
        ENV.delete("XD_GH_EXECUTABLE")
      end
      if old_arguments
        ENV["XD_GH_ARGUMENTS"] = old_arguments
      else
        ENV.delete("XD_GH_ARGUMENTS")
      end
      if old_auth_calls
        ENV["XD_GH_AUTH_CALLS"] = old_auth_calls
      else
        ENV.delete("XD_GH_AUTH_CALLS")
      end
      if old_gh_token
        ENV["GH_TOKEN"] = old_gh_token
      else
        ENV.delete("GH_TOKEN")
      end
      if old_github_token
        ENV["GITHUB_TOKEN"] = old_github_token
      else
        ENV.delete("GITHUB_TOKEN")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "does not expose failed gh credential output" do
    directory = File.join(
      Dir.tempdir,
      "xd-workflow-gh-token-error-#{Random::Secure.hex(12)}"
    )
    executable = File.join(directory, "gh")
    Dir.mkdir_p(directory)
    File.write(executable, <<-'SH')
      #!/bin/sh
      printf '%s\n' 'do-not-leak-this-token'
      exit 1
      SH
    File.chmod(executable, 0o700)

    old_executable = ENV["XD_GH_EXECUTABLE"]?
    old_gh_token = ENV["GH_TOKEN"]?
    old_github_token = ENV["GITHUB_TOKEN"]?
    begin
      ENV["XD_GH_EXECUTABLE"] = executable
      ENV.delete("GH_TOKEN")
      ENV.delete("GITHUB_TOKEN")
      Xd::Agent::WorkflowRun.resolve_token.should be_nil
    ensure
      if old_executable
        ENV["XD_GH_EXECUTABLE"] = old_executable
      else
        ENV.delete("XD_GH_EXECUTABLE")
      end
      if old_gh_token
        ENV["GH_TOKEN"] = old_gh_token
      else
        ENV.delete("GH_TOKEN")
      end
      if old_github_token
        ENV["GITHUB_TOKEN"] = old_github_token
      else
        ENV.delete("GITHUB_TOKEN")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "times out a gh credential lookup that stops responding" do
    directory = File.join(
      Dir.tempdir,
      "xd-workflow-gh-token-timeout-#{Random::Secure.hex(12)}"
    )
    executable = File.join(directory, "gh")
    Dir.mkdir_p(directory)
    File.write(executable, <<-'SH')
      #!/bin/sh
      printf '%s\n' 'do-not-leak-this-token'
      exec sleep 60
      SH
    File.chmod(executable, 0o700)

    old_executable = ENV["XD_GH_EXECUTABLE"]?
    old_gh_token = ENV["GH_TOKEN"]?
    old_github_token = ENV["GITHUB_TOKEN"]?
    begin
      ENV["XD_GH_EXECUTABLE"] = executable
      ENV.delete("GH_TOKEN")
      ENV.delete("GITHUB_TOKEN")
      started = Time.instant
      Xd::Agent::WorkflowRun.resolve_token(50.milliseconds).should be_nil
      (Time.instant - started).should be < 2.seconds
    ensure
      if old_executable
        ENV["XD_GH_EXECUTABLE"] = old_executable
      else
        ENV.delete("XD_GH_EXECUTABLE")
      end
      if old_gh_token
        ENV["GH_TOKEN"] = old_gh_token
      else
        ENV.delete("GH_TOKEN")
      end
      if old_github_token
        ENV["GITHUB_TOKEN"] = old_github_token
      else
        ENV.delete("GITHUB_TOKEN")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "caches a missing credential and never runs an unauthenticated gh poll" do
    directory = File.join(
      Dir.tempdir,
      "xd-workflow-gh-error-#{Random::Secure.hex(12)}"
    )
    executable = File.join(directory, "gh")
    auth_calls = File.join(directory, "auth-calls")
    unexpected_poll = File.join(directory, "unexpected-poll")
    Dir.mkdir_p(directory)
    File.write(executable, <<-'SH')
      #!/bin/sh
      if [ "$1" = auth ] && [ "$2" = token ]; then
        printf 'call\n' >> "$XD_GH_AUTH_CALLS"
        printf '%s\n' 'do-not-leak-this-token'
        exit 1
      fi
      : > "$XD_GH_UNEXPECTED_POLL"
      exit 1
      SH
    File.chmod(executable, 0o700)

    old_executable = ENV["XD_GH_EXECUTABLE"]?
    old_auth_calls = ENV["XD_GH_AUTH_CALLS"]?
    old_unexpected_poll = ENV["XD_GH_UNEXPECTED_POLL"]?
    old_gh_token = ENV["GH_TOKEN"]?
    old_github_token = ENV["GITHUB_TOKEN"]?
    begin
      ENV["XD_GH_EXECUTABLE"] = executable
      ENV["XD_GH_AUTH_CALLS"] = auth_calls
      ENV["XD_GH_UNEXPECTED_POLL"] = unexpected_poll
      ENV.delete("GH_TOKEN")
      ENV.delete("GITHUB_TOKEN")
      cache = Xd::Agent::WorkflowRun::StatusCache.new(failure_ttl: 0.seconds)
      errors = ["123%", "124%"].map do |id|
        run = Xd::Agent::WorkflowRun::Run.new(id, "owner/repo%", "")
        expect_raises(Xd::Agent::WorkflowRun::StatusError) do
          cache.fetch(run)
        end
      end
      File.read(auth_calls).lines.size.should eq(1)
      File.exists?(unexpected_poll).should be_false
      errors.each do |error|
        error.message.not_nil!.should_not contain("do-not-leak-this-token")
      end
    ensure
      if old_executable
        ENV["XD_GH_EXECUTABLE"] = old_executable
      else
        ENV.delete("XD_GH_EXECUTABLE")
      end
      if old_auth_calls
        ENV["XD_GH_AUTH_CALLS"] = old_auth_calls
      else
        ENV.delete("XD_GH_AUTH_CALLS")
      end
      if old_unexpected_poll
        ENV["XD_GH_UNEXPECTED_POLL"] = old_unexpected_poll
      else
        ENV.delete("XD_GH_UNEXPECTED_POLL")
      end
      if old_gh_token
        ENV["GH_TOKEN"] = old_gh_token
      else
        ENV.delete("GH_TOKEN")
      end
      if old_github_token
        ENV["GITHUB_TOKEN"] = old_github_token
      else
        ENV.delete("GITHUB_TOKEN")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "reports a status error when gh is interrupted" do
    directory = File.join(
      Dir.tempdir,
      "xd-workflow-gh-signal-#{Random::Secure.hex(12)}"
    )
    executable = File.join(directory, "gh")
    Dir.mkdir_p(directory)
    File.write(executable, <<-'SH')
      #!/bin/sh
      trap - INT
      kill -INT $$
      exit 130
      SH
    File.chmod(executable, 0o700)

    old_executable = ENV["XD_GH_EXECUTABLE"]?
    begin
      ENV["XD_GH_EXECUTABLE"] = executable
      run = Xd::Agent::WorkflowRun::Run.new("123%", "owner/repo%", "")
      error = expect_raises(Xd::Agent::WorkflowRun::StatusError) do
        Xd::Agent::WorkflowRun.fetch_status(run, "test-token")
      end
      error.message.not_nil!.should contain("GitHub CLI returned status")
    ensure
      if old_executable
        ENV["XD_GH_EXECUTABLE"] = old_executable
      else
        ENV.delete("XD_GH_EXECUTABLE")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "kills a GitHub CLI status request that stops responding" do
    directory = File.join(
      Dir.tempdir,
      "xd-workflow-gh-timeout-#{Random::Secure.hex(12)}"
    )
    executable = File.join(directory, "gh")
    Dir.mkdir_p(directory)
    File.write(executable, "#!/bin/sh\nexec sleep 60\n")
    File.chmod(executable, 0o700)

    old_executable = ENV["XD_GH_EXECUTABLE"]?
    begin
      ENV["XD_GH_EXECUTABLE"] = executable
      run = Xd::Agent::WorkflowRun::Run.new("123", "owner/repo", "")
      started = Time.instant
      error = expect_raises(Xd::Agent::WorkflowRun::StatusError) do
        Xd::Agent::WorkflowRun.fetch_cli_status(
          run,
          "test-token",
          50.milliseconds
        )
      end
      error.message.not_nil!.should contain("timed out")
      (Time.instant - started).should be < 2.seconds
    ensure
      if old_executable
        ENV["XD_GH_EXECUTABLE"] = old_executable
      else
        ENV.delete("XD_GH_EXECUTABLE")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "times runs and jobs from either GitHub spelling" do
    running = Xd::Agent::WorkflowRun.parse_status(
      %({"name":"nightly","status":"in_progress","conclusion":null,
         "run_started_at":"2026-08-03T10:00:00Z",
         "updated_at":"2026-08-03T10:01:00Z"})
    ).not_nil!
    running.started_at.should eq(Time.utc(2026, 8, 3, 10, 0, 0))
    # A run still going has no finish time, whatever it was last written.
    running.completed_at.should be_nil
    running.elapsed(Time.utc(2026, 8, 3, 10, 2, 30)).should eq(150.seconds)

    passed = Xd::Agent::WorkflowRun.parse_status(
      %({"name":"nightly","status":"completed","conclusion":"success",
         "startedAt":"2026-08-03T10:00:00Z",
         "updatedAt":"2026-08-03T10:04:05Z"})
    ).not_nil!
    passed.elapsed(Time.utc(2026, 8, 3, 12, 0, 0)).should eq(245.seconds)

    jobs = Xd::Agent::WorkflowRun.parse_jobs(
      %({"jobs":[
        {"id":101,"name":"linux","status":"in_progress","conclusion":null,
         "started_at":"2026-08-03T10:00:10Z","completed_at":null},
        {"databaseId":102,"name":"macos","status":"completed",
         "conclusion":"success","startedAt":"2026-08-03T10:00:10Z",
         "completedAt":"2026-08-03T10:01:10Z"},
        {"databaseId":103,"name":"windows","status":"queued","conclusion":null,
         "startedAt":"0001-01-01T00:00:00Z",
         "completedAt":"0001-01-01T00:00:00Z"}
      ]})
    ).not_nil!
    jobs[0].elapsed(Time.utc(2026, 8, 3, 10, 0, 40)).should eq(30.seconds)
    jobs[1].elapsed(Time.utc(2026, 8, 3, 12, 0, 0)).should eq(60.seconds)
    # The CLI writes a zero time for a job that has not started.
    jobs[2].started_at.should be_nil
    jobs[2].elapsed.should be_nil
  end

  it "reports no elapsed time for a run that finished without saying when" do
    finished = Xd::Agent::WorkflowRun.parse_status(
      %({"name":"nightly","status":"completed","conclusion":"success",
         "run_started_at":"2026-08-03T10:00:00Z"})
    ).not_nil!
    finished.elapsed(Time.utc(2026, 8, 3, 12, 0, 0)).should be_nil
  end

  it "rejects malformed workflow status replies" do
    Xd::Agent::WorkflowRun.parse_status("not json").should be_nil
    Xd::Agent::WorkflowRun.parse_status(
      %({"name":"nightly","conclusion":null})
    ).should be_nil
  end

  it "parses the daemon workflow status format" do
    fields = JSON.parse(%({
      "name":"nightly","state":"completed","conclusion":"success",
      "started_at":1785751200,"completed_at":1785751445,
      "jobs":[{"id":"101","name":"linux","state":"completed",
        "conclusion":"success","log":"Publish",
        "started_at":1785751205,"completed_at":1785751325}]
    })).as_h

    status = Xd::Agent::WorkflowRun.parse_wire_status(fields).not_nil!
    status.label.should eq("nightly · Passed")
    status.elapsed.should eq(245.seconds)
    status.jobs.first.id.should eq("101")
    status.jobs.first.log.should eq("Publish")
    status.jobs.first.elapsed.should eq(2.minutes)
  end

  it "shares active status and preserves it through transient failures" do
    now = Time.instant
    calls = 0
    unavailable = false
    status = Xd::Agent::WorkflowRun::Status.new(
      "nightly",
      "in_progress",
      nil,
      [] of Xd::Agent::WorkflowRun::Job
    )
    resolver = ->(_run : Xd::Agent::WorkflowRun::Run) : Xd::Agent::WorkflowRun::Status {
      calls += 1
      if unavailable
        raise Xd::Agent::WorkflowRun::StatusError.new("temporary")
      end
      status
    }
    cache = Xd::Agent::WorkflowRun::StatusCache.new(
      resolver,
      clock: -> { now },
      active_ttl: 10.seconds,
      failure_ttl: 30.seconds
    )
    run = Xd::Agent::WorkflowRun::Run.new("123", "owner/repo", "")

    cache.fetch(run).should eq(status)
    cache.fetch(run).should eq(status)
    calls.should eq(1)

    now += 11.seconds
    unavailable = true
    cache.fetch(run).should eq(status)
    calls.should eq(2)

    now += 10.seconds
    cache.fetch(run).should eq(status)
    calls.should eq(2)
  end

  it "keeps terminal workflow status for the daemon lifetime" do
    now = Time.instant
    calls = 0
    status = Xd::Agent::WorkflowRun::Status.new(
      "nightly",
      "completed",
      "success",
      [] of Xd::Agent::WorkflowRun::Job
    )
    resolver = ->(_run : Xd::Agent::WorkflowRun::Run) {
      calls += 1
      status
    }
    cache = Xd::Agent::WorkflowRun::StatusCache.new(
      resolver,
      clock: -> { now },
      active_ttl: 1.second
    )
    run = Xd::Agent::WorkflowRun::Run.new("123", "owner/repo", "")

    cache.fetch(run).should eq(status)
    now += 1.day
    cache.fetch(run).should eq(status)
    calls.should eq(1)
  end
end
