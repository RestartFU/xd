require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/voice/transcriber"

describe Xd::Voice::Transcriber do
  it "leaves CPU headroom for the desktop" do
    Xd::Voice::Transcriber.thread_count(1).should eq(1)
    Xd::Voice::Transcriber.thread_count(2).should eq(1)
    Xd::Voice::Transcriber.thread_count(4).should eq(2)
    Xd::Voice::Transcriber.thread_count(64).should eq(4)
  end

  it "runs the bundled speech CLI with the C transcription settings" do
    directory = File.join(
      Dir.tempdir,
      "xd-transcriber-#{Random::Secure.hex(12)}"
    )
    Dir.mkdir_p(directory)
    executable = File.join(directory, "whisper")
    arguments = File.join(directory, "arguments")
    File.write(executable, <<-'SH')
      #!/bin/sh
      set -eu
      printf '%s\n' "$@" > "$ARGUMENTS"
      printf '  transcribed voice prompt  \n'
      SH
    File.chmod(executable, 0o700)
    finished = Channel(Xd::Voice::Transcription).new(1)
    transcriber = Xd::Voice::Transcriber.new(
      resolver: -> { executable },
      environment: {"ARGUMENTS" => arguments}
    )

    begin
      transcriber.transcribe(
        Xd::Voice::Data.wav_from_s16(Bytes[0, 0, 0, 0]),
        "/models/local.bin"
      ) { |result| finished.send(result) }
      result = finished.receive
      result.text.should eq("transcribed voice prompt")
      result.error.should be_nil
      result.cancelled.should be_false

      argv = File.read_lines(arguments)
      argv.should contain("--model")
      argv.should contain("/models/local.bin")
      argv.should contain(Xd::Voice::Transcriber.thread_count.to_s)
      argv.should contain("--no-gpu")
      argv.should contain("--flash-attn")
      argv.should contain("--no-timestamps")
      argv.should contain("--no-prints")
      argv.should contain(Xd::Voice::Transcriber::PROMPT)
      argv.find { |value| value.ends_with?(".wav") }.should_not be_nil
      argv.find { |value| value.ends_with?(".wav") }
        .try { |path| File.exists?(path).should be_false }
    ensure
      transcriber.cancel
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
