require "../spec_helper"
require "../../src/xd/git_path"

describe Xd::GitPath do
  it "converts MSYS drive paths for native Windows APIs" do
    Xd::GitPath.native("/c/Users/test/repository", windows: true)
      .should eq("C:/Users/test/repository")
    Xd::GitPath.native("/D/worktree", windows: true)
      .should eq("D:/worktree")
  end

  it "keeps Unix and already-native paths unchanged" do
    Xd::GitPath.native("/home/test/repository", windows: true)
      .should eq("/home/test/repository")
    Xd::GitPath.native("C:/Users/test/repository", windows: true)
      .should eq("C:/Users/test/repository")
    Xd::GitPath.native("/c/repository", windows: false)
      .should eq("/c/repository")
  end

  it "writes Windows environment paths with Git-compatible separators" do
    Xd::GitPath.environment(
      "C:\\Users\\test\\.git\\xd-index",
      windows: true
    ).should eq("C:/Users/test/.git/xd-index")
  end
end
