module Xd
  module Storage
    SCHEMA_VERSION = 21

    BASE_SCHEMA = [
      <<-SQL,
        CREATE TABLE IF NOT EXISTS meta (
          key   TEXT PRIMARY KEY,
          value TEXT NOT NULL
        )
        SQL
      <<-SQL,
        CREATE TABLE IF NOT EXISTS chats (
          id         TEXT PRIMARY KEY,
          folder_id  TEXT NOT NULL,
          title      TEXT,
          backend    TEXT NOT NULL,
          session_id TEXT,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        )
        SQL
      "CREATE INDEX IF NOT EXISTS chats_folder ON chats (folder_id, updated_at DESC)",
      <<-SQL,
        CREATE TABLE IF NOT EXISTS chat_sessions (
          chat_id         TEXT NOT NULL REFERENCES chats (id) ON DELETE CASCADE,
          backend         TEXT NOT NULL,
          session_id      TEXT,
          last_message_id INTEGER NOT NULL DEFAULT 0,
          PRIMARY KEY (chat_id, backend)
        )
        SQL
      <<-SQL,
        CREATE TABLE IF NOT EXISTS messages (
          id         INTEGER PRIMARY KEY AUTOINCREMENT,
          chat_id    TEXT NOT NULL REFERENCES chats (id) ON DELETE CASCADE,
          role       TEXT NOT NULL,
          content    TEXT NOT NULL,
          raw_json   TEXT,
          created_at INTEGER NOT NULL
        )
        SQL
      "CREATE INDEX IF NOT EXISTS messages_chat ON messages (chat_id, id)",
      <<-SQL,
        CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5 (
          content,
          content='messages',
          content_rowid='id'
        )
        SQL
      <<-SQL,
        CREATE TRIGGER IF NOT EXISTS messages_fts_insert
        AFTER INSERT ON messages BEGIN
          INSERT INTO messages_fts (rowid, content)
          VALUES (new.id, new.content);
        END
        SQL
      <<-SQL,
        CREATE TRIGGER IF NOT EXISTS messages_fts_delete
        AFTER DELETE ON messages BEGIN
          INSERT INTO messages_fts (messages_fts, rowid, content)
          VALUES ('delete', old.id, old.content);
        END
        SQL
      <<-SQL,
        CREATE TRIGGER IF NOT EXISTS messages_fts_update
        AFTER UPDATE ON messages BEGIN
          INSERT INTO messages_fts (messages_fts, rowid, content)
          VALUES ('delete', old.id, old.content);
          INSERT INTO messages_fts (rowid, content)
          VALUES (new.id, new.content);
        END
        SQL
    ]

    AGENT_DEFAULTS_TRIGGER = <<-SQL
      CREATE TRIGGER remember_agent_defaults
      AFTER UPDATE OF backend, model, effort, access, plan, fast, claude_mode
      ON chats
      WHEN OLD.backend IS NOT NEW.backend
        OR OLD.model IS NOT NEW.model
        OR OLD.effort IS NOT NEW.effort
        OR OLD.access IS NOT NEW.access
        OR OLD.plan IS NOT NEW.plan
        OR OLD.fast IS NOT NEW.fast
        OR OLD.claude_mode IS NOT NEW.claude_mode
      BEGIN
        INSERT OR REPLACE INTO agent_defaults
          (singleton, backend, model, effort, access, plan, fast, claude_mode)
        VALUES (
          1, NEW.backend, NEW.model, NEW.effort, NEW.access, NEW.plan,
          NEW.fast, NEW.claude_mode
        );
      END
      SQL

    AGENT_DEFAULTS_SCHEMA = [
      <<-SQL,
        CREATE TABLE agent_defaults (
          singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
          backend   TEXT NOT NULL,
          model     TEXT,
          effort    TEXT,
          access    TEXT,
          plan      INTEGER NOT NULL,
          fast        INTEGER NOT NULL DEFAULT 0,
          claude_mode INTEGER NOT NULL DEFAULT 0
        )
        SQL
      AGENT_DEFAULTS_TRIGGER,
    ]
  end
end
