require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/catalog"

describe Xd::Agent::Catalog do
  it "names every model and lists each default" do
    Xd::Agent::Catalog.all.should_not be_empty
    Xd::Agent::Catalog.all.each do |backend|
      backend.models.should_not be_empty
      backend.models.map(&.id).should contain(backend.default_model)
      backend.models.each do |model|
        model.id.should_not be_empty
        model.display_name.should_not be_empty
      end
    end
  end

  it "looks up backends and labels models safely" do
    claude = Xd::Agent::Catalog.lookup("claude").not_nil!
    claude.model_label("claude-opus-5").should eq("Claude Opus 5")
    claude.model_label(nil).should eq("Claude Opus 5")
    claude.model_label("future-model").should eq("future-model")
    Xd::Agent::Catalog.lookup("future-backend").should be_nil
    Xd::Agent::Catalog.lookup(nil).should be_nil
  end

  it "offers backend-specific maximum reasoning efforts" do
    Xd::Agent::Catalog::CODEX.efforts.should contain(
      Xd::Agent::Effort::Ultra
    )
    Xd::Agent::Catalog::CODEX.efforts.should_not contain(
      Xd::Agent::Effort::UltraCode
    )
    Xd::Agent::Catalog::CLAUDE.efforts.should contain(
      Xd::Agent::Effort::UltraCode
    )
    Xd::Agent::Catalog::CLAUDE.efforts.should_not contain(
      Xd::Agent::Effort::Ultra
    )
    Xd::Agent::Effort.from_wire("ultra")
      .should eq(Xd::Agent::Effort::Ultra)
    Xd::Agent::Effort::Ultra.wire_name.should eq("ultra")
    Xd::Agent::Effort.from_wire("ultracode")
      .should eq(Xd::Agent::Effort::UltraCode)
    Xd::Agent::Effort::UltraCode.wire_name.should eq("ultracode")
  end

  it "builds resumable Claude arguments with access and effort" do
    claude = Xd::Agent::Catalog::CLAUDE
    plain = claude.build_argv(Xd::Agent::RunSpec.new("hello"))
    plain.should contain("--output-format")
    plain.should contain("stream-json")
    plain.should contain("--verbose")
    plain.should_not contain("--resume")

    resumed = claude.build_argv(Xd::Agent::RunSpec.new(
      "hello",
      model: "claude-opus-5",
      system_prompt: "answer in French",
      resume_session_id: "sess-1",
      effort: Xd::Agent::Effort::XHigh,
      access: Xd::Agent::Access::Edit
    ))
    resumed.each_cons(2).to_a.should contain(["--resume", "sess-1"])
    resumed.each_cons(2).to_a.should contain(["--model", "claude-opus-5"])
    resumed.each_cons(2).to_a.should contain(["--effort", "xhigh"])
    resumed.each_cons(2).to_a.should contain([
      "--permission-mode",
      "acceptEdits",
    ])

    ultracode = claude.build_argv(Xd::Agent::RunSpec.new(
      "hello",
      effort: Xd::Agent::Effort::UltraCode
    ))
    ultracode.each_cons(2).to_a.should contain([
      "--effort",
      "ultracode",
    ])
  end

  it "uses Codex app-server and carries developer instructions separately" do
    codex = Xd::Agent::Catalog::CODEX
    spec = Xd::Agent::RunSpec.new(
      "hello",
      system_prompt: "be brief",
      access: Xd::Agent::Access::Plan
    )

    codex.build_argv(spec).should eq([
      "codex", "app-server", "--listen", "stdio://",
    ])
    instructions = codex.developer_instructions(spec).not_nil!
    instructions.should contain("<plan_mode>")
    instructions.should contain("be brief")
    instructions.should contain(
      "Co-authored-by: Codex <codex@openai.com>"
    )
    instructions.should contain(
      "unless the user specifically asks you not to"
    )
  end

  it "defaults unknown access to read-only" do
    Xd::Agent::Access.from_wire("unknown")
      .should eq(Xd::Agent::Access::ReadOnly)
    Xd::Agent::Access.from_wire(nil)
      .should eq(Xd::Agent::Access::ReadOnly)
  end

  it "prefers Codex's refreshed model context window" do
    directory = File.join(
      Dir.tempdir,
      "xd-codex-models-#{Random::Secure.hex(12)}"
    )
    old_home = ENV["CODEX_HOME"]?

    begin
      Dir.mkdir_p(directory)
      File.write(File.join(directory, "models_cache.json"), {
        "models" => [
          {"slug" => "gpt-5.6-sol", "context_window" => 333_000},
        ],
      }.to_json)
      ENV["CODEX_HOME"] = directory
      Xd::Agent::Catalog::CODEX.context_window("gpt-5.6-sol")
        .should eq(333_000_u64)
    ensure
      if old_home
        ENV["CODEX_HOME"] = old_home
      else
        ENV.delete("CODEX_HOME")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
