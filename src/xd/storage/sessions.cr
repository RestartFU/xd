require "./messages"

module Xd
  module Storage
    record ContextUsage, used : UInt64, window : UInt64

    class Store
      def get_session_id(chat_id : String, backend : String) : String?
        database_error("Cannot read the session id") do
          @database.query_one?(
            <<-SQL,
              SELECT session_id
                FROM chat_sessions
               WHERE chat_id = ? AND backend = ?
              SQL
            chat_id,
            backend,
            as: String?
          )
        end
      end

      def set_session_id(
        chat_id : String,
        backend : String,
        session_id : String?,
      ) : Nil
        context = session_id ? "Cannot store the session id" :
                               "Cannot forget the session id"
        database_error(context) do
          if session_id
            @database.exec(
              <<-SQL,
                INSERT INTO chat_sessions (chat_id, backend, session_id)
                VALUES (?, ?, ?)
                ON CONFLICT (chat_id, backend)
                DO UPDATE SET session_id = excluded.session_id
                SQL
              chat_id,
              backend,
              session_id
            )
          else
            @database.exec(
              <<-SQL,
                UPDATE chat_sessions
                   SET session_id = NULL,
                       last_message_id = 0,
                       context_used = 0,
                       context_window = 0,
                       context_model = NULL
                 WHERE chat_id = ? AND backend = ?
                SQL
              chat_id,
              backend
            )
          end
        end
      end

      def get_last_seen(chat_id : String, backend : String) : Int64
        database_error("Cannot read what the assistant has seen") do
          @database.query_one?(
            <<-SQL,
              SELECT last_message_id
                FROM chat_sessions
               WHERE chat_id = ? AND backend = ?
              SQL
            chat_id,
            backend,
            as: Int64
          ) || 0_i64
        end
      end

      def set_last_seen(
        chat_id : String,
        backend : String,
        message_id : Int64,
      ) : Nil
        database_error("Cannot record what the assistant has seen") do
          @database.exec(
            <<-SQL,
              INSERT INTO chat_sessions (
                chat_id, backend, last_message_id
              )
              VALUES (?, ?, ?)
              ON CONFLICT (chat_id, backend)
              DO UPDATE SET last_message_id = excluded.last_message_id
              SQL
            chat_id,
            backend,
            message_id
          )
        end
      end

      def set_context_usage(
        chat_id : String,
        backend : String,
        model : String?,
        used : UInt64,
        window : UInt64,
      ) : Nil
        if used > Int64::MAX || window > Int64::MAX
          raise ArgumentError.new("context usage exceeds SQLite integer range")
        end

        database_error("Cannot store context usage") do
          @database.exec(
            <<-SQL,
              INSERT INTO chat_sessions (
                chat_id, backend, context_model, context_used, context_window
              )
              VALUES (?, ?, ?, ?, ?)
              ON CONFLICT (chat_id, backend) DO UPDATE SET
                context_model = excluded.context_model,
                context_used = excluded.context_used,
                context_window = excluded.context_window
              SQL
            chat_id,
            backend,
            model,
            used.to_i64,
            window.to_i64
          )
        end
      end

      def get_context_usage(
        chat_id : String,
        backend : String,
        model : String? = nil,
      ) : ContextUsage?
        database_error("Cannot read context usage") do
          usage = @database.query_one?(
            <<-SQL,
              SELECT context_model, context_used, context_window
                FROM chat_sessions
               WHERE chat_id = ? AND backend = ?
              SQL
            chat_id,
            backend,
            as: {String?, Int64, Int64}
          )
          next nil unless usage

          stored_model, used, window = usage
          next nil if model && stored_model != model
          next nil unless used > 0 && window > 0

          ContextUsage.new(used.to_u64, window.to_u64)
        end
      end
    end
  end
end
