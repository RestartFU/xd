require "../agent/environment"
require "../storage/workflow_state"
require "../workspace/service"
require "./filesystem"

module Xd
  module Daemon
    class Repository
      MAX_OUTPUT_BYTES = 8 * 1024 * 1024
      BASE_PATTERN     = /\A[A-Za-z0-9_\.\/-]{1,256}\z/
      URL_PATTERN      = /https:\/\/[^\s]+/
      STATE_SCRIPT     =
        "printf '%s\\n' \"$(git status --porcelain 2>/dev/null | head -n 1)\"; " \
        "git rev-parse --abbrev-ref HEAD 2>/dev/null || echo; " \
        "git rev-parse --abbrev-ref --symbolic-full-name '@{u}' " \
        "2>/dev/null || echo; " \
        "git rev-list --count '@{u}..HEAD' 2>/dev/null || echo 0; " \
        "for ref in " \
        "\"$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD)\" " \
        "origin/main origin/master main master; do " \
        "  [ -n \"$ref\" ] || continue; " \
        "  git rev-parse --verify --quiet \"$ref\" >/dev/null && " \
        "    { echo \"${ref##*/}\"; break; }; " \
        "done; " \
        "gh pr view --json url --jq .url 2>/dev/null || true"

      class Error < Exception
      end

      record State,
        visible : Bool,
        action : String,
        label : String,
        enabled : Bool,
        url : String? = nil

      record ActionResult, state : State, url : String? = nil

      def initialize(
        @store : Storage::Store,
        @workspaces : Workspace::Service,
        @filesystem : Filesystem,
      )
        @action_mutex = Mutex.new
      end

      def state(chat_id : String) : State
        state_at(@filesystem.workdir(chat_id))
      end

      def head_signature(chat_id : String) : String
        workdir = @filesystem.workdir(chat_id)
        output, status, _error = run(
          workdir,
          ["status", "--porcelain=v2", "--branch", "--untracked-files=no"]
        )
        return "" unless status.success?

        String.build do |io|
          output.each_line do |line|
            if line.starts_with?("# branch.oid ") ||
               line.starts_with?("# branch.head ")
              io << line
            end
          end
        end
      rescue Error | Filesystem::Error
        ""
      end

      def perform(
        chat_id : String,
        action : String,
        message : String?,
      ) : ActionResult
        @action_mutex.synchronize do
          workdir = @filesystem.workdir(chat_id)
          current = state_at(workdir)
          unless current.visible && current.enabled && current.action == action
            raise Error.new("Repository state changed. Try again.")
          end

          url = case action
                when "commit"
                  text = message.try(&.strip) || ""
                  if text.empty?
                    raise Error.new("Write a commit message first.")
                  end
                  checked(workdir, "git", ["add", "-A"])
                  checked(workdir, "git", ["commit", "-m", text])
                  nil
                when "push"
                  checked(workdir, "git", ["push", "-u", "origin", "HEAD"])
                  nil
                when "create-pr"
                  environment = Agent::Environment.host
                  environment["GH_BROWSER"] = "echo"
                  output = checked(
                    workdir,
                    "gh",
                    ["pr", "create", "--web"],
                    environment
                  )
                  web_url(output) || raise Error.new(
                    "GitHub CLI did not return a pull request URL."
                  )
                when "view-pr"
                  current.url || raise Error.new(
                    "GitHub CLI did not return a pull request URL."
                  )
                else
                  raise Error.new("No such Git action.")
                end

          ActionResult.new(state_at(workdir), url)
        end
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

      private def state_at(workdir : String) : State
        output, _status, _error = command(
          workdir,
          "sh",
          ["-c", STATE_SCRIPT]
        )
        lines = output.split('\n')
        return hidden_state if lines.size < 5

        dirty = lines[0]
        branch = lines[1]
        upstream = lines[2]
        ahead = lines[3].to_i?
        base = lines[4]
        url = lines[5]?.try do |value|
          stripped = value.strip
          stripped unless stripped.empty?
        end
        return hidden_state if branch.empty?

        action = if !dirty.empty?
                   "commit"
                 elsif (ahead || 0) > 0 || upstream.empty?
                   "push"
                 elsif branch != base
                   url ? "view-pr" : "create-pr"
                 else
                   "none"
                 end
        State.new(
          true,
          action,
          action_label(action),
          action != "none",
          url
        )
      rescue File::Error | IO::Error
        hidden_state
      end

      private def hidden_state : State
        State.new(false, "none", "Up to date", false)
      end

      private def action_label(action : String) : String
        case action
        when "commit"    then "Commit"
        when "push"      then "Push"
        when "create-pr" then "Create PR"
        when "view-pr"   then "View PR"
        else                  "Up to date"
        end
      end

      private def checked(
        workdir : String,
        executable : String,
        arguments : Array(String),
        environment : Hash(String, String) = Agent::Environment.host,
      ) : String
        output, status, error_text = command(
          workdir,
          executable,
          arguments,
          environment
        )
        unless status.success?
          detail = error_text.strip
          detail = output.strip if detail.empty?
          detail = "#{executable} refused the request." if detail.empty?
          raise Error.new(detail)
        end
        output
      rescue error : File::Error | IO::Error
        raise Error.new("Cannot run #{executable}: #{error.message}")
      end

      private def web_url(output : String) : String?
        output.match(URL_PATTERN).try(&.[0].rstrip(".,);"))
      end

      private def working_all(workdir : String, root : String) : String
        output = capture(
          workdir,
          ["--no-pager", "diff", working_treeish(workdir)]
        )
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

      private def working_treeish(workdir : String) : String
        head = run(
          workdir,
          ["rev-parse", "--verify", "--quiet", "HEAD"]
        )[1]
        return "HEAD" if head.success?

        empty = capture(
          workdir,
          ["hash-object", "-t", "tree", "/dev/null"]
        ).strip
        if empty.empty?
          raise Error.new("Git could not create an empty repository baseline.")
        end
        empty
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
        command(workdir, "git", arguments)
      rescue error : File::Error | IO::Error
        raise Error.new("Cannot run Git: #{error.message}")
      end

      private def command(
        workdir : String,
        executable : String,
        arguments : Array(String),
        environment : Hash(String, String) = Agent::Environment.host,
      ) : Tuple(String, Process::Status, String)
        output = IO::Memory.new
        error = IO::Memory.new
        status = Process.run(
          executable,
          arguments,
          chdir: workdir,
          env: environment,
          clear_env: true,
          output: output,
          error: error
        )
        {output.to_s, status, error.to_s}
      end
    end
  end
end
