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
  commit : Bool = true,
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
  git(folder, "commit", "-q", "-m", "initial") if commit

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

  it "builds commit and pull request evidence for Git writing" do
    with_repository do |repository, folder, chat_id|
      File.write(File.join(folder, "tracked.txt"), "after\n")
      commit_context = repository.draft_context(chat_id, "commit")
      commit_context.should start_with("Working tree diff:")
      commit_context.should contain("+after")

      git(folder, "add", "tracked.txt")
      git(folder, "commit", "-q", "-m", "Update tracked text")
      git(folder, "checkout", "-q", "-b", "feature")
      File.write(File.join(folder, "feature.txt"), "feature\n")
      git(folder, "add", "feature.txt")
      git(folder, "commit", "-q", "-m", "Add feature")

      pull_request_context = repository.draft_context(
        chat_id,
        "pull-request"
      )
      pull_request_context.should contain("Base branch: main")
      pull_request_context.should contain("Add feature")
      pull_request_context.should contain("+feature")
    end
  end

  it "reads staged, unstaged, and untracked files before the first commit" do
    with_repository(commit: false) do |repository, folder, chat_id|
      File.write(File.join(folder, "tracked.txt"), "after\n")
      File.write(File.join(folder, "new.txt"), "new\n")

      all = repository.read(chat_id, "working-all", nil, nil)
      all.should contain("tracked.txt")
      all.should contain("+after")
      all.should contain("new.txt")
      all.should contain("+new")
      repository.read(
        chat_id,
        "working-file",
        "tracked.txt",
        nil
      ).should contain("+after")
      repository.read(
        chat_id,
        "untracked-file",
        "new.txt",
        nil
      ).should contain("+new")
    end
  end

  it "uses one private-index diff for every untracked file" do
    with_repository do |repository, folder, chat_id|
      40.times do |index|
        File.write(
          File.join(folder, "generated-#{index}.txt"),
          "generated #{index}\n"
        )
      end

      actual_git = Process.find_executable("git").not_nil!
      wrapper = File.join(folder, "git-wrapper")
      calls = File.join(folder, "git-calls")
      File.write(
        wrapper,
        "#!/bin/sh\n" \
        "printf '%s\\n' \"$*\" >> #{Process.quote_posix(calls)}\n" \
        "exec #{Process.quote_posix(actual_git)} \"$@\"\n"
      )
      File.chmod(wrapper, 0o755)
      previous_path = ENV["PATH"]?
      ENV["PATH"] = "#{folder}:#{previous_path || ""}"
      File.rename(wrapper, File.join(folder, "git"))

      begin
        all = repository.read(chat_id, "working-all", nil, nil)
        all.should contain("generated-0.txt")
        all.should contain("generated-39.txt")
      ensure
        if previous_path
          ENV["PATH"] = previous_path
        else
          ENV.delete("PATH")
        end
      end

      commands = File.read(calls).lines
      commands.count(&.starts_with?("--no-pager diff ")).should eq(1)
      commands.any?(&.includes?("--no-index")).should be_false
    end
  end

  it "reads action state without a POSIX shell" do
    with_repository do |repository, folder, chat_id|
      File.write(File.join(folder, "tracked.txt"), "after\n")
      tools = File.join(folder, "tools")
      Dir.mkdir(tools)
      File.symlink(
        Process.find_executable("git").not_nil!,
        File.join(tools, "git")
      )
      previous_path = ENV["PATH"]?
      ENV["PATH"] = tools

      begin
        state = repository.state(chat_id)
        state.visible.should be_true
        state.action.should eq("commit")
      ensure
        if previous_path
          ENV["PATH"] = previous_path
        else
          ENV.delete("PATH")
        end
      end
    end
  end

  it "recognizes the checked-out remote base as up to date" do
    with_repository do |repository, folder, chat_id|
      git(folder, "add", "-A")
      git(folder, "commit", "-q", "-m", "Track workspace settings")
      git(folder, "checkout", "-q", "-b", "release/next")
      git(
        folder,
        "remote",
        "add",
        "origin",
        "https://example.invalid/repository.git"
      )
      git(
        folder,
        "update-ref",
        "refs/remotes/origin/release/next",
        "HEAD"
      )
      git(
        folder,
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/release/next"
      )
      git(
        folder,
        "branch",
        "--set-upstream-to=origin/release/next",
        "release/next"
      )

      state = repository.state(chat_id)
      state.visible.should be_true
      state.action.should eq("none")
      state.label.should eq("Up to date")
      state.enabled.should be_false
    end
  end

  it "leaves staged state and the real index unchanged" do
    with_repository do |repository, folder, chat_id|
      tracked = File.join(folder, "tracked.txt")
      File.write(tracked, "staged\n")
      git(folder, "add", "tracked.txt")
      File.write(tracked, "worktree\n")
      File.write(File.join(folder, "new.txt"), "new\n")
      index = File.join(folder, ".git", "index")
      before = File.read(index)

      all = repository.read(chat_id, "working-all", nil, nil)

      all.should contain("+worktree")
      all.should contain("+new")
      File.read(index).should eq(before)
      Dir.glob(File.join(folder, ".git", "xd-pane-index*")).should be_empty
    end
  end

  it "stops an oversized generated-file diff at the transport limit" do
    with_repository do |repository, folder, chat_id|
      File.write(
        File.join(folder, "generated.txt"),
        ("generated output\n" * 600_000)
      )

      expect_raises(
        Xd::Daemon::Repository::Error,
        "That diff is too large to send over the remote connection."
      ) do
        repository.read(chat_id, "working-all", nil, nil)
      end
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

  it "chooses and performs the next repository action" do
    with_repository do |repository, folder, chat_id|
      File.write(File.join(folder, "tracked.txt"), "after\n")

      dirty = repository.state(chat_id)
      dirty.visible.should be_true
      dirty.action.should eq("commit")
      dirty.label.should eq("Commit")
      dirty.enabled.should be_true

      committed = repository.perform(
        chat_id,
        "commit",
        "Update tracked text"
      )
      committed.url.should be_nil
      committed.state.action.should eq("push")
      committed.state.label.should eq("Push")
      output = IO::Memory.new
      status = Process.run(
        "git",
        ["log", "-1", "--format=%s"],
        chdir: folder,
        output: output,
        error: Process::Redirect::Close
      )
      status.success?.should be_true
      output.to_s.should eq("Update tracked text\n")
    end
  end

  it "hides actions outside a repository" do
    with_repository do |repository, folder, chat_id|
      git_dir = File.join(folder, ".git")
      hidden = File.join(folder, ".git-hidden")
      File.rename(git_dir, hidden)
      begin
        state = repository.state(chat_id)
        state.visible.should be_false
        state.enabled.should be_false
        state.label.should eq("Up to date")
      ensure
        File.rename(hidden, git_dir)
      end
    end
  end

  it "tracks both branch and commit HEAD changes" do
    with_repository do |repository, folder, chat_id|
      main = repository.head_signature(chat_id)
      main.should contain("# branch.head main")

      git(folder, "checkout", "-q", "-b", "feature")
      feature = repository.head_signature(chat_id)
      feature.should contain("# branch.head feature")
      feature.should_not eq(main)

      File.write(File.join(folder, "tracked.txt"), "after\n")
      git(folder, "add", "tracked.txt")
      git(folder, "commit", "-q", "-m", "feature change")
      repository.head_signature(chat_id).should_not eq(feature)
    end
  end
end
