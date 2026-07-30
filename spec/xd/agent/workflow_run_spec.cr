require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/workflow_run"

describe Xd::Agent::WorkflowRun do
  it "captures quoted and chained gh run commands" do
    message = "$ git push && gh run watch '123456' --repo=RestartFU/xd"
    stored = Xd::Agent::WorkflowRun.capture(message, "/missing")
    run = Xd::Agent::WorkflowRun.parse(stored).not_nil!
    run.id.should eq("123456")
    run.repository.should eq("RestartFU/xd")
    run.url.should eq(
      "https://github.com/RestartFU/xd/actions/runs/123456"
    )
  end

  it "discovers GitHub repository from the workdir remote" do
    directory = File.join(
      Dir.tempdir,
      "xd-workflow-run-#{Random::Secure.hex(12)}"
    )
    Dir.mkdir_p(directory)
    Process.run("git", ["init", "-q"], chdir: directory)
    Process.run(
      "git",
      ["remote", "add", "origin", "git@github.com:owner/repository.git"],
      chdir: directory
    )

    begin
      stored = Xd::Agent::WorkflowRun.capture(
        "$ gh run view 98765",
        directory
      )
      Xd::Agent::WorkflowRun.parse(stored).not_nil!.repository
        .should eq("owner/repository")
    ensure
      FileUtils.rm_r(directory)
    end
  end

  it "rejects unsafe repositories, run ids, and malformed records" do
    Xd::Agent::WorkflowRun.capture(
      "$ gh run watch nope -R owner/repository",
      "/missing"
    ).should eq("$ gh run watch nope -R owner/repository")
    Xd::Agent::WorkflowRun.capture(
      "$ gh run watch 12 -R owner/repo/extra",
      "/missing"
    ).should eq("$ gh run watch 12 -R owner/repo/extra")
    Xd::Agent::WorkflowRun.parse(
      "workflow_run\n12\nhttps://example.com/owner/repo/actions/runs/12"
    ).should be_nil
  end
end
