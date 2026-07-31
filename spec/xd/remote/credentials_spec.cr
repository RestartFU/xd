require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/remote/credentials"

private def with_remote_credentials(& : String ->) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-remote-credentials-#{Random::Secure.hex(12)}"
  )

  begin
    yield File.join(directory, "remote.json")
  ensure
    FileUtils.rm_r(directory) if Dir.exists?(directory)
  end
end

describe Xd::Remote::CredentialsFile do
  it "atomically stores a pinned remote using private permissions" do
    with_remote_credentials do |path|
      file = Xd::Remote::CredentialsFile.new(path)
      credentials = Xd::Remote::Credentials.new(
        " remote.example ",
        4001,
        "private-token",
        ("AB:" * 31) + "AB"
      )

      file.save(credentials)

      File.info(path).permissions.to_i.should eq(0o600)
      loaded = file.load.not_nil!
      loaded.host.should eq("remote.example")
      loaded.port.should eq(4001)
      loaded.token.should eq("private-token")
      loaded.fingerprint.should eq("ab" * 32)

      file.clear
      File.exists?(path).should be_false
      file.load.should be_nil
    end
  end

  it "refuses incomplete or invalid credential documents" do
    with_remote_credentials do |path|
      expect_raises(Xd::Remote::Credentials::Error, /host/) do
        Xd::Remote::Credentials.new("", 4001, "token", "ab" * 32)
      end
      expect_raises(Xd::Remote::Credentials::Error, /port/) do
        Xd::Remote::Credentials.new("host", 0, "token", "ab" * 32)
      end
      expect_raises(Xd::Remote::Credentials::Error, /fingerprint/) do
        Xd::Remote::Credentials.new("host", 4001, "token", "wrong")
      end

      Dir.mkdir_p(File.dirname(path))
      File.write(path, %({"version":1,"host":"host","port":4001}))
      expect_raises(Xd::Remote::CredentialsFile::Error) do
        Xd::Remote::CredentialsFile.new(path).load
      end
    end
  end
end
