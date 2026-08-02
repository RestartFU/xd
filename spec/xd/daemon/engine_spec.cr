require "../../spec_helper"
require "base64"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/engine"

private def parse_response(response : Xd::Protocol::Response) : JSON::Any
  JSON.parse(response.to_json)
end

private def engine_git(workdir : String, *arguments : String) : Nil
  status = Process.run(
    "git",
    arguments,
    chdir: workdir,
    output: Process::Redirect::Close,
    error: Process::Redirect::Close
  )
  status.success?.should be_true
end

private class EngineSessionHandle < Xd::Agent::SessionHandle
  getter canceled = false

  def cancel : Nil
    @canceled = true
  end
end

private class EngineLauncher < Xd::Agent::Launcher
  getter specs = [] of Xd::Agent::RunSpec
  getter handles = [] of EngineSessionHandle
  getter event_callbacks = [] of Proc(Xd::Agent::Event, Nil)
  getter finish_callbacks = [] of Proc(Bool, String?, Nil)

  def start(
    backend : Xd::Agent::Backend,
    spec : Xd::Agent::RunSpec,
    environment : Hash(String, String),
    secret_names : Array(String),
    on_event : Proc(Xd::Agent::Event, Nil),
    on_finished : Proc(Bool, String?, Nil),
  ) : Xd::Agent::SessionHandle
    handle = EngineSessionHandle.new
    @specs << spec
    @handles << handle
    @event_callbacks << on_event
    @finish_callbacks << on_finished
    handle
  end

  def emit(index : Int, event : Xd::Agent::Event) : Nil
    @event_callbacks[index].call(event)
  end

  def finish(index : Int, ok : Bool, message : String? = nil) : Nil
    @finish_callbacks[index].call(ok, message)
  end
end

private def with_daemon_engine(
  clock : Proc(Time::Instant) = -> { Time.instant },
  token_generator : Proc(String) = -> { Random::Secure.base64(32) },
  launcher : Xd::Agent::Launcher? = nil,
  authentication_resolver : Xd::Agent::Authentication::Resolver? = nil,
  authentication_environment : Hash(String, String)? = nil,
  agent_authorizer : Xd::Agent::Manager::Authorizer? = ->(_provider : String) : String? { nil },
  voice_model_factory : Xd::Daemon::VoiceJobs::ModelFactory? = nil,
  voice_transcriber_factory : Xd::Daemon::VoiceJobs::TranscriberFactory? = nil,
  peer_host : Proc(String) = -> { "192.168.1.20" },
  & : Xd::Storage::Store, Xd::Daemon::Engine ->
) : Nil
  path = File.join(
    Dir.tempdir,
    "xd-engine-#{Random::Secure.hex(12)}",
    "chats.db"
  )
  store = Xd::Storage::Store.new(path)
  engine = Xd::Daemon::Engine.new(
    store,
    clock: clock,
    token_generator: token_generator,
    launcher: launcher,
    authentication_resolver: authentication_resolver,
    authentication_environment: authentication_environment,
    agent_authorizer: agent_authorizer,
    voice_model_factory: voice_model_factory,
    voice_transcriber_factory: voice_transcriber_factory,
    peer_host: peer_host
  )

  begin
    yield store, engine
  ensure
    engine.close
    store.close
    FileUtils.rm_r(Path[path].dirname)
  end
end

describe Xd::Daemon::Engine do
  it "returns a response when a handler raises unexpectedly" do
    token_generator = -> { raise "entropy source failed" }
    with_daemon_engine(token_generator: token_generator) do |_store, engine|
      remote = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote)
      code = engine.arm_pairing(1.minute)

      response = engine.dispatch(remote, {
        "op"   => "pair",
        "code" => code,
        "name" => "failure proof",
      }.to_json)

      response.success?.should be_false
      response["error"].as_s.should eq("Internal daemon error.")
    end
  end

  it "publishes the assistant catalog a separate client cannot compile in" do
    with_daemon_engine do |_store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)

      response = engine.dispatch(local, %({"op":"agent-catalog"}))

      response.success?.should be_true
      backends = parse_response(response)["backends"].as_a
      backends.map(&.["id"].as_s).sort!.should eq(["claude", "codex"])

      backends.each do |backend|
        catalog = Xd::Agent::Catalog.lookup(backend["id"].as_s).not_nil!
        backend["name"].as_s.should eq(catalog.display_name)
        backend["default_model"].as_s.should eq(catalog.default_model)

        # set-option validates the model id, so the published ids have to be
        # exactly the ones it will accept.
        models = backend["models"].as_a.map(&.["id"].as_s)
        models.should eq(catalog.models.map(&.id))
        backend["efforts"].as_a.map(&.as_s)
          .should eq(catalog.efforts.map(&.wire_name))
      end
    end
  end

  it "accepts every published model for its own backend" do
    with_daemon_engine do |store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      chat = store.create_chat("folder", "Chat", "claude")

      backends = parse_response(
        engine.dispatch(local, %({"op":"agent-catalog"}))
      )["backends"].as_a

      backends.each do |backend|
        backend["models"].as_a.each do |model|
          response = engine.dispatch(local, {
            "op"      => "set-option",
            "chat"    => chat,
            "option"  => "model",
            "backend" => backend["id"].as_s,
            "value"   => model["id"].as_s,
          }.to_json)
          response.success?.should be_true
        end
      end
    end
  end

  it "reports what a client needs to offer a daemon update" do
    with_daemon_engine do |_store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)

      response = engine.dispatch(local, %({"op":"daemon-update"}))

      response.success?.should be_true
      status = parse_response(response)
      status["version"].as_s.should eq(Xd.version_string)
      status["state"].as_s.should eq("idle")
      # A build run from the spec suite is not an installed bundle, so there
      # is nothing it could replace.
      status["supported"].as_bool.should be_false
      status["available"].as_bool.should be_false
    end
  end

  it "refuses to install or restart where it cannot replace itself" do
    with_daemon_engine do |_store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)

      ["install", "restart"].each do |action|
        response = engine.dispatch(
          local,
          {"op" => "daemon-update", "action" => action}.to_json
        )
        response.success?.should be_false
        response["error"].as_s.should contain("cannot")
      end
    end
  end

  it "rejects an unknown update action" do
    with_daemon_engine do |_store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)

      response = engine.dispatch(
        local,
        %({"op":"daemon-update","action":"uninstall"})
      )

      response.success?.should be_false
      response["error"].as_s.should eq("No such daemon-update action.")
    end
  end

  it "uses the same dispatcher after transport authentication" do
    with_daemon_engine do |_store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      remote = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote)

      local_response = engine.dispatch(local, %({"op":"ping"}))
      remote_denied = engine.dispatch(remote, %({"op":"ping"}))

      local_response.success?.should be_true
      remote_denied.success?.should be_false
      remote_denied["error"].as_s.should eq(
        "Not authenticated. Say hello first."
      )
    end
  end

  it "routes agent authentication through local and remote dispatch" do
    with_daemon_engine(
      authentication_resolver: ->(_provider : String) { "/bin/false" },
      authentication_environment: {} of String => String
    ) do |_store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      remote = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote)
      remote.authenticated = true

      [local, remote].each do |connection|
        response = engine.dispatch(connection, %({"op":"agent-auth"}))
        response.success?.should be_true
        providers = response["providers"].as_a
        providers.map { |row| row["provider"].as_s }
          .should eq(["claude", "codex", "claude-mode"])
        providers.each do |provider|
          provider["needs_input"].as_bool.should be_false
          provider.as_h.has_key?("output").should be_false
        end
      end

      refused = engine.dispatch(local, {
        "op"       => "agent-auth-start",
        "provider" => "other",
      }.to_json)
      refused.success?.should be_false
      refused["error"].as_s.should eq("Unknown assistant: other")
    end
  end

  it "pairs once, stores only a token hash, then authenticates another connection" do
    with_daemon_engine(
      token_generator: -> { "secret-token" }
    ) do |store, engine|
      pairing_connection = Xd::Daemon::Connection.new(
        Xd::Daemon::Transport::Remote
      )
      code = engine.arm_pairing(5.minutes)

      pair = engine.dispatch(pairing_connection, {
        "op"   => "pair",
        "code" => code,
        "name" => "workstation",
      }.to_json)

      pair.success?.should be_true
      pair["token"].as_s.should eq("secret-token")
      pairing_connection.authenticated.should be_true
      store.device_name(
        Digest::SHA256.hexdigest("secret-token")
      ).should eq("workstation")

      second_pair = engine.dispatch(
        Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote),
        {"op" => "pair", "code" => code, "name" => "other"}.to_json
      )
      second_pair.success?.should be_false

      returning = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote)
      hello = engine.dispatch(returning, {
        "op"    => "hello",
        "token" => "secret-token",
      }.to_json)

      hello.success?.should be_true
      hello["device"].as_s.should eq("workstation")
      hello["version"].as_i64.should eq(1)
      returning.authenticated.should be_true
      engine.dispatch(returning, %({"op":"ping"})).success?.should be_true
    end
  end

  it "lets only a local client expose this engine and mint a peer code" do
    with_daemon_engine(
      token_generator: -> { "peer-token" }
    ) do |_store, engine|
      listened = nil.as({String, Int32}?)
      engine.peer_listener = ->(host : String, port : Int32) {
        listened = {host, port}
        43210
      }
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)

      response = engine.dispatch(local, {
        "op"   => "peer-pairing",
        "bind" => "127.0.0.1",
        "port" => 0,
      }.to_json)

      response.success?.should be_true
      listened.should eq({"127.0.0.1", 0})
      response["host"].as_s.should eq("192.168.1.20")
      response["port"].as_i64.should eq(43210)
      response["expires_in"].as_i64.should eq(300)

      paired = engine.dispatch(
        Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote),
        {
          "op"   => "pair",
          "code" => response["code"].as_s,
          "name" => "laptop",
        }.to_json
      )
      paired.success?.should be_true
      paired["token"].as_s.should eq("peer-token")

      remote = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote)
      remote.authenticated = true
      refused = engine.dispatch(remote, %({"op":"peer-pairing"}))
      refused.success?.should be_false
      refused["error"].as_s.should contain("daemon machine")
    end
  end

  it "rejects invalid peer listener ports without invoking the listener" do
    with_daemon_engine do |_store, engine|
      invoked = false
      engine.peer_listener = ->(_host : String, _port : Int32) {
        invoked = true
        4001
      }

      response = engine.dispatch(
        Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local),
        %({"op":"peer-pairing","port":70000})
      )

      response.success?.should be_false
      response["error"].as_s.should contain("Port must be")
      invoked.should be_false
    end
  end

  it "returns a useful local error when the peer listener cannot open" do
    with_daemon_engine do |_store, engine|
      engine.peer_listener = ->(_host : String, _port : Int32) {
        raise IO::Error.new("address already in use")
      }

      response = engine.dispatch(
        Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local),
        %({"op":"peer-pairing"})
      )

      response.success?.should be_false
      response["error"].as_s.should eq(
        "Cannot accept remote devices: address already in use"
      )
    end
  end

  it "rejects expired pairing codes" do
    now = Time.instant
    clock = -> { now }
    with_daemon_engine(clock: clock) do |_store, engine|
      code = engine.arm_pairing(5.seconds)
      now += 6.seconds

      response = engine.dispatch(
        Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote),
        {"op" => "pair", "code" => code}.to_json
      )

      response.success?.should be_false
      response["error"].as_s.should contain("No such pairing code")
    end
  end

  it "does not authenticate unknown tokens" do
    with_daemon_engine do |_store, engine|
      connection = Xd::Daemon::Connection.new(
        Xd::Daemon::Transport::Remote
      )

      response = engine.dispatch(connection, {
        "op"    => "hello",
        "token" => "unknown",
      }.to_json)

      response.success?.should be_false
      response["error"].as_s.should eq("Unknown device. Pair first.")
      connection.authenticated.should be_false
    end
  end

  it "runs local and authenticated remote chat commands identically" do
    with_daemon_engine do |store, engine|
      chat_id = store.create_chat("folder", "Chat", "claude")
      store.append_message(chat_id, "user", "hello")
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      remote = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote)
      remote.authenticated = true

      request = {"op" => "messages", "chat" => chat_id}.to_json
      parse_response(engine.dispatch(local, request)).should eq(
        parse_response(engine.dispatch(remote, request))
      )

      search = {"op" => "search", "query" => "hell"}.to_json
      parse_response(engine.dispatch(local, search)).should eq(
        parse_response(engine.dispatch(remote, search))
      )
      engine.dispatch(local, search)["results"].as_a
        .first["chat"].as_s.should eq(chat_id)

      engine.dispatch(local, {
        "op"   => "queue",
        "chat" => chat_id,
        "text" => "next",
      }.to_json).success?.should be_true
      chat = engine.dispatch(remote, {
        "op"   => "chat",
        "chat" => chat_id,
      }.to_json)
      chat["queue"].as_a.map(&.as_s).should eq(["next"])
    end
  end

  it "returns the selected backend's configured default effort" do
    directory = File.join(
      Dir.tempdir,
      "xd-engine-effort-#{Random::Secure.hex(12)}"
    )
    previous_home = ENV["HOME"]?

    begin
      Dir.mkdir_p(File.join(directory, ".claude"))
      File.write(
        File.join(directory, ".claude", "settings.json"),
        %({"effortLevel":"low"})
      )
      ENV["HOME"] = directory

      with_daemon_engine do |store, engine|
        chat_id = store.create_chat("folder", "Chat", "claude")
        local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
        state = engine.dispatch(local, {
          "op"   => "chat",
          "chat" => chat_id,
        }.to_json)

        state["effort"].as_s.should eq("low")
      end
    ensure
      if previous_home
        ENV["HOME"] = previous_home
      else
        ENV.delete("HOME")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "blocks signed-out local and remote turns before the launcher" do
    directory = File.join(
      Dir.tempdir,
      "xd-engine-auth-gate-#{Random::Secure.hex(12)}"
    )
    state = File.join(directory, "auth-state")
    Dir.mkdir_p(state)
    executable = File.join(directory, "agent-auth")
    FileUtils.cp(
      File.expand_path(
        "../../../tests/fixtures/agent-auth.sh",
        __DIR__
      ),
      executable
    )
    File.chmod(executable, 0o700)
    launcher = EngineLauncher.new

    begin
      with_daemon_engine(
        launcher: launcher,
        authentication_resolver: ->(_provider : String) { executable },
        authentication_environment: {"AUTH_STATE" => state},
        agent_authorizer: nil
      ) do |store, engine|
        chat_id = store.create_chat("folder", "Chat", "claude")
        local = Xd::Daemon::Connection.new(
          Xd::Daemon::Transport::Local
        )
        remote = Xd::Daemon::Connection.new(
          Xd::Daemon::Transport::Remote
        )
        remote.authenticated = true

        deadline = Time.instant + 3.seconds
        loop do
          current = engine.dispatch(local, {
            "op"   => "chat",
            "chat" => chat_id,
          }.to_json)
          break if current["auth_state"].as_s == "signed-out"
          fail "authentication status did not settle" if Time.instant >= deadline
          sleep 5.milliseconds
        end

        [local, remote].each do |connection|
          denied = engine.dispatch(connection, {
            "op"   => "send",
            "chat" => chat_id,
            "text" => "never reaches the provider",
          }.to_json)
          denied.success?.should be_false
          denied["error"].as_s.should eq(
            "Sign in to Claude Code before starting a turn."
          )
        end

        launcher.specs.should be_empty
        store.list_messages(chat_id).should be_empty
        store.get_chat(chat_id).daemon_working.should be_false
      end
    ensure
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "runs voice models and transcription on the selected daemon connection" do
    directory = File.join(
      Dir.tempdir,
      "xd-engine-voice-#{Random::Secure.hex(12)}"
    )
    Dir.mkdir_p(directory)
    model_path = File.join(directory, "model.bin")
    executable = File.join(directory, "whisper")
    File.write(model_path, "daemon-owned-model")
    File.write(executable, <<-'SH')
      #!/bin/sh
      set -eu
      printf 'daemon transcript\n'
      SH
    File.chmod(executable, 0o700)
    model_factory = -> {
      Xd::Voice::Model.new(override_path: model_path)
    }
    transcriber_factory = -> {
      Xd::Voice::Transcriber.new(
        resolver: -> { executable },
        environment: {} of String => String
      )
    }

    begin
      with_daemon_engine(
        voice_model_factory: model_factory,
        voice_transcriber_factory: transcriber_factory
      ) do |store, engine|
        chat_id = store.create_chat("folder", "Voice", "claude")
        local = Xd::Daemon::Connection.new(
          Xd::Daemon::Transport::Local
        )
        remote = Xd::Daemon::Connection.new(
          Xd::Daemon::Transport::Remote
        )
        remote.authenticated = true

        [local, remote].each do |connection|
          status = engine.dispatch(connection, {
            "op"   => "voice-model",
            "chat" => chat_id,
          }.to_json)
          status.success?.should be_true
          status["available"].as_bool.should be_true
        end

        events = Channel(Xd::Protocol::Event).new(2)
        subscription = engine.events.subscribe { |event| events.send(event) }
        begin
          started = engine.dispatch(remote, {
            "op"      => "voice-transcribe",
            "chat"    => chat_id,
            "request" => "remote-voice",
            "audio"   => Base64.strict_encode(Bytes[1, 2, 3, 4]),
          }.to_json)
          started.success?.should be_true

          event = events.receive
          event["event"].as_s.should eq("voice")
          event["state"].as_s.should eq("transcribed")
          event["request"].as_s.should eq("remote-voice")
          event["text"].as_s.should eq("daemon transcript")
          event.audience.should eq(remote.object_id)
          event.audience.should_not eq(local.object_id)
        ensure
          engine.events.unsubscribe(subscription)
        end
      end
    ensure
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "pages recent transcript rows at the active turn revision" do
    launcher = EngineLauncher.new

    with_daemon_engine(launcher: launcher) do |store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      folder = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Transcript",
      }.to_json)["id"].as_s
      chat = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder,
      }.to_json)["id"].as_s

      102.times do |index|
        store.append_message(chat, "event", "history-#{index}")
      end

      page = engine.dispatch(local, {
        "op"    => "messages",
        "chat"  => chat,
        "limit" => 101,
      }.to_json)
      page["total_messages"].as_i64.should eq(102)
      page["messages"].as_a.size.should eq(101)
      page["messages"].as_a.first["content"].as_s.should eq("history-1")

      engine.dispatch(local, {
        "op"   => "send",
        "chat" => chat,
        "text" => "keep going",
      }.to_json).success?.should be_true
      revision = store.last_message_id(chat)
      store.append_message(chat, "assistant", "live row")

      live = engine.dispatch(local, {
        "op"    => "messages",
        "chat"  => chat,
        "limit" => 101,
      }.to_json)
      live["total_messages"].as_i64.should eq(103)
      live["last_message_id"].as_i64.should eq(revision)
      live["messages"].as_a.map { |row| row["content"].as_s }
        .should_not contain("live row")
      live["messages"].as_a.last["content"].as_s.should eq("keep going")
    end
  end

  it "owns workspace and chat mutations behind the same protocol" do
    with_daemon_engine do |store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      created = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Lunar",
      }.to_json)
      folder_id = created["id"].as_s

      engine.dispatch(local, {
        "op"      => "set-folder-context",
        "folder"  => folder_id,
        "context" => "Use Crystal.",
      }.to_json).success?.should be_true

      chat = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder_id,
        "title"  => "Port daemon",
      }.to_json)
      chat.success?.should be_true

      tree = engine.dispatch(local, %({"op":"tree"}))
      tree["folders"].as_a.map { |folder| folder["name"].as_s }
        .should eq(["Lunar"])
      tree["chats"].as_a.map { |item| item["id"].as_s }
        .should eq([chat["id"].as_s])

      context = engine.dispatch(local, {
        "op"     => "folder-context",
        "folder" => folder_id,
      }.to_json)
      context["context"].as_s.should eq("Use Crystal.")

      project = File.join(Path[store.path].dirname, "project")
      Dir.mkdir(project)
      configured = engine.dispatch(local, {
        "op"      => "set-folder-settings",
        "folder"  => folder_id,
        "backend" => "codex",
        "model"   => "gpt-5.4",
        "workdir" => project,
        "repo"    => nil,
      }.to_json)
      configured.success?.should be_true

      settings = engine.dispatch(local, {
        "op"     => "folder-settings",
        "folder" => folder_id,
      }.to_json)
      settings["backend"].as_s.should eq("codex")
      settings["model"].as_s.should eq("gpt-5.4")
      settings["workdir"].as_s.should eq(project)
      settings["repo"].raw.should be_nil
      settings["effective_backend"].as_s.should eq("codex")
      settings["effective_workdir"].as_s.should eq(project)
    end
  end

  it "moves folders and individual chats through the protocol" do
    with_daemon_engine do |store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      source_id = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Source",
      }.to_json)["id"].as_s
      target_id = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Target",
      }.to_json)["id"].as_s
      child_id = engine.dispatch(local, {
        "op"     => "new-folder",
        "parent" => source_id,
        "name"   => "Child",
      }.to_json)["id"].as_s
      chat_id = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => source_id,
        "title"  => "Movable",
      }.to_json)["id"].as_s

      moved_folder = engine.process(local, {
        "op"     => "move-folder",
        "folder" => child_id,
        "parent" => target_id,
      }.to_json)
      moved_folder.response.success?.should be_true
      moved_folder.events.map { |event| event["event"].as_s }
        .should eq(["tree"])

      moved_chat = engine.process(local, {
        "op"     => "move-chat",
        "chat"   => chat_id,
        "folder" => target_id,
      }.to_json)
      moved_chat.response.success?.should be_true
      moved_chat.events.map { |event| event["event"].as_s }
        .should eq(["tree"])

      tree = engine.dispatch(local, %({"op":"tree"}))
      child = tree["folders"].as_a.find { |folder| folder["id"].as_s == child_id }
      child.not_nil!["parent"].as_s.should eq(target_id)
      chat = tree["chats"].as_a.find { |item| item["id"].as_s == chat_id }
      chat.not_nil!["folder"].as_s.should eq(target_id)
    end
  end

  it "opens a new chat in a hidden non-Git working directory" do
    with_daemon_engine do |store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      folder_id = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Hidden Local",
      }.to_json)["id"].as_s
      workdir = File.join(Path[store.path].dirname, ".local")
      Dir.mkdir(workdir)

      created = engine.dispatch(local, {
        "op"      => "new-chat",
        "folder"  => folder_id,
        "title"   => "Dot directory",
        "workdir" => workdir,
      }.to_json)
      created.success?.should be_true

      state = engine.dispatch(local, {
        "op"   => "chat",
        "chat" => created["id"].as_s,
      }.to_json)
      state.success?.should be_true
      File.realpath(state["workdir"].as_s).should eq(File.realpath(workdir))
      state["context"].as_s.should contain(".local")
    end
  end

  it "manages secret names without returning their values" do
    old_path = ENV["XD_AGENT_SECRETS_FILE"]?
    directory = File.join(
      Dir.tempdir,
      "xd-engine-secrets-#{Random::Secure.hex(12)}"
    )
    ENV["XD_AGENT_SECRETS_FILE"] = File.join(directory, "secrets.json")

    begin
      with_daemon_engine do |_store, engine|
        local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
        saved = engine.dispatch(local, {
          "op"      => "set-agent-secrets",
          "entries" => [
            {"name" => "API_TOKEN", "value" => "never-over-wire"},
          ],
        }.to_json)
        saved.success?.should be_true

        listed = engine.dispatch(local, %({"op":"agent-secrets"}))
        listed["names"].as_a.map(&.as_s).should eq(["API_TOKEN"])
        listed.to_json.should_not contain("never-over-wire")

        kept = engine.dispatch(local, {
          "op"      => "set-agent-secrets",
          "entries" => [{"name" => "API_TOKEN"}],
        }.to_json)
        kept.success?.should be_true
        Xd::Agent::Secrets.load
          .environment({} of String => String)["API_TOKEN"]
          .should eq("never-over-wire")
      end
    ensure
      if old_path
        ENV["XD_AGENT_SECRETS_FILE"] = old_path
      else
        ENV.delete("XD_AGENT_SECRETS_FILE")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "routes send and cancel through the daemon-owned agent manager" do
    launcher = EngineLauncher.new
    old_path = ENV["XD_AGENT_SECRETS_FILE"]?

    begin
      with_daemon_engine(launcher: launcher) do |_store, engine|
        ENV["XD_AGENT_SECRETS_FILE"] = File.join(
          Dir.tempdir,
          "xd-engine-agent-#{Random::Secure.hex(12)}.json"
        )
        local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
        seen = [] of Xd::Protocol::Event
        subscription = engine.events.subscribe { |event| seen << event }

        folder = engine.dispatch(local, {
          "op"   => "new-folder",
          "name" => "Agent",
        }.to_json)["id"].as_s
        chat = engine.dispatch(local, {
          "op"     => "new-chat",
          "folder" => folder,
        }.to_json)["id"].as_s

        sent = engine.dispatch(local, {
          "op"   => "send",
          "chat" => chat,
          "text" => "inspect",
        }.to_json)
        sent.success?.should be_true
        sent["queued"].as_bool.should be_false
        launcher.specs.first.prompt.should eq("inspect")
        seen.map { |event| event["event"].as_s }
          .should contain("turn-started")

        launcher.emit(0, Xd::Agent::Event.new(
          Xd::Agent::EventType::TextDelta,
          text: "Before."
        ))
        launcher.emit(0, Xd::Agent::Event.new(
          Xd::Agent::EventType::ToolUse,
          text: "Read src/main.cr"
        ))
        launcher.emit(0, Xd::Agent::Event.new(
          Xd::Agent::EventType::TextDelta,
          text: "After."
        ))
        state = engine.dispatch(local, {
          "op"   => "chat",
          "chat" => chat,
        }.to_json)
        state["working"].as_bool.should be_true
        state["turn_id"].as_i64.should be > 0
        state["turn_sequence"].as_i64.should eq(3)
        state["label"].as_s.should start_with("Claude Opus 5 · ")
        state["working_for"].as_i64.should be >= 0
        state["segment"].as_s.should eq("After.")
        state["items"].as_a.map do |item|
          {item["text"].as_s, item["tool"].as_bool}
        end.should eq([
          {"Before.", false},
          {"Read src/main.cr", true},
        ])

        engine.dispatch(local, {
          "op"   => "cancel",
          "chat" => chat,
        }.to_json).success?.should be_true
        launcher.handles.first.canceled.should be_true

        deleted = engine.dispatch(local, {
          "op"   => "delete-chat",
          "chat" => chat,
        }.to_json)
        deleted.success?.should be_true
        launcher.finish_callbacks.first.call(true, nil)
        engine.dispatch(local, {
          "op"   => "chat",
          "chat" => chat,
        }.to_json).success?.should be_false
        engine.events.unsubscribe(subscription)
      end
    ensure
      if path = ENV["XD_AGENT_SECRETS_FILE"]?
        File.delete?(path)
      end
      if old_path
        ENV["XD_AGENT_SECRETS_FILE"] = old_path
      else
        ENV.delete("XD_AGENT_SECRETS_FILE")
      end
    end
  end

  it "retains control-lane cancellation while send is still starting" do
    launcher = EngineLauncher.new
    entered = Channel(Nil).new(1)
    release = Channel(Nil).new(1)
    authorizer : Xd::Agent::Manager::Authorizer = ->(_provider : String) do
      entered.send(nil)
      release.receive
      nil.as(String?)
    end

    with_daemon_engine(
      launcher: launcher,
      agent_authorizer: authorizer
    ) do |_store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      folder = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Cancel race",
      }.to_json)["id"].as_s
      chat = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder,
      }.to_json)["id"].as_s

      sent = Channel(Xd::Protocol::Response).new(1)
      spawn do
        sent.send(engine.dispatch(local, {
          "op"   => "send",
          "chat" => chat,
          "text" => "stop before launch",
        }.to_json))
      end

      entered.receive
      stopped = engine.dispatch(local, {
        "op"   => "cancel",
        "chat" => chat,
      }.to_json)
      release.send(nil)

      stopped.success?.should be_true
      select
      when response = sent.receive
        response.success?.should be_true
      when timeout(2.seconds)
        fail("serialized send did not finish")
      end
      launcher.handles.first.canceled.should be_true
    end
  end

  it "edits, drops, and steers the persisted turn queue" do
    launcher = EngineLauncher.new

    with_daemon_engine(launcher: launcher) do |_store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      folder = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Queue",
      }.to_json)["id"].as_s
      chat = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder,
      }.to_json)["id"].as_s

      %w(running first second).each do |text|
        engine.dispatch(local, {
          "op"   => "send",
          "chat" => chat,
          "text" => text,
        }.to_json).success?.should be_true
      end
      engine.dispatch(local, {
        "op"       => "edit-queue",
        "chat"     => chat,
        "index"    => 1,
        "old-text" => "second",
        "text"     => "second edited",
      }.to_json).success?.should be_true
      engine.dispatch(local, {
        "op"    => "drop-queue",
        "chat"  => chat,
        "index" => 0,
      }.to_json).success?.should be_true
      engine.dispatch(local, {
        "op"   => "send",
        "chat" => chat,
        "text" => "third",
      }.to_json).success?.should be_true
      engine.dispatch(local, {
        "op"    => "steer-queue",
        "chat"  => chat,
        "index" => 1,
        "text"  => "third",
      }.to_json).success?.should be_true

      state = engine.dispatch(local, {
        "op"   => "chat",
        "chat" => chat,
      }.to_json)
      state["queue"].as_a.map(&.as_s).should eq([
        "third",
        "second edited",
      ])
      launcher.handles.first.canceled.should be_true
    end
  end

  it "creates and selects worktrees through the shared engine" do
    launcher = EngineLauncher.new

    with_daemon_engine(launcher: launcher) do |store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      folder = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Repository",
      }.to_json)["id"].as_s
      repository = File.join(
        Path[store.path].dirname,
        "Workspaces",
        "Repository"
      )
      engine_git(repository, "init", "-q", "-b", "main")
      engine_git(repository, "config", "user.email", "test@example.com")
      engine_git(repository, "config", "user.name", "Test")
      File.write(File.join(repository, "tracked.txt"), "initial\n")
      engine_git(repository, "add", "tracked.txt")
      engine_git(repository, "commit", "-q", "-m", "initial")

      first = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder,
      }.to_json)["id"].as_s
      engine.dispatch(local, {
        "op"     => "set-option",
        "chat"   => first,
        "option" => "new-worktree",
        "value"  => "true",
      }.to_json).success?.should be_true
      engine.dispatch(local, {
        "op"   => "send",
        "chat" => first,
        "text" => "Fix parser",
      }.to_json).success?.should be_true

      created = launcher.specs.first.workdir.not_nil!
      created.should_not eq(repository)
      File.directory?(created).should be_true
      first_state = engine.dispatch(local, {
        "op"   => "chat",
        "chat" => first,
      }.to_json)
      first_state["linked_worktree"].as_bool.should be_true
      first_state["worktrees"].as_a.size.should eq(2)
      first_state["context"].as_s.should contain(
        " · Repository (worktree) · "
      )

      second = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder,
      }.to_json)["id"].as_s
      selected = engine.dispatch(local, {
        "op"     => "set-option",
        "chat"   => second,
        "option" => "workspace",
        "value"  => created,
      }.to_json)
      selected.success?.should be_true
      engine.dispatch(local, {
        "op"   => "chat",
        "chat" => second,
      }.to_json).tap do |state|
        state["workdir"].as_s.should eq(created)
        state["context"].as_s.should contain(
          " · Repository (worktree) · "
        )
      end
    end
  end

  it "selects an assistant, model, and supported reasoning effort" do
    with_daemon_engine do |store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      folder = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Models",
      }.to_json)["id"].as_s
      chat = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder,
      }.to_json)["id"].as_s

      selected = engine.dispatch(local, {
        "op"      => "set-option",
        "chat"    => chat,
        "option"  => "model",
        "backend" => "codex",
        "value"   => "gpt-5.6-terra",
      }.to_json)
      selected.success?.should be_true
      state = engine.dispatch(local, {
        "op"   => "chat",
        "chat" => chat,
      }.to_json)
      state["backend"].as_s.should eq("codex")
      state["model"].as_s.should eq("gpt-5.6-terra")
      ultra = engine.dispatch(local, {
        "op"     => "set-option",
        "chat"   => chat,
        "option" => "effort",
        "value"  => "ultra",
      }.to_json)
      ultra.success?.should be_true
      engine.dispatch(local, {
        "op"   => "chat",
        "chat" => chat,
      }.to_json)["effort"].as_s.should eq("ultra")
      fast = engine.dispatch(local, {
        "op"     => "set-option",
        "chat"   => chat,
        "option" => "fast",
        "value"  => "true",
      }.to_json)
      fast.success?.should be_true
      engine.dispatch(local, {
        "op"   => "chat",
        "chat" => chat,
      }.to_json)["fast"].as_bool.should be_true
      claude_mode = engine.dispatch(local, {
        "op"     => "set-option",
        "chat"   => chat,
        "option" => "claude-mode",
        "value"  => "true",
      }.to_json)
      claude_mode.success?.should be_true
      mode_state = engine.dispatch(local, {
        "op"   => "chat",
        "chat" => chat,
      }.to_json)
      mode_state["claude_mode"].as_bool.should be_true
      mode_state["effort"].as_s.should eq("max")
      rejected_mode_ultra = engine.dispatch(local, {
        "op"     => "set-option",
        "chat"   => chat,
        "option" => "effort",
        "value"  => "ultra",
      }.to_json)
      rejected_mode_ultra.success?.should be_false
      mode_low = engine.dispatch(local, {
        "op"     => "set-option",
        "chat"   => chat,
        "option" => "effort",
        "value"  => "low",
      }.to_json)
      mode_low.success?.should be_true
      engine.dispatch(local, {
        "op"   => "chat",
        "chat" => chat,
      }.to_json)["effort"].as_s.should eq("low")
      switch = engine.dispatch(local, {
        "op"   => "messages",
        "chat" => chat,
      }.to_json)["messages"].as_a
      switch.map { |message| message["role"].as_s }.should eq(["event"])
      switch.first["content"].as_s.should eq("Switched to GPT-5.6 Terra")

      duplicate = engine.dispatch(local, {
        "op"      => "set-option",
        "chat"    => chat,
        "option"  => "model",
        "backend" => "codex",
        "value"   => "gpt-5.6-terra",
      }.to_json)
      duplicate.success?.should be_true
      unchanged_messages = engine.dispatch(local, {
        "op"   => "messages",
        "chat" => chat,
      }.to_json)["messages"].as_a
      unchanged_messages.size.should eq(1)

      rejected = engine.dispatch(local, {
        "op"      => "set-option",
        "chat"    => chat,
        "option"  => "model",
        "backend" => "claude",
        "value"   => "gpt-5.6-terra",
      }.to_json)
      rejected.success?.should be_false
      unchanged = engine.dispatch(local, {
        "op"   => "chat",
        "chat" => chat,
      }.to_json)
      unchanged["backend"].as_s.should eq("codex")
      unchanged["model"].as_s.should eq("gpt-5.6-terra")

      claude = engine.dispatch(local, {
        "op"      => "set-option",
        "chat"    => chat,
        "option"  => "model",
        "backend" => "claude",
        "value"   => "claude-opus-5",
      }.to_json)
      claude.success?.should be_true
      store.get_chat(chat).effort.should eq("low")
      store.get_chat(chat).fast.should be_false
      store.get_chat(chat).claude_mode.should be_false
      rejected_ultra = engine.dispatch(local, {
        "op"     => "set-option",
        "chat"   => chat,
        "option" => "effort",
        "value"  => "ultra",
      }.to_json)
      rejected_ultra.success?.should be_false
      rejected_fast = engine.dispatch(local, {
        "op"     => "set-option",
        "chat"   => chat,
        "option" => "fast",
        "value"  => "true",
      }.to_json)
      rejected_fast.success?.should be_false
    end
  end

  it "runs Git actions through the shared asynchronous daemon path" do
    with_daemon_engine do |store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      remote = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote)
      remote.authenticated = true
      folder_id = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Git Actions",
      }.to_json)["id"].as_s
      folder = File.join(
        Path[store.path].dirname,
        "Workspaces",
        "Git Actions"
      )
      engine_git(folder, "init", "-q", "-b", "main")
      engine_git(folder, "config", "user.email", "test@example.com")
      engine_git(folder, "config", "user.name", "Test")
      File.write(File.join(folder, "tracked.txt"), "before\n")
      engine_git(folder, "add", "tracked.txt")
      engine_git(folder, "commit", "-q", "-m", "initial")
      chat_id = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder_id,
      }.to_json)["id"].as_s
      File.write(File.join(folder, "tracked.txt"), "after\n")

      seen = [] of Xd::Protocol::Event
      subscription = engine.events.subscribe { |event| seen << event }
      state = engine.process(local, {
        "op"      => "git-state",
        "chat"    => chat_id,
        "request" => "local-state",
      }.to_json)
      state.response.success?.should be_true
      state.after_write.not_nil!.call

      deadline = Time.instant + 3.seconds
      until seen.any? { |event|
              event["event"].as_s == "git-state" &&
              event["request"].as_s == "local-state"
            }
        fail "Git state event did not arrive" if Time.instant >= deadline
        sleep 10.milliseconds
      end
      state_event = seen.find { |event|
        event["event"].as_s == "git-state" &&
          event["request"].as_s == "local-state"
      }.not_nil!
      state_event["action"].as_s.should eq("commit")

      action = engine.process(remote, {
        "op"      => "git-action",
        "chat"    => chat_id,
        "action"  => "commit",
        "message" => "Commit over TLS",
        "request" => "remote-action",
      }.to_json)
      action.response.success?.should be_true
      action.after_write.not_nil!.call

      deadline = Time.instant + 3.seconds
      until seen.any? { |event|
              event["event"].as_s == "git-action-finished" &&
              event["request"].as_s == "remote-action"
            }
        fail "Git action event did not arrive" if Time.instant >= deadline
        sleep 10.milliseconds
      end
      action_event = seen.find { |event|
        event["event"].as_s == "git-action-finished" &&
          event["request"].as_s == "remote-action"
      }.not_nil!
      action_event["success"].as_bool.should be_true
      action_event["action"].as_s.should eq("push")
      engine.events.unsubscribe(subscription)

      output = IO::Memory.new
      status = Process.run(
        "git",
        ["log", "-1", "--format=%s"],
        chdir: folder,
        output: output,
        error: Process::Redirect::Close
      )
      status.success?.should be_true
      output.to_s.should eq("Commit over TLS\n")
    end
  end

  it "drafts editable Git metadata through the selected assistant" do
    launcher = EngineLauncher.new
    with_daemon_engine(launcher: launcher) do |store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      folder_id = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Git Draft",
      }.to_json)["id"].as_s
      folder = File.join(Path[store.path].dirname, "Workspaces", "Git Draft")
      engine_git(folder, "init", "-q", "-b", "main")
      engine_git(folder, "config", "user.email", "test@example.com")
      engine_git(folder, "config", "user.name", "Test")
      File.write(File.join(folder, "tracked.txt"), "before\n")
      engine_git(folder, "add", "tracked.txt")
      engine_git(folder, "commit", "-q", "-m", "initial")
      chat_id = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder_id,
      }.to_json)["id"].as_s
      File.write(File.join(folder, "tracked.txt"), "after\n")

      seen = [] of Xd::Protocol::Event
      subscription = engine.events.subscribe { |event| seen << event }
      outcome = engine.process(local, {
        "op"      => "git-draft",
        "chat"    => chat_id,
        "kind"    => "commit",
        "backend" => "codex",
        "model"   => "gpt-5.6-terra",
        "request" => "draft-1",
      }.to_json)
      outcome.response.success?.should be_true
      outcome.after_write.not_nil!.call

      deadline = Time.instant + 3.seconds
      until launcher.specs.size == 1
        fail "Git draft agent did not start" if Time.instant >= deadline
        sleep 10.milliseconds
      end
      spec = launcher.specs.first
      spec.model.should eq("gpt-5.6-terra")
      spec.access.should eq(Xd::Agent::Access::ReadOnly)
      spec.prompt.should contain("Working tree diff:")
      spec.prompt.should contain("+after")

      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::TextDelta,
        text: %({"title":"fix: update tracked text","body":"Keeps state current."})
      ))
      launcher.finish(0, true)

      event = seen.find { |item|
        item["event"].as_s == "git-draft-finished" &&
          item["request"].as_s == "draft-1"
      }.not_nil!
      event["success"].as_bool.should be_true
      event["title"].as_s.should eq("fix: update tracked text")
      event["body"].as_s.should eq("Keeps state current.")
      store.list_messages(chat_id).should be_empty
      engine.events.unsubscribe(subscription)
    end
  end

  it "routes terminal sessions through the shared daemon engine" do
    with_daemon_engine do |_store, engine|
      local = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Local)
      folder = engine.dispatch(local, {
        "op"   => "new-folder",
        "name" => "Terminal",
      }.to_json)["id"].as_s
      chat = engine.dispatch(local, {
        "op"     => "new-chat",
        "folder" => folder,
      }.to_json)["id"].as_s

      seen = [] of Xd::Protocol::Event
      subscription = engine.events.subscribe { |event| seen << event }
      opened = engine.process(local, {
        "op"      => "terminal-open",
        "chat"    => chat,
        "columns" => 100,
        "rows"    => 30,
      }.to_json)
      opened.response.success?.should be_true
      opened.events.map { |event| event["event"].as_s }
        .should eq(["terminal-opened"])
      opened.after_write.not_nil!.call
      terminal_id = opened.response["id"].as_s

      engine.dispatch(local, {
        "op"       => "terminal-input",
        "terminal" => terminal_id,
        "data"     => Base64.strict_encode(
          "printf '\\nENGINE_PTY_OK\\n'\n"
        ),
      }.to_json).success?.should be_true

      deadline = Time.instant + 3.seconds
      until seen.any? { |event|
              event["event"].as_s == "terminal-output" &&
              Base64.decode_string(event["data"].as_s)
                .includes?("ENGINE_PTY_OK")
            }
        fail "terminal output did not arrive" if Time.instant >= deadline
        sleep 10.milliseconds
      end

      listed = engine.dispatch(local, {
        "op"   => "terminal-list",
        "chat" => chat,
      }.to_json)
      row = listed["terminals"].as_a.first
      row["id"].as_s.should eq(terminal_id)
      row["columns"].as_i.should eq(100)
      replay = row["replay"].as_a
        .compact_map { |item| item["data"]?.try(&.as_s?) }
        .map { |data| Base64.decode_string(data) }
        .join
      replay.should contain("ENGINE_PTY_OK")

      resized = engine.process(local, {
        "op"       => "terminal-resize",
        "terminal" => terminal_id,
        "columns"  => 120,
        "rows"     => 40,
      }.to_json)
      resized.response.success?.should be_true
      resized.events.first["event"].as_s.should eq("terminal-resized")
      resized.events.first["columns"].as_i.should eq(120)

      reused = engine.process(local, {
        "op"    => "terminal-open",
        "chat"  => chat,
        "reuse" => true,
      }.to_json)
      reused.response["id"].as_s.should eq(terminal_id)
      reused.events.should be_empty
      reused.after_write.should be_nil

      engine.dispatch(local, {
        "op"       => "terminal-kill",
        "terminal" => terminal_id,
      }.to_json).success?.should be_true
      engine.events.unsubscribe(subscription)
    end
  end
end
