require "../storage/worktree_containers"
require "./settings"

module Xd
  module Workspace
    LEGACY_WORKTREE_CONTAINER_MARKER = ".xd-worktrees"

    # Generated worktree containers live in SQLite. Marker files from older
    # versions are accepted only as migration input and removed after the
    # database registration succeeds.
    class WorktreeContainers
      def initialize(@store : Storage::Store)
      end

      def register(path : String) : Nil
        @store.register_worktree_container(path)
        remove_legacy_metadata(path)
      rescue error : Storage::Error | File::Error
        raise Error.new(
          "Cannot register the generated worktree container: #{error.message}"
        )
      end

      def registered?(path : String) : Bool
        marker = legacy_marker(path)
        if File.file?(marker)
          @store.register_worktree_container(path)
          remove_legacy_metadata(path)
          return true
        end
        registered = @store.worktree_container?(path)
        remove_legacy_metadata(path) if registered
        registered
      rescue error : Storage::Error | File::Error
        raise Error.new(
          "Cannot read the generated worktree container: #{error.message}"
        )
      end

      private def remove_legacy_metadata(path : String) : Nil
        File.delete?(legacy_marker(path))
        File.delete?(File.join(path, SETTINGS_FILE))
        File.delete?(File.join(path, LEGACY_SETTINGS_FILE))
      end

      private def legacy_marker(path : String) : String
        File.join(path, LEGACY_WORKTREE_CONTAINER_MARKER)
      end
    end
  end
end
