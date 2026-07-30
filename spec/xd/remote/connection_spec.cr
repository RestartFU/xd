require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/server"
require "../../../src/xd/remote/connection"

private def with_remote_connection(
  & : String, String, Xd::Storage::Store, Xd::Daemon::Engine ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-remote-connection-#{Random::Secure.hex(12)}"
  )
  socket = File.join(directory, "daemon.sock")
  credentials = File.join(directory, "remote.json")
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  engine = Xd::Daemon::Engine.new(store)

  begin
    yield socket, credentials, store, engine
  ensure
    engine.close
    store.close
    FileUtils.rm_r(directory) if Dir.exists?(directory)
  end
end

private def await_remote_state(
  connection : Xd::Remote::Connection,
  expected : Xd::Remote::ConnectionState,
) : Nil
  deadline = Time.instant + 2.seconds
  until connection.snapshot.state == expected
    fail "remote never reached #{expected}" if Time.instant >= deadline
    sleep 5.milliseconds
  end
end

describe Xd::Remote::Connection do
  it "reconnects and keeps endpoint subscribers across clients" do
    with_remote_connection do |socket, path, _store, engine|
      credentials = Xd::Remote::Credentials.new(
        "test-host",
        4001,
        "test-token",
        "ab" * 32
      )
      Xd::Remote::CredentialsFile.new(path).save(credentials)
      server = Xd::Daemon::Server.new(engine)
      server.listen_local(socket)
      connector = ->(_stored : Xd::Remote::Credentials) {
        Xd::Daemon::Client.local(socket)
      }
      connection = Xd::Remote::Connection.new(
        Xd::Remote::CredentialsFile.new(path),
        10.milliseconds,
        connector
      )
      states = Channel(Xd::Remote::ConnectionState).new(20)
      connection.on_state { |snapshot| states.send(snapshot.state) }

      begin
        await_remote_state(
          connection,
          Xd::Remote::ConnectionState::Connected
        )
        connection.call({
          "op" => JSON::Any.new("ping"),
        })["ok"].as_bool.should be_true

        server.close
        await_remote_state(
          connection,
          Xd::Remote::ConnectionState::Offline
        )

        server = Xd::Daemon::Server.new(engine)
        server.listen_local(socket)
        await_remote_state(
          connection,
          Xd::Remote::ConnectionState::Connected
        )
        connection.call({
          "op" => JSON::Any.new("ping"),
        })["ok"].as_bool.should be_true
        states.empty?.should be_false
      ensure
        connection.close
        server.close
      end
    end
  end

  it "normalizes pairing input and forgets credentials" do
    with_remote_connection do |socket, path, _store, engine|
      server = Xd::Daemon::Server.new(engine)
      server.listen_local(socket)
      paired_client = Xd::Daemon::Client.local(socket)
      pairer = ->(host : String, port : Int32, code : String, name : String) {
        host.should eq("remote.example")
        port.should eq(4242)
        code.should eq("ABCD1234")
        name.should eq("laptop")
        Xd::Daemon::RemotePairing.new(
          paired_client,
          "paired-token",
          "cd" * 32
        )
      }
      connection = Xd::Remote::Connection.new(
        Xd::Remote::CredentialsFile.new(path),
        10.milliseconds,
        pairer: pairer
      )

      begin
        connection.pair(
          " remote.example ",
          4242,
          "ab cd 1234",
          "laptop"
        )
        connection.connected?.should be_true
        connection.snapshot.host.should eq("remote.example")
        connection.snapshot.port.should eq(4242)
        Xd::Remote::CredentialsFile.new(path)
          .load.not_nil!.token.should eq("paired-token")

        connection.forget
        connection.configured?.should be_false
        File.exists?(path).should be_false
      ensure
        connection.close
        server.close
      end
    end
  end

  it "does not keep a pairing canceled while the connection was opening" do
    with_remote_connection do |socket, path, _store, engine|
      server = Xd::Daemon::Server.new(engine)
      server.listen_local(socket)
      paired_client = Xd::Daemon::Client.local(socket)
      pairer = ->(_host : String, _port : Int32, _code : String, _name : String) {
        Xd::Daemon::RemotePairing.new(
          paired_client,
          "paired-token",
          "cd" * 32
        )
      }
      connection = Xd::Remote::Connection.new(
        Xd::Remote::CredentialsFile.new(path),
        10.milliseconds,
        pairer: pairer
      )

      begin
        expect_raises(
          Xd::Daemon::Client::Error,
          "Pairing was cancelled."
        ) do
          connection.pair(
            "remote.example",
            4242,
            "ABCD1234",
            canceled: -> { true }
          )
        end
        connection.connected?.should be_false
        connection.configured?.should be_false
        File.exists?(path).should be_false
        paired_client.closed?.should be_true
      ensure
        connection.close
        server.close
      end
    end
  end
end
