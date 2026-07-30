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

describe Xd::Workspace::SettingsFile do
  it "round-trips nullable folder settings and preserves legacy ids" do
    with_workspace do |_service, _store, root|
      folder = File.join(root, "Legacy")
      Dir.mkdir(folder)
      legacy = File.join(folder, Xd::Workspace::LEGACY_SETTINGS_FILE)
      File.write(legacy, %({"id":"stable","backend":"codex"}))

      settings = Xd::Workspace::SettingsFile.ensure(folder)
      settings.id.should eq("stable")
      settings.backend.should eq("codex")
      Xd::Workspace::SettingsFile.save(settings, folder)
      File.exists?(legacy).should be_true
      File.exists?(
        File.join(folder, Xd::Workspace::SETTINGS_FILE)
      ).should be_false
    end
  end
end

describe Xd::Workspace::Service do
  it "scans top-level workspaces and only managed nested folders" do
    with_workspace do |service, _store, root|
      workspace = File.join(root, "Workspace")
      managed = File.join(workspace, "Managed")
      source = File.join(workspace, "src")
      Dir.mkdir_p(managed)
      Dir.mkdir(source)
      Xd::Workspace::SettingsFile.ensure(managed)

      snapshot = service.snapshot
      snapshot.folders.map(&.name).should eq(["Workspace", "Managed"])
      File.exists?(
        File.join(source, Xd::Workspace::SETTINGS_FILE)
      ).should be_false
    end
  end

  it "treats repositories as leaves" do
    with_workspace do |service, _store, root|
      repository = File.join(root, "Repo")
      nested = File.join(repository, "ManagedButHidden")
      Dir.mkdir_p(File.join(repository, ".git"))
      Dir.mkdir(nested)
      Xd::Workspace::SettingsFile.ensure(nested)

      service.snapshot.folders.map(&.name).should eq(["Repo"])
    end
  end

  it "creates, renames, moves, and recoverably trashes folders" do
    with_workspace do |service, _store, _root|
      first = service.create_folder(nil, "First")
      second = service.create_folder(nil, "Second")
      child = service.create_folder(first, "Child")
      service.rename_folder(child, "Renamed")
      service.move_folder(child, first)
      service.move_folder(child, second)

      service.find_folder(child).should end_with(
        File.join("Second", "Renamed")
      )
      trashed = service.trash_folder(child)
      File.directory?(trashed).should be_true
      expect_raises(Xd::Workspace::Error, /No such folder/) do
        service.find_folder(child)
      end
    end
  end

  it "inherits overrides and accumulates instructions root-first" do
    with_workspace do |service, _store, _root|
      parent_id = service.create_folder(nil, "Lunar")
      child_id = service.create_folder(parent_id, "Proxy")
      parent_path = service.find_folder(parent_id)
      child_path = service.find_folder(child_id)

      parent = Xd::Workspace::SettingsFile.ensure(parent_path)
      parent.backend = "claude"
      parent.model = "claude-opus"
      parent.workdir = "/code/proxy"
      parent.instructions = "Always answer in French."
      Xd::Workspace::SettingsFile.save(parent, parent_path)

      child = Xd::Workspace::SettingsFile.ensure(child_path)
      child.backend = "codex"
      child.instructions = "This is a Go codebase."
      Xd::Workspace::SettingsFile.save(child, child_path)

      resolved = service.resolve(child_id)
      resolved.backend.should eq("codex")
      resolved.model.should eq("claude-opus")
      resolved.workdir.should eq("/code/proxy")
      resolved.instructions.should eq(
        "Always answer in French.\n\nThis is a Go codebase."
      )
      inherited = service.inherited_settings(child_id)
      inherited.backend.should eq("claude")
      inherited.model.should eq("claude-opus")
      inherited.workdir.should eq("/code/proxy")
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
end
