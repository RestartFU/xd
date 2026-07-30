require "../../spec_helper"
require "../../../src/xd/agent/image_reference"

describe Xd::Agent::ImageReference do
  it "extracts image lines without exposing daemon paths as prose" do
    parsed = Xd::Agent::ImageReference.parse(
      "compare these\n[image: /cache/one.png]\r\n" \
      "[image: /cache/two.png]"
    ).not_nil!

    parsed.remainder.should eq("compare these")
    parsed.paths.should eq([
      "/cache/one.png",
      "/cache/two.png",
    ])
  end

  it "leaves inline references as ordinary prose" do
    Xd::Agent::ImageReference.parse(
      "look at [image: /cache/one.png]"
    ).should be_nil
  end
end
