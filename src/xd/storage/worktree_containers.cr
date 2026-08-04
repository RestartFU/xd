require "./store"

module Xd
  module Storage
    class Store
      def register_worktree_container(path : String) : String
        normalized = normalize_worktree_container(path)
        now = now_seconds
        database_error("Cannot register the worktree container") do
          @database.exec(
            <<-SQL,
              INSERT INTO worktree_containers (
                path, created_at, updated_at
              )
              VALUES (?, ?, ?)
              ON CONFLICT (path) DO UPDATE SET
                updated_at = excluded.updated_at
              SQL
            normalized,
            now,
            now
          )
        end
        normalized
      end

      def worktree_container?(path : String) : Bool
        normalized = normalize_worktree_container(path)
        database_error("Cannot read the worktree container") do
          @database.query_one?(
            "SELECT 1 FROM worktree_containers WHERE path = ?",
            normalized,
            as: Int32
          ) != nil
        end
      end

      def forget_worktree_container(path : String) : Nil
        normalized = normalize_worktree_container(path)
        database_error("Cannot forget the worktree container") do
          @database.exec(
            "DELETE FROM worktree_containers WHERE path = ?",
            normalized
          )
        end
      end

      private def normalize_worktree_container(path : String) : String
        File.realpath(path)
      rescue File::Error
        File.expand_path(path)
      end
    end
  end
end
