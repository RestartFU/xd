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
    parsed.parts.should eq([
      Xd::Agent::ImageReference::Part.new("compare these", nil),
      Xd::Agent::ImageReference::Part.new(nil, "/cache/one.png"),
      Xd::Agent::ImageReference::Part.new(nil, "/cache/two.png"),
    ])
  end

  it "preserves prose and image order" do
    parsed = Xd::Agent::ImageReference.parse(
      "before\n[image: /cache/one.png]\nafter"
    ).not_nil!

    parsed.parts.should eq([
      Xd::Agent::ImageReference::Part.new("before", nil),
      Xd::Agent::ImageReference::Part.new(nil, "/cache/one.png"),
      Xd::Agent::ImageReference::Part.new("after", nil),
    ])
    parsed.remainder.should eq("before\nafter")
  end

  it "leaves inline references as ordinary prose" do
    Xd::Agent::ImageReference.parse(
      "look at [image: /cache/one.png]"
    ).should be_nil
  end
end
