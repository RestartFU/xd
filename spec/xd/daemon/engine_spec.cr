require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/engine"

private def parse_response(response : Xd::Protocol::Response) : JSON::Any
  JSON.parse(response.to_json)
end

private def with_daemon_engine(
  clock : Proc(Time::Instant) = -> { Time.instant },
  token_generator : Proc(String) = -> { Random::Secure.base64(32) },
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
    token_generator: token_generator
  )

  begin
    yield store, engine
  ensure
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

  it "owns workspace and chat mutations behind the same protocol" do
    with_daemon_engine do |_store, engine|
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
          "op" => "set-agent-secrets",
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
end
