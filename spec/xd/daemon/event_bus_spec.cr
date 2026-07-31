require "../../spec_helper"
require "../../../src/xd/daemon/event_bus"

describe Xd::Daemon::EventBus do
  it "assigns event ids in publication order" do
    bus = Xd::Daemon::EventBus.new
    received = [] of Int64
    bus.subscribe { |event| received << event["id"].as_i64 }

    bus.publish(Xd::Protocol::Event.new(
      "first",
      900_i64,
      {} of String => JSON::Any
    ))
    bus.publish(Xd::Protocol::Event.new(
      "second",
      100_i64,
      {} of String => JSON::Any
    ))

    received.should eq([1_i64, 2_i64])
  end
end
