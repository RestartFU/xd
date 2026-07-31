require "../../spec_helper"
require "../../../src/xd/ui/diff_pane"

describe Xd::UI::DiffPane do
  it "recognizes the daemon's non-repository error exactly" do
    Xd::UI::DiffPane.error_title(
      "This chat is not in a Git repository."
    ).should eq("Not a Git Repository")
  end

  it "keeps size and unknown failures distinct" do
    Xd::UI::DiffPane.error_title(
      "Diff output is too large."
    ).should eq("Diff Too Large")
    Xd::UI::DiffPane.error_title(
      "Git process failed."
    ).should eq("Could Not Read Changes")
  end
end
