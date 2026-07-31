require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/integrations/discord_presence"

{% unless flag?(:win32) %}
  private def read_discord_frame(io : IO) : {UInt32, String}
    header = Bytes.new(8)
    io.read_fully(header)
    input = IO::Memory.new(header)
    opcode = input.read_bytes(UInt32, IO::ByteFormat::LittleEndian)
    length = input.read_bytes(UInt32, IO::ByteFormat::LittleEndian)
    payload = Bytes.new(length.to_i)
    io.read_fully(payload)
    {opcode, String.new(payload)}
  end

  private def write_discord_frame(
    io : IO,
    opcode : UInt32,
    payload : String,
  ) : Nil
    io.write_bytes(opcode, IO::ByteFormat::LittleEndian)
    io.write_bytes(payload.bytesize.to_u32, IO::ByteFormat::LittleEndian)
    io << payload
    io.flush
  end
{% end %}

describe Xd::DiscordPresence do
  it "builds the legacy privacy-safe activity payload" do
    payload = JSON.parse(
      Xd::DiscordPresence.activity("Agent working", 1234_i64, 42_i64, 7_u64)
    )

    payload["cmd"].as_s.should eq("SET_ACTIVITY")
    payload["nonce"].as_s.should eq("7")
    args = payload["args"]
    args["pid"].as_i64.should eq(42_i64)
    activity = args["activity"]
    activity["details"].as_s.should eq("Building with AI")
    activity["state"].as_s.should eq("Agent working")
    activity["timestamps"]["start"].as_i64.should eq(1234_i64)
    activity.as_h.has_key?("chat").should be_false
    activity.as_h.has_key?("workspace").should be_false
    activity.as_h.has_key?("repository").should be_false
  end

  {% unless flag?(:win32) %}
    it "handshakes and publishes without using the default scheduler" do
      directory = File.join(
        Dir.tempdir,
        "xd-discord-#{Random::Secure.hex(12)}"
      )
      Dir.mkdir(directory)
      socket_path = File.join(directory, "discord-ipc-0")
      server = UNIXServer.new(socket_path)
      previous_runtime = ENV["XDG_RUNTIME_DIR"]?
      ENV["XDG_RUNTIME_DIR"] = directory
      payloads = Channel({String, String}).new(1)
      presence : Xd::DiscordPresence? = nil

      begin
        spawn do
          client = server.accept
          handshake = read_discord_frame(client)
          write_discord_frame(client, 1_u32, %({"evt":"READY"}))
          activity = read_discord_frame(client)
          write_discord_frame(client, 1_u32, %({"cmd":"SET_ACTIVITY"}))
          payloads.send({handshake[1], activity[1]})
          client.close
        end

        presence = Xd::DiscordPresence.new
        select
        when payload = payloads.receive
          JSON.parse(payload[0])["client_id"].as_s.should eq(
            Xd::DiscordPresence::APPLICATION_ID
          )
          JSON.parse(payload[1])["args"]["activity"]["state"].as_s
            .should eq(Xd::DiscordPresence::DEFAULT_STATE)
        when timeout(3.seconds)
          fail "Discord presence did not publish"
        end
      ensure
        presence.try(&.close)
        server.close
        if previous_runtime
          ENV["XDG_RUNTIME_DIR"] = previous_runtime
        else
          ENV.delete("XDG_RUNTIME_DIR")
        end
        FileUtils.rm_r(directory) if Dir.exists?(directory)
      end
    end
  {% end %}
end
