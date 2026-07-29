require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/storage/store"

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
    now = 100_i64
    clock = -> { now }

    begin
      store = Xd::Storage::Store.new(path, clock)
      store.schema_version.should eq(Xd::Storage::SCHEMA_VERSION)
      store.add_device("token-hash", "workstation")
      store.close

      now = 200_i64
      reopened = Xd::Storage::Store.new(path, clock)
      reopened.device_name("token-hash").should eq("workstation")
      reopened.device_name("missing").should be_nil
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
          <<-SQL,
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
      end
    ensure
      remove_database(path)
    end
  end
end
