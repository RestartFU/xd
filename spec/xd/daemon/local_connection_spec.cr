require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/local_connection"
require "../../../src/xd/daemon/server"

private def await_local_connection(
  connection : Xd::Daemon::LocalConnection,
  connected : Bool,
) : Nil
  deadline = Time.instant + 2.seconds
  until connection.connected? == connected
    fail "local connection did not reach #{connected}" if Time.instant >= deadline
    sleep 5.milliseconds
  end
end

describe Xd::Daemon::LocalConnection do
  it "reconnects while retaining endpoint subscribers" do
    directory = File.join(
      Dir.tempdir,
      "xd-local-connection-#{Random::Secure.hex(12)}"
    )
    path = File.join(directory, "daemon.sock")
    store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
    engine = Xd::Daemon::Engine.new(store)
    server = Xd::Daemon::Server.new(engine)
    connection : Xd::Daemon::LocalConnection? = nil

    begin
      server.listen_local(path)
      initial = Xd::Daemon::Client.local(path)
      connection = Xd::Daemon::LocalConnection.new(
        path,
        initial_client: initial,
        retry_delay: 10.milliseconds
      )
      events = Channel(Hash(String, JSON::Any)).new(1)
      connection.subscribe { |event| events.send(event) }
      connection.call({"op" => JSON::Any.new("ping")})

      server.close
      await_local_connection(connection, false)
      server = Xd::Daemon::Server.new(engine)
      server.listen_local(path)
      await_local_connection(connection, true)

      connection.call({
        "op"   => JSON::Any.new("new-folder"),
        "name" => JSON::Any.new("Reconnected"),
      })
      select
      when event = events.receive
        event["event"].as_s.should eq("tree")
      when timeout(2.seconds)
        fail "local subscriber did not survive reconnect"
      end
    ensure
      connection.try(&.close)
      server.close
      engine.close
      store.close
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
