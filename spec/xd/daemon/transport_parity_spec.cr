require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/certificate"
require "../../../src/xd/daemon/client"
require "../../../src/xd/daemon/server"

private class MatrixHandle < Xd::Agent::SessionHandle
  getter canceled = false

  def cancel : Nil
    @canceled = true
  end
end

private class MatrixLauncher < Xd::Agent::Launcher
  getter backends = [] of String
  getter specs = [] of Xd::Agent::RunSpec
  getter environments = [] of Hash(String, String)
  getter secret_names = [] of Array(String)
  getter handles = [] of MatrixHandle

  def start(
    backend : Xd::Agent::Backend,
    spec : Xd::Agent::RunSpec,
    environment : Hash(String, String),
    secret_names : Array(String),
    on_event : Proc(Xd::Agent::Event, Nil),
    on_finished : Proc(Bool, String?, Nil),
  ) : Xd::Agent::SessionHandle
    handle = MatrixHandle.new
    @backends << backend.id
    @specs << spec
    @environments << environment
    @secret_names << secret_names
    @handles << handle
    handle
  end
end

private record TransportTrace,
  tree : Array(String),
  folder_context : String,
  folder_backend : String?,
  folder_model : String?,
  directory_entries : Array(String),
  file_entries : Array(String),
  file_content : String,
  diff_contains_change : Bool,
  chat_title : String,
  chat_backend : String,
  chat_model : String?,
  chat_effort : String?,
  chat_access : String?,
  chat_plan : Bool,
  chat_fast : Bool,
  chat_queue : Array(String),
  chat_working : Bool,
  message_roles : Array(String),
  message_contents : Array(String),
  search_snippets : Array(String),
  secret_names : Array(String),
  run_backend : String,
  run_model : String?,
  run_effort : String,
  run_access : String,
  run_fast : Bool,
  run_prompt : String,
  run_secret_names : Array(String),
  run_secret_value : String?,
  canceled : Bool,
  terminal_dimensions : {Int64, Int64},
  voice_model_available : Bool,
  deleted : Bool

private def matrix_call(
  client : Xd::Daemon::Client,
  fields,
) : Hash(String, JSON::Any)
  client.call(JSON.parse(fields.to_json).as_h)
end

private def matrix_git(path : String, *arguments : String) : Nil
  status = Process.run(
    "git",
    arguments,
    chdir: path,
    output: Process::Redirect::Close,
    error: Process::Redirect::Close
  )
  status.success?.should be_true
end

private def await_matrix_events(
  events : Array(Hash(String, JSON::Any)),
  mutex : Mutex,
  required : Array(String),
) : Nil
  deadline = Time.instant + 3.seconds
  loop do
    names = mutex.synchronize do
      events.compact_map { |event| event["event"]?.try(&.as_s?) }
    end
    return if required.all? { |name| names.includes?(name) }
    fail "transport events did not settle: #{names}" if Time.instant >= deadline
    sleep 5.milliseconds
  end
end

private def run_transport_matrix(
  transport : Xd::Daemon::Transport,
) : TransportTrace
  directory = File.join(
    Dir.tempdir,
    "xd-transport-matrix-#{transport}-#{Random::Secure.hex(12)}"
  )
  Dir.mkdir_p(directory)
  old_secrets = ENV["XD_AGENT_SECRETS_FILE"]?
  old_cache = ENV["XDG_CACHE_HOME"]?
  old_name = ENV["XD_DATA_NAME"]?
  ENV["XD_AGENT_SECRETS_FILE"] = File.join(directory, "secrets.json")
  ENV["XDG_CACHE_HOME"] = File.join(directory, "cache")
  ENV["XD_DATA_NAME"] = "xd-transport-matrix"

  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  workspaces = Xd::Workspace::Service.new(
    File.join(directory, "Workspaces"),
    store
  )
  launcher = MatrixLauncher.new
  engine = Xd::Daemon::Engine.new(
    store,
    workspaces,
    token_generator: -> { "matrix-token" },
    launcher: launcher,
    authentication_resolver: ->(_provider : String) { "/bin/false" },
    authentication_environment: {} of String => String,
    agent_authorizer: ->(_provider : String) : String? { nil }
  )
  server = Xd::Daemon::Server.new(engine)
  client : Xd::Daemon::Client? = nil

  begin
    client = if transport.local?
               socket = File.join(directory, "daemon.sock")
               server.listen_local(socket)
               Xd::Daemon::Client.local(socket)
             else
               certificate = File.join(directory, "certificate.pem")
               private_key = File.join(directory, "private-key.pem")
               Xd::Daemon::Certificate.ensure_pair(
                 certificate,
                 private_key
               )
               port = server.listen_remote(
                 "127.0.0.1",
                 0,
                 certificate,
                 private_key
               )
               Xd::Daemon::Client.pair_remote(
                 "127.0.0.1",
                 port,
                 engine.arm_pairing(1.minute, "matrix client"),
                 "matrix-client"
               ).client
             end
    endpoint = client.not_nil!
    events = [] of Hash(String, JSON::Any)
    event_mutex = Mutex.new
    endpoint.subscribe do |event|
      event_mutex.synchronize { events << event }
    end

    matrix_call(endpoint, {"op" => "ping"})["ok"].as_bool.should be_true
    folder_id = matrix_call(endpoint, {
      "op"   => "new-folder",
      "name" => "Parity",
    })["id"].as_s
    folder = workspaces.find_folder(folder_id)
    Dir.mkdir(File.join(folder, "src"))
    File.write(File.join(folder, "note.txt"), "before\n")
    matrix_git(folder, "init", "-q", "-b", "main")
    matrix_git(folder, "config", "user.email", "matrix@example.com")
    matrix_git(folder, "config", "user.name", "Matrix")
    matrix_git(folder, "add", "note.txt")
    matrix_git(folder, "commit", "-q", "-m", "initial")

    matrix_call(endpoint, {
      "op"      => "set-folder-context",
      "folder"  => folder_id,
      "context" => "Use one daemon.",
    })
    matrix_call(endpoint, {
      "op"      => "set-folder-settings",
      "folder"  => folder_id,
      "backend" => nil,
      "model"   => nil,
      "workdir" => folder,
      "repo"    => folder,
    })
    folder_state = matrix_call(endpoint, {
      "op"     => "folder-settings",
      "folder" => folder_id,
    })
    folder_context = matrix_call(endpoint, {
      "op"     => "folder-context",
      "folder" => folder_id,
    })["context"].as_s

    chat_id = matrix_call(endpoint, {
      "op"     => "new-chat",
      "folder" => folder_id,
      "title"  => "Transport",
    })["id"].as_s
    matrix_call(endpoint, {
      "op"    => "rename-chat",
      "chat"  => chat_id,
      "title" => "Transport parity",
    })
    matrix_call(endpoint, {
      "op"      => "set-option",
      "chat"    => chat_id,
      "option"  => "model",
      "backend" => "codex",
      "value"   => "gpt-5.6-terra",
    })
    {
      "effort" => "xhigh",
      "access" => "full",
      "plan"   => "true",
      "fast"   => "true",
    }.each do |option, value|
      matrix_call(endpoint, {
        "op"     => "set-option",
        "chat"   => chat_id,
        "option" => option,
        "value"  => value,
      })
    end

    matrix_call(endpoint, {
      "op"   => "queue",
      "chat" => chat_id,
      "text" => "first queued",
    })
    matrix_call(endpoint, {
      "op"       => "edit-queue",
      "chat"     => chat_id,
      "index"    => 0,
      "old-text" => "first queued",
      "text"     => "edited queued",
    })
    matrix_call(endpoint, {
      "op"   => "queue",
      "chat" => chat_id,
      "text" => "remove queued",
    })
    matrix_call(endpoint, {
      "op"    => "drop-queue",
      "chat"  => chat_id,
      "index" => 1,
    })

    matrix_call(endpoint, {
      "op"      => "set-agent-secrets",
      "entries" => [{
        "name"  => "PARITY_TOKEN",
        "value" => "daemon-owned",
      }],
    })
    listed_secrets = matrix_call(endpoint, {
      "op" => "agent-secrets",
    })["names"].as_a.map(&.as_s)

    directory_entries = matrix_call(endpoint, {
      "op"   => "list-dir",
      "path" => folder,
    })["entries"].as_a.map(&.as_s)
    file_entries = matrix_call(endpoint, {
      "op"     => "file-browse",
      "chat"   => chat_id,
      "action" => "list",
      "path"   => "",
    })["entries"].as_a.map { |entry| entry["name"].as_s }
    matrix_call(endpoint, {
      "op"      => "file-browse",
      "chat"    => chat_id,
      "action"  => "write",
      "path"    => "note.txt",
      "content" => "after\n",
    })
    file_content = matrix_call(endpoint, {
      "op"     => "file-browse",
      "chat"   => chat_id,
      "action" => "read",
      "path"   => "note.txt",
    })["content"].as_s
    diff = matrix_call(endpoint, {
      "op"   => "diff-read",
      "chat" => chat_id,
      "read" => "working-all",
    })["output"].as_s

    sent = matrix_call(endpoint, {
      "op"   => "send",
      "chat" => chat_id,
      "text" => "transport sentinel",
    })
    sent["queued"].as_bool.should be_false
    chat = matrix_call(endpoint, {
      "op"   => "chat",
      "chat" => chat_id,
    })
    messages = matrix_call(endpoint, {
      "op"   => "messages",
      "chat" => chat_id,
    })["messages"].as_a
    search = matrix_call(endpoint, {
      "op"    => "search",
      "query" => "transport sentinel",
    })["results"].as_a

    terminal_id = matrix_call(endpoint, {
      "op"      => "terminal-open",
      "chat"    => chat_id,
      "columns" => 90,
      "rows"    => 24,
    })["id"].as_s
    matrix_call(endpoint, {
      "op"       => "terminal-resize",
      "terminal" => terminal_id,
      "columns"  => 110,
      "rows"     => 32,
    })
    terminal = matrix_call(endpoint, {
      "op"   => "terminal-list",
      "chat" => chat_id,
    })["terminals"].as_a.first
    dimensions = {
      terminal["columns"].as_i64,
      terminal["rows"].as_i64,
    }
    matrix_call(endpoint, {
      "op"       => "terminal-kill",
      "terminal" => terminal_id,
    })

    voice_available = matrix_call(endpoint, {
      "op"   => "voice-model",
      "chat" => chat_id,
    })["available"].as_bool
    matrix_call(endpoint, {
      "op"   => "cancel",
      "chat" => chat_id,
    })
    await_matrix_events(
      events,
      event_mutex,
      ["tree", "changed", "queued", "turn-started", "terminal-opened",
       "terminal-resized"]
    )

    spec = launcher.specs.first
    handle = launcher.handles.first
    backend = launcher.backends.first
    environment = launcher.environments.first
    run_secret_names = launcher.secret_names.first

    tree = matrix_call(endpoint, {"op" => "tree"})
    matrix_call(endpoint, {
      "op"   => "delete-chat",
      "chat" => chat_id,
    })
    deleted = matrix_call(endpoint, {"op" => "tree"})["chats"].as_a.empty?

    TransportTrace.new(
      tree["folders"].as_a.map { |item| item["name"].as_s },
      folder_context,
      folder_state["backend"].as_s?,
      folder_state["model"].as_s?,
      directory_entries,
      file_entries,
      file_content,
      diff.includes?("-before") && diff.includes?("+after"),
      chat["title"].as_s,
      chat["backend"].as_s,
      chat["model"]?.try(&.as_s?),
      chat["effort"]?.try(&.as_s?),
      chat["access"]?.try(&.as_s?),
      chat["plan"].as_bool,
      chat["fast"].as_bool,
      chat["queue"].as_a.map(&.as_s),
      chat["working"].as_bool,
      messages.map { |message| message["role"].as_s },
      messages.map { |message| message["content"].as_s },
      search.map { |result| result["snippet"].as_s },
      listed_secrets,
      backend,
      spec.model,
      spec.effort.wire_name,
      spec.access.wire_name,
      spec.fast,
      spec.prompt,
      run_secret_names,
      environment["PARITY_TOKEN"]?,
      handle.canceled,
      dimensions,
      voice_available,
      deleted
    )
  ensure
    client.try(&.close)
    server.close
    engine.close
    store.close
    if old_secrets
      ENV["XD_AGENT_SECRETS_FILE"] = old_secrets
    else
      ENV.delete("XD_AGENT_SECRETS_FILE")
    end
    if old_cache
      ENV["XDG_CACHE_HOME"] = old_cache
    else
      ENV.delete("XDG_CACHE_HOME")
    end
    if old_name
      ENV["XD_DATA_NAME"] = old_name
    else
      ENV.delete("XD_DATA_NAME")
    end
    FileUtils.rm_r(directory) if Dir.exists?(directory)
  end
end

describe "daemon transport parity" do
  it "passes the same stateful protocol matrix over Unix and paired TLS" do
    local = run_transport_matrix(Xd::Daemon::Transport::Local)
    remote = run_transport_matrix(Xd::Daemon::Transport::Remote)

    remote.should eq(local)
    local.should eq(TransportTrace.new(
      ["Parity"],
      "Use one daemon.",
      nil,
      nil,
      ["src"],
      ["src", "note.txt"],
      "after\n",
      true,
      "Transport parity",
      "codex",
      "gpt-5.6-terra",
      "xhigh",
      "full",
      true,
      true,
      ["edited queued"],
      true,
      ["event", "user"],
      ["Switched to GPT-5.6 Terra", "transport sentinel"],
      ["transport sentinel"],
      ["PARITY_TOKEN"],
      "codex",
      "gpt-5.6-terra",
      "xhigh",
      "plan",
      true,
      "transport sentinel",
      ["PARITY_TOKEN"],
      "daemon-owned",
      true,
      {110_i64, 32_i64},
      false,
      true
    ))
  end
end
