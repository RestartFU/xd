require "../../spec_helper"

describe "desktop shortcut editor keyboard handling" do
  it "lets focused prompt entries receive key presses" do
    source = File.read("src/xd/ui/shortcut_dialog.cr")
    keyboard = source
      .split("        keys = Gtk::EventControllerKey.new", 2)[1]
      .split("        window.add_controller(keys)", 2)[0]

    keyboard.should_not contain("propagation_phase = :capture")
    keyboard.should contain("if keyval == Gdk::KEY_Escape")
    keyboard.should contain("false")
  end
end
