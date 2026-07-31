require "../../spec_helper"
require "../../../src/xd/protocol/message"

describe Xd::Protocol::Request do
  it "decodes an operation and retains its arguments" do
    request = Xd::Protocol::Request.parse(%({"op":"send","chat":"chat-1"}))

    request.operation.should eq(Xd::Protocol::Operation::Send)
    request.string?("chat").should eq("chat-1")
  end

  it "rejects malformed and non-object input" do
    expect_raises(Xd::Protocol::Error, "Not a JSON object") do
      Xd::Protocol::Request.parse("[1, 2]")
    end
    expect_raises(Xd::Protocol::Error, "Not a JSON object") do
      Xd::Protocol::Request.parse("{")
    end
  end

  it "rejects missing and unknown operation names" do
    expect_raises(Xd::Protocol::Error, "Unknown op") do
      Xd::Protocol::Request.parse(%({"chat":"chat-1"}))
    end
    expect_raises(Xd::Protocol::Error, "Unknown op") do
      Xd::Protocol::Request.parse(%({"op":"launch-missiles"}))
    end
  end
end

describe Xd::Protocol::Response do
  it "produces the existing daemon response shape" do
    response = Xd::Protocol::Response.ok({
      "device"  => JSON::Any.new("laptop"),
      "version" => JSON::Any.new(1_i64),
    })

    JSON.parse(response.to_json).should eq(JSON.parse(
      %({"ok":true,"device":"laptop","version":1})
    ))
  end

  it "escapes protocol errors as JSON" do
    response = Xd::Protocol::Response.error(%(bad "request"))

    response.success?.should be_false
    JSON.parse(response.to_json)["error"].as_s.should eq(%(bad "request"))
  end
end
