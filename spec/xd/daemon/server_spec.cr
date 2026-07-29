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

  it "fans mutation events to every connected local client after response" do
    directory = File.join(
      Dir.tempdir,
      "xd-server-events-#{Random::Secure.hex(12)}"
    )
    database_path = File.join(directory, "chats.db")
    socket_path = File.join(directory, "daemon.sock")
    store = Xd::Storage::Store.new(database_path)
    server = Xd::Daemon::Server.new(Xd::Daemon::Engine.new(store))

    begin
      server.listen_local(socket_path)
      UNIXSocket.open(socket_path) do |first|
        UNIXSocket.open(socket_path) do |second|
          first.read_timeout = 2.seconds
          second.read_timeout = 2.seconds

          [first, second].each do |client|
            client.puts %({"op":"ping"})
            JSON.parse(client.gets.not_nil!)["ok"].as_bool.should be_true
          end

          first.puts %({"op":"new-folder","name":"Shared"})
          response = JSON.parse(first.gets.not_nil!)
          first_event = JSON.parse(first.gets.not_nil!)
          second_event = JSON.parse(second.gets.not_nil!)

          response["ok"].as_bool.should be_true
          first_event["event"].as_s.should eq("tree")
          second_event.should eq(first_event)
          first_event["id"].as_i64.should eq(1)
        end
      end
    ensure
      server.close
      store.close
      FileUtils.rm_r(directory)
    end
  end
end
