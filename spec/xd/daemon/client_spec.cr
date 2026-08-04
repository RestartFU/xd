require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/certificate"
require "../../../src/xd/daemon/client"
require "../../../src/xd/daemon/server"
require "../../support/local_endpoint"

private def with_client_server(
  workspace_monitor_interval = Xd::Daemon::WorkspaceMonitor::INTERVAL,
  & : Xd::Daemon::Server, Xd::Daemon::Engine, Xd::Storage::Store, String ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-client-#{Random::Secure.hex(12)}"
  )
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  engine = Xd::Daemon::Engine.new(
    store,
    token_generator: -> { "client-token" },
    workspace_monitor_interval: workspace_monitor_interval
  )
  server = Xd::Daemon::Server.new(engine)

  begin
    yield server, engine, store, directory
  ensure
    server.close
    engine.close
    store.close
    FileUtils.rm_r(directory)
  end
end

private def await_auth_event(
  events : Array(Hash(String, JSON::Any)),
  mutex : Mutex,
  provider : String,
  &ready : Hash(String, JSON::Any) -> Bool
) : Hash(String, JSON::Any)
  deadline = Time.instant + 3.seconds
  last_event : Hash(String, JSON::Any)? = nil
  loop do
    last_event = mutex.synchronize do
      events.reverse.find do |event|
        event["event"]?.try(&.as_s?) == "agent-auth-changed" &&
          event["provider"]?.try(&.as_s?) == provider
      end
    end
    return last_event if last_event && ready.call(last_event)
    if Time.instant >= deadline
      fail "#{provider} remote authentication did not settle: #{last_event}"
    end
    sleep 5.milliseconds
  end
end

private def await_cli_event(
  events : Array(Hash(String, JSON::Any)),
  mutex : Mutex,
  provider : String,
  &ready : Hash(String, JSON::Any) -> Bool
) : Hash(String, JSON::Any)
  deadline = Time.instant + 3.seconds
  last_event : Hash(String, JSON::Any)? = nil
  loop do
    last_event = mutex.synchronize do
      events.reverse.find do |event|
        event["event"]?.try(&.as_s?) == "agent-cli-changed" &&
          event["provider"]?.try(&.as_s?) == provider
      end
    end
    return last_event if last_event && ready.call(last_event)
    if Time.instant >= deadline
      fail "#{provider} remote CLI version did not settle: #{last_event}"
    end
    sleep 5.milliseconds
  end
end

describe Xd::Daemon::Client do
  it "keeps account checks responsive during a blocked workflow refresh" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-workflow-block-#{Random::Secure.hex(12)}"
    )
    socket_path = File.join(directory, "daemon.sock")
    store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
    resolver_started = Channel(Nil).new(1)
    release_resolver = Channel(Nil).new(1)
    workflow_done = Channel(Exception?).new(1)
    status = Xd::Agent::WorkflowRun::Status.new(
      "nightly",
      "in_progress",
      nil,
      [] of Xd::Agent::WorkflowRun::Job
    )
    engine = Xd::Daemon::Engine.new(
      store,
      authentication_resolver: ->(_provider : String) { "/bin/true" },
      authentication_environment: {} of String => String,
      cli_version_resolver: ->(_provider : String) { "/bin/true" },
      cli_version_environment: {} of String => String,
      workflow_status_resolver: ->(_run : Xd::Agent::WorkflowRun::Run) {
        resolver_started.send(nil)
        release_resolver.receive
        status
      }
    )
    server = Xd::Daemon::Server.new(engine)
    client : Xd::Daemon::Client? = nil
    released = false

    begin
      server.listen_local(socket_path)
      client = Xd::Daemon::Client.local(
        socket_path,
        request_timeout: 2.seconds
      )
      spawn do
        begin
          client.not_nil!.call({
            "op"   => JSON::Any.new("workflow-status"),
            "text" => JSON::Any.new(
              "workflow_run\n123\n" \
              "https://github.com/owner/repo/actions/runs/123"
            ),
          })
          workflow_done.send(nil)
        rescue error
          workflow_done.send(error)
        end
      end

      select
      when resolver_started.receive
      when timeout(1.second)
        fail "workflow resolver did not start"
      end

      client.call({"op" => JSON::Any.new("tree")})["folders"]
        .as_a.should be_empty
      response = client.call({"op" => JSON::Any.new("agent-auth")})
      response["ok"].as_bool.should be_true
      response = client.call({"op" => JSON::Any.new("agent-clis")})
      response["ok"].as_bool.should be_true
      client.closed?.should be_false

      release_resolver.send(nil)
      released = true
      select
      when error = workflow_done.receive
        raise error if error
      when timeout(1.second)
        fail "workflow request did not finish"
      end
    ensure
      unless released
        select
        when release_resolver.send(nil)
        else
        end
      end
      client.try(&.close)
      server.close
      engine.close
      store.close
      FileUtils.rm_r(directory)
    end
  end

  it "stays connected when account checks stop responding" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-auth-timeout-#{Random::Secure.hex(12)}"
    )
    executable = File.join(directory, "stuck-assistant")
    socket_path = File.join(directory, "daemon.sock")
    Dir.mkdir_p(directory)
    store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
    File.write(executable, "#!/bin/sh\nexec /bin/sleep 60\n")
    File.chmod(executable, 0o700)
    engine = Xd::Daemon::Engine.new(
      store,
      authentication_resolver: ->(_provider : String) { executable },
      authentication_environment: {} of String => String,
      authentication_timeout: 50.milliseconds,
      cli_version_resolver: ->(_provider : String) { executable },
      cli_version_environment: {} of String => String,
      cli_version_timeout: 50.milliseconds
    )
    server = Xd::Daemon::Server.new(engine)
    client : Xd::Daemon::Client? = nil

    begin
      server.listen_local(socket_path)
      client = Xd::Daemon::Client.local(socket_path)
      events = [] of Hash(String, JSON::Any)
      events_mutex = Mutex.new
      client.subscribe do |event|
        events_mutex.synchronize { events << event }
      end

      client.call({"op" => JSON::Any.new("agent-auth")})
      client.call({"op" => JSON::Any.new("agent-clis")})
      client.call({"op" => JSON::Any.new("ping")})["ok"]
        .as_bool.should be_true

      await_auth_event(events, events_mutex, "codex") do |event|
        event["state"].as_s == "failed" &&
          event["detail"]?.try(&.as_s.includes?("timed out")) == true
      end
      await_cli_event(events, events_mutex, "codex") do |event|
        event["state"].as_s == "failed" &&
          event["detail"]?.try(&.as_s.includes?("timed out")) == true
      end

      client.closed?.should be_false
      client.call({"op" => JSON::Any.new("ping")})["ok"]
        .as_bool.should be_true
    ensure
      client.try(&.close)
      server.close
      engine.close
      store.close
      FileUtils.rm_r(directory)
    end
  end

  it "publishes tree changes after a managed folder is deleted externally" do
    with_client_server(10.milliseconds) do |server, _engine, _store, directory|
      path = File.join(directory, "daemon.sock")
      server.listen_local(path)
      client = Xd::Daemon::Client.local(path)
      events = Channel(Hash(String, JSON::Any)).new(4)
      client.subscribe { |event| events.send(event) }

      client.call({
        "op"   => JSON::Any.new("new-folder"),
        "name" => JSON::Any.new("Manual deletion"),
      })
      select
      when event = events.receive
        event["event"].as_s.should eq("tree")
      when timeout(1.second)
        fail "folder creation did not publish its tree event"
      end

      folder = File.join(directory, "Workspaces", "Manual deletion")
      File.exists?(
        File.join(folder, Xd::Workspace::SETTINGS_FILE)
      ).should be_false
      FileUtils.rm_r(folder)

      select
      when event = events.receive
        event["event"].as_s.should eq("tree")
      when timeout(1.second)
        fail "external folder deletion did not publish a tree event"
      end
      client.call({
        "op" => JSON::Any.new("tree"),
      })["folders"].as_a.should be_empty
      client.close
    end
  end

  it "matches out-of-order replies by request id" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-order-#{Random::Secure.hex(12)}"
    )
    path = File.join(directory, "daemon.sock")
    Dir.mkdir_p(directory)
    server = XdSpec::LocalEndpoint::Server.new(path)
    spawn do
      socket = server.accept
      first = JSON.parse(socket.gets.not_nil!).as_h
      second = JSON.parse(socket.gets.not_nil!).as_h
      [second, first].each do |request|
        socket << {
          Xd::Protocol::REQUEST_ID => request[Xd::Protocol::REQUEST_ID],
          "ok"                     => true,
          "value"                  => request["value"],
        }.to_json << '\n'
        socket.flush
      end
    ensure
      socket.try(&.close)
    end

    client = Xd::Daemon::Client.local(path)
    answers = Channel(Tuple(String, String)).new(2)
    {"first", "second"}.each do |value|
      spawn do
        response = client.call({
          "op"    => JSON::Any.new("ping"),
          "value" => JSON::Any.new(value),
        })
        answers.send({value, response["value"].as_s})
      end
    end
    received = {answers.receive, answers.receive}.to_a.to_h
    received.should eq({
      "first"  => "first",
      "second" => "second",
    })
  ensure
    client.try(&.close)
    server.try(&.close)
    FileUtils.rm_r(directory) if directory && Dir.exists?(directory)
  end

  it "keeps reading replies after an event subscriber fails" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-subscriber-#{Random::Secure.hex(12)}"
    )
    path = File.join(directory, "daemon.sock")
    Dir.mkdir_p(directory)
    server = XdSpec::LocalEndpoint::Server.new(path)
    start = Channel(Nil).new
    spawn do
      socket = server.accept
      start.receive
      socket << %({"event":"test"}) << '\n'
      socket.flush
      request = JSON.parse(socket.gets.not_nil!).as_h
      socket << {
        Xd::Protocol::REQUEST_ID => request[Xd::Protocol::REQUEST_ID],
        "ok"                     => true,
        "value"                  => "still alive",
      }.to_json << '\n'
      socket.flush
    ensure
      socket.try(&.close)
    end

    client = Xd::Daemon::Client.local(path)
    client.subscribe { |_event| raise "subscriber failed" }
    start.send(nil)
    answer = Channel(Tuple(Hash(String, JSON::Any)?, String?)).new(1)
    spawn do
      begin
        response = client.call({"op" => JSON::Any.new("ping")})
        answer.send({response, nil})
      rescue error
        answer.send({nil, error.message})
      end
    end

    select
    when result = answer.receive
      result[1].should be_nil
      result[0].not_nil!["value"].as_s.should eq("still alive")
    when timeout(2.seconds)
      fail "event subscriber killed the reply reader"
    end
  ensure
    client.try(&.close)
    server.try(&.close)
    FileUtils.rm_r(directory) if directory && Dir.exists?(directory)
  end

  it "allows an event subscriber to make a request" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-reentrant-#{Random::Secure.hex(12)}"
    )
    path = File.join(directory, "daemon.sock")
    Dir.mkdir_p(directory)
    server = XdSpec::LocalEndpoint::Server.new(path)
    start = Channel(Nil).new
    spawn do
      socket = server.accept
      start.receive
      socket << %({"event":"request-needed"}) << '\n'
      socket.flush
      request = JSON.parse(socket.gets.not_nil!).as_h
      socket << {
        Xd::Protocol::REQUEST_ID => request[Xd::Protocol::REQUEST_ID],
        "ok"                     => true,
        "value"                  => "subscriber reply",
      }.to_json << '\n'
      socket.flush
    ensure
      socket.try(&.close)
    end

    client = Xd::Daemon::Client.local(path)
    answer = Channel(Tuple(String?, String?)).new(1)
    client.subscribe do |_event|
      begin
        response = client.call({"op" => JSON::Any.new("ping")})
        answer.send({response["value"].as_s, nil})
      rescue error
        answer.send({nil, error.message})
      end
    end
    start.send(nil)

    select
    when result = answer.receive
      result.should eq({"subscriber reply", nil})
    when timeout(2.seconds)
      fail "event subscriber deadlocked the reply reader"
    end
  ensure
    client.try(&.close)
    server.try(&.close)
    FileUtils.rm_r(directory) if directory && Dir.exists?(directory)
  end

  it "times out while writing to a backpressured transport" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-write-timeout-#{Random::Secure.hex(12)}"
    )
    path = File.join(directory, "daemon.sock")
    Dir.mkdir_p(directory)
    server = XdSpec::LocalEndpoint::Server.new(path)
    spawn do
      socket = server.accept
      sleep 1.second
    ensure
      socket.try(&.close)
    end

    client = Xd::Daemon::Client.local(
      path,
      request_timeout: 20.milliseconds
    )
    started = Time.instant
    expect_raises(Xd::Daemon::Client::Error, /connection failed/i) do
      client.call({
        "op"      => JSON::Any.new("ping"),
        "payload" => JSON::Any.new("x" * (4 * 1024 * 1024)),
      })
    end
    (Time.instant - started).should be < 1.second
    client.closed?.should be_true
  ensure
    client.try(&.close)
    server.try(&.close)
    FileUtils.rm_r(directory) if directory && Dir.exists?(directory)
  end

  it "times out one request and ignores its late reply" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-timeout-#{Random::Secure.hex(12)}"
    )
    path = File.join(directory, "daemon.sock")
    Dir.mkdir_p(directory)
    server = XdSpec::LocalEndpoint::Server.new(path)
    release = Channel(Nil).new(1)
    late_sent = Channel(Nil).new(1)
    spawn do
      socket = server.accept
      first = JSON.parse(socket.gets.not_nil!).as_h
      release.receive
      socket << {
        Xd::Protocol::REQUEST_ID => first[Xd::Protocol::REQUEST_ID],
        "ok"                     => true,
        "value"                  => "late",
      }.to_json << '\n'
      socket.flush
      late_sent.send(nil)

      second = JSON.parse(socket.gets.not_nil!).as_h
      socket << {
        Xd::Protocol::REQUEST_ID => second[Xd::Protocol::REQUEST_ID],
        "ok"                     => true,
        "value"                  => "current",
      }.to_json << '\n'
      socket.flush
    ensure
      socket.try(&.close)
    end

    client = Xd::Daemon::Client.local(
      path,
      request_timeout: 200.milliseconds
    )
    expect_raises(
      Xd::Daemon::Client::TimeoutError,
      "Daemon request timed out."
    ) do
      client.call({"op" => JSON::Any.new("ping")})
    end

    release.send(nil)
    late_sent.receive
    response = client.call({"op" => JSON::Any.new("ping")})
    response["value"].as_s.should eq("current")
    client.closed?.should be_false
  ensure
    client.try(&.close)
    server.try(&.close)
    FileUtils.rm_r(directory) if directory && Dir.exists?(directory)
  end

  it "delivers daemon events after another observer fails" do
    with_client_server do |server, engine, _store, directory|
      engine.events.subscribe { |_event| raise "observer failed" }
      path = File.join(directory, "daemon.sock")
      server.listen_local(path)
      client = Xd::Daemon::Client.local(path)
      events = Channel(Hash(String, JSON::Any)).new(1)
      client.subscribe { |event| events.send(event) }

      client.call({
        "op"   => JSON::Any.new("new-folder"),
        "name" => JSON::Any.new("Subscriber proof"),
      })
      select
      when event = events.receive
        event["event"].as_s.should eq("tree")
      when timeout(2.seconds)
        fail "failed daemon observer blocked later subscribers"
      end
      client.close
    end
  end

  it "yields while a daemon continuously streams events" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-burst-#{Random::Secure.hex(12)}"
    )
    path = File.join(directory, "daemon.sock")
    Dir.mkdir_p(directory)
    server = XdSpec::LocalEndpoint::Server.new(path)
    start = Channel(Nil).new
    total = 20_000
    spawn do
      socket = server.accept
      start.receive
      total.times do |index|
        socket << {
          "event" => "burst",
          "id"    => index,
        }.to_json << '\n'
      end
      socket.flush
    ensure
      socket.try(&.close)
    end

    client = Xd::Daemon::Client.local(path)
    finished = Channel(Nil).new(1)
    count = 0
    client.subscribe do |_event|
      count += 1
      finished.send(nil) if count == total
    end

    start.send(nil)
    heartbeat = Channel(Time::Instant).new(1)
    started = Time.instant
    spawn { heartbeat.send(Time.instant) }
    select
    when tick = heartbeat.receive
      (tick - started).should be < 250.milliseconds
    when timeout(250.milliseconds)
      fail "continuous daemon events starved the scheduler"
    end
    select
    when finished.receive
    when timeout(3.seconds)
      fail "daemon event burst did not finish"
    end
  ensure
    client.try(&.close)
    server.try(&.close)
    FileUtils.rm_r(directory) if directory && Dir.exists?(directory)
  end

  it "uses ordered calls and events over local IPC" do
    with_client_server do |server, _engine, _store, directory|
      path = File.join(directory, "daemon.sock")
      server.listen_local(path)
      client = Xd::Daemon::Client.local(path)
      events = Channel(Hash(String, JSON::Any)).new(1)
      client.subscribe { |event| events.send(event) }

      response = client.call({
        "op"   => JSON::Any.new("new-folder"),
        "name" => JSON::Any.new("Client"),
      })
      response["id"].as_s.should_not be_empty
      select
      when event = events.receive
        event["event"].as_s.should eq("tree")
      when timeout(2.seconds)
        fail "client did not receive tree event"
      end
      client.close
    end
  end

  it "pairs, pins, and authenticates remote TLS" do
    with_client_server do |server, engine, _store, directory|
      certificate = File.join(directory, "certificate.pem")
      private_key = File.join(directory, "private-key.pem")
      Xd::Daemon::Certificate.ensure_pair(certificate, private_key)
      port = server.listen_remote(
        "127.0.0.1",
        0,
        certificate,
        private_key
      )
      code = engine.arm_pairing(1.minute)

      paired = Xd::Daemon::Client.pair_remote(
        "127.0.0.1",
        port,
        code,
        "crystal-client"
      )
      paired.token.should eq("client-token")
      paired.fingerprint.size.should eq(64)
      paired.client.call({
        "op" => JSON::Any.new("ping"),
      })["ok"].as_bool.should be_true
      paired.client.close

      expect_raises(Xd::Daemon::Client::Error, /certificate changed/) do
        Xd::Daemon::Client.remote(
          "127.0.0.1",
          port,
          "client-token",
          "00" * 32
        )
      end
    end
  end

  it "manages structured assistant accounts over paired TLS" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-auth-#{Random::Secure.hex(12)}"
    )
    state = File.join(directory, "auth-state")
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
            printf '%s\n' '{"loggedIn":true,"authMethod":"claudeAi"}'
            exit 0
          fi
          printf '%s\n' '{"loggedIn":false,"authMethod":"none"}'
          exit 1
          ;;
        "login --device-auth")
          printf 'Open https://auth.openai.com/codex/device\n'
          printf 'Enter this one-time code\nREMOTE-CODE\n'
          while ! test -f "$AUTH_STATE/codex-authorized"; do
            /bin/sleep 0.01
          done
          : > "$AUTH_STATE/codex"
          ;;
        "auth login")
          printf 'Visit https://claude.com/cai/oauth/authorize?state=remote\n'
          printf 'Paste code here if prompted > '
          IFS= read -r code
          test "$code" = "REMOTE-CLAUDE"
          : > "$AUTH_STATE/claude"
          ;;
        "logout")
          /bin/rm -f "$AUTH_STATE/codex"
          ;;
        "auth logout")
          /bin/rm -f "$AUTH_STATE/claude"
          ;;
        *)
          exit 2
          ;;
      esac
      SH
    File.chmod(executable, 0o700)

    store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
    engine = Xd::Daemon::Engine.new(
      store,
      token_generator: -> { "auth-token" },
      authentication_resolver: ->(_provider : String) { executable },
      authentication_environment: {"AUTH_STATE" => state}
    )
    server = Xd::Daemon::Server.new(engine)
    client : Xd::Daemon::Client? = nil

    begin
      certificate = File.join(directory, "certificate.pem")
      private_key = File.join(directory, "private-key.pem")
      Xd::Daemon::Certificate.ensure_pair(certificate, private_key)
      port = server.listen_remote(
        "127.0.0.1",
        0,
        certificate,
        private_key
      )
      code = engine.arm_pairing(1.minute)
      paired = Xd::Daemon::Client.pair_remote(
        "127.0.0.1",
        port,
        code,
        "account-manager"
      )
      client = paired.client
      events = [] of Hash(String, JSON::Any)
      events_mutex = Mutex.new
      client.subscribe do |event|
        events_mutex.synchronize { events << event }
      end

      client.call({"op" => JSON::Any.new("agent-auth")})
      ["codex", "claude"].each do |provider|
        await_auth_event(events, events_mutex, provider) do |event|
          event["state"].as_s == "signed-out"
        end
      end

      client.call({
        "op"       => JSON::Any.new("agent-auth-start"),
        "provider" => JSON::Any.new("codex"),
      })
      prompt = await_auth_event(events, events_mutex, "codex") do |event|
        event["device_code"]?.try(&.as_s?) == "REMOTE-CODE"
      end
      prompt["login_url"].as_s.should eq(
        "https://auth.openai.com/codex/device"
      )
      prompt.has_key?("output").should be_false
      File.touch(File.join(state, "codex-authorized"))
      await_auth_event(events, events_mutex, "codex") do |event|
        event["state"].as_s == "signed-in"
      end

      client.call({
        "op"       => JSON::Any.new("agent-auth-start"),
        "provider" => JSON::Any.new("claude"),
      })
      claude = await_auth_event(events, events_mutex, "claude") do |event|
        event["needs_input"].as_bool
      end
      claude["login_url"].as_s.should start_with(
        "https://claude.com/cai/oauth/authorize?"
      )
      client.call({
        "op"       => JSON::Any.new("agent-auth-input"),
        "provider" => JSON::Any.new("claude"),
        "input"    => JSON::Any.new("REMOTE-CLAUDE"),
      })
      await_auth_event(events, events_mutex, "claude") do |event|
        event["state"].as_s == "signed-in"
      end

      ["codex", "claude"].each do |provider|
        client.call({
          "op"       => JSON::Any.new("agent-auth-logout"),
          "provider" => JSON::Any.new(provider),
        })
        await_auth_event(events, events_mutex, provider) do |event|
          event["state"].as_s == "signed-out"
        end
      end

      event = events_mutex.synchronize do
        events.find do |candidate|
          candidate["event"]?.try(&.as_s?) == "agent-auth-changed"
        end
      end
      event.should_not be_nil
      event.not_nil!.has_key?("output").should be_false
    ensure
      client.try(&.close)
      server.close
      engine.close
      store.close
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "reads bundled assistant CLI versions over paired TLS" do
    directory = File.join(
      Dir.tempdir,
      "xd-client-cli-update-#{Random::Secure.hex(12)}"
    )
    state = File.join(directory, "versions")
    Dir.mkdir_p(state)
    script = <<-'SH'
      #!/bin/sh
      set -eu
      name=${0##*/}
      case "$*" in
        "--version")
          printf '%s %s\n' "$name" "$(cat "$CLI_STATE/$name")"
          ;;
        *)
          exit 2
          ;;
      esac
      SH
    %w(codex claude).each do |name|
      File.write(File.join(directory, name), script)
      File.chmod(File.join(directory, name), 0o700)
      File.write(File.join(state, name), "1.0.0\n")
    end

    store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
    engine = Xd::Daemon::Engine.new(
      store,
      token_generator: -> { "cli-version-token" },
      cli_version_resolver: ->(provider : String) {
        File.join(directory, provider)
      },
      cli_version_environment: {"CLI_STATE" => state}
    )
    server = Xd::Daemon::Server.new(engine)
    client : Xd::Daemon::Client? = nil

    begin
      certificate = File.join(directory, "certificate.pem")
      private_key = File.join(directory, "private-key.pem")
      Xd::Daemon::Certificate.ensure_pair(certificate, private_key)
      port = server.listen_remote(
        "127.0.0.1",
        0,
        certificate,
        private_key
      )
      paired = Xd::Daemon::Client.pair_remote(
        "127.0.0.1",
        port,
        engine.arm_pairing(1.minute),
        "cli-update-manager"
      )
      client = paired.client
      events = [] of Hash(String, JSON::Any)
      events_mutex = Mutex.new
      client.subscribe do |event|
        events_mutex.synchronize { events << event }
      end

      client.call({"op" => JSON::Any.new("agent-clis")})
      %w(codex claude).each do |provider|
        checked = await_cli_event(events, events_mutex, provider) do |event|
          event["state"].as_s == "idle" &&
            event["version"]?.try(&.as_s?).try(&.ends_with?("1.0.0")) == true
        end
        checked["version"].as_s.should end_with("1.0.0")
      end
    ensure
      client.try(&.close)
      server.close
      engine.close
      store.close
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "reports a dropped connection exactly once" do
    with_client_server do |server, _engine, _store, directory|
      path = File.join(directory, "daemon.sock")
      server.listen_local(path)
      client = Xd::Daemon::Client.local(path)
      closed = Channel(String).new(2)
      client.on_disconnect { |message| closed.send(message) }

      server.close

      select
      when message = closed.receive
        message.should contain("Daemon")
      when timeout(2.seconds)
        fail "client did not report the disconnect"
      end
      client.close
      select
      when closed.receive
        fail "client reported the disconnect twice"
      when timeout(50.milliseconds)
      end
      client.closed?.should be_true
    end
  end
end
