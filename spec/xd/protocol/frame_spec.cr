require "../../spec_helper"
require "../../../src/xd/protocol/frame"

describe Xd::Protocol do
  it "reads adjacent bounded newline frames" do
    input = IO::Memory.new("first\nsecond\n")

    Xd::Protocol.read_frame(input, 16).should eq("first")
    Xd::Protocol.read_frame(input, 16).should eq("second")
    Xd::Protocol.read_frame(input, 16).should be_nil
  end

  it "rejects a frame before buffering beyond its limit" do
    input = IO::Memory.new(("x" * 32) + "\n")

    expect_raises(Xd::Protocol::FrameTooLarge) do
      Xd::Protocol.read_frame(input, 16)
    end
    input.pos.should eq(17)
  end

  it "accepts limit-sized content and excludes the delimiter" do
    input = IO::Memory.new(("x" * 16) + "\n")

    Xd::Protocol.read_frame(input, 16).should eq("x" * 16)
  end
end
