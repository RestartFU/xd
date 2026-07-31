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
      "--root", File.join(directory, "root"),
      "--database", File.join(directory, "db"),
      "--socket", File.join(directory, "socket"),
      "--certificate", File.join(directory, "cert"),
      "--private-key", File.join(directory, "key"),
    ])

    options.port.should eq(0)
    options.bind.should eq("127.0.0.1")
    options.pair.should be_true
    options.root.should eq(File.join(directory, "root"))
    options.database.should eq(File.join(directory, "db"))
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
