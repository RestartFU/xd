require "../../spec_helper"
require "../../../src/xd/voice/data"

describe Xd::Voice::Data do
  it "builds the exact mono PCM WAV header" do
    pcm = Bytes[1, 2, 3, 4]
    wav = Xd::Voice::Data.wav_from_s16(pcm)

    wav.size.should eq(48)
    String.new(wav[0, 4]).should eq("RIFF")
    wav[4, 4].should eq(Bytes[40, 0, 0, 0])
    String.new(wav[8, 8]).should eq("WAVEfmt ")
    wav[16, 4].should eq(Bytes[16, 0, 0, 0])
    wav[20, 2].should eq(Bytes[1, 0])
    wav[22, 2].should eq(Bytes[1, 0])
    wav[24, 4].should eq(Bytes[0x80, 0x3e, 0, 0])
    wav[28, 4].should eq(Bytes[0, 0x7d, 0, 0])
    wav[32, 2].should eq(Bytes[2, 0])
    wav[34, 2].should eq(Bytes[16, 0])
    String.new(wav[36, 4]).should eq("data")
    wav[40, 4].should eq(Bytes[4, 0, 0, 0])
    wav[44, 4].should eq(pcm)
  end

  it "converts signed 16-bit PCM into float samples" do
    pcm = Bytes[0x00, 0x80, 0x00, 0x00, 0xff, 0x7f]
    wav = Xd::Voice::Data.wav_from_s16(pcm)
    samples = Xd::Voice::Data.wav_to_f32(wav)

    samples.size.should eq(3)
    samples[0].should eq(-1_f32)
    samples[1].should eq(0_f32)
    samples[2].should be_close(32767_f32 / 32768_f32, 0.00001_f32)
  end

  it "rejects invalid and truncated WAV data" do
    expect_raises(
      Xd::Voice::Error,
      "Recorded audio has an invalid WAV header."
    ) do
      Xd::Voice::Data.wav_to_f32("not a wav".to_slice)
    end

    wav = Xd::Voice::Data.wav_from_s16(Bytes[1, 2, 3, 4])
    wav[40] = 8
    expect_raises(
      Xd::Voice::Error,
      "Recorded audio data is truncated."
    ) do
      Xd::Voice::Data.wav_to_f32(wav)
    end
  end

  it "checks the pinned model metadata" do
    Xd::Voice::Data.model_metadata_valid?(
      Xd::Voice::MODEL_SIZE,
      Xd::Voice::MODEL_SHA256
    ).should be_true
    Xd::Voice::Data.model_metadata_valid?(
      Xd::Voice::MODEL_SIZE - 1,
      Xd::Voice::MODEL_SHA256
    ).should be_false
    Xd::Voice::Data.model_metadata_valid?(
      Xd::Voice::MODEL_SIZE,
      "a" * 64
    ).should be_false
  end
end
