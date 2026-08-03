require "./store"

module Xd
  module Storage
    record WorkspaceFolder,
      id : String,
      root_path : String,
      relative_path : String,
      backend : String?,
      model : String?,
      workdir : String?,
      repo : String?,
      instructions : String?

    class Store
      WORKSPACE_FOLDER_COLUMNS =
        "id, root_path, relative_path, backend, model, workdir, repo, instructions"

      def list_workspace_folders(root_path : String) : Array(WorkspaceFolder)
        database_error("Cannot list workspace folders") do
          @database.query_all(
            <<-SQL,
              SELECT #{WORKSPACE_FOLDER_COLUMNS}
                FROM workspace_folders
               WHERE root_path = ?
               ORDER BY LENGTH(relative_path), relative_path
              SQL
            root_path
          ) { |row| workspace_folder_from_row(row) }
        end
      end

      def workspace_folder(
        root_path : String,
        relative_path : String,
      ) : WorkspaceFolder?
        database_error("Cannot read workspace folder") do
          @database.query_one?(
            <<-SQL,
              SELECT #{WORKSPACE_FOLDER_COLUMNS}
                FROM workspace_folders
               WHERE root_path = ? AND relative_path = ?
              SQL
            root_path,
            relative_path
          ) { |row| workspace_folder_from_row(row) }
        end
      end

      def workspace_folder_by_id(
        root_path : String,
        id : String,
      ) : WorkspaceFolder?
        database_error("Cannot read workspace folder") do
          @database.query_one?(
            <<-SQL,
              SELECT #{WORKSPACE_FOLDER_COLUMNS}
                FROM workspace_folders
               WHERE root_path = ? AND id = ?
              SQL
            root_path,
            id
          ) { |row| workspace_folder_from_row(row) }
        end
      end

      def workspace_folder_with_id(id : String) : WorkspaceFolder?
        database_error("Cannot read workspace folder") do
          @database.query_one?(
            <<-SQL,
              SELECT #{WORKSPACE_FOLDER_COLUMNS}
                FROM workspace_folders
               WHERE id = ?
              SQL
            id
          ) { |row| workspace_folder_from_row(row) }
        end
      end

      def save_workspace_folder(folder : WorkspaceFolder) : Nil
        now = now_seconds
        database_error("Cannot save workspace folder") do
          @database.exec(
            <<-SQL,
              INSERT INTO workspace_folders (
                id, root_path, relative_path, backend, model, workdir, repo,
                instructions, created_at, updated_at
              )
              VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
              ON CONFLICT (id) DO UPDATE SET
                root_path = excluded.root_path,
                relative_path = excluded.relative_path,
                backend = excluded.backend,
                model = excluded.model,
                workdir = excluded.workdir,
                repo = excluded.repo,
                instructions = excluded.instructions,
                updated_at = excluded.updated_at
              SQL
            folder.id,
            folder.root_path,
            folder.relative_path,
            folder.backend,
            folder.model,
            folder.workdir,
            folder.repo,
            folder.instructions,
            now,
            now
          )
        end
      end

      def update_workspace_settings(
        id : String,
        backend : String?,
        model : String?,
        workdir : String?,
        repo : String?,
        instructions : String?,
      ) : Nil
        database_error("Cannot update workspace settings") do
          result = @database.exec(
            <<-SQL,
              UPDATE workspace_folders
                 SET backend = ?,
                     model = ?,
                     workdir = ?,
                     repo = ?,
                     instructions = ?,
                     updated_at = ?
               WHERE id = ?
              SQL
            backend,
            model,
            workdir,
            repo,
            instructions,
            now_seconds,
            id
          )
          raise NotFoundError.new("No such workspace folder.") if result.rows_affected != 1
        end
      end

      def relocate_workspace_subtree(
        root_path : String,
        old_relative_path : String,
        new_relative_path : String,
      ) : Nil
        return if old_relative_path == new_relative_path

        database_error("Cannot relocate workspace folders") do
          @database.transaction do |transaction|
            connection = transaction.connection
            rows = connection.query_all(
              <<-SQL,
                SELECT id, relative_path
                  FROM workspace_folders
                 WHERE root_path = ?
                SQL
              root_path,
              as: {String, String}
            ).select do |_id, relative_path|
              relative_path == old_relative_path ||
                relative_path.starts_with?(
                  old_relative_path + File::SEPARATOR
                )
            end

            rows.each do |id, relative_path|
              suffix = relative_path.byte_slice(
                old_relative_path.bytesize,
                relative_path.bytesize - old_relative_path.bytesize
              )
              replacement = if suffix && !suffix.empty?
                              new_relative_path + suffix
                            else
                              new_relative_path
                            end
              connection.exec(
                <<-SQL,
                  UPDATE workspace_folders
                     SET relative_path = ?, updated_at = ?
                   WHERE id = ? AND root_path = ?
                  SQL
                replacement,
                now_seconds,
                id,
                root_path
              )
            end
          end
        end
      end

      private def workspace_folder_from_row(row : DB::ResultSet) : WorkspaceFolder
        WorkspaceFolder.new(
          row.read(String),
          row.read(String),
          row.read(String),
          row.read(String?),
          row.read(String?),
          row.read(String?),
          row.read(String?),
          row.read(String?)
        )
      end
    end
  end
end
