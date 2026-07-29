require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/repository"

private def git(workdir : String, *arguments : String) : Nil
  status = Process.run(
    "git",
    arguments,
    chdir: workdir,
    output: Process::Redirect::Close,
    error: Process::Redirect::Close
  )
  status.success?.should be_true
end

private def with_repository(
  & : Xd::Daemon::Repository, String, String ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-repository-#{Random::Secure.hex(12)}"
  )
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  workspaces = Xd::Workspace::Service.new(
    File.join(directory, "Workspaces"),
    store
  )
  folder_id = workspaces.create_folder(nil, "Project")
  folder = workspaces.find_folder(folder_id)
  chat_id = store.create_chat(folder_id, "Chat", "claude")
  filesystem = Xd::Daemon::Filesystem.new(store, workspaces)
  repository = Xd::Daemon::Repository.new(
    store,
    workspaces,
    filesystem
  )

  git(folder, "init", "-q", "-b", "main")
  git(folder, "config", "user.email", "test@example.com")
  git(folder, "config", "user.name", "Test")
  File.write(File.join(folder, "tracked.txt"), "before\n")
  git(folder, "add", "tracked.txt")
  git(folder, "commit", "-q", "-m", "initial")

  begin
    yield repository, folder, chat_id
  ensure
    store.close
    FileUtils.rm_r(directory)
  end
end

describe Xd::Daemon::Repository do
  it "reads base, status, tracked, and untracked diffs" do
    with_repository do |repository, folder, chat_id|
      File.write(File.join(folder, "tracked.txt"), "after\n")
      Dir.mkdir(File.join(folder, "nested"))
      File.write(File.join(folder, "nested", "new.txt"), "new\n")

      repository.read(chat_id, "base", nil, nil).should eq("main\n")
      status = repository.read(chat_id, "working-status", nil, nil)
      status.should contain("tracked.txt")
      status.should contain("?? nested/new.txt")
      repository.read(
        chat_id,
        "working-file",
        "tracked.txt",
        nil
      ).should contain("+after")
      repository.read(
        chat_id,
        "untracked-file",
        "nested/new.txt",
        nil
      ).should contain("+new")
      all = repository.read(chat_id, "working-all", nil, nil)
      all.should contain("+after")
      all.should contain("nested/new.txt")
    end
  end

  it "rejects unsafe base names and paths" do
    with_repository do |repository, _folder, chat_id|
      expect_raises(Xd::Daemon::Repository::Error, /valid base/) do
        repository.read(chat_id, "branch-all", nil, "--output=/tmp/x")
      end
      expect_raises(Xd::Daemon::Repository::Error, /outside/) do
        repository.read(chat_id, "working-file", "../../etc/passwd", nil)
      end
    end
  end
end
