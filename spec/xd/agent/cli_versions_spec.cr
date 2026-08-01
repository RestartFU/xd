require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/cli_versions"

private def await_cli_versions(
  versions : Xd::Agent::CliVersions,
  &ready : Array(Xd::Agent::CliVersions::Snapshot) -> Bool
) : Array(Xd::Agent::CliVersions::Snapshot)
  deadline = Time.instant + 3.seconds
  loop do
    snapshots = versions.snapshots
    return snapshots if ready.call(snapshots)
    fail "CLI versions did not settle: #{snapshots}" if Time.instant >= deadline
    sleep 5.milliseconds
  end
end

describe Xd::Agent::CliVersions do
  it "reads all bundled assistant versions without invoking an updater" do
    directory = File.join(
      Dir.tempdir,
      "xd-agent-cli-versions-#{Random::Secure.hex(12)}"
    )
    marker = File.join(directory, "unexpected-command")
    Dir.mkdir_p(directory)
    script = <<-'SH'
      #!/bin/sh
      set -eu
      if [ "$*" != "--version" ]; then
        printf '%s\n' "$*" > "$COMMAND_MARKER"
        exit 2
      fi
      if [ "${DISABLE_AUTOUPDATER:-}" != "1" ]; then
        printf 'autoupdater enabled\n' > "$COMMAND_MARKER"
        exit 3
      fi
      printf '%s 1.0.0\n' "${0##*/}"
      SH
    %w(codex claude claude-code-proxy).each do |name|
      executable = File.join(directory, name)
      File.write(executable, script)
      File.chmod(executable, 0o700)
    end

    events = [] of Hash(String, JSON::Any)
    versions = Xd::Agent::CliVersions.new(
      ->(_name : String, fields : Hash(String, JSON::Any)) {
        events << fields
        nil
      },
      resolver: ->(name : String) { File.join(directory, name) },
      environment: {"COMMAND_MARKER" => marker}
    )

    begin
      versions.refresh
      checked = await_cli_versions(versions) do |snapshots|
        snapshots.all? { |snapshot| snapshot.state.idle? && snapshot.version }
      end
      checked.map(&.version).should contain("codex 1.0.0")
      checked.map(&.version).should contain("claude 1.0.0")
      checked.map(&.version).should contain("claude-code-proxy 1.0.0")
      File.exists?(marker).should be_false
      events.any? do |event|
        event["state"].as_s == "checking"
      end.should be_true
      events.any? do |event|
        {"updating", "updated"}.includes?(event["state"].as_s)
      end.should be_false
    ensure
      versions.close
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
