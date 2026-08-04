require "../../spec_helper"
require "../../../src/xd/agent/claude_proxy"

describe Xd::Agent::ClaudeProxy do
  it "allows cold proxy startup without exceeding daemon request timeout" do
    Xd::Agent::ClaudeProxy::START_TIMEOUT.should be > 10.seconds
    Xd::Agent::ClaudeProxy::START_TIMEOUT.should be < 30.seconds
  end
end
