require "../../spec_helper"
require "digest/sha256"
require "file_utils"
require "http/server"
require "random/secure"
require "../../../src/xd/voice/model"

describe Xd::Voice::Model do
  it "downloads, verifies, and reuses a model atomically" do
    directory = File.join(
      Dir.tempdir,
      "xd-voice-model-#{Random::Secure.hex(12)}"
    )
    path = File.join(directory, "speech", "model.bin")
    payload = ("model-data-" * 4096).to_slice
    digest = Digest::SHA256.hexdigest(payload)
    requests = 0
    server = HTTP::Server.new do |context|
      requests += 1
      context.response.content_length = payload.size
      context.response.write(payload)
    end
    address = server.bind_tcp("127.0.0.1", 0)
    spawn server.listen

    begin
      model = Xd::Voice::Model.new(
        path: path,
        url: "http://127.0.0.1:#{address.port}/model",
        expected_size: payload.size.to_u64,
        expected_sha256: digest
      )
      progress = [] of Int32
      model.find.should be_nil
      model.ensure_available { |value| progress << value }.should eq(path)
      model.find.should eq(path)
      File.read(path).to_slice.should eq(payload)
      File.info(path).permissions.to_i.should eq(0o600)
      File.read("#{path}.sha256").strip.should eq(digest)
      progress.last.should eq(100)

      model.ensure_available { |_value| }.should eq(path)
      requests.should eq(1)
    ensure
      server.close
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "honors a valid model override without trusting a missing one" do
    file = File.tempfile("xd-voice-override")
    begin
      Xd::Voice::Model.new(override_path: file.path).find
        .should eq(file.path)
      Xd::Voice::Model.new(override_path: "#{file.path}-missing").find
        .should be_nil
    ensure
      file.close
      File.delete(file.path) if File.exists?(file.path)
    end
  end
end
