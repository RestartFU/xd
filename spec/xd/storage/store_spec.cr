require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/storage/store"
require "../../../src/xd/storage/worktree_containers"
require "../../../src/xd/storage/workspaces"

private def database_path : String
  File.join(
    Dir.tempdir,
    "xd-storage-#{Random::Secure.hex(12)}",
    "chats.db"
  )
end

private def remove_database(path : String) : Nil
  directory = Path[path].dirname.to_s
  FileUtils.rm_r(directory) if Dir.exists?(directory)
end

describe Xd::Storage::Store do
  it "creates the current schema and persists paired devices" do
    path = database_path
    now = 100_000_000_i64
    clock = -> { now }

    begin
      store = Xd::Storage::Store.new(path, clock)
      store.schema_version.should eq(Xd::Storage::SCHEMA_VERSION)
      store.add_device("token-hash", "workstation")
      store.remote_listener.should be_nil
      store.save_remote_listener("127.0.0.1", 43123)
      store.close

      now = 200_000_000_i64
      reopened = Xd::Storage::Store.new(path, clock)
      reopened.device_name("token-hash").should eq("workstation")
      reopened.device_name("missing").should be_nil
      reopened.remote_listener.should eq({"127.0.0.1", 43123})
      reopened.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        values = database.query_one(
          <<-SQL,
            SELECT created_at, last_seen
              FROM devices
             WHERE token_hash = 'token-hash'
            SQL
          as: {Int64, Int64}
        )
        values.should eq({100_i64, 200_i64})
      end
    ensure
      remove_database(path)
    end
  end
  it "persists workspace metadata in the database" do
    path = database_path
    root = File.join(Path[path].dirname, "Workspaces")
    folder = Xd::Storage::WorkspaceFolder.new(
      "stable",
      root,
      "Project",
      "codex",
      "gpt-5",
      "/code",
      "/repo",
      "Use concise answers.",
      %(["Review the diff"])
    )

    begin
      store = Xd::Storage::Store.new(path)
      store.save_workspace_folder(folder)
      store.workspace_folder(root, "Project").should eq(folder)
      store.global_shortcuts.should be_empty
      store.save_global_shortcuts(["Run all tests"])
      store.close

      reopened = Xd::Storage::Store.new(path)
      reopened.workspace_folder_by_id(root, "stable").should eq(folder)
      reopened.global_shortcuts.should eq(["Run all tests"])
      reopened.update_workspace_settings(
        "stable",
        "claude",
        nil,
        nil,
        nil,
        "Updated."
      )
      updated = reopened.workspace_folder(root, "Project").not_nil!
      updated.backend.should eq("claude")
      updated.instructions.should eq("Updated.")
      updated.shortcuts.should eq(%(["Review the diff"]))
      reopened.update_workspace_shortcuts(
        "stable",
        %(["Check formatting"])
      )
      reopened.workspace_folder(root, "Project").not_nil!.shortcuts
        .should eq(%(["Check formatting"]))
      reopened.relocate_workspace_subtree(root, "Project", "Renamed")
      reopened.workspace_folder(root, "Renamed").not_nil!.id.should eq("stable")
      reopened.workspace_folder(root, "Project").should be_nil
      reopened.close
    ensure
      remove_database(path)
    end
  end
  it "persists normalized generated-worktree containers" do
    path = database_path
    directory = Path[path].dirname.to_s
    container = File.join(directory, "worktrees")
    alias_path = File.join(directory, "worktree-alias")

    begin
      Dir.mkdir_p(container)
      File.symlink(container, alias_path)
      store = Xd::Storage::Store.new(path)
      store.register_worktree_container(alias_path).should eq(
        File.realpath(container)
      )
      store.worktree_container?(container).should be_true
      store.close

      reopened = Xd::Storage::Store.new(path)
      reopened.worktree_container?(alias_path).should be_true
      reopened.forget_worktree_container(container)
      reopened.worktree_container?(alias_path).should be_false
      reopened.close
    ensure
      remove_database(path)
    end
  end
  it "rejects paired devices without a connecting-device name" do
    path = database_path
    store = Xd::Storage::Store.new(path)

    begin
      expect_raises(Xd::Daemon::DeviceStoreError, /cannot be empty/) do
        store.add_device("token-hash", " ")
      end
    ensure
      store.close
      remove_database(path)
    end
  end

  it "lists, renames, and revokes paired devices" do
    path = database_path
    store = Xd::Storage::Store.new(path, -> { 100_000_000_i64 })

    begin
      store.add_device("first-hash", "First device")
      store.add_device("second-hash", "Second device")

      devices = store.list_devices
      devices.map(&.id).sort.should eq(["first-hash", "second-hash"])
      devices.map(&.name).sort.should eq(["First device", "Second device"])
      devices.each do |device|
        device.created_at.should eq(100_i64)
        device.last_seen.should eq(100_i64)
      end

      store.rename_device("first-hash", "Renamed device")
      store.device_name("first-hash").should eq("Renamed device")
      store.revoke_device("second-hash")
      store.list_devices.map(&.id).should eq(["first-hash"])
      expect_raises(Xd::Daemon::DeviceStoreError, "Unknown device.") do
        store.revoke_device("missing-hash")
      end
    ensure
      store.close
      remove_database(path)
    end
  end
  it "upgrades a version-one database without losing chats" do
    path = database_path
    Dir.mkdir_p(Path[path].dirname, 0o700)

    begin
      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        Xd::Storage::BASE_SCHEMA.each { |statement| database.exec(statement) }
        database.exec(
          "INSERT INTO meta (key, value) VALUES ('schema_version', '1')"
        )
        database.exec(
          <<-SQL
            INSERT INTO chats (
              id, folder_id, title, backend, created_at, updated_at
            )
            VALUES ('chat-1', 'folder-1', 'kept', 'codex', 10, 20)
            SQL
        )
      end

      store = Xd::Storage::Store.new(path)
      store.schema_version.should eq(Xd::Storage::SCHEMA_VERSION)
      store.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        title = database.query_one(
          "SELECT title FROM chats WHERE id = 'chat-1'",
          as: String
        )
        title.should eq("kept")

        columns = database.query_all(
          "SELECT name FROM pragma_table_info('chats')",
          as: String
        )
        columns.should contain("daemon_working")
        columns.should contain("last_user_message_at")
        columns.should contain("original_workdir")
        columns.should contain("fast")
        columns.should contain("claude_mode")
      end
    ensure
      remove_database(path)
    end
  end

  it "creates workspace metadata while upgrading a version-21 database" do
    path = database_path

    begin
      store = Xd::Storage::Store.new(path)
      store.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        database.exec("DROP TABLE workspace_folders")
        database.exec("DROP TABLE worktree_containers")
        database.exec(
          "UPDATE meta SET value = '21' WHERE key = 'schema_version'"
        )
      end

      upgraded = Xd::Storage::Store.new(path)
      upgraded.schema_version.should eq(Xd::Storage::SCHEMA_VERSION)
      upgraded.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        columns = database.query_all(
          "SELECT name FROM pragma_table_info('workspace_folders')",
          as: String
        )
        columns.should contain("id")
        columns.should contain("relative_path")
        columns.should contain("instructions")
        columns.should contain("shortcuts")
        worktree_columns = database.query_all(
          "SELECT name FROM pragma_table_info('worktree_containers')",
          as: String
        )
        worktree_columns.should contain("path")
      end
    ensure
      remove_database(path)
    end
  end

  it "rejects databases from a newer xd version" do
    path = database_path

    begin
      store = Xd::Storage::Store.new(path)
      store.close
      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        newer = Xd::Storage::SCHEMA_VERSION + 1
        database.exec(
          "UPDATE meta SET value = ? WHERE key = 'schema_version'",
          newer.to_s
        )
      end

      expect_raises(Xd::Storage::Error, /requires a newer xd version/) do
        Xd::Storage::Store.new(path)
      end

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        version = database.query_one(
          "SELECT value FROM meta WHERE key = 'schema_version'",
          as: String
        )
        version.should eq((Xd::Storage::SCHEMA_VERSION + 1).to_s)
      end
    ensure
      remove_database(path)
    end
  end

  it "repairs version 17 when daemon working already exists" do
    path = database_path

    begin
      store = Xd::Storage::Store.new(path)
      store.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        database.exec(
          "UPDATE meta SET value = '17' WHERE key = 'schema_version'"
        )
      end

      repaired = Xd::Storage::Store.new(path)
      repaired.schema_version.should eq(Xd::Storage::SCHEMA_VERSION)
      repaired.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        columns = database.query_all(
          <<-SQL,
            SELECT name
              FROM pragma_table_info('chats')
             WHERE name = 'daemon_working'
            SQL
          as: String
        )
        columns.should eq(["daemon_working"])
      end
    ensure
      remove_database(path)
    end
  end

  it "repairs version 18 when fast mode already exists" do
    path = database_path

    begin
      store = Xd::Storage::Store.new(path)
      store.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        database.exec(
          "UPDATE meta SET value = '18' WHERE key = 'schema_version'"
        )
      end

      repaired = Xd::Storage::Store.new(path)
      repaired.schema_version.should eq(Xd::Storage::SCHEMA_VERSION)
      repaired.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        chat_columns = database.query_all(
          <<-SQL,
            SELECT name
              FROM pragma_table_info('chats')
             WHERE name = 'fast'
            SQL
          as: String
        )
        default_columns = database.query_all(
          <<-SQL,
            SELECT name
              FROM pragma_table_info('agent_defaults')
             WHERE name = 'fast'
            SQL
          as: String
        )
        chat_columns.should eq(["fast"])
        default_columns.should eq(["fast"])
      end
    ensure
      remove_database(path)
    end
  end

  it "repairs version 19 when Claude mode already exists" do
    path = database_path

    begin
      store = Xd::Storage::Store.new(path)
      store.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        database.exec(
          "UPDATE meta SET value = '19' WHERE key = 'schema_version'"
        )
      end

      repaired = Xd::Storage::Store.new(path)
      repaired.schema_version.should eq(Xd::Storage::SCHEMA_VERSION)
      repaired.close

      DB.open("sqlite3://#{URI.encode_path(path)}") do |database|
        chat_columns = database.query_all(
          "SELECT name FROM pragma_table_info('chats') " \
          "WHERE name = 'claude_mode'",
          as: String
        )
        default_columns = database.query_all(
          "SELECT name FROM pragma_table_info('agent_defaults') " \
          "WHERE name = 'claude_mode'",
          as: String
        )
        chat_columns.should eq(["claude_mode"])
        default_columns.should eq(["claude_mode"])
      end
    ensure
      remove_database(path)
    end
  end
end
