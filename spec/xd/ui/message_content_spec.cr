require "../../spec_helper"
require "../../../src/xd/ui/message_content"

describe Xd::UI::MessageContent do
  it "keeps ordinary assistant text as one prose part" do
    parts = Xd::UI::MessageContent.parse("Hello **there**.")

    parts.should eq([
      Xd::UI::MessagePart.new(
        Xd::UI::MessagePartKind::Prose,
        "Hello **there**."
      ),
    ])
  end

  it "marks analysis parts without changing their Markdown preparation" do
    prepared = Xd::UI::MessageContent.prepare(
      "<analysis>\n**thought**\n</analysis>\n<summary>\nanswer\n</summary>"
    )

    prepared.map(&.section).should eq([
      Xd::Agent::AssistantSectionKind::Analysis,
      Xd::Agent::AssistantSectionKind::Normal,
    ])
    prepared.first.markup.not_nil!.should contain("<b>thought</b>")
    prepared.last.markup.not_nil!.should contain("answer")
  end

  it "splits fenced code from surrounding prose" do
    parts = Xd::UI::MessageContent.parse(
      "Before\n```crystal\nputs :ok\n```\nAfter"
    )

    parts.map(&.kind).should eq([
      Xd::UI::MessagePartKind::Prose,
      Xd::UI::MessagePartKind::Code,
      Xd::UI::MessagePartKind::Prose,
    ])
    parts.map(&.text).should eq([
      "Before",
      "puts :ok",
      "After",
    ])
  end

  it "keeps a fenced language and prepares highlighted code" do
    text = "```go\nfunc main() { println(\"<ok>\") }\n```"
    parsed = Xd::UI::MessageContent.parse(text)

    parsed.size.should eq(1)
    parsed.first.kind.should eq(Xd::UI::MessagePartKind::Code)
    parsed.first.language.should eq(Xd::SyntaxLanguage::Go)

    prepared = Xd::UI::MessageContent.prepare(text)
    prepared.size.should eq(1)
    prepared.first.language.should eq(Xd::SyntaxLanguage::Go)
    prepared.first.markup.not_nil!
      .should contain(%(<span foreground="#dc8add">func</span>))
    prepared.first.markup.not_nil!.should contain("&lt;ok&gt;")
  end

  it "leaves unknown and unlabelled code fences uncoloured" do
    unknown = Xd::UI::MessageContent.prepare(
      "```typescript\nconst value = 1\n```"
    )
    plain = Xd::UI::MessageContent.prepare("```\nplain\n```")

    unknown.first.kind.should eq(Xd::UI::MessagePartKind::Code)
    unknown.first.language.should eq(Xd::SyntaxLanguage::None)
    unknown.first.markup.should be_nil
    plain.first.markup.should be_nil
  end

  it "keeps an unfinished fence as code while streaming" do
    parts = Xd::UI::MessageContent.parse(
      "Before\n```text\nunfinished"
    )

    parts.last.kind.should eq(Xd::UI::MessagePartKind::Code)
    parts.last.text.should eq("unfinished")
  end

  it "identifies diff fences exactly" do
    diff = Xd::UI::MessageContent.parse(
      "```diff\n-old\n+new\n```"
    )
    other = Xd::UI::MessageContent.parse(
      "```diff extra\n-old\n+new\n```"
    )

    diff.first.kind.should eq(Xd::UI::MessagePartKind::Diff)
    other.first.kind.should eq(Xd::UI::MessagePartKind::Code)
  end

  it "pulls complete tables into standalone cards" do
    parts = Xd::UI::MessageContent.parse(
      "Results:\n\n" \
      "| old | new |\n" \
      "|---|---|\n" \
      "| 1 | 2 |\n\n" \
      "Done."
    )

    parts.map(&.kind).should eq([
      Xd::UI::MessagePartKind::Prose,
      Xd::UI::MessagePartKind::Table,
      Xd::UI::MessagePartKind::Prose,
    ])
    parts[1].text.should contain("old")
    parts[1].text.should contain("─┼─")
    parts[1].text.should_not contain("|---|")
  end

  it "leaves pipe prose in the prose label" do
    parts = Xd::UI::MessageContent.parse(
      "Run foo | bar.\nStill ordinary prose."
    )

    parts.size.should eq(1)
    parts.first.kind.should eq(Xd::UI::MessagePartKind::Prose)
    parts.first.text.should contain("foo | bar")
  end

  it "prepares large response blocks in bounded UTF-8 chunks" do
    text = ("é" * 90_000) + "\n" + ("tail " * 20_000)
    prepared = Xd::UI::MessageContent.prepare(text, 64 * 1024)

    prepared.size.should be > 2
    prepared.each do |part|
      part.kind.should eq(Xd::UI::MessagePartKind::Prose)
      part.text.bytesize.should be <= 64 * 1024
      part.text.valid_encoding?.should be_true
      part.markup.should_not be_nil
    end
    prepared.map(&.text).join.should eq(text)
  end
end
