require "../../spec_helper"
require "base64"
require "file_utils"
require "random/secure"
require "socket"
require "../../../src/xd/daemon/certificate"
require "../../../src/xd/daemon/server"
require "../../support/local_endpoint"

describe Xd::Daemon::Server do
  it "serves the shared engine over private local IPC" do
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

      client = XdSpec::LocalEndpoint.connect(socket_path)
      begin
        client.puts %({"op":"ping"})
        JSON.parse(client.gets.not_nil!)["ok"].as_bool.should be_true
      ensure
        client.close
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

  it "replaces a stale socket but not a live daemon" do
    directory = File.join(
      Dir.tempdir,
      "xd-server-stale-#{Random::Secure.hex(12)}"
    )
    socket_path = File.join(directory, "daemon.sock")
    first_store = Xd::Storage::Store.new(
      File.join(directory, "first.db")
    )
    second_store = Xd::Storage::Store.new(
      File.join(directory, "second.db")
    )
    first = Xd::Daemon::Server.new(Xd::Daemon::Engine.new(first_store))
    second = Xd::Daemon::Server.new(Xd::Daemon::Engine.new(second_store))

    begin
      first.listen_local(socket_path)
      expect_raises(IO::Error, /already running/) do
        second.listen_local(socket_path)
      end
      first.close
      File.exists?(socket_path).should be_false

      # A crashed daemon leaves only its endpoint metadata.
      XdSpec::LocalEndpoint.leave_stale(socket_path)
      second.listen_local(socket_path)
      client = XdSpec::LocalEndpoint.connect(socket_path)
      begin
        client.puts %({"op":"ping"})
        JSON.parse(client.gets.not_nil!)["ok"].as_bool.should be_true
      ensure
        client.close
      end
    ensure
      first.close
      second.close
      first_store.close
      second_store.close
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
      first = XdSpec::LocalEndpoint.connect(socket_path)
      begin
        second = XdSpec::LocalEndpoint.connect(socket_path)
        begin
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
        ensure
          second.close
        end
      ensure
        first.close
      end
    ensure
      server.close
      store.close
      FileUtils.rm_r(directory)
    end
  end

  it "disconnects a slow event client without blocking the daemon" do
    directory = File.join(
      Dir.tempdir,
      "xd-server-backpressure-#{Random::Secure.hex(12)}"
    )
    database_path = File.join(directory, "chats.db")
    socket_path = File.join(directory, "daemon.sock")
    store = Xd::Storage::Store.new(database_path)
    engine = Xd::Daemon::Engine.new(store)
    server = Xd::Daemon::Server.new(engine)
    slow : XdSpec::LocalEndpoint::Connection? = nil
    fast : Xd::Daemon::Client? = nil

    begin
      server.listen_local(socket_path)
      slow = XdSpec::LocalEndpoint.connect(socket_path)
      slow.read_timeout = 2.seconds
      slow.puts %({"op":"ping"})
      JSON.parse(slow.gets.not_nil!)["ok"].as_bool.should be_true

      payload = "x" * (64 * 1024)
      finished = Channel(Exception?).new(1)
      spawn do
        begin
          1_024.times do |index|
            engine.events.publish(Xd::Protocol::Event.new(
              "burst",
              index.to_i64,
              {"payload" => JSON::Any.new(payload)}
            ))
          end
          finished.send(nil)
        rescue error
          finished.send(error)
        end
      end

      select
      when error = finished.receive
        raise error if error
      when timeout(2.seconds)
        fail "slow event client blocked daemon publication"
      end

      fast = Xd::Daemon::Client.local(socket_path)
      fast.call({"op" => JSON::Any.new("ping")})["ok"]
        .as_bool.should be_true
    ensure
      fast.try(&.close)
      slow.try(&.close)
      server.close
      engine.close
      store.close
      FileUtils.rm_r(directory)
    end
  end

  it "serves the same session engine over TLS" do
    directory = File.join(
      Dir.tempdir,
      "xd-server-tls-#{Random::Secure.hex(12)}"
    )
    database_path = File.join(directory, "chats.db")
    certificate = File.join(directory, "certificate.pem")
    key = File.join(directory, "private-key.pem")
    store = Xd::Storage::Store.new(database_path)
    engine = Xd::Daemon::Engine.new(
      store,
      token_generator: -> { "tls-token" }
    )
    code = engine.arm_pairing(1.minute)
    server = Xd::Daemon::Server.new(engine)

    begin
      Xd::Daemon::Certificate.ensure_pair(certificate, key)
      port = server.listen_remote(
        "127.0.0.1",
        0,
        certificate,
        key
      )
      server.listen_remote(
        "127.0.0.1",
        0,
        certificate,
        key
      ).should eq(port)
      context = OpenSSL::SSL::Context::Client.new
      context.verify_mode = OpenSSL::SSL::VerifyMode::NONE
      socket = TCPSocket.new("127.0.0.1", port)

      OpenSSL::SSL::Socket::Client.open(
        socket,
        context,
        sync_close: true
      ) do |client|
        client.puts({
          "op"   => "pair",
          "code" => code,
          "name" => "tls-test",
        }.to_json)
        client.flush
        JSON.parse(client.gets.not_nil!)["token"].as_s.should eq("tls-token")

        client.puts %({"op":"ping"})
        client.flush
        JSON.parse(client.gets.not_nil!)["ok"].as_bool.should be_true
      end
    ensure
      server.close
      store.close
      FileUtils.rm_r(directory)
    end
  end

  it "returns remote voice results only to the requesting TLS client" do
    directory = File.join(
      Dir.tempdir,
      "xd-server-voice-tls-#{Random::Secure.hex(12)}"
    )
    database_path = File.join(directory, "chats.db")
    certificate = File.join(directory, "certificate.pem")
    key = File.join(directory, "private-key.pem")
    model_path = File.join(directory, "model.bin")
    executable = File.join(directory, "whisper")
    Dir.mkdir_p(directory)
    File.write(model_path, "remote-daemon-model")
    File.write(executable, <<-'SH')
      #!/bin/sh
      set -eu
      printf 'private remote transcript\n'
      SH
    File.chmod(executable, 0o700)

    store = Xd::Storage::Store.new(database_path)
    chat_id = store.create_chat("folder", "Remote Voice", "claude")
    engine = Xd::Daemon::Engine.new(
      store,
      token_generator: -> { "voice-token" },
      voice_model_factory: -> {
        Xd::Voice::Model.new(override_path: model_path)
      },
      voice_transcriber_factory: -> {
        Xd::Voice::Transcriber.new(
          resolver: -> { executable },
          environment: {} of String => String
        )
      }
    )
    code = engine.arm_pairing(1.minute)
    server = Xd::Daemon::Server.new(engine)

    begin
      Xd::Daemon::Certificate.ensure_pair(certificate, key)
      port = server.listen_remote("127.0.0.1", 0, certificate, key)
      context = OpenSSL::SSL::Context::Client.new
      context.verify_mode = OpenSSL::SSL::VerifyMode::NONE
      first_socket = TCPSocket.new("127.0.0.1", port)
      second_socket = TCPSocket.new("127.0.0.1", port)

      OpenSSL::SSL::Socket::Client.open(
        first_socket,
        context,
        sync_close: true
      ) do |first|
        OpenSSL::SSL::Socket::Client.open(
          second_socket,
          context,
          sync_close: true
        ) do |second|
          first.read_timeout = 2.seconds
          second.read_timeout = 2.seconds
          first.puts({
            "op"   => "pair",
            "code" => code,
            "name" => "voice-requester",
          }.to_json)
          first.flush
          JSON.parse(first.gets.not_nil!)["token"].as_s
            .should eq("voice-token")

          second.puts({
            "op"    => "hello",
            "token" => "voice-token",
          }.to_json)
          second.flush
          JSON.parse(second.gets.not_nil!)["ok"].as_bool.should be_true

          first.puts({
            "op"      => "voice-transcribe",
            "chat"    => chat_id,
            "request" => "tls-voice",
            "audio"   => Base64.strict_encode(Bytes[1, 2, 3, 4]),
          }.to_json)
          first.flush
          messages = 2.times.map do
            JSON.parse(first.gets.not_nil!)
          end
          response = messages.find { |message| !message["ok"]?.nil? }
          response.not_nil!["ok"].as_bool.should be_true
          event = messages.find { |message| !message["event"]?.nil? }
            .not_nil!
          event["event"].as_s.should eq("voice")
          event["request"].as_s.should eq("tls-voice")
          event["text"].as_s.should eq("private remote transcript")

          leaked = Channel(String?).new(1)
          spawn do
            begin
              leaked.send(second.gets)
            rescue IO::Error
            end
          end
          select
          when line = leaked.receive
            fail("voice result leaked to second TLS client: #{line}")
          when timeout(250.milliseconds)
          end
        end
      end
    ensure
      server.close
      engine.close
      store.close
      FileUtils.rm_r(directory)
    end
  end
end
