require "../../spec_helper"

describe Xd::Protocol::Operation do
  it "round-trips every wire name" do
    operations = Xd::Protocol::Operation.values.reject(&.==(
      Xd::Protocol::Operation::Invalid
    ))

    operations.size.should eq(36)

    operations.each do |operation|
      operation.wire_name.should_not be_empty
      Xd::Protocol::Operation.from_wire?(operation.wire_name).should eq(operation)
    end
  end

  it "rejects unknown operations" do
    Xd::Protocol::Operation.from_wire?("").should be_nil
    Xd::Protocol::Operation.from_wire?("not-an-operation").should be_nil
  end

  it "allows only handshake operations before authentication" do
    Xd::Protocol::Operation::Pair.authentication_required?.should be_false
    Xd::Protocol::Operation::Hello.authentication_required?.should be_false
    Xd::Protocol::Operation::Tree.authentication_required?.should be_true
    Xd::Protocol::Operation::Send.authentication_required?.should be_true
    Xd::Protocol::Operation::Ping.authentication_required?.should be_true
  end
end
