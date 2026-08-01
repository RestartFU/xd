require "../../spec_helper"
require "../../../src/xd/ui/update_channel"

describe Xd::UI::UpdateChannel do
  it "uses matching release metadata and installers" do
    nightly = Xd::UI::UpdateChannel::Channel::Nightly
    release = Xd::UI::UpdateChannel::Channel::Release
    Xd::UI::UpdateChannel.check_url(nightly).should end_with("/tags/nightly")
    Xd::UI::UpdateChannel.installer_url(release).should contain("/latest/")
    command = Xd::UI::UpdateChannel.install_command(
      nightly,
      "/bundle/libexec/curl"
    )
    command.should contain("XD_ALLOW_RUNNING_INSTALL=1")
    command.should contain("XD_CURL=/bundle/libexec/curl")
  end

  it "reads and compares channel identities" do
    nightly = Xd::UI::UpdateChannel::Channel::Nightly
    release = Xd::UI::UpdateChannel::Channel::Release
    body = %({"target_commitish":"abcdef123","tag_name":"v1.2.3"})
    Xd::UI::UpdateChannel.latest_from_reply(nightly, body).should eq("abcdef123")
    Xd::UI::UpdateChannel.latest_from_reply(release, body).should eq("v1.2.3")
    Xd::UI::UpdateChannel.newer?(nightly, "abcdef123", current_commit: "abcdef1").should be_false
    Xd::UI::UpdateChannel.newer?(release, "v1.2.4", current_version: "1.2.3").should be_true
  end
end
