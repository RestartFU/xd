require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/workspace/service"

private def with_workspace(
  & : Xd::Workspace::Service, Xd::Storage::Store, String ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-workspace-#{Random::Secure.hex(12)}"
  )
  root = File.join(directory, "Workspaces")
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  service = Xd::Workspace::Service.new(root, store)

  begin
    yield service, store, root
  ensure
    store.close
    FileUtils.rm_r(directory)
  end
end

describe Xd::Workspace::Service do
  it "consumes legacy sidecars into SQLite" do
    directory = File.join(
      Dir.tempdir,
      "xd-workspace-import-#{Random::Secure.hex(12)}"
    )
    root = File.join(directory, "Workspaces")
    folder = File.join(root, "Legacy")
    store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
    begin
      Dir.mkdir_p(folder)
      sidecar = File.join(folder, Xd::Workspace::SETTINGS_FILE)
      File.write(
        sidecar,
        %({"id":"legacy-id","backend":"codex","model":"codex-model","workdir":"/legacy/work","repo":"/legacy/repo","instructions":"Keep it short."})
      )

      service = Xd::Workspace::Service.new(root, store)
      service.snapshot.folders.map(&.id).should eq(["legacy-id"])
      settings = service.folder_settings("legacy-id")
      settings.backend.should eq("codex")
      settings.model.should eq("codex-model")
      settings.workdir.should eq("/legacy/work")
      settings.repo.should eq("/legacy/repo")
      service.folder_context("legacy-id").should eq("Keep it short.")

      File.exists?(sidecar).should be_false
      service.set_folder_context("legacy-id", "Database wins.")
      service.folder_context("legacy-id").should eq("Database wins.")

      store.close
      store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
      service = Xd::Workspace::Service.new(root, store)
      reopened = service.folder_settings("legacy-id")
      reopened.backend.should eq("codex")
      reopened.model.should eq("codex-model")
      reopened.workdir.should eq("/legacy/work")
      reopened.repo.should eq("/legacy/repo")
      service.folder_context("legacy-id").should eq("Database wins.")
      File.exists?(sidecar).should be_false
    ensure
      store.close
      FileUtils.rm_r(directory)
    end
  end

  it "consumes legacy .hy.json metadata without creating a current sidecar" do
    with_workspace do |service, _store, root|
      folder = File.join(root, "Hyphen")
      Dir.mkdir(folder)
      legacy = File.join(folder, Xd::Workspace::LEGACY_SETTINGS_FILE)
      File.write(
        legacy,
        %({"id":"legacy-hy-id","backend":"codex","instructions":"Legacy context."})
      )

      service.snapshot.folders.map(&.id).should eq(["legacy-hy-id"])
      service.folder_context("legacy-hy-id").should eq("Legacy context.")
      File.exists?(File.join(folder, Xd::Workspace::SETTINGS_FILE)).should be_false
      File.exists?(legacy).should be_false
    end
  end

  it "scans top-level workspaces and only database-managed nested folders" do
    with_workspace do |service, _store, root|
      workspace = File.join(root, "Workspace")
      managed = File.join(workspace, "Managed")
      source = File.join(workspace, "src")
      Dir.mkdir_p(managed)
      Dir.mkdir(source)
      workspace_id = service.snapshot.folders.first.id
      service.create_folder(workspace_id, "Managed")

      snapshot = service.snapshot
      snapshot.folders.map(&.name).should eq(["Workspace", "Managed"])
      File.exists?(
        File.join(source, Xd::Workspace::SETTINGS_FILE)
      ).should be_false
    end
  end

  it "changes its tree signature after managed folders disappear" do
    with_workspace do |service, _store, root|
      parent = File.join(root, "Workspace")
      child = File.join(parent, "Managed")
      Dir.mkdir_p(child)
      parent_id = service.snapshot.folders.first.id
      service.create_folder(parent_id, "Managed")
      before = service.tree_signature

      FileUtils.rm_r(child)

      service.tree_signature.should_not eq(before)
    end
  end

  it "treats repositories as leaves" do
    with_workspace do |service, _store, root|
      repository = File.join(root, "Repo")
      nested = File.join(repository, "ManagedButHidden")
      Dir.mkdir_p(File.join(repository, ".git"))
      Dir.mkdir(nested)

      service.snapshot.folders.map(&.name).should eq(["Repo"])
    end
  end

  it "hides generated worktree containers" do
    with_workspace do |service, store, root|
      visible = File.join(root, "Visible")
      container = File.join(root, "worktrees")
      checkout = File.join(container, "Repo", "task", "Repo")
      Dir.mkdir(visible)
      Dir.mkdir_p(checkout)
      generated_id = service.create_folder(nil, "Generated")
      generated_child_id = service.create_folder(generated_id, "Nested")
      File.write(
        File.join(
          container,
          Xd::Workspace::LEGACY_WORKTREE_CONTAINER_MARKER
        ),
        "generated\n"
      )
      File.write(
        File.join(container, Xd::Workspace::SETTINGS_FILE),
        %({"id":"stale-worktree-container"})
      )
      File.write(
        File.join(container, Xd::Workspace::LEGACY_SETTINGS_FILE),
        %({"id":"older-stale-worktree-container"})
      )
      File.write(
        File.join(
          service.find_folder(generated_id),
          Xd::Workspace::LEGACY_WORKTREE_CONTAINER_MARKER
        ),
        "generated\n"
      )

      service.snapshot.folders.map(&.name).should eq(["Visible"])
      store.worktree_container?(container).should be_true
      File.exists?(
        File.join(
          container,
          Xd::Workspace::LEGACY_WORKTREE_CONTAINER_MARKER
        )
      ).should be_false
      File.exists?(
        File.join(container, Xd::Workspace::SETTINGS_FILE)
      ).should be_false
      File.exists?(
        File.join(container, Xd::Workspace::LEGACY_SETTINGS_FILE)
      ).should be_false
      generated = File.join(root, "Generated")
      store.worktree_container?(generated).should be_true
      File.exists?(
        File.join(
          generated,
          Xd::Workspace::LEGACY_WORKTREE_CONTAINER_MARKER
        )
      ).should be_false
      expect_raises(Xd::Workspace::Error, /No such folder/) do
        service.find_folder(generated_id)
      end
      expect_raises(Xd::Workspace::Error, /No such folder/) do
        service.find_folder(generated_child_id)
      end
      File.exists?(
        File.join(container, Xd::Workspace::SETTINGS_FILE)
      ).should be_false
    end
  end

  it "adopts an existing unmanaged directory as a workspace" do
    with_workspace do |service, _store, root|
      existing = File.join(root, "Existing")
      preserved = File.join(existing, "keep.txt")
      Dir.mkdir(existing)
      File.write(preserved, "keep\n")

      id = service.create_folder(nil, "Existing")

      service.find_folder(id).should eq(existing)
      File.read(preserved).should eq("keep\n")
      service.folder_settings(id).id.should eq(id)
      File.exists?(
        File.join(existing, Xd::Workspace::SETTINGS_FILE)
      ).should be_false
      service.create_folder(nil, "Existing").should eq(id)
    end
  end

  it "does not adopt a non-directory" do
    with_workspace do |service, _store, root|
      File.write(File.join(root, "Existing"), "file\n")

      expect_raises(Xd::Workspace::Error, /already something/) do
        service.create_folder(nil, "Existing")
      end
    end
  end

  it "creates, renames, moves, and recoverably trashes folders" do
    with_workspace do |service, store, _root|
      first = service.create_folder(nil, "First")
      second = service.create_folder(nil, "Second")
      child = service.create_folder(first, "Child")
      chat_id = store.create_chat(child, "Kept chat", "claude")
      service.rename_folder(child, "Renamed")
      service.move_folder(child, first)
      service.move_folder(child, second)

      service.find_folder(child).should end_with(
        File.join("Second", "Renamed")
      )
      service.snapshot.chats.map(&.id).should contain(chat_id)
      store.get_chat(chat_id).folder_id.should eq(child)
      trashed = service.trash_folder(child)
      File.directory?(trashed).should be_true
      expect_raises(Xd::Workspace::Error, /No such folder/) do
        service.find_folder(child)
      end
    end
  end

  it "inherits overrides and accumulates instructions root-first" do
    with_workspace do |service, _store, root|
      parent_id = service.create_folder(nil, "Lunar")
      child_id = service.create_folder(parent_id, "Proxy")
      workdir = File.join(root, "code")
      Dir.mkdir(workdir)

      service.set_folder_settings(
        parent_id,
        "claude",
        "claude-opus",
        workdir,
        nil
      )
      service.set_folder_context(parent_id, "Always answer in French.")
      service.set_global_shortcuts(["Review the diff", "Run tests"])
      service.set_workspace_shortcuts(
        parent_id,
        ["Run tests", "Check parent"]
      )
      service.set_folder_settings(child_id, "codex", nil, nil, nil)
      service.set_folder_context(child_id, "This is a Go codebase.")
      service.set_workspace_shortcuts(child_id, ["Check child"])

      resolved = service.resolve(child_id)
      resolved.backend.should eq("codex")
      resolved.model.should eq("claude-opus")
      resolved.workdir.should eq(workdir)
      resolved.instructions.should eq(
        "Always answer in French.\n\nThis is a Go codebase."
      )
      service.resolve_shortcuts(child_id).should eq([
        "Review the diff",
        "Run tests",
        "Check parent",
        "Check child",
      ])
      service.workspace_shortcuts(child_id).should eq(["Check child"])
      inherited = service.inherited_settings(child_id)
      inherited.backend.should eq("claude")
      inherited.model.should eq("claude-opus")
      inherited.workdir.should eq(workdir)
      inherited.backend_from.should be_nil
      inherited.model_from.should be_nil
      inherited.workdir_from.should be_nil

      grandchild_id = service.create_folder(child_id, "API")
      grandchild_inherited = service.inherited_settings(grandchild_id)
      grandchild_inherited.backend.should eq("codex")
      grandchild_inherited.model.should eq("claude-opus")
      grandchild_inherited.backend_from.should be_nil
      grandchild_inherited.model_from.should eq("Lunar")
      service.folder_ids(child_id).should eq([parent_id, child_id])
      service.describe_place(child_id, "/code/proxy").should eq(
        "[This conversation belongs to the folder “Lunar / Proxy” in the " \
        "user’s xd workspace tree, and you are running in /code/proxy. If " \
        "that directory holds nothing but a dotfile, it is the folder " \
        "itself rather than a checkout: say so and ask which repository is " \
        "meant, instead of searching the machine for one.]"
      )
    end
  end

  it "rejects hidden, traversing, and self-nesting mutations" do
    with_workspace do |service, _store, _root|
      parent = service.create_folder(nil, "Parent")
      child = service.create_folder(parent, "Child")

      [".hidden", "../escape", "a/b", "a\\b"].each do |name|
        expect_raises(Xd::Workspace::Error) do
          service.create_folder(nil, name)
        end
      end
      expect_raises(Xd::Workspace::Error, /inside itself/) do
        service.move_folder(parent, child)
      end
    end
  end

  it "connects a new workspace to a selected repository" do
    with_workspace do |service, _store, root|
      repository = File.join(File.dirname(root), "project")
      Dir.mkdir(repository)

      folder_id = service.create_folder(nil, "Project", repository)

      settings = service.folder_settings(folder_id)
      settings.repo.should eq(repository)
      service.resolve(folder_id).workdir.should eq(repository)
      child_id = service.create_folder(folder_id, "Child")
      service.inherited_settings(child_id).workdir.should eq(repository)
    end
  end

  it "does not move folders inside repository leaves" do
    with_workspace do |service, _store, _root|
      repository = service.create_folder(nil, "Repository")
      nested = service.create_folder(repository, "Nested")
      source = service.create_folder(nil, "Source")
      Dir.mkdir(File.join(service.find_folder(repository), ".git"))

      expect_raises(Xd::Workspace::Error, /No such folder/) do
        service.find_folder(nested)
      end
      expect_raises(Xd::Workspace::Error, /No such folder/) do
        service.folder_context(nested)
      end
      expect_raises(Xd::Workspace::Error, /inside a repository/) do
        service.move_folder(source, repository)
      end
      service.find_folder(source).should end_with(File.join("Source"))
    end
  end
end
