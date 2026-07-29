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

private def with_agent_manager(
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
    }
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
