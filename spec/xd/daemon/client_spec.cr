require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/certificate"
require "../../../src/xd/daemon/client"
require "../../../src/xd/daemon/server"

private def with_client_server(
  & : Xd::Daemon::Server, Xd::Daemon::Engine, Xd::Storage::Store, String ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-client-#{Random::Secure.hex(12)}"
  )
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  engine = Xd::Daemon::Engine.new(
    store,
    token_generator: -> { "client-token" }
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

describe Xd::Daemon::Client do
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
