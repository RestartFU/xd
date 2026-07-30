require "../spec_helper"
require "../../src/xd/unified_diff"

describe Xd::UnifiedDiff do
  it "parses display rows, line numbers, and totals" do
    patch = <<-DIFF
      diff --git a/src/a.c b/src/a.c
      index 1111111..2222222 100644
      --- a/src/a.c
      +++ b/src/a.c
      @@ -10,2 +10,3 @@ function
       unchanged
      -gone
      +new
      +extra
      \\ No newline at end of file
      DIFF
    parsed = Xd::UnifiedDiff.parse(patch)

    parsed.lines.size.should eq(7)
    parsed.additions.should eq(2)
    parsed.deletions.should eq(1)
    parsed.lines[0].should eq(
      Xd::DiffLine.new(Xd::DiffLineKind::File, "src/a.c")
    )
    parsed.lines[1].should eq(
      Xd::DiffLine.new(
        Xd::DiffLineKind::Hunk,
        "@@ -10,2 +10,3 @@ function",
        10,
        10
      )
    )
    parsed.lines[2].should eq(
      Xd::DiffLine.new(
        Xd::DiffLineKind::Context,
        "unchanged",
        10,
        10
      )
    )
    parsed.lines[3].old_line.should eq(11)
    parsed.lines[3].new_line.should eq(0)
    parsed.lines[4].old_line.should eq(0)
    parsed.lines[4].new_line.should eq(11)
    parsed.lines[6].kind.should eq(Xd::DiffLineKind::Meta)
  end

  it "keeps meaningful metadata and quoted target paths" do
    patch = <<-DIFF
      diff --git "a/image old.png" "b/image new.png"
      new file mode 100644
      Binary files /dev/null and b/image new.png differ
      DIFF
    parsed = Xd::UnifiedDiff.parse(patch)

    parsed.lines.map(&.text).should eq([
      "image new.png",
      "new file mode 100644",
      "Binary files /dev/null and b/image new.png differ",
    ])
  end

  it "counts visible rows and exposes exact backgrounds" do
    lines = [
      Xd::DiffLine.new(Xd::DiffLineKind::File, "a.c"),
      Xd::DiffLine.new(Xd::DiffLineKind::Hunk, "@@ -1 +1 @@"),
      Xd::DiffLine.new(Xd::DiffLineKind::Removed, "old"),
      Xd::DiffLine.new(Xd::DiffLineKind::Added, "new"),
    ]

    Xd::UnifiedDiff.display_rows(lines, true).should eq(4)
    Xd::UnifiedDiff.display_rows(lines, false).should eq(3)
    Xd::DiffLineKind::File.background.should eq("#2a2a2d")
    Xd::DiffLineKind::Added.background.should eq("#183522")
    Xd::DiffLineKind::Removed.background.should eq("#3a1d1b")
    Xd::DiffLineKind::Context.background.should be_nil
  end
end
