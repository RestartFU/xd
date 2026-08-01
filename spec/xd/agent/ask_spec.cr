require "../../spec_helper"
require "../../../src/xd/agent/ask"

describe Xd::Agent::Ask do
  it "extracts the last valid choice block" do
    text = <<-TEXT
      This explains <ask>bad</ask>.

      Choose now.

      <ask>
      Which implementation?
      - Keep parser
      * Replace parser
      - Add tests
      </ask>
      TEXT

    parsed = Xd::Agent::Ask.parse(text).not_nil!
    parsed.ask.question.should eq("Which implementation?")
    parsed.ask.options.should eq([
      "Keep parser",
      "Replace parser",
      "Add tests",
    ])
    parsed.ask.accepts_input.should be_false
    parsed.remainder.should eq(
      "This explains <ask>bad</ask>.\n\nChoose now."
    )
  end

  it "accepts input-only questions and joins multiline prompts" do
    parsed = Xd::Agent::Ask.parse(<<-TEXT).not_nil!
      Before.
      <ask>
      Which branch
      should receive this?
      <input>
      </ask>
      After.
      TEXT

    parsed.ask.question.should eq("Which branch should receive this?")
    parsed.ask.options.should be_empty
    parsed.ask.accepts_input.should be_true
    parsed.remainder.should eq("Before.\n\nAfter.")
  end

  it "strips duplicate valid blocks and keeps the last actionable ask" do
    text = <<-TEXT
      Findings.

      <ask>
      Which implementation?
      - Restore it
      - Start fresh
      </ask>

      More context.

      <ask>
      What should v1 cover?
      - Chat only
      - Full parity
      </ask>

      Closing prose.
      TEXT

    parsed = Xd::Agent::Ask.parse(text).not_nil!
    parsed.ask.question.should eq("What should v1 cover?")
    parsed.ask.options.should eq(["Chat only", "Full parity"])
    parsed.remainder.should eq(
      "Findings.\n\nMore context.\n\nClosing prose."
    )
  end

  it "rejects blocks without a real choice or input" do
    Xd::Agent::Ask.parse("<ask>\nQuestion?\n- Only one\n</ask>")
      .should be_nil
  end

  it "caps choices at six" do
    text = "<ask>\nPick.\n" +
           (1..8).map { |number| "- Option #{number}\n" }.join +
           "</ask>"
    Xd::Agent::Ask.parse(text).not_nil!.ask.options.size.should eq(6)
  end

  it "holds valid, open, and partial blocks during streaming" do
    Xd::Agent::Ask.visible_bytes("Reply <").should eq(6)
    Xd::Agent::Ask.visible_bytes("Reply <as").should eq(6)
    Xd::Agent::Ask.visible_bytes("Reply <ask>\nQuestion").should eq(6)
    Xd::Agent::Ask.visible_bytes(
      "Reply <ask>\nQuestion?\n- Yes\n- No\n</ask>"
    ).should eq(6)
    multiple = "Reply <ask>\nFirst?\n- Yes\n- No\n</ask>\n" \
               "middle <ask>\nSecond?\n- A\n- B\n</ask>"
    Xd::Agent::Ask.visible_bytes(multiple).should eq(6)
  end

  it "releases invalid tags and counts UTF-8 bytes" do
    invalid = "Réply <ask>\nNot a choice\n</ask>"
    Xd::Agent::Ask.visible_bytes(invalid).should eq(invalid.bytesize)
    Xd::Agent::Ask.visible_bytes("Réply <").should eq("Réply ".bytesize)
  end
end
