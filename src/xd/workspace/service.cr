require "file_utils"
require "digest/sha256"
require "set"
require "uuid"
require "../storage/workflow_state"
require "../storage/workspaces"
require "./settings"

module Xd
  module Workspace
    record FolderSummary,
      id : String,
      name : String,
      parent : String?

    record ChatSummary,
      id : String,
      folder : String,
      title : String?,
      backend : String,
      working : Bool

    record TreeSnapshot,
      folders : Array(FolderSummary),
      chats : Array(ChatSummary)

    class Service
      MAX_SHORTCUTS      =   24
      MAX_SHORTCUT_BYTES = 4096

      getter root : String

      def initialize(root : String, @store : Storage::Store)
        @root = File.expand_path(root)
        Dir.mkdir_p(@root, 0o700)
        reconcile_metadata
      end

      def snapshot : TreeSnapshot
        rows = reconcile_metadata
        folders = [] of FolderSummary
        chats = [] of ChatSummary

        directory_children(@root).each do |path|
          scan_folder(path, rows, nil, folders, chats)
        end

        TreeSnapshot.new(folders, chats)
      end

      # Filesystem topology is reconciled into SQLite before calculating the
      # signature. Legacy sidecars are read only during that reconciliation;
      # database settings are authoritative afterwards.
      def tree_signature : String
        rows = reconcile_metadata
        digest = Digest::SHA256.new
        append_tree_signature(digest, @root, rows, root: true)
        digest.final.hexstring
      end

      def find_folder(folder_id : String) : String
        row = workspace_for_id(folder_id)
        path = path_for(row.relative_path)
        return path if visible_folder_path?(path)

        raise Error.new("No such folder on the daemon.")
      end

      def folder_ids(folder_id : String) : Array(String)
        folder = find_folder(folder_id)
        ancestor_paths(folder).compact_map do |path|
          @store.workspace_folder(@root, relative_path(path)).try(&.id)
        end
      end

      def create_folder(
        parent_id : String?,
        name : String,
        repo : String? = nil,
      ) : String
        validate_name!(name)
        repository = directory_setting(repo, "repository")
        reconcile_metadata
        parent = parent_id ? find_folder(parent_id) : @root
        path = File.join(parent, name)
        relative = relative_path(path)
        created = false
        if info = File.info?(path, follow_symlinks: false)
          unless info.type.directory?
            raise Error.new("There is already something of that name there.")
          end
          if row = @store.workspace_folder(@root, relative)
            return row.id
          end
        else
          Dir.mkdir(path, 0o700)
          created = true
        end

        id = UUID.random.to_s
        begin
          @store.save_workspace_folder(
            Storage::WorkspaceFolder.new(
              id,
              @root,
              relative,
              nil,
              nil,
              nil,
              repository,
              nil,
              "[]"
            )
          )
          id
        rescue error
          Dir.delete(path) if created && Dir.empty?(path)
          raise error
        end
      rescue error : File::Error
        raise Error.new("Cannot create folder: #{error.message}")
      end

      def rename_folder(folder_id : String, name : String) : Nil
        validate_name!(name)
        row = workspace_for_id(folder_id)
        path = path_for(row.relative_path)
        destination = File.join(Path[path].parent, name)
        if File.exists?(destination)
          raise Error.new("There is already a folder of that name there.")
        end

        File.rename(path, destination)
        begin
          @store.relocate_workspace_subtree(
            @root,
            row.relative_path,
            relative_path(destination)
          )
        rescue error
          File.rename(destination, path) rescue nil
          raise error
        end
      rescue error : File::Error
        raise Error.new("Cannot rename folder: #{error.message}")
      end

      def move_folder(folder_id : String, parent_id : String?) : Nil
        row = workspace_for_id(folder_id)
        path = path_for(row.relative_path)
        parent = parent_id ? find_folder(parent_id) : @root
        path_prefix = path + File::SEPARATOR

        return if File.dirname(path) == parent

        if parent == path || parent.starts_with?(path_prefix)
          raise Error.new("A folder cannot be moved inside itself.")
        end

        if File.exists?(File.join(parent, ".git"))
          raise Error.new("A folder cannot be moved inside a repository.")
        end

        destination = File.join(parent, File.basename(path))
        if File.exists?(destination)
          raise Error.new("There is already a folder of that name there.")
        end

        File.rename(path, destination)
        begin
          @store.relocate_workspace_subtree(
            @root,
            row.relative_path,
            relative_path(destination)
          )
        rescue error
          File.rename(destination, path) rescue nil
          raise error
        end
      rescue error : File::Error
        raise Error.new("Cannot move folder: #{error.message}")
      end

      # Cross-platform, recoverable app trash. Hidden from the workspace tree.
      def trash_folder(folder_id : String) : String
        row = workspace_for_id(folder_id)
        path = path_for(row.relative_path)
        trash = File.join(@root, ".Trash")
        Dir.mkdir_p(trash, 0o700)
        destination = File.join(
          trash,
          "#{Time.utc.to_unix}-#{UUID.random}-#{File.basename(path)}"
        )
        File.rename(path, destination)
        begin
          @store.relocate_workspace_subtree(
            @root,
            row.relative_path,
            relative_path(destination)
          )
        rescue error
          File.rename(destination, path) rescue nil
          raise error
        end
        destination
      rescue error : File::Error
        raise Error.new("Cannot trash folder: #{error.message}")
      end

      def folder_context(folder_id : String) : String?
        workspace_for_id(folder_id).instructions
      end

      def set_folder_context(
        folder_id : String,
        context : String?,
      ) : Nil
        row = workspace_for_id(folder_id)
        stripped = context.try(&.strip)
        @store.update_workspace_settings(
          row.id,
          row.backend,
          row.model,
          row.workdir,
          row.repo,
          stripped && !stripped.empty? ? stripped : nil
        )
      end

      def folder_settings(folder_id : String) : Settings
        row = workspace_for_id(folder_id)
        Settings.new(
          id: row.id,
          backend: row.backend,
          model: row.model,
          workdir: row.workdir,
          repo: row.repo,
          instructions: row.instructions,
          shortcuts: parse_shortcuts(row.shortcuts)
        )
      end

      def global_shortcuts : Array(String)
        @store.global_shortcuts
      end

      def set_global_shortcuts(shortcuts : Array(String)) : Array(String)
        cleaned = clean_shortcuts(shortcuts)
        @store.save_global_shortcuts(cleaned)
        cleaned
      end

      def workspace_shortcuts(folder_id : String) : Array(String)
        parse_shortcuts(workspace_for_id(folder_id).shortcuts)
      end

      def set_workspace_shortcuts(
        folder_id : String,
        shortcuts : Array(String),
      ) : Array(String)
        row = workspace_for_id(folder_id)
        cleaned = clean_shortcuts(shortcuts)
        @store.update_workspace_shortcuts(row.id, cleaned.to_json)
        cleaned
      end

      def resolve_shortcuts(folder_id : String) : Array(String)
        folder = find_folder(folder_id)
        resolved = [] of String
        seen = Set(String).new
        (@store.global_shortcuts + ancestor_paths(folder).flat_map do |path|
          settings = workspace_settings_at(path)
          settings.try(&.shortcuts) || [] of String
        end).each do |prompt|
          resolved << prompt if seen.add?(prompt)
        end
        resolved
      end

      # Values a folder receives when its own scalar settings are blank.
      #
      # This mirrors the C settings dialog: resolve from the parent upward,
      # preserving the nearest named ancestor for inherited subtitles.
      def inherited_settings(
        folder_id : String,
        default_backend : String = "claude",
      ) : SettingsInheritance
        folder = find_folder(folder_id)
        parent = File.dirname(folder)
        paths = within_root?(parent) ? ancestor_paths(parent) : [] of String
        backend = nil
        model = nil
        workdir = nil
        repo = nil
        backend_from = nil
        model_from = nil
        workdir_from = nil
        repo_from = nil

        paths.each do |path|
          settings = workspace_settings_at(path)
          next unless settings

          source = path == parent ? nil : File.basename(path)
          if value = settings.backend
            backend = value
            backend_from = source
          end
          if value = settings.model
            model = value
            model_from = source
          end
          if value = settings.workdir
            workdir = value
            workdir_from = source
          end
          if value = settings.repo
            repo = value
            repo_from = source
          end
        end

        workdir ||= repo
        workdir ||= parent if within_root?(parent)
        SettingsInheritance.new(
          backend || default_backend,
          model,
          workdir,
          repo,
          backend_from,
          model_from,
          workdir_from,
          repo_from
        )
      end

      def set_folder_settings(
        folder_id : String,
        backend : String?,
        model : String?,
        workdir : String?,
        repo : String?,
      ) : Nil
        row = workspace_for_id(folder_id)
        @store.update_workspace_settings(
          row.id,
          clean(backend),
          clean(model),
          directory_setting(workdir, "working directory"),
          directory_setting(repo, "repository"),
          row.instructions
        )
      end

      def resolve(
        folder_id : String,
        default_backend : String = "claude",
      ) : EffectiveSettings
        folder = find_folder(folder_id)
        paths = ancestor_paths(folder)
        backend = nil
        model = nil
        workdir = nil
        repo = nil
        instructions = [] of String

        paths.each do |path|
          settings = workspace_settings_at(path)
          next unless settings

          backend = settings.backend if settings.backend
          model = settings.model if settings.model
          workdir = settings.workdir if settings.workdir
          repo = settings.repo if settings.repo
          if text = settings.instructions
            instructions << text unless text.empty?
          end
        end

        EffectiveSettings.new(
          backend || default_backend,
          model,
          workdir || repo || folder,
          repo,
          instructions.empty? ? nil : instructions.join("\n\n")
        )
      end

      def resolve_workdir(folder_id : String, chat_workdir : String?) : String
        chat_workdir || resolve(folder_id).workdir
      end

      def describe_place(folder_id : String, workdir : String?) : String
        folder = find_folder(folder_id)
        chain = ancestor_paths(folder)
          .reject(&.==(@root))
          .map { |path| File.basename(path) }
          .join(" / ")

        unless workdir && !workdir.empty?
          return "[This conversation belongs to the folder “#{chain}” " \
                 "in the user’s xd workspace tree.]"
        end

        "[This conversation belongs to the folder “#{chain}” in the user’s " \
        "xd workspace tree, and you are running in #{workdir}. If that " \
        "directory holds nothing but a dotfile, it is the folder itself " \
        "rather than a checkout: say so and ask which repository is meant, " \
        "instead of searching the machine for one.]"
      end

      private def scan_folder(
        path : String,
        rows : Hash(String, Storage::WorkspaceFolder),
        parent_id : String?,
        folders : Array(FolderSummary),
        chats : Array(ChatSummary),
      ) : Nil
        row = rows[relative_path(path)]? || return
        id = row.id
        folders << FolderSummary.new(id, File.basename(path), parent_id)
        @store.list_chats(id).each do |chat|
          chats << ChatSummary.new(
            chat.id,
            id,
            chat.title,
            chat.backend,
            chat.daemon_working
          )
        end

        return if File.exists?(File.join(path, ".git"))

        directory_children(path).each do |child|
          next unless rows.has_key?(relative_path(child))
          scan_folder(child, rows, id, folders, chats)
        end
      end

      # Existing database rows are authoritative at their stored paths. Legacy
      # ids can reconnect a folder moved outside xd; a DB-only folder has no
      # portable filesystem identity, so arbitrary external renames cannot be
      # correlated safely without reintroducing an on-disk marker.
      private def reconcile_metadata : Hash(String, Storage::WorkspaceFolder)
        existing = @store.list_workspace_folders(@root)
        by_path = existing.to_h { |row| {row.relative_path, row} }
        by_id = existing.to_h { |row| {row.id, row} }
        used = Set(String).new
        rows = {} of String => Storage::WorkspaceFolder

        if by_path.has_key?("") || SettingsFile.managed?(@root)
          reconcile_metadata_path(
            @root,
            by_path,
            by_id,
            used,
            rows
          )
        end
        directory_children(@root).each do |path|
          reconcile_metadata_path(path, by_path, by_id, used, rows)
        end
        rows
      end

      private def reconcile_metadata_path(
        path : String,
        by_path : Hash(String, Storage::WorkspaceFolder),
        by_id : Hash(String, Storage::WorkspaceFolder),
        used : Set(String),
        rows : Hash(String, Storage::WorkspaceFolder),
      ) : Nil
        relative = relative_path(path)
        current = by_path[relative]?
        legacy = SettingsFile.managed?(path) ? SettingsFile.load?(path) : nil
        row = current

        if row.nil? && (legacy_id = legacy.try(&.id))
          candidate = by_id[legacy_id]? || @store.workspace_folder_with_id(legacy_id)
          if candidate &&
             !used.includes?(candidate.id) &&
             (candidate.root_path == @root ||
             !stored_workspace_root_exists?(candidate))
            row = Storage::WorkspaceFolder.new(
              candidate.id,
              @root,
              relative,
              candidate.backend,
              candidate.model,
              candidate.workdir,
              candidate.repo,
              candidate.instructions,
              candidate.shortcuts
            )
          end
        end

        if row.nil? || used.includes?(row.id)
          id = legacy.try(&.id)
          id = nil if id && used.includes?(id)
          if id && (existing = @store.workspace_folder_with_id(id)) &&
             existing.root_path != @root
            id = nil
          end
          row = Storage::WorkspaceFolder.new(
            id || UUID.random.to_s,
            @root,
            relative,
            legacy.try(&.backend),
            legacy.try(&.model),
            legacy.try(&.workdir),
            legacy.try(&.repo),
            legacy.try(&.instructions),
            imported_shortcuts(legacy)
          )
        end

        @store.save_workspace_folder(row) if current != row
        by_path[relative] = row
        by_id[row.id] = row
        used << row.id
        rows[relative] = row

        return if File.exists?(File.join(path, ".git"))

        directory_children(path).each do |child|
          child_relative = relative_path(child)
          next unless by_path.has_key?(child_relative) ||
                      SettingsFile.managed?(child)
          reconcile_metadata_path(child, by_path, by_id, used, rows)
        end
      end

      private def workspace_for_id(folder_id : String) : Storage::WorkspaceFolder
        reconcile_metadata
        row = @store.workspace_folder_by_id(@root, folder_id) ||
              raise Error.new("No such folder on the daemon.")
        path = path_for(row.relative_path)
        unless visible_folder_path?(path)
          raise Error.new("No such folder on the daemon.")
        end
        row
      end

      private def stored_workspace_root_exists?(
        folder : Storage::WorkspaceFolder,
      ) : Bool
        info = File.info?(folder.root_path, follow_symlinks: false)
        info ? info.type.directory? : false
      end

      private def workspace_settings_at(path : String) : Settings?
        @store.workspace_folder(@root, relative_path(path)).try do |row|
          Settings.new(
            id: row.id,
            backend: row.backend,
            model: row.model,
            workdir: row.workdir,
            repo: row.repo,
            instructions: row.instructions,
            shortcuts: parse_shortcuts(row.shortcuts)
          )
        end
      end

      private def path_for(relative : String) : String
        relative.empty? ? @root : File.join(@root, relative)
      end

      private def relative_path(path : String) : String
        return "" if path == @root

        prefix = @root + File::SEPARATOR
        path.starts_with?(prefix) ? path.byte_slice(prefix.bytesize, path.bytesize - prefix.bytesize) : path
      end

      private def visible_folder_path?(path : String) : Bool
        return false unless within_root?(path)
        return false unless path != @root

        info = File.info?(path, follow_symlinks: false)
        return false unless info && info.type.directory?
        return false if File.file?(File.join(path, WORKTREE_CONTAINER_MARKER))

        relative = relative_path(path)
        return false if relative.split(File::SEPARATOR).any? do |part|
                          part.starts_with?('.')
                        end

        ancestor = File.dirname(path)
        while ancestor != @root
          info = File.info?(ancestor, follow_symlinks: false)
          return false unless info && info.type.directory?
          return false if File.exists?(File.join(ancestor, ".git"))
          return false if File.file?(
                            File.join(ancestor, WORKTREE_CONTAINER_MARKER)
                          )
          ancestor = File.dirname(ancestor)
        end
        true
      end

      private def directory_children(path : String) : Array(String)
        Dir.children(path)
          .reject { |name| name.starts_with?('.') }
          .sort
          .compact_map do |name|
            child = File.join(path, name)
            info = File.info?(child, follow_symlinks: false)
            marker = File.join(child, WORKTREE_CONTAINER_MARKER)
            child if info && info.type.directory? && !File.file?(marker)
          end
      rescue error : File::Error
        raise Error.new("Cannot scan #{path}: #{error.message}")
      end

      private def append_tree_signature(
        digest : Digest::SHA256,
        path : String,
        rows : Hash(String, Storage::WorkspaceFolder),
        root : Bool,
      ) : Nil
        directory_children(path).each do |child|
          row = rows[relative_path(child)]?
          next if row.nil? && !root

          digest.update(relative_path(child))
          digest.update(Bytes[0_u8])
          if row
            digest.update(row.id)
            digest.update(Bytes[0_u8])
            digest.update(row.backend || "")
            digest.update(Bytes[0_u8])
            digest.update(row.model || "")
            digest.update(Bytes[0_u8])
            digest.update(row.workdir || "")
            digest.update(Bytes[0_u8])
            digest.update(row.repo || "")
            digest.update(Bytes[0_u8])
            digest.update(row.instructions || "")
            digest.update(Bytes[0_u8])
            digest.update(row.shortcuts)
          end
          digest.update(Bytes[0_u8])

          if File.exists?(File.join(child, ".git"))
            digest.update("repository")
          else
            append_tree_signature(digest, child, rows, root: false)
          end
        end
      rescue error : File::Error
        raise Error.new("Cannot scan #{path}: #{error.message}")
      end

      private def ancestor_paths(folder : String) : Array(String)
        paths = [] of String
        current = folder

        loop do
          paths.unshift(current)
          break if current == @root

          parent = File.dirname(current)
          break if parent == current || !within_root?(parent)
          current = parent
        end
        paths
      end

      private def clean_shortcuts(shortcuts : Array(String)) : Array(String)
        if shortcuts.size > MAX_SHORTCUTS
          raise Error.new("A shortcut list can contain at most #{MAX_SHORTCUTS} prompts.")
        end

        cleaned = [] of String
        shortcuts.each do |prompt|
          value = prompt.strip
          next if value.empty? || cleaned.includes?(value)
          if value.bytesize > MAX_SHORTCUT_BYTES
            raise Error.new(
              "A shortcut prompt can contain at most #{MAX_SHORTCUT_BYTES} bytes."
            )
          end
          cleaned << value
        end
        cleaned
      end

      private def parse_shortcuts(value : String) : Array(String)
        clean_shortcuts(Array(String).from_json(value))
      rescue error : JSON::ParseException | JSON::SerializableError
        raise Error.new("Cannot read workspace shortcuts: #{error.message}")
      end

      private def imported_shortcuts(settings : Settings?) : String
        clean_shortcuts(settings.try(&.shortcuts) || [] of String).to_json
      end

      private def within_root?(path : String) : Bool
        path == @root || path.starts_with?(@root + File::SEPARATOR)
      end

      private def clean(value : String?) : String?
        stripped = value.try(&.strip)
        stripped if stripped && !stripped.empty?
      end

      private def directory_setting(
        value : String?,
        label : String,
      ) : String?
        path = clean(value)
        return nil unless path

        expanded = File.expand_path(path)
        unless File.directory?(expanded)
          raise Error.new("The #{label} must be an existing directory.")
        end
        expanded
      end

      private def validate_name!(name : String) : Nil
        if name.empty? ||
           name.starts_with?('.') ||
           name.includes?('/') ||
           name.includes?('\\')
          raise Error.new(
            "A folder name cannot be empty or hidden, or contain a path separator."
          )
        end
      end
    end
  end
end
