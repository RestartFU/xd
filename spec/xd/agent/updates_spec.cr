require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/updates"

private def await_cli_updates(
  updates : Xd::Agent::Updates,
  &ready : Array(Xd::Agent::Updates::Snapshot) -> Bool
) : Array(Xd::Agent::Updates::Snapshot)
  deadline = Time.instant + 3.seconds
  loop do
    snapshots = updates.snapshots
    return snapshots if ready.call(snapshots)
    fail "CLI updates did not settle: #{snapshots}" if Time.instant >= deadline
    sleep 5.milliseconds
  end
end

describe Xd::Agent::Updates do
  it "checks and updates both official CLIs asynchronously" do
    directory = File.join(
      Dir.tempdir,
      "xd-agent-updates-#{Random::Secure.hex(12)}"
    )
    state = File.join(directory, "state")
    Dir.mkdir_p(state)
    script = <<-'SH'
      #!/bin/sh
      set -eu
      name=${0##*/}
      case "$*" in
        "--version")
          printf '%s %s\n' "$name" "$(cat "$CLI_STATE/$name")"
          ;;
        "update")
          printf 'updating %s\n' "$name"
          printf '2.0.0\n' > "$CLI_STATE/$name"
          ;;
        *)
          exit 2
          ;;
      esac
      SH
    %w(codex claude).each do |name|
      executable = File.join(directory, name)
      File.write(executable, script)
      File.chmod(executable, 0o700)
      File.write(File.join(state, name), "1.0.0\n")
    end

    events = [] of Hash(String, JSON::Any)
    begun = [] of String
    finished = [] of Tuple(String, Bool)
    updates = Xd::Agent::Updates.new(
      ->(_name : String, fields : Hash(String, JSON::Any)) {
        events << fields
        nil
      },
      resolver: ->(name : String) { File.join(directory, name) },
      environment: {"CLI_STATE" => state},
      begin_update: ->(provider : String) {
        begun << provider
        nil
      },
      finish_update: ->(provider : String, success : Bool) {
        finished << {provider, success}
        nil
      }
    )

    begin
      updates.refresh
      checked = await_cli_updates(updates) do |snapshots|
        snapshots.all? { |snapshot| snapshot.state.idle? && snapshot.version }
      end
      checked.map(&.version).should contain("codex 1.0.0")
      checked.map(&.version).should contain("claude 1.0.0")

      updates.update_all
      updated = await_cli_updates(updates) do |snapshots|
        snapshots.all?(&.state.updated?)
      end
      updated.map(&.version).should contain("codex 2.0.0")
      updated.map(&.version).should contain("claude 2.0.0")
      updated.each do |snapshot|
        snapshot.detail.not_nil!.should contain("Updated from")
      end
      begun.sort.should eq(["claude", "codex"])
      finished.sort_by(&.[0]).should eq([
        {"claude", true},
        {"codex", true},
      ])
      events.any? do |event|
        event["state"].as_s == "updating"
      end.should be_true
    ensure
      updates.close
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "reports update admission errors without starting a process" do
    updates = Xd::Agent::Updates.new(
      resolver: ->(_name : String) { "/bin/false" },
      environment: {} of String => String,
      begin_update: ->(_provider : String) {
        raise Xd::Agent::Updates::Error.new("Stop active turns first.")
      }
    )

    begin
      expect_raises(
        Xd::Agent::Updates::Error,
        "Stop active turns first."
      ) do
        updates.update_all
      end
      updates.snapshots.all?(&.state.idle?).should be_true
    ensure
      updates.close
    end
  end
end
