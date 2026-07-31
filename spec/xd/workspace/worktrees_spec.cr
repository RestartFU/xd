require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/workspace/worktrees"

private def worktree_git(workdir : String, *arguments : String) : Nil
  status = Process.run(
    "git",
    arguments,
    chdir: workdir,
    output: Process::Redirect::Close,
    error: Process::Redirect::Close
  )
  status.success?.should be_true
end

describe Xd::Workspace::Worktrees do
  it "creates, lists, selects, and restores daemon-owned worktrees" do
    directory = File.join(
      Dir.tempdir,
      "xd-worktrees-#{Random::Secure.hex(12)}"
    )
    store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
    workspaces = Xd::Workspace::Service.new(
      File.join(directory, "Workspaces"),
      store
    )
    folder_id = workspaces.create_folder(nil, "Project")
    repository = workspaces.find_folder(folder_id)
    home_alias = "#{directory}-home"
    File.symlink(directory, home_alias)

    worktree_git(repository, "init", "-q", "-b", "main")
    worktree_git(repository, "config", "user.email", "test@example.com")
    worktree_git(repository, "config", "user.name", "Test")
    File.write(File.join(repository, "tracked.txt"), "initial\n")
    worktree_git(repository, "add", "tracked.txt")
    worktree_git(repository, "commit", "-q", "-m", "initial")

    begin
      service = Xd::Workspace::Worktrees.new(store, workspaces)
      service.describe(repository, home: home_alias).should eq(
        "⎇ main · Project · ~/Workspaces/Project"
      )
      first_id = store.create_chat(folder_id, "Chat", "claude")
      store.set_new_worktree(first_id, true)
      first = store.get_chat(first_id)

      created = service.prepare(first, "Fix parser!")
      created.should contain("fix-parser")
      File.directory?(created).should be_true
      stored = store.get_chat(first_id)
      stored.workdir.should eq(created)
      stored.original_workdir.should eq(repository)
      stored.new_worktree.should be_false

      state = service.state(stored)
      state.linked.should be_true
      state.worktrees.size.should eq(2)
      service.describe(created, home: home_alias).should contain(
        " · Project (worktree) · ~/Workspaces/worktrees/Project/fix-parser"
      )
      named = state.worktrees.map(&.branch).compact.any? do |branch|
        branch.starts_with?("xd/fix-parser-")
      end
      named.should be_true

      second_id = store.create_chat(folder_id, "Other", "claude")
      selected = service.select(store.get_chat(second_id), created)
      selected.should eq(created)
      store.get_chat(second_id).workdir.should eq(created)
      service.registered_path(repository, created).should eq(created)
      service.registered_path(repository, directory).should be_nil
      service.describe(directory, home: home_alias).should eq(
        "~ — not a repository"
      )

      worktree_git(repository, "worktree", "remove", "--force", created)
      service.resolve(store.get_chat(first_id)).should eq(repository)
      store.get_chat(first_id).workdir.should eq(repository)
      store.get_chat(first_id).original_workdir.should be_nil
    ensure
      store.close
      File.delete?(home_alias)
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
