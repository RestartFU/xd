require "../../spec_helper"
require "../../../src/xd/ui/command_suggestions"

describe Xd::UI::CommandSuggestions do
  it "shows every command for a bare slash" do
    commands = %w(review simplify compact)

    Xd::UI::CommandSuggestions.matches(commands, "/")
      .should eq(commands)
  end

  it "matches ASCII prefixes without case sensitivity" do
    commands = %w(review reload simplify)

    Xd::UI::CommandSuggestions.matches(commands, "/RE")
      .should eq(%w(review reload))
  end

  it "hides after whitespace or without a leading slash" do
    commands = %w(review simplify)

    Xd::UI::CommandSuggestions.matches(commands, "rev")
      .should be_empty
    Xd::UI::CommandSuggestions.matches(commands, "/review ")
      .should be_empty
    Xd::UI::CommandSuggestions.matches(commands, "/review\nnow")
      .should be_empty
  end

  it "bounds untrusted command payloads and visible matches" do
    nodes = 250.times.map do |index|
      JSON::Any.new("/command-#{index}")
    end.to_a
    commands = Xd::UI::CommandSuggestions.normalize(nodes)

    commands.size.should eq(Xd::UI::CommandSuggestions::MAX_COMMANDS)
    Xd::UI::CommandSuggestions.matches(commands, "/").size
      .should eq(Xd::UI::CommandSuggestions::MAX_MATCHES)
  end
end
