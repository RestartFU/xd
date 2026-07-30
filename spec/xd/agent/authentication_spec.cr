require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/authentication"

private def await_auth_state(
  authentication : Xd::Agent::Authentication,
  provider : String,
  expected : Xd::Agent::Authentication::State,
) : Xd::Agent::Authentication::Snapshot
  deadline = Time.instant + 3.seconds
  loop do
    snapshot = authentication.snapshots.find(&.provider.==(provider))
    return snapshot if snapshot && snapshot.state == expected
    if Time.instant >= deadline
      fail "#{provider} never reached #{expected}: #{snapshot.inspect}"
    end
    sleep 5.milliseconds
  end
end

private def with_authentication_fixture(
  & : Xd::Agent::Authentication, Array(String) ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-authentication-#{Random::Secure.hex(12)}"
  )
  state = File.join(directory, "state")
  executable = File.join(directory, "agent-auth")
  Dir.mkdir_p(state)
  File.write(executable, <<-'SH')
    #!/bin/sh
    set -eu

    case "$*" in
      "login status")
        if test -f "$AUTH_STATE/codex"; then
          echo "Logged in using ChatGPT" >&2
          exit 0
        fi
        echo "Not logged in" >&2
        exit 1
        ;;
      "auth status --json")
        if test -f "$AUTH_STATE/claude"; then
          printf '%s\n' '{"loggedIn":true,"authMethod":"claudeAi","apiProvider":"firstParty"}'
          exit 0
        fi
        printf '%s\n' '{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}'
        exit 1
        ;;
      "login --device-auth")
        printf 'Open https://auth.example/device and enter CODEX-1234\n'
        touch "$AUTH_STATE/codex"
        ;;
      "auth login")
        printf 'Paste code here > '
        IFS= read -r code
        test "$code" = "CLAUDE-1234"
        touch "$AUTH_STATE/claude"
        ;;
      "logout")
        rm -f "$AUTH_STATE/codex"
        ;;
      "auth logout")
        rm -f "$AUTH_STATE/claude"
        ;;
      *)
        echo "unexpected arguments: $*" >&2
        exit 2
        ;;
    esac
    SH
  File.chmod(executable, 0o700)

  events = [] of String
  events_mutex = Mutex.new
  authentication = Xd::Agent::Authentication.new(
    ->(name : String, _fields : Hash(String, JSON::Any)) {
      events_mutex.synchronize { events << name }
    },
    resolver: ->(_provider : String) { executable },
    environment: {"AUTH_STATE" => state}
  )

  begin
    yield authentication, events
  ensure
    authentication.close
    FileUtils.rm_r(directory) if Dir.exists?(directory)
  end
end

describe Xd::Agent::Authentication do
  it "runs both CLI authentication lifecycles without blocking callers" do
    with_authentication_fixture do |authentication, events|
      authentication.refresh
      await_auth_state(
        authentication,
        "codex",
        Xd::Agent::Authentication::State::SignedOut
      )
      await_auth_state(
        authentication,
        "claude",
        Xd::Agent::Authentication::State::SignedOut
      )

      authentication.login("codex")
      codex = await_auth_state(
        authentication,
        "codex",
        Xd::Agent::Authentication::State::SignedIn
      )
      codex.output.should contain("https://auth.example/device")

      authentication.login("claude")
      deadline = Time.instant + 3.seconds
      until authentication.snapshots
              .find(&.provider.==("claude"))
              .try(&.output.includes?("Paste code"))
        fail "Claude never requested its code" if Time.instant >= deadline
        sleep 5.milliseconds
      end
      authentication.input("claude", "CLAUDE-1234")
      claude = await_auth_state(
        authentication,
        "claude",
        Xd::Agent::Authentication::State::SignedIn
      )
      claude.detail.should eq("Signed in with claudeAi.")

      authentication.logout("codex")
      authentication.logout("claude")
      await_auth_state(
        authentication,
        "codex",
        Xd::Agent::Authentication::State::SignedOut
      )
      await_auth_state(
        authentication,
        "claude",
        Xd::Agent::Authentication::State::SignedOut
      )

      events.should contain("agent-auth-output")
      events.should contain("agent-auth-changed")
    end
  end

  it "rejects invalid providers, input, and idle cancellation" do
    with_authentication_fixture do |authentication, _events|
      expect_raises(
        Xd::Agent::Authentication::Error,
        "Unknown assistant: other"
      ) do
        authentication.login("other")
      end
      expect_raises(
        Xd::Agent::Authentication::Error,
        "Claude Code is not waiting for input."
      ) do
        authentication.input("claude", "code")
      end
      expect_raises(
        Xd::Agent::Authentication::Error,
        "Codex is not signing in."
      ) do
        authentication.cancel("codex")
      end
    end
  end
end
