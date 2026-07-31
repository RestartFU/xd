require "../../spec_helper"
require "../../../src/xd/voice/recorder"

describe Xd::Voice::Recorder do
  it "selects the platform capture backend without changing PCM geometry" do
    Xd::Voice::Recorder::BACKEND.should eq(:portaudio)
    Xd::Voice::Recorder::CHUNK_FRAMES.should eq(1_600)
    Xd::Voice::Recorder::CHUNK_BYTES.should eq(3_200)
    Xd::Voice::Recorder::MAX_BYTES.should eq(3_840_000)
    typeof(
      Xd::Voice::Recorder.new.record { |_recording| nil }
    ).should eq(Nil)
  end

  it "loads the native PortAudio runtime" do
    LibPortAudio.version.should be > 0
  end
end
