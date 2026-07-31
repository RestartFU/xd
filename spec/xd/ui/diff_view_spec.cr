require "../../spec_helper"
require "../../../src/xd/ui/diff_view"

describe Xd::UI::DiffView do
  it "prepares inline diff markup without constructing GTK widgets" do
    patch = <<-PATCH
      diff --git a/file.cr b/file.cr
      --- a/file.cr
      +++ b/file.cr
      @@ -1 +1 @@
      -old
      +new
      PATCH

    prepared = Xd::UI::DiffView.prepare(patch)

    prepared.rows.should eq(4)
    prepared.additions.should eq(1)
    prepared.deletions.should eq(1)
    prepared.markup.row_kinds.size.should eq(4)
    prepared.markup.markup.should contain("file.cr")
  end

  it "bounds markup work for large inline diffs" do
    changed = (1..200).map { |line| "+line #{line}" }.join('\n')
    patch = <<-PATCH
      diff --git a/file.txt b/file.txt
      --- a/file.txt
      +++ b/file.txt
      @@ -0,0 +1,200 @@
      #{changed}
      PATCH

    prepared = Xd::UI::DiffView.prepare(patch)

    prepared.rows.should eq(202)
    prepared.markup.row_kinds.size.should eq(
      Xd::UI::DiffView::INLINE_PREVIEW_ROWS + 1
    )
    prepared.markup.markup.should contain(
      "Showing first #{Xd::UI::DiffView::INLINE_PREVIEW_ROWS} of 202 rows"
    )
  end
end
