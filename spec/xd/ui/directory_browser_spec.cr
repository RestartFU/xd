require "../../spec_helper"
require "../../../src/xd/ui/directory_browser"

describe Xd::UI::DirectoryBrowser do
  it "prepares only string directory entries away from GTK" do
    entries = Xd::UI::DirectoryBrowser.prepare_entries([
      JSON::Any.new("src"),
      JSON::Any.new(3_i64),
      JSON::Any.new("docs"),
    ])

    entries.should eq(["src", "docs"])
  end

  it "bounds each GTK directory batch" do
    Xd::UI::DirectoryBrowser.entry_batch_finish(0, 205).should eq(80)
    Xd::UI::DirectoryBrowser.entry_batch_finish(80, 205).should eq(160)
    Xd::UI::DirectoryBrowser.entry_batch_finish(160, 205).should eq(205)
  end
end
