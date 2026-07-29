require "../../spec_helper"
require "../../../src/xd/daemon/engine"

private def parse_response(response : Xd::Protocol::Response) : JSON::Any
  JSON.parse(response.to_json)
end

describe Xd::Daemon::Engine do
  it "uses the same dispatcher after transport authentication" do
    store = Xd::Daemon::MemoryDeviceStore.new
    engine = Xd::Daemon::Engine.new(store)
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

  it "pairs once, stores only a token hash, then authenticates another connection" do
    store = Xd::Daemon::MemoryDeviceStore.new
    engine = Xd::Daemon::Engine.new(
      store,
      token_generator: -> { "secret-token" }
    )
    pairing_connection = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote)
    code = engine.arm_pairing(5.minutes)

    pair = engine.dispatch(pairing_connection, {
      "op"   => "pair",
      "code" => code,
      "name" => "workstation",
    }.to_json)

    pair.success?.should be_true
    pair["token"].as_s.should eq("secret-token")
    pairing_connection.authenticated.should be_true
    store.devices.keys.should_not contain("secret-token")
    store.devices.values.should contain("workstation")

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

  it "rejects expired pairing codes" do
    now = Time.instant
    clock = -> { now }
    engine = Xd::Daemon::Engine.new(
      Xd::Daemon::MemoryDeviceStore.new,
      clock: clock
    )
    code = engine.arm_pairing(5.seconds)
    now += 6.seconds

    response = engine.dispatch(
      Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote),
      {"op" => "pair", "code" => code}.to_json
    )

    response.success?.should be_false
    response["error"].as_s.should contain("No such pairing code")
  end

  it "does not authenticate unknown tokens" do
    engine = Xd::Daemon::Engine.new(Xd::Daemon::MemoryDeviceStore.new)
    connection = Xd::Daemon::Connection.new(Xd::Daemon::Transport::Remote)

    response = engine.dispatch(connection, {
      "op"    => "hello",
      "token" => "unknown",
    }.to_json)

    response.success?.should be_false
    response["error"].as_s.should eq("Unknown device. Pair first.")
    connection.authenticated.should be_false
  end
end
