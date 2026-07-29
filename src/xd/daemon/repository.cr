require "../storage/workflow_state"
require "../workspace/service"
require "./filesystem"

module Xd
  module Daemon
    class Repository
      MAX_OUTPUT_BYTES = 8 * 1024 * 1024
      BASE_PATTERN     = /\A[A-Za-z0-9_\.\/-]{1,256}\z/

      class Error < Exception
      end

      def initialize(
        @store : Storage::Store,
        @workspaces : Workspace::Service,
        @filesystem : Filesystem,
      )
      end

      def read(
        chat_id : String,
        kind : String,
        path : String?,
        base : String?,
      ) : String
        workdir = @filesystem.workdir(chat_id)
        root = repository_root(workdir)

        output = case kind
                 when "base"
                   find_base(workdir)
                 when "working-status"
                   capture(
                     workdir,
                     ["status", "--porcelain", "--untracked-files=all"]
                   )
                 when "branch-status"
                   capture(
                     workdir,
                     ["--no-pager", "diff", "--name-status", range(base)]
                   )
                 when "working-all"
                   working_all(workdir, root)
                 when "branch-all"
                   capture(
                     workdir,
                     ["--no-pager", "diff", range(base)]
                   )
                 when "working-file"
                   selected = safe_path(workdir, root, path)
                   capture(
                     workdir,
                     ["--no-pager", "diff", "HEAD", "--", selected]
                   )
                 when "untracked-file"
                   selected = safe_path(workdir, root, path)
                   capture(
                     workdir,
                     [
                       "--no-pager", "diff", "--no-index", "--",
                       "/dev/null", selected,
                     ]
                   )
                 when "branch-file"
                   selected = safe_path(workdir, root, path)
                   capture(
                     workdir,
                     [
                       "--no-pager", "diff", range(base), "--", selected,
                     ]
                   )
                 else
                   raise Error.new("No such diff read type.")
                 end

        if output.bytesize > MAX_OUTPUT_BYTES
          raise Error.new(
            "That diff is too large to send over the remote connection."
          )
        end
        output
      rescue error : Error
        raise error
      rescue error : File::Error | IO::Error
        raise Error.new(error.message || "Cannot read the repository")
      end

      private def find_base(workdir : String) : String
        candidates = [] of String
        symbolic = capture(
          workdir,
          ["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"],
          errors: false
        ).strip
        candidates << symbolic unless symbolic.empty?
        candidates.concat(["origin/main", "origin/master", "main", "master"])

        candidates.each do |candidate|
          status = run(
            workdir,
            ["rev-parse", "--verify", "--quiet", candidate]
          )[1]
          return "#{candidate}\n" if status.success?
        end
        ""
      end

      private def working_all(workdir : String, root : String) : String
        output = capture(workdir, ["--no-pager", "diff", "HEAD"])
        untracked = capture(
          workdir,
          ["ls-files", "--others", "--exclude-standard", "-z"]
        )
        untracked.split('\0').each do |path|
          next if path.empty?
          selected = safe_path(workdir, root, path)
          output += capture(
            workdir,
            [
              "--no-pager", "diff", "--no-index", "--",
              "/dev/null", selected,
            ]
          )
          if output.bytesize > MAX_OUTPUT_BYTES
            raise Error.new(
              "That diff is too large to send over the remote connection."
            )
          end
        end
        output
      end

      private def repository_root(workdir : String) : String
        root = capture(workdir, ["rev-parse", "--show-toplevel"]).strip
        if root.empty?
          raise Error.new("This chat is not in a Git repository.")
        end
        File.realpath(root)
      rescue Error
        raise Error.new("This chat is not in a Git repository.")
      end

      private def range(base : String?) : String
        unless base &&
               BASE_PATTERN.matches?(base) &&
               !base.starts_with?('-') &&
               !base.includes?("..")
          raise Error.new("A valid base branch is required.")
        end
        "#{base}...HEAD"
      end

      private def safe_path(
        workdir : String,
        root : String,
        path : String?,
      ) : String
        unless path && !path.empty? && !Path[path].absolute?
          raise Error.new("That diff path is outside the repository.")
        end

        candidate = File.expand_path(path, workdir)
        unless candidate == root ||
               candidate.starts_with?(root + File::SEPARATOR)
          raise Error.new("That diff path is outside the repository.")
        end
        path
      end

      private def capture(
        workdir : String,
        arguments : Array(String),
        errors : Bool = true,
      ) : String
        output, status, error_text = run(workdir, arguments)
        if output.bytesize > MAX_OUTPUT_BYTES
          raise Error.new(
            "That diff is too large to send over the remote connection."
          )
        end
        unless output.valid_encoding?
          raise Error.new("Git returned text with an invalid encoding.")
        end

        if errors && output.empty? && !status.success?
          message = error_text.strip
          raise Error.new(message) unless message.empty?
        end
        output
      end

      private def run(
        workdir : String,
        arguments : Array(String),
      ) : Tuple(String, Process::Status, String)
        output = IO::Memory.new
        error = IO::Memory.new
        status = Process.run(
          "git",
          arguments,
          chdir: workdir,
          output: output,
          error: error
        )
        {output.to_s, status, error.to_s}
      rescue error : File::Error | IO::Error
        raise Error.new("Cannot run Git: #{error.message}")
      end
    end
  end
end
