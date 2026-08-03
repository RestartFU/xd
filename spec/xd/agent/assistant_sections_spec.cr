require "../../spec_helper"
require "../../../src/xd/agent/assistant_sections"

describe Xd::Agent::AssistantSections do
  it "lifts analysis and unwraps summary while preserving order" do
    sections = Xd::Agent::AssistantSections.parse(
      "Before\n<analysis>\n**thought**\n</analysis>\n" \
      "<summary>\n**answer**\n</summary>\nAfter"
    )

    sections.map(&.kind).should eq([
      Xd::Agent::AssistantSectionKind::Normal,
      Xd::Agent::AssistantSectionKind::Analysis,
      Xd::Agent::AssistantSectionKind::Normal,
      Xd::Agent::AssistantSectionKind::Normal,
    ])
    sections.map(&.text).should eq([
      "Before",
      "**thought**",
      "**answer**",
      "After",
    ])
  end

  it "leaves wrapper examples inside fenced code literal" do
    text = "```text\n<analysis>\ninside\n</analysis>\n```"

    sections = Xd::Agent::AssistantSections.parse(text)

    sections.should eq([
      Xd::Agent::AssistantSection.new(
        Xd::Agent::AssistantSectionKind::Normal,
        text
      ),
    ])
  end

  it "leaves malformed wrappers literal" do
    text = "<analysis>\nunfinished\n<summary>\nanswer\n</summary>"

    Xd::Agent::AssistantSections.parse(text).should eq([
      Xd::Agent::AssistantSection.new(
        Xd::Agent::AssistantSectionKind::Normal,
        text
      ),
    ])
  end

  it "hides analysis from a live projection and keeps summary visible" do
    text = "<analysis>\nthinking\n</analysis>\n<summary>\nanswer\n</summary>"

    Xd::Agent::AssistantSections.stream(text).should eq("answer")
  end

  it "withholds partial wrapper tags while streaming" do
    Xd::Agent::AssistantSections.stream("Before\n<ana").should eq("Before")
    Xd::Agent::AssistantSections.stream("<summary>\nanswer\n</sum").should eq("answer")
  end
end
