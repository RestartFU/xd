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
      tree_before = workspaces.tree_signature

      created = service.prepare(first, "Fix parser!")
      created.should contain("fix-parser")
      File.directory?(created).should be_true
      container = File.join(
        directory,
        "Workspaces",
        "worktrees"
      )
      marker = File.join(
        container,
        Xd::Workspace::LEGACY_WORKTREE_CONTAINER_MARKER
      )
      File.exists?(marker).should be_false
      store.worktree_container?(container).should be_true
      workspaces.snapshot.folders.map(&.name).should eq(["Project"])
      workspaces.tree_signature.should eq(tree_before)

      # Worktrees created by older XD builds had no database registration.
      # Loading one recognizes XD's exact layout and persists the container.
      store.forget_worktree_container(container)
      stored = store.get_chat(first_id)
      stored.workdir.should eq(created)
      stored.original_workdir.should eq(repository)
      stored.new_worktree.should be_false

      state = service.state(stored)
      store.worktree_container?(container).should be_true
      File.exists?(marker).should be_false
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

      expect_raises(Xd::Workspace::Worktrees::Error) do
        service.remove(store.get_chat(first_id), directory)
      end
      expect_raises(Xd::Workspace::Worktrees::Error) do
        service.remove(store.get_chat(first_id), created)
      end
      store.delete_chat(second_id)

      File.write(File.join(created, "untracked.txt"), "dirty\n")
      expect_raises(Xd::Workspace::Worktrees::Error) do
        service.remove(store.get_chat(first_id), created)
      end
      File.delete(File.join(created, "untracked.txt"))

      main_id = store.create_chat(
        folder_id,
        "Main",
        "claude",
        workdir: repository
      )
      service.select(store.get_chat(main_id), repository).should eq(
        File.realpath(repository)
      )
      expect_raises(Xd::Workspace::Worktrees::Error) do
        service.remove(store.get_chat(main_id), repository)
      end
      store.delete_chat(main_id)

      current_id = store.create_chat(
        folder_id,
        "Current",
        "claude",
        workdir: created
      )
      service.select(store.get_chat(current_id), created).should eq(created)
      expect_raises(Xd::Workspace::Worktrees::Error) do
        service.remove(store.get_chat(current_id), created)
      end
      store.delete_chat(current_id)

      message_id = store.create_chat(folder_id, "Messages", "claude")
      store.set_new_worktree(message_id, true)
      message_worktree = service.prepare(
        store.get_chat(message_id),
        "Messages"
      )
      store.append_message(message_id, "user", "first")
      expect_raises(Xd::Workspace::Worktrees::Error) do
        service.remove(store.get_chat(message_id), message_worktree)
      end
      store.delete_chat(message_id)

      branch = state.worktrees.find { |item| item.path == created }
        .try(&.branch).not_nil!

      service.remove(store.get_chat(first_id), created)
      File.directory?(created).should be_false
      worktree_git(
        repository,
        "show-ref",
        "--verify",
        "--quiet",
        "refs/heads/#{branch}"
      )
      service.resolve(store.get_chat(first_id)).should eq(repository)
      restored = store.get_chat(first_id)
      restored.workdir.should eq(repository)
      restored.original_workdir.should be_nil
    ensure
      store.close
      File.delete?(home_alias)
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
