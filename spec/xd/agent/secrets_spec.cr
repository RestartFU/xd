require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/secrets"

private def with_secret_path(& : String, String ->) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-secrets-#{Random::Secure.hex(12)}"
  )
  path = File.join(directory, "agent-secrets.json")

  begin
    yield path, directory
  ensure
    FileUtils.rm_r(directory) if Dir.exists?(directory)
  end
end

describe Xd::Agent::Secrets do
  it "round trips sorted names without exposing values" do
    with_secret_path do |path, _directory|
      secrets = Xd::Agent::Secrets.load(path)
      secrets.set("ZEBRA_TOKEN", "hidden-zebra")
      secrets.set("ALPHA_KEY", "hidden-alpha")
      secrets.save

      File.info(path).permissions.to_i.should eq(0o600)
      loaded = Xd::Agent::Secrets.load(path)
      loaded.names.should eq(["ALPHA_KEY", "ZEBRA_TOKEN"])
      loaded.environment({} of String => String).should eq({
        "ALPHA_KEY"   => "hidden-alpha",
        "ZEBRA_TOKEN" => "hidden-zebra",
      })

      prompt = loaded.prompt.not_nil!
      prompt.should contain("ALPHA_KEY")
      prompt.should contain("ZEBRA_TOKEN")
      prompt.should_not contain("hidden-alpha")
      prompt.should_not contain("hidden-zebra")
    end
  end

  it "validates environment names and stored values" do
    Xd::Agent::Secrets.valid_name?("CLOUDFLARE_API_TOKEN").should be_true
    Xd::Agent::Secrets.valid_name?("_PRIVATE").should be_true
    Xd::Agent::Secrets.valid_name?("9TOKEN").should be_false
    Xd::Agent::Secrets.valid_name?("HAS-DASH").should be_false
    Xd::Agent::Secrets.valid_name?("").should be_false

    with_secret_path do |path, directory|
      expect_raises(Xd::Agent::Secrets::Error) do
        Xd::Agent::Secrets.new(path).set("HAS-DASH", "value")
      end

      Dir.mkdir_p(directory)
      File.write(path, %({"version":1,"secrets":{"TOKEN":""}}))
      expect_raises(Xd::Agent::Secrets::Error, /invalid secret/) do
        Xd::Agent::Secrets.load(path)
      end
    end
  end

  it "overlays global and folder scopes from outermost to innermost" do
    with_secret_path do |path, _directory|
      global = Xd::Agent::Secrets.load(path)
      parent = Xd::Agent::Secrets.for_folder("parent", path)
      child = Xd::Agent::Secrets.for_folder("child", path)

      global.set("SHARED_TOKEN", "global")
      global.set("GLOBAL_ONLY", "global-only")
      parent.set("SHARED_TOKEN", "parent")
      parent.set("PARENT_ONLY", "parent-only")
      child.set("SHARED_TOKEN", "child")
      global.save
      parent.save
      child.save

      values = Xd::Agent::Secrets
        .effective(["parent", "child"], path)
        .environment({} of String => String)
      values["SHARED_TOKEN"].should eq("child")
      values["GLOBAL_ONLY"].should eq("global-only")
      values["PARENT_ONLY"].should eq("parent-only")
    end
  end
end
