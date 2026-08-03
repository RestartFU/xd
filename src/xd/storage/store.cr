require "db"
require "json"
require "sqlite3"
require "uri"
require "../daemon/device_store"
require "./schema"

module Xd
  module Storage
    class Error < Daemon::DeviceStoreError
    end

    class NotFoundError < Error
    end

    class Store < Daemon::DeviceStore
      getter path : String

      @database : DB::Database
      @queue_mutex = Mutex.new

      def initialize(
        @path : String,
        @clock : Proc(Int64) = -> { Time.utc.to_unix_ms * 1_000 },
      )
        directory = Path[@path].dirname
        Dir.mkdir_p(directory, 0o700) unless directory.to_s == "."

        @database = open_database(@path)
        begin
          migrate
        rescue error
          @database.close
          raise error
        end
      end

      def close : Nil
        @database.close
      end

      def schema_version : Int32
        @database.query_one(
          "SELECT value FROM meta WHERE key = 'schema_version'",
          as: String
        ).to_i
      end

      def add_device(token_hash : String, name : String) : Nil
        normalized = Daemon::DeviceStore.normalize_name(name)
        now = now_seconds
        @database.exec(
          <<-SQL,
            INSERT INTO devices (token_hash, name, created_at, last_seen)
            VALUES (?, ?, ?, ?)
            SQL
          token_hash,
          normalized,
          now,
          now
        )
      rescue error : DB::Error
        raise Daemon::DeviceStoreError.new(
          "Cannot pair the device: #{error.message}"
        )
      end

      def device_name(token_hash : String) : String?
        @database.query_one?(
          <<-SQL,
            UPDATE devices
               SET last_seen = ?
             WHERE token_hash = ?
            RETURNING name
            SQL
          now_seconds,
          token_hash,
          as: String
        )
      rescue error : DB::Error
        raise Error.new(
          "Cannot authenticate the device: #{error.message}"
        )
      end

      def list_devices : Array(Daemon::Device)
        @database.query_all(
          <<-SQL,
            SELECT token_hash, name, created_at, last_seen
              FROM devices
             ORDER BY last_seen DESC, created_at DESC
            SQL
          as: {String, String, Int64, Int64}
        ).map do |token_hash, name, created_at, last_seen|
          Daemon::Device.new(token_hash, name, created_at, last_seen)
        end
      rescue error : DB::Error
        raise Error.new(
          "Cannot list paired devices: #{error.message}"
        )
      end

      def rename_device(token_hash : String, name : String) : Nil
        normalized = Daemon::DeviceStore.normalize_name(name)
        renamed = @database.query_one?(
          <<-SQL,
            UPDATE devices
               SET name = ?
             WHERE token_hash = ?
            RETURNING token_hash
            SQL
          normalized,
          token_hash,
          as: String
        )
        return if renamed

        raise Daemon::DeviceStoreError.new("Unknown device.")
      rescue error : DB::Error
        raise Error.new(
          "Cannot rename the device: #{error.message}"
        )
      end

      def revoke_device(token_hash : String) : Nil
        revoked = @database.query_one?(
          <<-SQL,
            DELETE FROM devices
             WHERE token_hash = ?
            RETURNING token_hash
            SQL
          token_hash,
          as: String
        )
        return if revoked

        raise Daemon::DeviceStoreError.new("Unknown device.")
      rescue error : DB::Error
        raise Error.new(
          "Cannot revoke the device: #{error.message}"
        )
      end

      def remote_listener : {String, Int32}?
        value = @database.query_one?(
          "SELECT value FROM meta WHERE key = 'remote_listener'",
          as: String
        )
        return unless value

        parsed = JSON.parse(value)
        bind = parsed["bind"].as_s
        port = parsed["port"].as_i
        unless 0 <= port <= UInt16::MAX
          raise Error.new("Saved remote listener port is invalid.")
        end
        {bind, port.to_i32}
      rescue error : DB::Error | JSON::ParseException | TypeCastError | KeyError
        raise Error.new(
          "Cannot read remote listener settings: #{error.message}"
        )
      end

      def save_remote_listener(bind : String, port : Int32) : Nil
        value = {
          "bind" => bind,
          "port" => port,
        }.to_json
        @database.exec(
          <<-SQL,
            INSERT INTO meta (key, value)
            VALUES ('remote_listener', ?)
            ON CONFLICT (key) DO UPDATE SET value = excluded.value
            SQL
          value
        )
      rescue error : DB::Error
        raise Error.new(
          "Cannot save remote listener settings: #{error.message}"
        )
      end

      private def migrate : Nil
        @database.transaction do |transaction|
          connection = transaction.connection
          BASE_SCHEMA.each { |statement| connection.exec(statement) }

          version = connection.query_one?(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            as: String
          ).try(&.to_i) || 0

          connection.exec(
            "ALTER TABLE chats ADD COLUMN workdir TEXT"
          ) if version < 2
          connection.exec(
            "ALTER TABLE chats ADD COLUMN model TEXT"
          ) if version < 3

          if version < 4
            connection.exec <<-SQL
              INSERT OR IGNORE INTO chat_sessions (
                chat_id, backend, session_id
              )
              SELECT id, backend, session_id
                FROM chats
               WHERE session_id IS NOT NULL
              SQL
          end

          if version == 4
            connection.exec <<-SQL
              CREATE TABLE chat_sessions_new (
                chat_id         TEXT NOT NULL
                                REFERENCES chats (id) ON DELETE CASCADE,
                backend         TEXT NOT NULL,
                session_id      TEXT,
                last_message_id INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (chat_id, backend)
              )
              SQL
            connection.exec <<-SQL
              INSERT OR IGNORE INTO chat_sessions_new (
                chat_id, backend, session_id
              )
              SELECT chat_id, backend, session_id FROM chat_sessions
              SQL
            connection.exec "DROP TABLE chat_sessions"
            connection.exec(
              "ALTER TABLE chat_sessions_new RENAME TO chat_sessions"
            )
          end

          connection.exec(
            "ALTER TABLE chats ADD COLUMN plan INTEGER NOT NULL DEFAULT 0"
          ) if version < 7

          if version < 10
            connection.exec <<-SQL
              CREATE TABLE IF NOT EXISTS devices (
                token_hash TEXT PRIMARY KEY,
                name       TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_seen  INTEGER NOT NULL
              )
              SQL
          end

          connection.exec(
            "ALTER TABLE chats ADD COLUMN queued TEXT"
          ) if version < 11
          connection.exec(
            "ALTER TABLE chats ADD COLUMN new_worktree INTEGER NOT NULL DEFAULT 0"
          ) if version < 12

          if version < 13
            connection.exec(
              "ALTER TABLE chat_sessions " \
              "ADD COLUMN context_used INTEGER NOT NULL DEFAULT 0"
            )
            connection.exec(
              "ALTER TABLE chat_sessions " \
              "ADD COLUMN context_window INTEGER NOT NULL DEFAULT 0"
            )
            connection.exec(
              "ALTER TABLE chat_sessions ADD COLUMN context_model TEXT"
            )
          end

          if version < 9
            connection.exec(
              "ALTER TABLE chats " \
              "ADD COLUMN terminal_open INTEGER NOT NULL DEFAULT 0"
            )
            connection.exec(
              "ALTER TABLE chats " \
              "ADD COLUMN diff_open INTEGER NOT NULL DEFAULT 0"
            )
          end

          connection.exec(
            "ALTER TABLE messages ADD COLUMN label TEXT"
          ) if version < 8

          if version < 6
            connection.exec "ALTER TABLE chats ADD COLUMN effort TEXT"
            connection.exec "ALTER TABLE chats ADD COLUMN access TEXT"
          end

          if version < 14
            AGENT_DEFAULTS_SCHEMA.each do |statement|
              connection.exec(statement)
            end
          end

          connection.exec(
            "ALTER TABLE chats " \
            "ADD COLUMN resume_after_restart INTEGER NOT NULL DEFAULT 0"
          ) if version < 15
          connection.exec(
            "ALTER TABLE chats ADD COLUMN original_workdir TEXT"
          ) if version < 16

          if version < 17
            connection.exec(
              "ALTER TABLE chats " \
              "ADD COLUMN last_user_message_at INTEGER NOT NULL DEFAULT 0"
            )
            connection.exec <<-SQL
              UPDATE chats
                 SET last_user_message_at = COALESCE(
                   (
                     SELECT MAX(messages.created_at) * 1000000
                       FROM messages
                      WHERE messages.chat_id = chats.id
                        AND messages.role = 'user'
                   ),
                   chats.created_at * 1000000
                 )
              SQL
            connection.exec <<-SQL
              CREATE INDEX chats_folder_user_message
                          ON chats (folder_id, last_user_message_at DESC)
              SQL
          end

          if version < 18
            chat_columns = connection.query_all(
              "SELECT name FROM pragma_table_info('chats')",
              as: String
            )
            unless chat_columns.includes?("daemon_working")
              connection.exec(
                "ALTER TABLE chats " \
                "ADD COLUMN daemon_working INTEGER NOT NULL DEFAULT 0"
              )
            end
          end

          if version < 19
            chat_columns = connection.query_all(
              "SELECT name FROM pragma_table_info('chats')",
              as: String
            )
            unless chat_columns.includes?("fast")
              connection.exec(
                "ALTER TABLE chats " \
                "ADD COLUMN fast INTEGER NOT NULL DEFAULT 0"
              )
            end
            default_columns = connection.query_all(
              "SELECT name FROM pragma_table_info('agent_defaults')",
              as: String
            )
            unless default_columns.includes?("fast")
              connection.exec(
                "ALTER TABLE agent_defaults " \
                "ADD COLUMN fast INTEGER NOT NULL DEFAULT 0"
              )
            end
            connection.exec "DROP TRIGGER IF EXISTS remember_agent_defaults"
            connection.exec AGENT_DEFAULTS_TRIGGER
          end

          if version < 20
            chat_columns = connection.query_all(
              "SELECT name FROM pragma_table_info('chats')",
              as: String
            )
            unless chat_columns.includes?("claude_mode")
              connection.exec(
                "ALTER TABLE chats " \
                "ADD COLUMN claude_mode INTEGER NOT NULL DEFAULT 0"
              )
            end
            default_columns = connection.query_all(
              "SELECT name FROM pragma_table_info('agent_defaults')",
              as: String
            )
            unless default_columns.includes?("claude_mode")
              connection.exec(
                "ALTER TABLE agent_defaults " \
                "ADD COLUMN claude_mode INTEGER NOT NULL DEFAULT 0"
              )
            end
            connection.exec "DROP TRIGGER IF EXISTS remember_agent_defaults"
            connection.exec AGENT_DEFAULTS_TRIGGER
          end
          if version < 21
            chat_columns = connection.query_all(
              "SELECT name FROM pragma_table_info('chats')",
              as: String
            )
            unless chat_columns.includes?("draft")
              connection.exec(
                "ALTER TABLE chats " \
                "ADD COLUMN draft TEXT NOT NULL DEFAULT ''"
              )
            end
            unless chat_columns.includes?("draft_attachments")
              connection.exec(
                "ALTER TABLE chats " \
                "ADD COLUMN draft_attachments TEXT NOT NULL DEFAULT '[]'"
              )
            end
            unless chat_columns.includes?("draft_revision")
              connection.exec(
                "ALTER TABLE chats " \
                "ADD COLUMN draft_revision INTEGER NOT NULL DEFAULT 0"
              )
            end
          end

          connection.exec(
            <<-SQL,
              INSERT INTO meta (key, value)
              VALUES ('schema_version', ?)
              ON CONFLICT (key) DO UPDATE SET value = excluded.value
              SQL
            SCHEMA_VERSION.to_s
          )
        end
      rescue error : DB::Error
        raise Error.new(
          "Cannot migrate the chat database: #{error.message}"
        )
      end

      private def open_database(path : String) : DB::Database
        uri = "sqlite3://#{URI.encode_path(path)}" \
              "?journal_mode=wal&synchronous=normal&foreign_keys=on"
        DB.open(uri)
      rescue error : DB::Error
        raise Error.new(
          "Cannot open the chat database: #{error.message}"
        )
      end

      private def now_microseconds : Int64
        @clock.call
      end

      private def now_seconds : Int64
        now_microseconds // 1_000_000
      end

      private def database_error(context : String, & : -> T) : T forall T
        yield
      rescue error : DB::Error
        raise Error.new("#{context}: #{error.message}")
      end
    end
  end
end
