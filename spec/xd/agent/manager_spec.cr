require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/manager"

private class FakeSessionHandle < Xd::Agent::SessionHandle
  getter canceled = false

  def cancel : Nil
    @canceled = true
  end
end

private class FakeLauncher < Xd::Agent::Launcher
  getter backends = [] of String
  getter specs = [] of Xd::Agent::RunSpec
  getter environments = [] of Hash(String, String)
  getter secret_names = [] of Array(String)
  getter handles = [] of FakeSessionHandle
  getter event_callbacks = [] of Proc(Xd::Agent::Event, Nil)
  getter finish_callbacks = [] of Proc(Bool, String?, Nil)
  getter closed = false

  def start(
    backend : Xd::Agent::Backend,
    spec : Xd::Agent::RunSpec,
    environment : Hash(String, String),
    secret_names : Array(String),
    on_event : Proc(Xd::Agent::Event, Nil),
    on_finished : Proc(Bool, String?, Nil),
  ) : Xd::Agent::SessionHandle
    handle = FakeSessionHandle.new
    @backends << backend.id
    @specs << spec
    @environments << environment
    @secret_names << secret_names
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

  def close : Nil
    @closed = true
  end
end

private def manager_git(workdir : String, *arguments : String) : Nil
  status = Process.run(
    "git",
    arguments,
    chdir: workdir,
    output: Process::Redirect::Close,
    error: Process::Redirect::Close
  )
  status.success?.should be_true
end

private def with_agent_manager(
  authorizer : Xd::Agent::Manager::Authorizer = ->(_provider : String) : String? { nil },
  & : Xd::Agent::Manager, Xd::Storage::Store, Xd::Workspace::Service, String, FakeLauncher, Array(Tuple(String, Hash(String, JSON::Any))) ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-manager-#{Random::Secure.hex(12)}"
  )
  old_secrets = ENV["XD_AGENT_SECRETS_FILE"]?
  ENV["XD_AGENT_SECRETS_FILE"] = File.join(directory, "secrets.json")
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  workspaces = Xd::Workspace::Service.new(
    File.join(directory, "Workspaces"),
    store
  )
  folder_id = workspaces.create_folder(nil, "Project")
  launcher = FakeLauncher.new
  events = [] of Tuple(String, Hash(String, JSON::Any))
  manager = Xd::Agent::Manager.new(
    store,
    workspaces,
    launcher,
    ->(name : String, fields : Hash(String, JSON::Any)) {
      events << {name, fields}
      nil
    },
    authorizer: authorizer
  )

  begin
    yield manager, store, workspaces, folder_id, launcher, events
  ensure
    manager.close
    store.close
    if old_secrets
      ENV["XD_AGENT_SECRETS_FILE"] = old_secrets
    else
      ENV.delete("XD_AGENT_SECRETS_FILE")
    end
    FileUtils.rm_r(directory)
  end
end

describe Xd::Agent::Manager do
  it "rejects unsigned assistants before storing or launching a turn" do
    checked = [] of String
    authorizer = ->(provider : String) : String? {
      checked << provider
      "Sign in to Claude Code before starting a turn."
    }
    with_agent_manager(authorizer) do |manager, store, _workspaces, folder_id, launcher, _events|
      chat_id = store.create_chat(folder_id, "New Chat", "claude")

      expect_raises(
        Xd::Agent::Manager::Error,
        "Sign in to Claude Code before starting a turn."
      ) do
        manager.send(chat_id, "do not send this upstream")
      end

      checked.should eq(["claude"])
      launcher.specs.should be_empty
      store.list_messages(chat_id).should be_empty
      store.get_chat(chat_id).title.should eq("New Chat")
      store.get_chat(chat_id).daemon_working.should be_false
    end
  end

  it "coordinates CLI updates with active and new turns" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, _events|
      chat_id = store.create_chat(folder_id, "Chat", "codex")

      manager.begin_backend_update("codex")
      expect_raises(
        Xd::Agent::Manager::Error,
        "Codex is updating. Try again when it finishes."
      ) do
        manager.send(chat_id, "blocked during replacement")
      end
      launcher.specs.should be_empty
      store.list_messages(chat_id).should be_empty
      manager.finish_backend_update("codex", true)

      manager.send(chat_id, "running")
      expect_raises(
        Xd::Agent::Manager::Error,
        "Stop active assistant turns before updating bundled CLIs."
      ) do
        manager.begin_backend_update("codex")
      end
      launcher.finish(0, true)
    end
  end

  it "stores and broadcasts a complete streamed turn" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, events|
      chat_id = store.create_chat(folder_id, "New Chat", "claude")

      manager.send(chat_id, "inspect this").started?.should be_true
      store.get_chat(chat_id).daemon_working.should be_true
      store.get_chat(chat_id).title.should eq("inspect this")
      launcher.specs.first.prompt.should eq("inspect this")
      launcher.specs.first.system_prompt.not_nil!
        .should contain("<asking_the_user>")
      launcher.environments.first["DISABLE_AUTOUPDATER"].should eq("1")

      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::SessionStarted,
        session_id: "session-1"
      ))
      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::Commands,
        commands: ["review"]
      ))
      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::TextDelta,
        text: "before"
      ))
      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::ToolUse,
        text: "Read src/main.cr"
      ))
      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::TextDelta,
        text: "after"
      ))
      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::Usage,
        context_used: 120_u64,
        context_window: 1_000_u64
      ))
      launcher.finish(0, true)

      store.get_chat(chat_id).daemon_working.should be_false
      store.get_session_id(chat_id, "claude").should eq("session-1")
      store.get_context_usage(chat_id, "claude", "claude-opus-5")
        .should eq(Xd::Storage::ContextUsage.new(120_u64, 1_000_u64))
      messages = store.list_messages(chat_id)
      messages.map(&.role).should eq(
        ["user", "assistant", "tool", "assistant", "duration"]
      )
      messages.map(&.content)[0...4].should eq([
        "inspect this",
        "before",
        "Read src/main.cr",
        "after",
      ])
      events.map(&.[0]).should contain("turn-started")
      events.map(&.[0]).should contain("commands")
      events.map(&.[0]).should contain("text")
      events.map(&.[0]).should contain("tool")
      events.map(&.[0]).should contain("turn-finished")
      manager.commands(chat_id).should eq(["review"])
    end
  end

  it "persists sends behind a running turn and starts them in order" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, _events|
      chat_id = store.create_chat(folder_id, "Chat", "claude")

      manager.send(chat_id, "first").started?.should be_true
      manager.send(chat_id, "second").queued?.should be_true
      store.get_chat(chat_id).queue.should eq(["second"])

      launcher.finish(0, true)
      launcher.specs.size.should eq(2)
      launcher.specs[1].prompt.should eq("second")
      store.get_chat(chat_id).queue.should be_empty
      store.get_chat(chat_id).daemon_working.should be_true

      launcher.finish(1, true)
      store.get_chat(chat_id).daemon_working.should be_false
      store.list_messages(chat_id)
        .select(&.role.==("user"))
        .map(&.content)
        .should eq(["first", "second"])
    end
  end

  it "retries one stale resumed session without duplicating the user row" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, events|
      chat_id = store.create_chat(folder_id, "Chat", "claude")
      store.set_session_id(chat_id, "claude", "stale-session")

      manager.send(chat_id, "continue")
      launcher.specs[0].resume_session_id.should eq("stale-session")

      launcher.finish(0, false, "Session no longer exists")

      launcher.specs.size.should eq(2)
      launcher.specs[1].resume_session_id.should be_nil
      launcher.specs[1].prompt.should eq("continue")
      store.list_messages(chat_id).map(&.role).should eq(["user"])
      store.list_messages(chat_id).map(&.content).should eq(["continue"])
      events.count(&.[0].==("turn-started")).should eq(2)
      events.count(&.[0].==("turn-finished")).should eq(0)

      launcher.emit(1, Xd::Agent::Event.new(
        Xd::Agent::EventType::TextDelta,
        text: "Recovered."
      ))
      launcher.finish(1, true)

      store.list_messages(chat_id).map(&.role).should eq([
        "user",
        "assistant",
        "duration",
      ])
      store.list_messages(chat_id).map(&.content).first(2).should eq([
        "continue",
        "Recovered.",
      ])
      events.count(&.[0].==("turn-finished")).should eq(1)
    end
  end

  it "does not retry the stale-session fallback more than once" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, events|
      chat_id = store.create_chat(folder_id, "Chat", "claude")
      store.set_session_id(chat_id, "claude", "stale-session")

      manager.send(chat_id, "continue")
      launcher.finish(0, false, "First failure")
      launcher.finish(1, false)

      launcher.specs.size.should eq(2)
      store.get_session_id(chat_id, "claude").should be_nil
      store.get_last_seen(chat_id, "claude").should eq(0)
      store.list_messages(chat_id).map(&.role).should eq([
        "user",
        "duration",
        "error",
      ])
      store.list_messages(chat_id).last.content.should eq(
        "The backend stopped unexpectedly."
      )
      finished = events.select(&.[0].==("turn-finished"))
      finished.size.should eq(1)
      finished.first[1]["error"].as_s.should eq(
        "The backend stopped unexpectedly."
      )
    end
  end

  it "stores a labeled no-reply row when a successful turn is silent" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, events|
      chat_id = store.create_chat(folder_id, "Chat", "claude")

      manager.send(chat_id, "hello?")
      launcher.finish(0, true)

      messages = store.list_messages(chat_id)
      messages.map(&.role).should eq([
        "user",
        "assistant",
        "duration",
      ])
      messages[1].content.should eq("(no reply)")
      messages[1].label.not_nil!.should start_with("Claude Opus 5 · ")
      store.get_last_seen(chat_id, "claude").should eq(messages.last.id)
      finished = events.reverse.find(&.[0].==("turn-finished")).not_nil![1]
      finished["silent"].as_bool.should be_true
      finished["duration"].as_i64.should be >= 0
      finished["last_message_id"].as_i64.should eq(messages.last.id)
    end
  end

  it "keeps last-seen at the previous success after an output failure" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, _events|
      chat_id = store.create_chat(folder_id, "Chat", "claude")
      store.append_message(chat_id, "user", "old question")
      previous = store.append_message(chat_id, "assistant", "old answer")
      store.set_session_id(chat_id, "claude", "live-session")
      store.set_last_seen(chat_id, "claude", previous)

      manager.send(chat_id, "continue")
      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::TextDelta,
        text: "Partial."
      ))
      launcher.finish(0, false, "Connection lost")

      launcher.specs.size.should eq(1)
      store.get_last_seen(chat_id, "claude").should eq(previous)
      store.list_messages(chat_id).last.content.should eq("Connection lost")
    end
  end

  it "hides an ask block while streaming and reports waiting" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, events|
      chat_id = store.create_chat(folder_id, "Chat", "claude")
      manager.send(chat_id, "decide")

      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::TextDelta,
        text: "Context.<as"
      ))
      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::TextDelta,
        text: "k>\nChoose?\n- First\n- Second\n</ask>"
      ))
      launcher.finish(0, true)

      streamed = events.select(&.[0].==("text"))
      streamed.map { |event| event[1]["text"].as_s }.should eq(["Context."])
      stored = store.list_messages(chat_id).find(&.role.==("assistant"))
        .not_nil!.content
      stored.should eq(
        "Context.<ask>\nChoose?\n- First\n- Second\n</ask>"
      )
      finished = events.reverse.find(&.[0].==("turn-finished")).not_nil!
      finished[1]["waiting"].as_bool.should be_true
      finished[1]["question"].as_s.should eq("Choose?")
      finished[1]["options"].as_a.map(&.as_s).should eq([
        "First",
        "Second",
      ])
      finished[1]["accepts_input"].as_bool.should be_false
    end
  end

  it "snapshots a running turn for clients that join late" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, _events|
      chat_id = store.create_chat(folder_id, "Chat", "claude")
      manager.send(chat_id, "inspect")

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
        text: "After.<ask>\nChoose?\n- Yes\n- No\n</ask>"
      ))

      snapshot = manager.active_turn(chat_id).not_nil!
      snapshot.label.should start_with("Claude Opus 5 · ")
      snapshot.working_for.should be >= 0
      snapshot.items.should eq([
        Xd::Agent::TurnItem.new("Before.", false),
        Xd::Agent::TurnItem.new("Read src/main.cr", true),
      ])
      snapshot.segment.should eq("After.")

      launcher.finish(0, true)
      manager.active_turn(chat_id).should be_nil
    end
  end

  it "accepts only a registered workspace report" do
    with_agent_manager do |manager, store, workspaces, folder_id, launcher, events|
      repository = workspaces.find_folder(folder_id)
      worktree = File.join(File.dirname(workspaces.root), "linked")
      manager_git(repository, "init", "-q", "-b", "main")
      manager_git(repository, "config", "user.email", "test@example.com")
      manager_git(repository, "config", "user.name", "Test")
      File.write(File.join(repository, "tracked.txt"), "initial\n")
      manager_git(repository, "add", "tracked.txt")
      manager_git(repository, "commit", "-q", "-m", "initial")
      manager_git(repository, "worktree", "add", "-q", "-b", "linked", worktree)
      chat_id = store.create_chat(folder_id, "Chat", "claude")

      manager.send(chat_id, "move")
      launcher.emit(0, Xd::Agent::Event.new(
        Xd::Agent::EventType::TextDelta,
        text: "Moved.\n<workspace>#{worktree}</workspace>"
      ))
      launcher.finish(0, true)

      store.get_chat(chat_id).workdir.should eq(File.realpath(worktree))
      assistant = store.list_messages(chat_id)
        .find(&.role.==("assistant")).not_nil!
      assistant.content.should eq("Moved.")
      events.select(&.[0].==("text"))
        .map { |event| event[1]["text"].as_s }
        .should eq(["Moved.\n"])
    end
  end

  it "cancels the daemon-owned session" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, _events|
      chat_id = store.create_chat(folder_id, "Chat", "claude")
      manager.send(chat_id, "work")

      manager.cancel(chat_id)

      launcher.handles.first.canceled.should be_true
    end
  end

  it "runs selected options and restores access after plan mode" do
    with_agent_manager do |manager, store, _workspaces, folder_id, launcher, _events|
      chat_id = store.create_chat(folder_id, "Chat", "claude")
      store.set_model_selection(chat_id, "codex", "gpt-5.6-terra")
      store.set_effort(chat_id, "xhigh")
      store.set_access(chat_id, "full")
      store.set_plan(chat_id, true)

      manager.send(chat_id, "plan this")
      launcher.backends.first.should eq("codex")
      launcher.specs.first.model.should eq("gpt-5.6-terra")
      launcher.specs.first.effort.should eq(Xd::Agent::Effort::XHigh)
      launcher.specs.first.access.should eq(Xd::Agent::Access::Plan)
      manager.active_turn(chat_id).not_nil!.label.should eq(
        "GPT-5.6 Terra · Extra high"
      )
      launcher.finish(0, true)

      store.set_plan(chat_id, false)
      manager.send(chat_id, "build this")
      launcher.specs[1].model.should eq("gpt-5.6-terra")
      launcher.specs[1].effort.should eq(Xd::Agent::Effort::XHigh)
      launcher.specs[1].access.should eq(Xd::Agent::Access::Full)
      launcher.finish(1, true)
    end
  end

  it "injects scoped secret values only into the child environment" do
    with_agent_manager do |manager, store, workspaces, folder_id, launcher, _events|
      child_id = workspaces.create_folder(folder_id, "Child")
      global = Xd::Agent::Secrets.load
      global.set("GLOBAL_TOKEN", "global-value")
      global.save
      scoped = Xd::Agent::Secrets.for_folder(child_id)
      scoped.set("PROJECT_KEY", "project-value")
      scoped.save
      chat_id = store.create_chat(child_id, "Chat", "claude")

      manager.send(chat_id, "use credentials")

      launcher.environments.first["GLOBAL_TOKEN"].should eq("global-value")
      launcher.environments.first["PROJECT_KEY"].should eq("project-value")
      launcher.secret_names.first.should eq(["GLOBAL_TOKEN", "PROJECT_KEY"])
      prompt = launcher.specs.first.system_prompt.not_nil!
      prompt.should contain("GLOBAL_TOKEN")
      prompt.should contain("PROJECT_KEY")
      prompt.should_not contain("global-value")
      prompt.should_not contain("project-value")
    end
  end
end
