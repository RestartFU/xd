require "../../spec_helper"
require "../../../src/xd/protocol/frame"
require "../../../src/xd/protocol/message"
require "../../../src/xd/protocol/operation"
require "../../../src/xd/voice/data"

# The mobile client is built against `docs/protocol.md`, not against this
# source tree. These assertions pin the parts of the wire contract it depends
# on, so a rename or a limit change fails here rather than on a phone.
describe "mobile wire contract" do
  it "keeps the operation names the mobile client sends" do
    {
      "pair"        => Xd::Protocol::Operation::Pair,
      "hello"       => Xd::Protocol::Operation::Hello,
      "tree"        => Xd::Protocol::Operation::Tree,
      "chat"        => Xd::Protocol::Operation::Chat,
      "messages"    => Xd::Protocol::Operation::Messages,
      "new-chat"    => Xd::Protocol::Operation::NewChat,
      "send"        => Xd::Protocol::Operation::Send,
      "queue"       => Xd::Protocol::Operation::Queue,
      "drop-queue"  => Xd::Protocol::Operation::DropQueue,
      "cancel"      => Xd::Protocol::Operation::Cancel,
      "set-option"    => Xd::Protocol::Operation::SetOption,
      "agent-catalog" => Xd::Protocol::Operation::AgentCatalog,
      "ping"          => Xd::Protocol::Operation::Ping,
      # Voice: the phone captures audio and the daemon does everything after.
      "voice-model"          => Xd::Protocol::Operation::VoiceModel,
      "voice-model-download" => Xd::Protocol::Operation::VoiceModelDownload,
      "voice-transcribe"     => Xd::Protocol::Operation::VoiceTranscribe,
      "voice-cancel"         => Xd::Protocol::Operation::VoiceCancel,
    }.each do |wire, operation|
      Xd::Protocol::Operation.from_wire?(wire).should eq(operation)
      operation.wire_name.should eq(wire)
    end
  end

  it "greets without authentication and guards everything else" do
    Xd::Protocol::Operation::Pair.authentication_required?.should be_false
    Xd::Protocol::Operation::Hello.authentication_required?.should be_false

    Xd::Protocol::Operation::Tree.authentication_required?.should be_true
    Xd::Protocol::Operation::Send.authentication_required?.should be_true
    # A paired phone must not be able to enrol further devices.
    Xd::Protocol::Operation::PeerPairing.authentication_required?.should be_true
  end

  it "keeps the audio format the mobile client records in" do
    # The phone opens its microphone at exactly this rate and writes this WAV
    # header. The daemon does not resample, so a change here is a change the
    # phone has to make too.
    Xd::Voice::SAMPLE_RATE.should eq(16_000_u32)
    Xd::Voice::CHANNELS.should eq(1_u16)

    header = Xd::Voice::Data.wav_from_s16(Bytes.new(4))
    String.new(header[0, 4]).should eq("RIFF")
    String.new(header[8, 8]).should eq("WAVEfmt ")
    String.new(header[36, 4]).should eq("data")
    header.size.should eq(48)
  end

  it "keeps the framing limits the client buffers against" do
    Xd::Protocol::AUTH_FRAME_LIMIT.should eq(64 * 1024)
    Xd::Protocol::FRAME_LIMIT.should eq(96 * 1024 * 1024)
  end

  it "echoes the private request id that multiplexes replies" do
    Xd::Protocol::REQUEST_ID.should eq("_xd_request")

    request = Xd::Protocol::Request.parse(
      %({"op":"ping","#{Xd::Protocol::REQUEST_ID}":42})
    )
    request.operation.should eq(Xd::Protocol::Operation::Ping)
    request.int?(Xd::Protocol::REQUEST_ID).should eq(42_i64)
  end

  it "reports failures as ok:false with a reason" do
    body = JSON.parse(Xd::Protocol::Response.error("Nope").to_json).as_h
    body["ok"].as_bool.should be_false
    body["error"].as_s.should eq("Nope")
  end

  it "names every event so unknown ones can be ignored by name" do
    event = Xd::Protocol::Event.new(
      "turn-finished",
      7_i64,
      {"chat" => JSON::Any.new("chat-1")}
    )
    body = JSON.parse(event.to_json).as_h
    body["event"].as_s.should eq("turn-finished")
    body["id"].as_i64.should eq(7_i64)
    body["chat"].as_s.should eq("chat-1")
  end
end
