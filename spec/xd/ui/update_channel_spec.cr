require "../../spec_helper"
require "../../../src/xd/ui/update_channel"

describe Xd::UI::UpdateChannel do
  it "uses release endpoints and install commands" do
    channel = Xd::UI::UpdateChannel::Channel::Release
    Xd::UI::UpdateChannel.check_url(channel).should eq(
      "https://api.github.com/repos/RestartFU/xd/releases/latest"
    )
    Xd::UI::UpdateChannel.install_command(channel).should eq(
      "curl -fsSL https://github.com/RestartFU/xd/releases/latest/" \
      "download/install.sh | sh -s -- --release"
    )
  end

  it "uses the rolling nightly tag" do
    channel = Xd::UI::UpdateChannel::Channel::Nightly
    Xd::UI::UpdateChannel.check_url(channel).should eq(
      "https://api.github.com/repos/RestartFU/xd/releases/tags/nightly"
    )
    Xd::UI::UpdateChannel.install_command(channel).should eq(
      "curl -fsSL https://github.com/RestartFU/xd/releases/download/" \
      "nightly/install.sh | sh"
    )
  end

  it "reads channel identity from release JSON" do
    nightly = Xd::UI::UpdateChannel::Channel::Nightly
    release = Xd::UI::UpdateChannel::Channel::Release
    body = %({"target_commitish":"abcdef123","tag_name":"v1.2.3"})

    Xd::UI::UpdateChannel.latest_from_reply(nightly, body)
      .should eq("abcdef123")
    Xd::UI::UpdateChannel.latest_from_reply(release, body)
      .should eq("v1.2.3")
    Xd::UI::UpdateChannel.latest_from_reply(release, "no")
      .should be_nil
  end

  it "compares nightly commits and release versions" do
    nightly = Xd::UI::UpdateChannel::Channel::Nightly
    release = Xd::UI::UpdateChannel::Channel::Release

    Xd::UI::UpdateChannel.newer?(
      nightly,
      "abcdef123456",
      current_commit: "abcdef1"
    ).should be_false
    Xd::UI::UpdateChannel.newer?(
      nightly,
      "fedcba9",
      current_commit: "abcdef1"
    ).should be_true
    Xd::UI::UpdateChannel.newer?(
      nightly,
      "fedcba9",
      current_commit: ""
    ).should be_false
    Xd::UI::UpdateChannel.newer?(
      release,
      "v1.2.3",
      current_version: "1.2.3"
    ).should be_false
    Xd::UI::UpdateChannel.newer?(
      release,
      "v1.2.4",
      current_version: "1.2.3"
    ).should be_true
  end
end
