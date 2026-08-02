require "../spec_helper"
require "file_utils"
require "random/secure"
require "../../src/xd/cli"

describe Xd::CLI do
  it "identifies Crystal builds to native packagers" do
    output = IO::Memory.new

    Xd::CLI.new(output).run(["--bundle-runtime"]).should eq(0)
    output.to_s.should eq("crystal\n")
  end

  it "reports incomplete native bundles" do
    output = IO::Memory.new
    errors = IO::Memory.new
    directory = File.join(
      Dir.tempdir,
      "xd-native-cli-#{Random::Secure.hex(12)}"
    )

    Xd::CLI.new(output, errors).run([
      "--validate-native-bundle",
      "windows",
      directory,
    ]).should eq(1)
    errors.to_s.should contain("native bundle missing Crystal executable")
  end

  it "parses daemon paths, bind address, ephemeral port, and pairing" do
    directory = File.join(
      Dir.tempdir,
      "xd-cli-#{Random::Secure.hex(12)}"
    )
    options = Xd::CLI.new.parse_serve([
      "--port", "0",
      "--bind", "127.0.0.1",
      "--pair",
      "--device-name", "dev workstation",
      "--root", File.join(directory, "root"),
      "--database", File.join(directory, "db"),
      "--socket", File.join(directory, "socket"),
      "--certificate", File.join(directory, "cert"),
      "--private-key", File.join(directory, "key"),
    ])

    options.port.should eq(0)
    options.bind.should eq("127.0.0.1")
    options.pair.should be_true
    options.device_name.should eq("dev workstation")
    options.root.should eq(File.join(directory, "root"))
    options.database.should eq(File.join(directory, "db"))
  end

  it "requires an owner device name when pairing" do
    expect_raises(Xd::CLI::UsageError, /--pair requires --device-name/) do
      Xd::CLI.new.parse_serve(["--pair"])
    end
  end

  it "rejects invalid ports" do
    expect_raises(Xd::CLI::UsageError, /Invalid port/) do
      Xd::CLI.new.parse_serve(["--port", "70000"])
    end
  end

  it "does not expose the removed daemon updater" do
    expect_raises(Xd::CLI::UsageError, /Invalid option: --auto-update/) do
      Xd::CLI.new.parse_serve(["--auto-update"])
    end
  end

  it "asks a running daemon for a pairing code instead of starting another" do
    directory = File.join(
      Dir.tempdir,
      "xd-cli-pair-running-#{Random::Secure.hex(12)}"
    )
    database = File.join(directory, "chats.db")
    socket = File.join(directory, "daemon.sock")
    certificate = File.join(directory, "certificate.pem")
    key = File.join(directory, "private-key.pem")
    store = Xd::Storage::Store.new(database)
    engine = Xd::Daemon::Engine.new(
      store,
      token_generator: -> { "cli-peer-token" }
    )
    server = Xd::Daemon::Server.new(engine)
    paired : Xd::Daemon::RemotePairing? = nil

    begin
      engine.peer_listener = ->(bind : String, port : Int32) {
        Xd::Daemon::Certificate.ensure_pair(certificate, key)
        server.listen_remote(bind, port, certificate, key)
      }
      server.listen_local(socket)
      output = IO::Memory.new
      errors = IO::Memory.new

      Xd::CLI.new(output, errors).run([
        "serve",
        "--pair",
        "--device-name", "cli owner",
        "--socket", socket,
        "--bind", "127.0.0.1",
        "--port", "0",
        "--certificate", certificate,
        "--private-key", key,
      ]).should eq(0)

      errors.to_s.should be_empty
      output.to_s.should contain("attached to running daemon")
      code = output.to_s.match(/one use\): ([A-Z0-9-]+)/)
        .not_nil![1]
      port = server.remote_port.not_nil!
      paired = Xd::Daemon::Client.pair_remote(
        "127.0.0.1",
        port,
        code,
        "cli laptop"
      )
      paired.not_nil!.token.should eq("cli-peer-token")
    ensure
      paired.try(&.client.close)
      server.close
      engine.close
      store.close
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end

describe Xd::Daemon::Certificate do
  it "creates and reuses a private self-signed identity" do
    directory = File.join(
      Dir.tempdir,
      "xd-certificate-#{Random::Secure.hex(12)}"
    )
    certificate = File.join(directory, "cert.pem")
    key = File.join(directory, "key.pem")

    begin
      Xd::Daemon::Certificate.ensure_pair(certificate, key)
      File.read(certificate).should contain("BEGIN CERTIFICATE")
      File.read(key).should contain("BEGIN PRIVATE KEY")
      File.info(key).permissions.to_i.should eq(0o600)
      before = File.read(certificate)

      Xd::Daemon::Certificate.ensure_pair(certificate, key)
      File.read(certificate).should eq(before)
    ensure
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
