require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/workspace/clone"

private def with_clone_directory(& : String ->) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-clone-#{Random::Secure.hex(12)}"
  )
  Dir.mkdir_p(directory)
  begin
    yield directory
  ensure
    FileUtils.rm_r(directory) if Dir.exists?(directory)
  end
end

describe Xd::Workspace::Clone do
  it "accepts the addresses Git accepts" do
    clone = Xd::Workspace::Clone

    clone.normalize("https://github.com/owner/repo.git")
      .should eq("https://github.com/owner/repo.git")
    clone.normalize("  git@github.com:owner/repo.git  ")
      .should eq("git@github.com:owner/repo.git")
    clone.normalize("ssh://git@example.com:22/owner/repo.git")
      .should eq("ssh://git@example.com:22/owner/repo.git")
    clone.normalize("file:///srv/mirrors/repo.git")
      .should eq("file:///srv/mirrors/repo.git")
    clone.normalize(nil).should be_nil
    clone.normalize("   ").should be_nil
  end

  it "refuses anything that is not one" do
    clone = Xd::Workspace::Clone

    # A leading dash would reach git as an option instead of an address.
    expect_raises(Xd::Workspace::Clone::Error) do
      clone.normalize("--upload-pack=touch /tmp/pwned")
    end
    expect_raises(Xd::Workspace::Clone::Error) do
      clone.normalize("https://example.com/repo.git ; rm -rf /")
    end
    expect_raises(Xd::Workspace::Clone::Error) do
      clone.normalize("ftp://example.com/repo.git")
    end
    expect_raises(Xd::Workspace::Clone::Error) do
      clone.normalize("/srv/repo.git")
    end
    expect_raises(Xd::Workspace::Clone::Error) do
      clone.normalize("https://example.com/#{"x" * 600}")
    end
  end

  it "clones into the empty folder it is given" do
    with_clone_directory do |directory|
      source = File.join(directory, "source.git")
      Process.run("git", ["init", "-q", "--bare", source])
      destination = File.join(directory, "workspace")
      Dir.mkdir_p(destination)

      Xd::Workspace::Clone.run("file://#{source}", destination)
      File.exists?(File.join(destination, ".git")).should be_true
    end
  end

  it "refuses a folder that already holds something" do
    with_clone_directory do |directory|
      source = File.join(directory, "source.git")
      Process.run("git", ["init", "-q", "--bare", source])
      destination = File.join(directory, "workspace")
      Dir.mkdir_p(destination)
      File.write(File.join(destination, "notes.txt"), "mine\n")

      expect_raises(
        Xd::Workspace::Clone::Error,
        /already has something in it/
      ) do
        Xd::Workspace::Clone.run("file://#{source}", destination)
      end
    end
  end

  it "reports what Git said when a clone fails" do
    with_clone_directory do |directory|
      destination = File.join(directory, "workspace")
      Dir.mkdir_p(destination)

      error = expect_raises(Xd::Workspace::Clone::Error) do
        Xd::Workspace::Clone.run(
          "file://#{File.join(directory, "missing.git")}",
          destination
        )
      end
      message = error.message.not_nil!
      message.should_not be_empty
      message.should_not start_with("fatal: ")
    end
  end
end
