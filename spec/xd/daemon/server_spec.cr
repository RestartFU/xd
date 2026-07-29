require "../../spec_helper"
require "file_utils"
require "random/secure"
require "socket"
require "../../../src/xd/daemon/server"

describe Xd::Daemon::Server do
  it "serves the shared engine over private Unix IPC" do
    directory = File.join(
      Dir.tempdir,
      "xd-server-#{Random::Secure.hex(12)}"
    )
    database_path = File.join(directory, "chats.db")
    socket_path = File.join(directory, "daemon.sock")
    store = Xd::Storage::Store.new(database_path)
    server = Xd::Daemon::Server.new(Xd::Daemon::Engine.new(store))

    begin
      server.listen_local(socket_path)
      File.info(socket_path).permissions.to_i.should eq(0o600)

      UNIXSocket.open(socket_path) do |client|
        client.puts %({"op":"ping"})
        JSON.parse(client.gets.not_nil!)["ok"].as_bool.should be_true
      end
    ensure
      server.close
      store.close
      FileUtils.rm_r(directory)
    end
  end

  it "refuses to replace a non-socket local endpoint" do
    directory = File.join(
      Dir.tempdir,
      "xd-server-file-#{Random::Secure.hex(12)}"
    )
    database_path = File.join(directory, "chats.db")
    socket_path = File.join(directory, "daemon.sock")
    store = Xd::Storage::Store.new(database_path)
    server = Xd::Daemon::Server.new(Xd::Daemon::Engine.new(store))
    File.write(socket_path, "keep")

    begin
      expect_raises(IO::Error, /Refusing to replace/) do
        server.listen_local(socket_path)
      end
      File.read(socket_path).should eq("keep")
    ensure
      server.close
      store.close
      FileUtils.rm_r(directory)
    end
  end
end
