require "uuid"
require "./models"
require "./store"

module Xd
  module Storage
    CHAT_COLUMNS = <<-SQL
      id, folder_id, title, backend, workdir, model, effort, access, plan,
      created_at, updated_at, terminal_open, diff_open, queued, new_worktree,
      original_workdir, daemon_working
      SQL

    class Store
      def create_chat(
        folder_id : String,
        title : String?,
        backend : String,
        model : String? = nil,
        effort : String? = nil,
        workdir : String? = nil,
      ) : String
        database_error("Cannot create the chat") do
          defaults = @database.query_one?(
            <<-SQL,
              SELECT backend, model, effort, access, plan
                FROM agent_defaults
               WHERE singleton = 1
              SQL
            as: {String, String?, String?, String?, Bool}
          )

          actual_backend = defaults.try(&.[0]) || backend
          actual_model = defaults ? defaults[1] : model
          actual_effort = defaults ? defaults[2] : effort
          actual_access = defaults.try(&.[3])
          actual_plan = defaults.try(&.[4]) || false
          id = UUID.random.to_s
          now = now_microseconds

          @database.exec(
            <<-SQL,
              INSERT INTO chats (
                id, folder_id, title, backend, model, effort, access, plan,
                workdir, created_at, updated_at, last_user_message_at
              )
              VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
              SQL
            id,
            folder_id,
            title,
            actual_backend,
            actual_model,
            actual_effort,
            actual_access,
            actual_plan,
            workdir,
            now // 1_000_000,
            now // 1_000_000,
            now
          )
          id
        end
      end

      def get_chat(chat_id : String) : Chat
        database_error("Cannot read the chat") do
          chat = @database.query_one?(
            "SELECT #{CHAT_COLUMNS} FROM chats WHERE id = ?",
            chat_id
          ) { |row| chat_from_row(row) }
          chat || raise NotFoundError.new("No chat #{chat_id}")
        end
      end

      def list_chats(folder_id : String) : Array(Chat)
        database_error("Cannot list chats") do
          @database.query_all(
            <<-SQL,
              SELECT #{CHAT_COLUMNS}
                FROM chats
               WHERE folder_id = ?
               ORDER BY last_user_message_at DESC, created_at DESC
              SQL
            folder_id
          ) { |row| chat_from_row(row) }
        end
      end

      def set_chat_title(chat_id : String, title : String?) : Nil
        update_chat_column(
          "title",
          title,
          chat_id,
          "Cannot rename the chat"
        )
      end

      def set_backend(chat_id : String, backend : String) : Nil
        update_chat_column(
          "backend",
          backend,
          chat_id,
          "Cannot change the backend"
        )
      end

      def set_model(chat_id : String, model : String?) : Nil
        update_chat_column(
          "model",
          model,
          chat_id,
          "Cannot change the model"
        )
      end

      def set_model_selection(
        chat_id : String,
        backend : String,
        model : String,
      ) : Nil
        database_error("Cannot change the assistant and model") do
          @database.exec(
            <<-SQL,
              UPDATE chats
                 SET backend = ?, model = ?, updated_at = ?
               WHERE id = ?
              SQL
            backend,
            model,
            now_seconds,
            chat_id
          )
        end
      end

      def set_effort(chat_id : String, effort : String?) : Nil
        update_chat_column(
          "effort",
          effort,
          chat_id,
          "Cannot change the effort"
        )
      end

      def set_access(chat_id : String, access : String?) : Nil
        update_chat_column(
          "access",
          access,
          chat_id,
          "Cannot change the access level"
        )
      end

      def set_plan(chat_id : String, plan : Bool) : Nil
        database_error("Cannot change plan mode") do
          @database.exec(
            "UPDATE chats SET plan = ?, updated_at = ? WHERE id = ?",
            plan,
            now_seconds,
            chat_id
          )
        end
      end

      def set_panes(
        chat_id : String,
        terminal_open : Bool,
        diff_open : Bool,
      ) : Nil
        database_error("Cannot remember the open panes") do
          @database.exec(
            <<-SQL,
              UPDATE chats
                 SET terminal_open = ?, diff_open = ?
               WHERE id = ?
              SQL
            terminal_open,
            diff_open,
            chat_id
          )
        end
      end

      def set_daemon_working(chat_id : String, working : Bool) : Nil
        database_error("Cannot update the daemon turn") do
          result = @database.exec(
            "UPDATE chats SET daemon_working = ? WHERE id = ?",
            working,
            chat_id
          )
          if result.rows_affected != 1
            raise NotFoundError.new("No chat #{chat_id}")
          end
        end
      end

      def clear_daemon_working : Nil
        database_error("Cannot clear daemon turn state") do
          @database.exec(
            "UPDATE chats SET daemon_working = 0 WHERE daemon_working != 0"
          )
        end
      end

      def delete_chat(chat_id : String) : Nil
        database_error("Cannot delete the chat") do
          @database.exec("DELETE FROM chats WHERE id = ?", chat_id)
        end
      end

      private def update_chat_column(
        column : String,
        value : String?,
        chat_id : String,
        context : String,
      ) : Nil
        database_error(context) do
          @database.exec(
            "UPDATE chats SET #{column} = ?, updated_at = ? WHERE id = ?",
            value,
            now_seconds,
            chat_id
          )
        end
      end

      private def chat_from_row(row : DB::ResultSet) : Chat
        Chat.new(
          id: row.read(String),
          folder_id: row.read(String),
          title: row.read(String?),
          backend: row.read(String),
          workdir: row.read(String?),
          model: row.read(String?),
          effort: row.read(String?),
          access: row.read(String?),
          plan: row.read(Bool),
          created_at: row.read(Int64),
          updated_at: row.read(Int64),
          terminal_open: row.read(Bool),
          diff_open: row.read(Bool),
          queue: Storage.queue_from_column(row.read(String?)),
          new_worktree: row.read(Bool),
          original_workdir: row.read(String?),
          daemon_working: row.read(Bool)
        )
      end
    end
  end
end
