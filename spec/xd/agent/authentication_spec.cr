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
  noisy : Bool = false,
  & : Xd::Agent::Authentication, Array(String) ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-authentication-#{Random::Secure.hex(12)}"
  )
  state = File.join(directory, "state")
  executable = File.join(directory, "agent-auth")
  Dir.mkdir_p(state)
  fixture = File.expand_path(
    "../../../tests/fixtures/agent-auth.sh",
    __DIR__
  )
  FileUtils.cp(fixture, executable)
  File.chmod(executable, 0o700)

  events = [] of String
  events_mutex = Mutex.new
  authentication = Xd::Agent::Authentication.new(
    ->(name : String, _fields : Hash(String, JSON::Any)) {
      events_mutex.synchronize { events << name }
    },
    resolver: ->(_provider : String) { executable },
    environment: {
      "AUTH_STATE" => state,
      "AUTH_NOISY" => noisy ? "1" : "0",
    }
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
      await_auth_state(
        authentication,
        "claude-mode",
        Xd::Agent::Authentication::State::SignedOut
      )

      authentication.login("codex")
      deadline = Time.instant + 3.seconds
      codex_prompt = nil
      until codex_prompt = authentication.snapshots.find do |snapshot|
              snapshot.provider == "codex" &&
              snapshot.login_url &&
              snapshot.device_code
            end
        fail "Codex never returned structured login data" if Time.instant >= deadline
        sleep 5.milliseconds
      end
      codex_prompt.not_nil!.login_url.should eq(
        "https://auth.openai.com/codex/device"
      )
      codex_prompt.not_nil!.device_code.should eq("ABCD-EFGH")
      codex_prompt.not_nil!.needs_input.should be_false
      authentication.input("codex", "CONTINUE")
      codex = await_auth_state(
        authentication,
        "codex",
        Xd::Agent::Authentication::State::SignedIn
      )
      codex.login_url.should be_nil
      codex.device_code.should be_nil

      authentication.login("claude")
      deadline = Time.instant + 3.seconds
      claude_prompt = nil
      until claude_prompt = authentication.snapshots.find do |snapshot|
              snapshot.provider == "claude" &&
              snapshot.login_url &&
              snapshot.needs_input
            end
        fail "Claude never requested its code" if Time.instant >= deadline
        sleep 5.milliseconds
      end
      claude_prompt.not_nil!.login_url.not_nil!.should start_with(
        "https://claude.com/cai/oauth/authorize?"
      )
      claude_prompt.not_nil!.device_code.should be_nil
      authentication.input("claude", "CLAUDE-1234")
      claude = await_auth_state(
        authentication,
        "claude",
        Xd::Agent::Authentication::State::SignedIn
      )
      claude.detail.should eq("Signed in with claudeAi.")

      authentication.login("claude-mode")
      proxy = await_auth_state(
        authentication,
        "claude-mode",
        Xd::Agent::Authentication::State::SignedIn
      )
      proxy.detail.should eq("Authenticated with Codex")

      authentication.logout("codex")
      authentication.logout("claude")
      authentication.logout("claude-mode")
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
      await_auth_state(
        authentication,
        "claude-mode",
        Xd::Agent::Authentication::State::SignedOut
      )

      events.should contain("agent-auth-changed")
      events.should_not contain("agent-auth-output")
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

  it "keeps noisy login output scheduler-friendly and coalesces events" do
    with_authentication_fixture(noisy: true) do |authentication, events|
      authentication.login("codex")
      heartbeat = 0
      running = true
      spawn do
        while running
          heartbeat += 1
          Fiber.yield
        end
      end

      deadline = Time.instant + 3.seconds
      until authentication.snapshots.any? do |snapshot|
              snapshot.provider == "codex" &&
              snapshot.login_url &&
              snapshot.device_code
            end
        fail "Codex noisy login never returned structured data" if Time.instant >= deadline
        sleep 5.milliseconds
      end
      running = false

      heartbeat.should be > 10
      events.count("agent-auth-changed").should be <= 4
      authentication.cancel("codex")
    end
  end
end
