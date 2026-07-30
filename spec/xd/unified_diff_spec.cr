require "../spec_helper"
require "../../src/xd/unified_diff"

private def valid_diff_markup?(markup : String) : Bool
  Pango.parse_markup(markup, -1, '\0')
rescue Pango::PangoError
  false
end

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

  it "splits a complete patch into counted virtual file sections" do
    patch = <<-DIFF
      diff --git a/src/a.c b/src/a.c
      @@ -1 +1,2 @@
      -old
      +new
      +extra
      diff --git a/src/b.c b/src/b.c
      @@ -4 +4 @@
      -before
      +after
      DIFF
    sections = Xd::UnifiedDiff.file_sections(
      Xd::UnifiedDiff.parse(patch).lines
    )

    sections.should eq([
      Xd::DiffFileSection.new("src/a.c", 0, 5, 2, 1),
      Xd::DiffFileSection.new("src/b.c", 5, 9, 1, 1),
    ])
  end

  it "keeps metadata-only patches in one Changes section" do
    lines = [
      Xd::DiffLine.new(Xd::DiffLineKind::Meta, "mode changed"),
    ]

    Xd::UnifiedDiff.file_sections(lines).should eq([
      Xd::DiffFileSection.new("Changes", 0, 1, 0, 0),
    ])
    Xd::UnifiedDiff.file_sections([] of Xd::DiffLine).should be_empty
  end

  it "formats one safe layout with totals, gutters, and a limit footer" do
    patch = <<-DIFF
      diff --git a/src/a.c b/src/a.c
      @@ -1,2 +1,2 @@
      -old <value>
      +new & value
       same
      DIFF
    lines = Xd::UnifiedDiff.parse(patch).lines
    result = Xd::UnifiedDiff.markup(lines, true, 3)

    valid_diff_markup?(result.markup).should be_true
    result.markup.should contain(
      %(foreground="#ffbe6f" weight="bold">src/a.c</span>)
    )
    result.markup.should contain("+1</span>")
    result.markup.should contain("−1</span>")
    result.markup.should contain("old &lt;value&gt;")
    result.markup.should contain("Showing first 3 of 5 rows")
    result.markup.should_not contain("background=")
    result.row_kinds.should eq([
      Xd::DiffLineKind::File,
      Xd::DiffLineKind::Hunk,
      Xd::DiffLineKind::Removed,
      Xd::DiffLineKind::Meta,
    ])
  end

  it "uses full-line change colours only for unknown languages" do
    patch = <<-DIFF
      diff --git a/notes.txt b/notes.txt
      @@ -1 +1 @@
      -removed line
      +added line
      DIFF
    result = Xd::UnifiedDiff.markup(
      Xd::UnifiedDiff.parse(patch).lines,
      true
    )

    result.markup.should contain(
      %(foreground="#f66151">removed line</span>)
    )
    result.markup.should contain(
      %(foreground="#57e389">added line</span>)
    )
    result.row_kinds.should eq([
      Xd::DiffLineKind::File,
      Xd::DiffLineKind::Hunk,
      Xd::DiffLineKind::Removed,
      Xd::DiffLineKind::Added,
    ])
  end

  it "colours known code while keeping old and new lexer states separate" do
    patch = <<-DIFF
      diff --git a/src/a.c b/src/a.c
      @@ -1,3 +1,3 @@
      -  x = 1; /* opened
      +  return 2;
       };
      DIFF
    result = Xd::UnifiedDiff.markup(
      Xd::UnifiedDiff.parse(patch).lines,
      true
    )

    result.markup.should contain(
      %(foreground="#dc8add">return</span>)
    )
    result.markup.should contain(
      %(foreground="#ffbe6f">2</span>)
    )
    result.markup.should_not contain(
      %(foreground="#57e389">  return 2;</span>)
    )
  end

  it "primes independent slices from their file and hunk state" do
    patch = <<-DIFF
      diff --git a/src/a.c b/src/a.c
      @@ -1,4 +1,4 @@
       /* a comment that
      -   runs past int
      +   runs past int too
       */
      DIFF
    lines = Xd::UnifiedDiff.parse(patch).lines
    result = Xd::UnifiedDiff.markup_slice(lines, true, 3, 5)

    valid_diff_markup?(result.markup).should be_true
    result.markup.should contain(%(foreground="#8b8e8f"))
    result.markup.should_not contain(
      %(foreground="#dc8add">int</span>)
    )
    result.markup.should_not contain("Showing first")
    result.row_kinds.should eq([
      Xd::DiffLineKind::Removed,
      Xd::DiffLineKind::Added,
    ])
  end

  it "truncates display text on a valid UTF-8 boundary" do
    text = "a" * 1023 + "é" + "tail"
    shown = Xd::UnifiedDiff.display_text(text)

    shown.valid_encoding?.should be_true
    shown.should eq("a" * 1023 + "…")
  end
end
