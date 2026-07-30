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
end
