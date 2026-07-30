require "../../spec_helper"
require "file_utils"
require "random/secure"
require "socket"
require "../../../src/xd/daemon/client"
require "../../../src/xd/daemon/server"

private def with_local_ipc_server(
  & : Xd::Daemon::Server, Xd::Storage::Store, String ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-local-ipc-#{Random::Secure.hex(12)}"
  )
  Dir.mkdir_p(directory)
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  engine = Xd::Daemon::Engine.new(store)
  server = Xd::Daemon::Server.new(engine)

  begin
    yield server, store, directory
  ensure
    server.close
    engine.close
    store.close
    FileUtils.rm_r(directory) if Dir.exists?(directory)
  end
end

describe Xd::Daemon::LocalIPC do
  it "keeps local calls private and transport-independent" do
    with_local_ipc_server do |server, _store, directory|
      path = File.join(directory, "daemon.sock")
      server.listen_local(path)

      {% unless flag?(:win32) %}
        File.info(path).permissions.to_i.should eq(0o600)
      {% end %}

      client = Xd::Daemon::Client.local(path)
      client.call({
        "op" => JSON::Any.new("ping"),
      })["ok"].as_bool.should be_true
      client.close

      server.close
      File.exists?(path).should be_false
      {% if flag?(:win32) || flag?(:xd_loopback_local) %}
        Dir.exists?("#{path}.lock").should be_false
      {% end %}
    end
  end

  {% if flag?(:win32) || flag?(:xd_loopback_local) %}
    it "rejects unauthenticated loopback clients" do
      with_local_ipc_server do |server, _store, directory|
        path = File.join(directory, "daemon.sock")
        server.listen_local(path)
        descriptor = Xd::Daemon::LocalIPC.read(path)
        expect_raises(Xd::Daemon::LocalIPC::InUse, /already running/) do
          Xd::Daemon::LocalIPC.claim(path, 20.milliseconds)
        end

        socket = TCPSocket.new("127.0.0.1", descriptor.port)
        socket.puts %({"op":"ping"})
        response = JSON.parse(socket.gets.not_nil!)
        response["ok"].as_bool.should be_false
        socket.close

        client = Xd::Daemon::Client.local(path)
        client.call({
          "op" => JSON::Any.new("ping"),
        })["ok"].as_bool.should be_true
        client.close
      end
    end

    it "preserves invalid endpoints and replaces stale descriptors" do
      with_local_ipc_server do |server, _store, directory|
        path = File.join(directory, "daemon.sock")
        File.write(path, "keep")

        expect_raises(IO::Error, /Refusing to replace/) do
          server.listen_local(path)
        end
        File.read(path).should eq("keep")
        Dir.exists?("#{path}.lock").should be_false

        File.delete(path)
        probe = TCPServer.new("127.0.0.1", 0)
        stale_port = probe.local_address.as(Socket::IPAddress).port
        probe.close
        stale_token = "0" * (Xd::Daemon::LocalIPC::TOKEN_BYTES * 2)
        Xd::Daemon::LocalIPC.publish(path, stale_port, stale_token)
        stale_lock = "#{path}.lock"
        Dir.mkdir(stale_lock)
        claimed = Xd::Daemon::LocalIPC.claim(path, 20.milliseconds)
        claimed.should eq(stale_lock)
        Xd::Daemon::LocalIPC.release(claimed)

        server.listen_local(path)
        descriptor = Xd::Daemon::LocalIPC.read(path)
        descriptor.token.should_not eq(stale_token)
        Xd::Daemon::Client.local(path).close
      end
    end
  {% end %}
end
