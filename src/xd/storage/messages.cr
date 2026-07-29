require "./chats"

module Xd
  module Storage
    MESSAGE_COLUMNS = \
      "id, chat_id, role, content, raw_json, created_at, label"

    record RecentMessages, messages : Array(Message), total : Int64

    class Store
      def append_message(
        chat_id : String,
        role : String,
        content : String = "",
        raw_json : String? = nil,
        label : String? = nil,
      ) : Int64
        database_error("Cannot store the message") do
          @database.transaction do |transaction|
            connection = transaction.connection
            now = now_microseconds
            inserted = connection.exec(
              <<-SQL,
                INSERT INTO messages (
                  chat_id, role, content, raw_json, created_at, label
                )
                VALUES (?, ?, ?, ?, ?, ?)
                SQL
              chat_id,
              role,
              content,
              raw_json,
              now // 1_000_000,
              label
            )

            if role == "user"
              connection.exec(
                <<-SQL,
                  UPDATE chats
                     SET updated_at = ?, last_user_message_at = ?
                   WHERE id = ?
                  SQL
                now // 1_000_000,
                now,
                chat_id
              )
            else
              connection.exec(
                "UPDATE chats SET updated_at = ? WHERE id = ?",
                now // 1_000_000,
                chat_id
              )
            end

            inserted.last_insert_id
          end.not_nil!
        end
      end

      def update_message(message_id : Int64, content : String) : Nil
        database_error("Cannot update the message") do
          result = @database.exec(
            "UPDATE messages SET content = ? WHERE id = ?",
            content,
            message_id
          )
          if result.rows_affected != 1
            raise NotFoundError.new("No message #{message_id}")
          end
        end
      end

      def delete_message(message_id : Int64) : Nil
        database_error("Cannot remove the message") do
          result = @database.exec(
            "DELETE FROM messages WHERE id = ?",
            message_id
          )
          if result.rows_affected != 1
            raise NotFoundError.new("No message #{message_id}")
          end
        end
      end

      def last_message_id(chat_id : String) : Int64
        database_error("Cannot read the conversation") do
          @database.query_one(
            "SELECT COALESCE(MAX(id), 0) FROM messages WHERE chat_id = ?",
            chat_id,
            as: Int64
          )
        end
      end

      def list_messages(chat_id : String) : Array(Message)
        query_messages(
          <<-SQL,
            SELECT #{MESSAGE_COLUMNS}
              FROM messages
             WHERE chat_id = ?
             ORDER BY id
            SQL
          chat_id
        )
      end

      def list_messages_since(
        chat_id : String,
        after_id : Int64,
      ) : Array(Message)
        query_messages(
          <<-SQL,
            SELECT #{MESSAGE_COLUMNS}
              FROM messages
             WHERE chat_id = ? AND id > ?
             ORDER BY id
            SQL
          chat_id,
          after_id
        )
      end

      def list_recent_messages(
        chat_id : String,
        limit : Int,
      ) : RecentMessages
        list_recent_messages_through(chat_id, Int64::MAX, limit)
      end

      def list_recent_messages_through(
        chat_id : String,
        through_id : Int64,
        limit : Int,
      ) : RecentMessages
        raise ArgumentError.new("limit must be positive") unless limit > 0

        database_error("Cannot read recent conversation") do
          total = @database.query_one(
            <<-SQL,
              SELECT COUNT(*)
                FROM messages
               WHERE chat_id = ? AND id <= ?
              SQL
            chat_id,
            through_id,
            as: Int64
          )

          messages = @database.query_all(
            <<-SQL,
              SELECT id, chat_id, role, content, NULL, created_at, label
                FROM messages
               WHERE chat_id = ? AND id <= ?
               ORDER BY id DESC
               LIMIT ?
              SQL
            chat_id,
            through_id,
            limit
          ) { |row| message_from_row(row) }

          RecentMessages.new(messages.reverse!, total)
        end
      end

      def search(query : String, limit : Int) : Array(Message)
        database_error("Cannot search") do
          @database.query_all(
            <<-SQL,
              SELECT m.id, m.chat_id, m.role, m.content, m.raw_json,
                     m.created_at, m.label
                FROM messages_fts f
                JOIN messages m ON m.id = f.rowid
               WHERE f.messages_fts MATCH ?
               ORDER BY f.rank
               LIMIT ?
              SQL
            query,
            limit
          ) { |row| message_from_row(row) }
        end
      end

      def list_folder_ids : Array(String)
        database_error("Cannot list folders") do
          @database.query_all(
            "SELECT DISTINCT folder_id FROM chats",
            as: String
          )
        end
      end

      private def query_messages(query : String, *arguments) : Array(Message)
        database_error("Cannot read the conversation") do
          @database.query_all(query, *arguments) do |row|
            message_from_row(row)
          end
        end
      end

      private def message_from_row(row : DB::ResultSet) : Message
        Message.new(
          id: row.read(Int64),
          chat_id: row.read(String),
          role: row.read(String),
          content: row.read(String),
          raw_json: row.read(String?),
          created_at: row.read(Int64),
          label: row.read(String?)
        )
      end
    end
  end
end
