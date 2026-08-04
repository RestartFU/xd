require "../../spec_helper"

describe "desktop authentication refresh" do
  it "reloads the active chat for its daemon's authentication events" do
    source = File.read("src/xd/ui/window.cr")
    handler = source
      .split("      private def handle_event(", 2)[1]
      .split("      private def defer_turn_recovery_event?(", 2)[0]
    auth_event = handler
      .split("        when \"agent-auth-changed\"", 2)[1]

    auth_event.should contain("return unless @client.same?(endpoint)")
    auth_event.should contain(
      "load_chat_state(recover_turn: false) if @active_chat"
    )
    auth_event.should_not contain("event[\"provider\"]")
  end
end
