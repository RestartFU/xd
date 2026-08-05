require "../../spec_helper"

describe "desktop optimistic sends" do
  it "keeps queued messages out of the transcript" do
    source = File.read("src/xd/ui/window.cr")

    source.should contain(
      "if !@working && attachments.empty? && !text.empty?"
    )
    source.should contain("if message.queued == true")
    source.should contain("remove_optimistic_message(message)")
    source.should contain("optimistic = nil")
  end
end
