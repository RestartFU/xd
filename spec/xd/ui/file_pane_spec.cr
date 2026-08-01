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

  it "splits preview text into UTF-8-safe GTK batches" do
    prefix = "a" * (Xd::UI::FilePane::PREVIEW_BATCH - 1)
    text = prefix + "🪨" + ("b" * Xd::UI::FilePane::PREVIEW_BATCH)
    chunks = [] of String
    offset = 0

    while offset < text.bytesize
      chunk = Xd::UI::FilePane.preview_chunk(text, offset)
      chunks << chunk
      offset += chunk.bytesize
    end

    chunks.join.should eq(text)
    chunks.each(&.valid_encoding?.should(be_true))
    chunks.each do |chunk|
      chunk.bytesize.should be <= Xd::UI::FilePane::PREVIEW_BATCH
    end
  end

  it "prepares minimal UTF-8-safe preview changes" do
    old_text = "before\naéz\nafter"
    new_text = "before\naêz!\nafter"
    change = Xd::UI::FilePane.text_change(old_text, new_text).not_nil!

    rebuilt = old_text[0, change.start] +
              change.replacement +
              old_text[change.finish..]
    rebuilt.should eq(new_text)
    change.start.should eq(8)
    change.finish.should eq(10)
    change.replacement.should eq("êz!")
    Xd::UI::FilePane.text_change(new_text, new_text).should be_nil
  end
end
