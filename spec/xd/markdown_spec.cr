require "../spec_helper"
require "../../src/xd/markdown"

private def valid_markup?(markup : String) : Bool
  check = markup.gsub(/<a href="[^"]*">|<\/a>/, "")
  Pango.parse_markup(check, -1, '\0')
rescue Pango::PangoError
  false
end

describe Xd::Markdown do
  it "renders inline spans" do
    Xd::Markdown.to_pango("a **strong** word")
      .should eq("a <b>strong</b> word")
    Xd::Markdown.to_pango("an *emphasised* word")
      .should eq("an <i>emphasised</i> word")
    Xd::Markdown.to_pango("call `g_free()` on it")
      .should eq("call <tt>g_free()</tt> on it")
  end

  it "escapes markup characters" do
    result = Xd::Markdown.to_pango("compare a < b && c > d")

    result.should eq("compare a &lt; b &amp;&amp; c &gt; d")
    valid_markup?(result).should be_true
  end

  it "keeps partial streaming input valid" do
    partials = [
      "**",
      "**bol",
      "here is `some cod",
      "```\nint main (void)\n{",
      "*",
      "# Headi",
    ]

    partials.each do |partial|
      valid_markup?(Xd::Markdown.to_pango(partial)).should be_true
    end
  end

  it "supports underscore emphasis without corrupting identifiers" do
    Xd::Markdown.to_pango("call some_long_name and _stress this_")
      .should eq("call some_long_name and <i>stress this</i>")
  end

  it "renders fenced code blocks" do
    result = Xd::Markdown.to_pango(
      "before\n```c\nint x = 1 < 2;\n```\nafter"
    )

    valid_markup?(result).should be_true
    result.should contain("<tt>")
    result.should contain("</tt>")
    result.should contain("int x = 1 &lt; 2;")
  end

  it "renders headings visibly" do
    result = Xd::Markdown.to_pango("## Summary")

    result.should contain(%(size="large"))
    result.should contain("<b>Summary</b>")
    valid_markup?(result).should be_true
  end

  it "does not turn hash-prefixed prose into headings" do
    [
      "#1 fixed. Moving to #2.",
      "#include <stdio.h>",
      "####### too many hashes",
    ].each do |line|
      result = Xd::Markdown.to_pango(line)

      result.should_not contain("size=")
      result.should_not contain("<b>")
      result.should start_with("#")
      valid_markup?(result).should be_true
    end
  end

  it "handles empty and nil input" do
    Xd::Markdown.to_pango("").should eq("")
    Xd::Markdown.to_pango(nil).should eq("")
  end

  it "renders safe links and escapes URL attributes" do
    result = Xd::Markdown.to_pango(
      "see [PR #54](https://github.com/x/practice/pull/54) now"
    )
    amp = Xd::Markdown.to_pango("[q](https://x.dev/a?b=1&c=2)")

    result.should contain(
      %(<a href="https://github.com/x/practice/pull/54">PR #54</a>)
    )
    amp.should contain("b=1&amp;c=2")
  end

  it "autolinks bare URLs without swallowing punctuation" do
    result = Xd::Markdown.to_pango(
      "see https://github.com/RestartFU/xd/issues/5 now"
    )
    punctuation = Xd::Markdown.to_pango(
      "(https://example.com/a_(b)). Next."
    )
    plain = Xd::Markdown.urls_to_pango(
      "**literal** https://example.com/a?b=1&c=2"
    )

    result.should contain(
      %(<a href="https://github.com/RestartFU/xd/issues/5">) +
      "https://github.com/RestartFU/xd/issues/5</a>"
    )
    punctuation.should contain(
      %(<a href="https://example.com/a_(b)">) +
      "https://example.com/a_(b)</a>)"
    )
    plain.should contain("**literal**")
    plain.should contain(
      %(<a href="https://example.com/a?b=1&amp;c=2">) +
      "https://example.com/a?b=1&amp;c=2</a>"
    )
  end

  it "scans long plain text without copying every remaining suffix" do
    plain = ("héllo & world " * 8_000) +
            "https://example.com/a?b=1&c=2"

    result = Xd::Markdown.urls_to_pango(plain)

    result.should start_with("héllo &amp; world")
    result.should contain(
      %(<a href="https://example.com/a?b=1&amp;c=2">) +
      "https://example.com/a?b=1&amp;c=2</a>"
    )
  end

  it "renders list bullets and nesting" do
    result = Xd::Markdown.to_pango("- first\n- second\n  - nested")

    result.should contain("• first")
    result.should contain("• second")
    result.should contain("  • nested")
    result.should_not contain("- first")
  end

  it "renders CommonMark block forms" do
    result = Xd::Markdown.to_pango(
      "> quoted **text**\n>\n> continued\n\n" \
      "3. third\n4. fourth\n\n---\n\n" \
      "    indented <code>"
    )

    result.should contain("│ quoted <b>text</b>")
    result.should contain("3. third\n4. fourth")
    result.should contain("──")
    result.should contain(
      %(<tt><span background="#181818">indented &lt;code&gt;)
    )
    valid_markup?(result).should be_true
  end

  it "renders nested CommonMark inline forms" do
    result = Xd::Markdown.to_pango(
      "***bold italic***, ~~literal~~, and \\*literal\\*"
    )

    result.should contain("<i><b>bold italic</b></i>")
    result.should contain("~~literal~~")
    result.should contain("*literal*")
    valid_markup?(result).should be_true
  end

  it "renders tables as monospace grids" do
    result = Xd::Markdown.to_pango(
      "| metric | old | new |\n" \
      "|---|---|---|\n" \
      "| ack_rtt_p50 | 269ms | 41ms |\n" \
      "| corrections | 0 | 0 |"
    )

    result.should contain("<tt><b>metric")
    result.should contain("ack_rtt_p50")
    result.should contain("269ms")
    result.should contain("│")
    result.should contain("┼")
    result.should_not contain("|---|")
    valid_markup?(result).should be_true
  end

  it "converts only standalone tables to plain grids" do
    table = "| metric | old | new |\n" \
            "|---|---|---|\n" \
            "| ack_rtt_p50 | 269ms | 41ms |\n" \
            "| corrections | 0 | 0 |"

    grid = Xd::Markdown.table_to_text(table)
    grid.should_not be_nil
    grid = grid.not_nil!
    grid.should contain("metric")
    grid.should contain("ack_rtt_p50")
    grid.should contain("│")
    grid.should contain("┼")
    grid.should_not contain("<tt>")
    Xd::Markdown.table_to_text(
      "Run foo | bar.\nStill prose."
    ).should be_nil
    Xd::Markdown.table_to_text(
      "Results:\n\n| old | new |\n|---|---|\n| 1 | 2 |"
    ).should be_nil
  end

  it "does not render pipe prose as a table" do
    result = Xd::Markdown.to_pango(
      "Run foo | bar.\nThis is still ordinary prose."
    )

    result.should_not contain("<tt>")
    result.should contain("foo | bar")
    valid_markup?(result).should be_true
  end

  it "renders images and drops unsafe links" do
    image = Xd::Markdown.to_pango(
      "![diagram](https://example.com/image.png)"
    )
    unsafe = Xd::Markdown.to_pango(
      "[do not run](javascript:alert(1))"
    )

    image.should contain(
      %(Image: <a href="https://example.com/image.png">diagram</a>)
    )
    unsafe.should eq("do not run")
  end

  it "keeps raw HTML literal" do
    result = Xd::Markdown.to_pango(
      %(<span size="999999">small</span>)
    )

    result.should contain(
      "&lt;span size=&quot;999999&quot;&gt;small&lt;/span&gt;"
    )
    valid_markup?(result).should be_true
  end
end
