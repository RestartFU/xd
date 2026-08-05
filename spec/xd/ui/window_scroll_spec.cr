require "../../spec_helper"

describe "desktop transcript joins" do
  it "masks only the initial bottom seek while history mounts" do
    source = File.read("src/xd/ui/window.cr")

    source.should contain("begin_bottom_jump(mask: true)")
    source.should contain("load_messages(mask_initial: changed)")
    source.should contain(
      "begin_bottom_jump(mask: @masked_history_request == request)"
    )
    source.should contain(
      "@transcript_scroll.opacity = @bottom_jump_masked ? 0.0 : 1.0"
    )
    source.should contain("@bottom_jump_masked = false")
  end
end
