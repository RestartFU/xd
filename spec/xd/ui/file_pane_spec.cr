require "../../spec_helper"
require "../../../src/xd/ui/file_pane"

private def file_entry(name : String, directory : Bool) : JSON::Any
  JSON::Any.new({
    "name"      => JSON::Any.new(name),
    "directory" => JSON::Any.new(directory),
  })
end

describe Xd::UI::FilePane do
  it "prepares directory entries away from GTK callbacks" do
    entries = Xd::UI::FilePane.prepare_entries([
      file_entry("z.txt", false),
      file_entry("src", true),
      JSON::Any.new({} of String => JSON::Any),
      file_entry("a.txt", false),
    ])

    entries.map(&.name).should eq(["src", "a.txt", "z.txt"])
  end

  it "bounds each GTK directory-row batch" do
    Xd::UI::FilePane.entry_batch_finish(0, 205).should eq(80)
    Xd::UI::FilePane.entry_batch_finish(80, 205).should eq(160)
    Xd::UI::FilePane.entry_batch_finish(160, 205).should eq(205)
  end
end
