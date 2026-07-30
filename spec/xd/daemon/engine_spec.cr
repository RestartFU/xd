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
end

private def with_daemon_engine(
  clock : Proc(Time::Instant) = -> { Time.instant },
  token_generator : Proc(String) = -> { Random::Secure.base64(32) },
  launcher : Xd::Agent::Launcher? = nil,
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
    launcher: launcher
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
