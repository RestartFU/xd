require "./sessions"

module Xd
  module Storage
    class ConflictError < Error
    end

    class Store
      def switch_workdir(
        chat_id : String,
        workdir : String,
        original_workdir : String,
      ) : Nil
        validate_workdirs(workdir, original_workdir)
        database_error("Cannot change the working directory") do
          @database.exec(
            <<-SQL,
              UPDATE chats
                 SET workdir = ?,
                     original_workdir = COALESCE(original_workdir, ?),
                     updated_at = ?
               WHERE id = ?
              SQL
            workdir,
            original_workdir,
            now_seconds,
            chat_id
          )
        end
      end

      def restore_workdir(chat_id : String, workdir : String) : Nil
        raise ArgumentError.new("workdir cannot be empty") if workdir.empty?

        database_error("Cannot restore the working directory") do
          @database.exec(
            <<-SQL,
              UPDATE chats
                 SET workdir = ?, original_workdir = NULL, updated_at = ?
               WHERE id = ?
              SQL
            workdir,
            now_seconds,
            chat_id
          )
        end
      end

      def set_new_worktree(chat_id : String, enabled : Bool) : Nil
        database_error("Cannot change the workspace") do
          result = @database.exec(
            <<-SQL,
              UPDATE chats
                 SET new_worktree = ?, updated_at = ?
               WHERE id = ?
                 AND NOT EXISTS (
                   SELECT 1 FROM messages WHERE chat_id = ?
                 )
              SQL
            enabled,
            now_seconds,
            chat_id,
            chat_id
          )
          workspace_changed!(result.rows_affected)
        end
      end

      def use_existing_worktree(
        chat_id : String,
        workdir : String,
        original_workdir : String,
      ) : Nil
        validate_workdirs(workdir, original_workdir)
        database_error("Cannot change the workspace") do
          result = @database.exec(
            <<-SQL,
              UPDATE chats
                 SET workdir = ?,
                     original_workdir = COALESCE(original_workdir, ?),
                     new_worktree = 0,
                     updated_at = ?
               WHERE id = ?
                 AND NOT EXISTS (
                   SELECT 1 FROM messages WHERE chat_id = ?
                 )
              SQL
            workdir,
            original_workdir,
            now_seconds,
            chat_id,
            chat_id
          )
          workspace_changed!(result.rows_affected)
        end
      end

      def worktree_referenced_by_other_chat?(
        chat_id : String,
        workdir : String,
      ) : Bool
        raise ArgumentError.new("workdir cannot be empty") if workdir.empty?

        database_error("Cannot check worktree references") do
          references = @database.query_one(
            <<-SQL,
              SELECT COUNT(*)
                FROM chats
               WHERE id != ?
                 AND (workdir = ? OR original_workdir = ?)
              SQL
            chat_id,
            workdir,
            workdir,
            as: Int64
          )
          references > 0
        end
      end

      def restore_selected_worktree(
        chat_id : String,
        expected_workdir : String,
        expected_original_workdir : String,
      ) : Nil
        validate_workdirs(expected_workdir, expected_original_workdir)
        database_error("Cannot restore the original workspace") do
          result = @database.exec(
            <<-SQL,
              UPDATE chats
                 SET workdir = ?,
                     original_workdir = NULL,
                     new_worktree = 0,
                     updated_at = ?
               WHERE id = ?
                 AND workdir = ?
                 AND original_workdir = ?
                 AND NOT EXISTS (
                   SELECT 1 FROM messages WHERE chat_id = ?
                 )
              SQL
            expected_original_workdir,
            now_seconds,
            chat_id,
            expected_workdir,
            expected_original_workdir,
            chat_id
          )
          if result.rows_affected != 1
            raise ConflictError.new(
              "The workspace changed before the worktree was removed."
            )
          end
        end
      end

      def use_worktree(
        chat_id : String,
        workdir : String,
        original_workdir : String,
      ) : Nil
        validate_workdirs(workdir, original_workdir)
        database_error("Cannot use the new worktree") do
          result = @database.exec(
            <<-SQL,
              UPDATE chats
                 SET workdir = ?,
                     original_workdir = COALESCE(original_workdir, ?),
                     new_worktree = 0,
                     updated_at = ?
               WHERE id = ?
                 AND new_worktree = 1
                 AND NOT EXISTS (
                   SELECT 1 FROM messages WHERE chat_id = ?
                 )
              SQL
            workdir,
            original_workdir,
            now_seconds,
            chat_id,
            chat_id
          )
          if result.rows_affected != 1
            raise ConflictError.new(
              "The workspace changed before the worktree was ready."
            )
          end
        end
      end

      def set_queue(chat_id : String, messages : Array(String)) : Nil
        replacement = messages.dup
        mutate_queue(chat_id) do |current|
          current.replace(replacement)
          true
        end
      end

      def queue_append(chat_id : String, text : String) : Nil
        raise ArgumentError.new("queued text cannot be empty") if text.empty?

        mutate_queue(chat_id) do |messages|
          messages << text
          true
        end
      end

      def queue_remove(chat_id : String, position : Int) : Nil
        mutate_queue(chat_id) do |messages|
          if position >= 0 && position < messages.size
            messages.delete_at(position)
            true
          else
            false
          end
        end
      end

      def queue_replace(
        chat_id : String,
        position : Int,
        old_text : String,
        new_text : String,
      ) : Nil
        raise ArgumentError.new("queued text cannot be empty") if new_text.empty?

        mutate_queue(chat_id) do |messages|
          if position < 0 ||
             position >= messages.size ||
             messages[position] != old_text
            raise ConflictError.new("That queued message changed; try again.")
          end

          messages[position] = new_text
          true
        end
      end

      def queue_promote(chat_id : String, position : Int) : Nil
        mutate_queue(chat_id) do |messages|
          if position < 0 || position >= messages.size
            raise ConflictError.new(
              "That queued message no longer exists."
            )
          end
          next false if position == 0

          selected = messages.delete_at(position)
          messages.unshift(selected)
          true
        end
      end

      def queue_take_first(chat_id : String) : String?
        first : String? = nil
        mutate_queue(chat_id) do |messages|
          if messages.empty?
            false
          else
            first = messages.shift
            true
          end
        end
        first
      end

      private def mutate_queue(
        chat_id : String,
        & : Array(String) -> Bool
      ) : Nil
        @queue_mutex.synchronize do
          database_error("Cannot update the queue") do
            @database.transaction do |transaction|
              connection = transaction.connection
              stored = connection.query_one?(
                "SELECT COALESCE(queued, '') FROM chats WHERE id = ?",
                chat_id,
                as: String
              )
              unless stored
                raise NotFoundError.new("No chat #{chat_id}")
              end

              messages = Storage.queue_from_column(stored)
              if yield messages
                result = connection.exec(
                  <<-SQL,
                    UPDATE chats
                       SET queued = ?, updated_at = ?
                     WHERE id = ?
                    SQL
                  Storage.queue_to_column(messages),
                  now_seconds,
                  chat_id
                )
                unless result.rows_affected == 1
                  raise NotFoundError.new("No chat #{chat_id}")
                end
              end
            end
          end
        end
      end

      def mark_resumes(chat_ids : Array(String)) : Nil
        database_error("Cannot mark interrupted chats") do
          @database.transaction do |transaction|
            chat_ids.each do |chat_id|
              result = transaction.connection.exec(
                <<-SQL,
                  UPDATE chats
                     SET resume_after_restart = 1
                   WHERE id = ?
                  SQL
                chat_id
              )
              if result.rows_affected != 1
                raise NotFoundError.new("No chat #{chat_id}")
              end
            end
          end
        end
      end

      def take_resumes : Array(String)
        database_error("Cannot read interrupted chats") do
          @database.transaction do |transaction|
            connection = transaction.connection
            chat_ids = connection.query_all(
              <<-SQL,
                SELECT id
                  FROM chats
                 WHERE resume_after_restart = 1
                 ORDER BY updated_at
                SQL
              as: String
            )
            connection.exec(
              <<-SQL
                UPDATE chats
                   SET resume_after_restart = 0
                 WHERE resume_after_restart = 1
                SQL
            )
            chat_ids
          end.not_nil!
        end
      end

      private def validate_workdirs(
        workdir : String,
        original_workdir : String,
      ) : Nil
        raise ArgumentError.new("workdir cannot be empty") if workdir.empty?
        if original_workdir.empty?
          raise ArgumentError.new("original workdir cannot be empty")
        end
      end

      private def workspace_changed!(rows_affected : Int64) : Nil
        if rows_affected != 1
          raise ConflictError.new(
            "The workspace can only be changed before the first message."
          )
        end
      end
    end
  end
end
