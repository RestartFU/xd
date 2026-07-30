require "file_utils"
require "../git_path"
require "./tool_diff"

module Xd
  module Agent
    # Captures worktree changes between file-edit tool events without touching
    # the user's index. Each snapshot is a Git tree written through a private
    # temporary index beside the real one.
    class GitDiffTracker
      FILE_CHANGE_PREFIX = ToolDiff::PREFIX
      DIFF_LIMIT         = ToolDiff::LIMIT

      getter root : String

      @previous_tree : String

      private def initialize(
        @root : String,
        @pathspec : String,
        @previous_tree : String,
      )
      end

      def self.open(workdir : String) : GitDiffTracker?
        root = repository_root(workdir)
        return unless root

        current = File.realpath(workdir)
        prefix = root + File::SEPARATOR
        return unless current == root || current.starts_with?(prefix)
        pathspec = current == root ? "." : Path[current].relative_to(Path[root]).to_s
        previous = snapshot_tree(root, pathspec)
        previous ? new(root, pathspec, previous) : nil
      rescue File::Error
        nil
      end

      def capture(message : String) : String
        return message unless self.class.file_change?(message)
        return message if self.class.patch(message)

        current = self.class.snapshot_tree(@root, @pathspec)
        return message unless current

        patch = self.class.git(
          @root,
          [
            "--no-pager", "diff", "--no-ext-diff", "--no-color",
            @previous_tree, current, "--", @pathspec,
          ]
        )
        @previous_tree = current
        return message unless patch
        patch = patch.rstrip
        return message if patch.empty?

        if patch.bytesize > DIFF_LIMIT
          patch = patch.byte_slice(0, DIFF_LIMIT)
          until patch.valid_encoding?
            patch = patch.byte_slice(0, patch.bytesize - 1)
          end
          patch += "\n… diff truncated …"
        end
        FILE_CHANGE_PREFIX + patch
      end

      def self.file_change?(message : String?) : Bool
        return false unless message
        message == "file_change" ||
          message.starts_with?("file_change  ") ||
          message.starts_with?(FILE_CHANGE_PREFIX)
      end

      def self.patch(message : String?) : String?
        return unless message
        return unless message.starts_with?(FILE_CHANGE_PREFIX)

        patch = message.byte_slice(FILE_CHANGE_PREFIX.bytesize)
        patch if patch.starts_with?("diff --git ")
      end

      private def self.repository_root(workdir : String) : String?
        output = git(workdir, ["rev-parse", "--show-toplevel"])
        return unless output
        path = GitPath.native(output.strip)
        return if path.empty?
        File.realpath(path)
      rescue File::Error
        nil
      end

      protected def self.snapshot_tree(
        root : String,
        pathspec : String,
      ) : String?
        index_output = git(root, ["rev-parse", "--git-path", "index"])
        return unless index_output
        reported = GitPath.native(index_output.strip)
        return if reported.empty?
        user_index = Path[reported].absolute? ? reported : File.expand_path(reported, root)
        index_dir = File.dirname(user_index)
        temporary = File.tempfile("xd-diff-index", dir: index_dir)
        index_path = temporary.path
        temporary.close
        seeded = false

        begin
          if File.file?(user_index)
            FileUtils.cp(user_index, index_path)
            seeded = true
          else
            File.delete(index_path)
            git(root, ["read-tree", "HEAD"], index_path: index_path)
          end

          added = git(
            root,
            ["add", "-A", "--", pathspec],
            index_path: index_path
          )
          if !added && seeded
            File.delete?(index_path)
            git(root, ["read-tree", "HEAD"], index_path: index_path)
            added = git(
              root,
              ["add", "-A", "--", pathspec],
              index_path: index_path
            )
          end
          return unless added

          tree = git(root, ["write-tree"], index_path: index_path)
          value = tree.try(&.strip)
          value unless value.nil? || value.empty?
        ensure
          File.delete?(index_path)
        end
      rescue File::Error
        nil
      end

      protected def self.git(
        workdir : String,
        arguments : Array(String),
        index_path : String? = nil,
      ) : String?
        output = IO::Memory.new
        error = IO::Memory.new
        environment = {"GIT_LITERAL_PATHSPECS" => "1"}
        if index_path
          environment["GIT_INDEX_FILE"] = GitPath.environment(index_path)
        end
        status = Process.run(
          "git",
          arguments,
          env: environment,
          chdir: workdir,
          output: output,
          error: error
        )
        return unless status.success?
        text = output.to_s
        text if text.valid_encoding?
      rescue File::Error | IO::Error
        nil
      end
    end
  end
end
