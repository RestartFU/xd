require "../../spec_helper"

describe "desktop sidebar context menus" do
  it "lets GTK popup menus after setting their pointer rectangle" do
    source = File.read("src/xd/ui/sidebar.cr")
    method = source
      .split("      private def present_menu(", 2)[1]
      .split("      private def pointer_key(", 2)[0]

    method.should contain("popover.halign = :start")
    method.should contain("popover.valign = :start")
    method.should contain("popover.parent = anchor")
    method.should contain("popover.pointing_to = pointing_to")
    method.should contain("popover.popup")
    method.should_not contain("popover.present")
  end
end
