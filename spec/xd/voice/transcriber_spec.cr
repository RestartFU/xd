require "../../spec_helper"
require "../../../src/xd/voice/transcriber"

describe Xd::Voice::Transcriber do
  it "leaves CPU headroom for the desktop" do
    Xd::Voice::Transcriber.thread_count(1).should eq(1)
    Xd::Voice::Transcriber.thread_count(2).should eq(1)
    Xd::Voice::Transcriber.thread_count(4).should eq(2)
    Xd::Voice::Transcriber.thread_count(64).should eq(4)
  end

  it "starts a resident fast English CPU recognizer" do
    argv = Xd::Voice::Transcriber.server_arguments(
      "/bundle/whisper-server",
      "/models/base.en.bin",
      52_000
    )

    argv.first.should eq("/bundle/whisper-server")
    argv.should contain("/models/base.en.bin")
    argv.should contain("52000")
    argv.should contain("en")
    argv.should contain("--best-of")
    argv.should contain("1")
    argv.should contain("--beam-size")
    argv.should contain("-1")
    argv.should contain("--no-gpu")
    argv.should contain("--flash-attn")
    argv.should contain(Xd::Voice::Transcriber::PROMPT)
  end
end
