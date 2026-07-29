require "file_utils"
require "../storage/workflow_state"
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
      getter root : String

      def initialize(root : String, @store : Storage::Store)
        @root = File.expand_path(root)
        Dir.mkdir_p(@root, 0o700)
      end

      def snapshot : TreeSnapshot
        folders = [] of FolderSummary
        chats = [] of ChatSummary

        directory_children(@root).each do |path|
          settings = SettingsFile.ensure(path)
          scan_folder(path, settings, nil, folders, chats)
        end

        TreeSnapshot.new(folders, chats)
      end

      def find_folder(folder_id : String) : String
        found = find_folder?(@root, folder_id, root: true)
        found || raise Error.new("No such folder on the daemon.")
      end

      def folder_ids(folder_id : String) : Array(String)
        folder = find_folder(folder_id)
        ancestor_paths(folder).compact_map do |path|
          SettingsFile.load?(path).try(&.id)
        end
      end

      def create_folder(parent_id : String?, name : String) : String
        validate_name!(name)
        parent = parent_id ? find_folder(parent_id) : @root
        path = File.join(parent, name)
        raise Error.new("There is already a folder of that name there.") if File.exists?(path)

        Dir.mkdir(path, 0o700)
        begin
          SettingsFile.ensure(path).id.not_nil!
        rescue error
          Dir.delete(path) if Dir.empty?(path)
          raise error
        end
      rescue error : File::Error
        raise Error.new("Cannot create folder: #{error.message}")
      end

      def rename_folder(folder_id : String, name : String) : Nil
        validate_name!(name)
        path = find_folder(folder_id)
        destination = File.join(Path[path].parent, name)
        if File.exists?(destination)
          raise Error.new("There is already a folder of that name there.")
        end

        File.rename(path, destination)
      rescue error : File::Error
        raise Error.new("Cannot rename folder: #{error.message}")
      end

      def move_folder(folder_id : String, parent_id : String?) : Nil
        path = find_folder(folder_id)
        parent = parent_id ? find_folder(parent_id) : @root
        path_prefix = path + File::SEPARATOR

        if parent == path || parent.starts_with?(path_prefix)
          raise Error.new("A folder cannot be moved inside itself.")
        end

        destination = File.join(parent, File.basename(path))
        if File.exists?(destination)
          raise Error.new("There is already a folder of that name there.")
        end

        File.rename(path, destination)
      rescue error : File::Error
        raise Error.new("Cannot move folder: #{error.message}")
      end

      # Cross-platform, recoverable app trash. Hidden from the workspace tree.
      def trash_folder(folder_id : String) : String
        path = find_folder(folder_id)
        trash = File.join(@root, ".Trash")
        Dir.mkdir_p(trash, 0o700)
        destination = File.join(
          trash,
          "#{Time.utc.to_unix}-#{UUID.random}-#{File.basename(path)}"
        )
        File.rename(path, destination)
        destination
      rescue error : File::Error
        raise Error.new("Cannot trash folder: #{error.message}")
      end

      def folder_context(folder_id : String) : String?
        SettingsFile.ensure(find_folder(folder_id)).instructions
      end

      def set_folder_context(
        folder_id : String,
        context : String?,
      ) : Nil
        path = find_folder(folder_id)
        settings = SettingsFile.ensure(path)
        stripped = context.try(&.strip)
        settings.instructions = if stripped && !stripped.empty?
                                  stripped
                                end
        SettingsFile.save(settings, path)
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
          settings = SettingsFile.load?(path)
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
          workdir || folder,
          repo,
          instructions.empty? ? nil : instructions.join("\n\n")
        )
      end

      def resolve_workdir(folder_id : String, chat_workdir : String?) : String
        chat_workdir || resolve(folder_id).workdir
      end

      private def scan_folder(
        path : String,
        settings : Settings,
        parent_id : String?,
        folders : Array(FolderSummary),
        chats : Array(ChatSummary),
      ) : Nil
        id = settings.id.not_nil!
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
          next unless SettingsFile.managed?(child)

          child_settings = SettingsFile.ensure(child)
          scan_folder(child, child_settings, id, folders, chats)
        end
      end

      private def find_folder?(
        path : String,
        folder_id : String,
        root : Bool = false,
      ) : String?
        unless root
          if settings = SettingsFile.load?(path)
            return path if settings.id == folder_id
          end
          return nil if File.exists?(File.join(path, ".git"))
        end

        directory_children(path).each do |child|
          next unless root || SettingsFile.managed?(child)
          if found = find_folder?(child, folder_id)
            return found
          end
        end
        nil
      end

      private def directory_children(path : String) : Array(String)
        Dir.children(path)
          .reject { |name| name.starts_with?('.') }
          .sort
          .compact_map do |name|
            child = File.join(path, name)
            info = File.info?(child, follow_symlinks: false)
            child if info && info.type.directory?
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

      private def within_root?(path : String) : Bool
        path == @root || path.starts_with?(@root + File::SEPARATOR)
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
