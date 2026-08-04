require "../git_path"
require "../storage/workflow_state"
require "./service"
require "./worktree_containers"

module Xd
  module Workspace
    record Worktree,
      path : String,
      branch : String?,
      detached : Bool,
      main : Bool,
      current : Bool

    record WorktreeState,
      workdir : String,
      linked : Bool,
      worktrees : Array(Worktree)

    class Worktrees
      MAX_OUTPUT_BYTES = 8 * 1024 * 1024

      class Error < Exception
      end

      def initialize(
        @store : Storage::Store,
        @workspaces : Service,
      )
        @worktree_containers = WorktreeContainers.new(@store)
      end

      def state(chat : Storage::Chat) : WorktreeState
        workdir = resolve(chat)
        items = list(workdir)
        register_generated_container(items)
        current = items.find(&.current)
        WorktreeState.new(
          workdir,
          !!current && !current.main,
          items
        )
      end

      # The daemon owns this description so Unix and TLS clients show the
      # same checkout identity instead of independently probing Git.
      def describe(
        workdir : String?,
        home : String = Path.home.to_s,
      ) : String
        return "No working directory" unless workdir

        normalized_workdir = normalize(workdir)
        normalized_home = normalize(home)
        shown = if normalized_workdir == normalized_home
                  "~"
                elsif normalized_workdir.starts_with?(
                        normalized_home + File::SEPARATOR
                      )
                  "~#{normalized_workdir[normalized_home.size..]}"
                else
                  normalized_workdir
                end
        items = list(normalized_workdir)
        current = items.find(&.current) || items.first?
        return "#{shown} — not a repository" unless current

        parts = [] of String
        if branch = current.branch
          prefix = current.detached ? "detached at" : "⎇"
          parts << "#{prefix} #{branch}"
        end
        name = File.basename(current.path)
        name += " (worktree)" unless current.main
        parts << name
        parts << shown
        parts.join(" · ")
      rescue Error
        "#{shown} — not a repository"
      end

      def resolve(chat : Storage::Chat) : String
        workdir = @workspaces.resolve_workdir(
          chat.folder_id,
          chat.workdir
        )
        return workdir if File.directory?(workdir)

        if original = chat.original_workdir
          if File.directory?(original)
            @store.restore_workdir(chat.id, original)
            return original
          end
        end
        workdir
      end

      def prepare(
        chat : Storage::Chat,
        name_hint : String?,
      ) : String
        source = resolve(chat)
        return source unless chat.new_worktree

        target = create(source, chat.id, name_hint)
        @store.use_worktree(chat.id, target, source)
        target
      end

      def select(chat : Storage::Chat, requested : String?) : String
        unless requested && !requested.empty?
          raise Error.new("An existing worktree path is required.")
        end

        source = resolve(chat)
        selected = list(source).find do |item|
          same_path?(item.path, requested)
        end
        unless selected
          raise Error.new(
            "That path is not a worktree of this repository."
          )
        end

        @store.use_existing_worktree(
          chat.id,
          selected.path,
          source
        )
        selected.path
      rescue error : Storage::Error
        raise Error.new(error.message || "Cannot change the workspace.")
      end

      def remove(
        chat : Storage::Chat,
        requested_path : String,
      ) : Nil
        unless !requested_path.empty? && Path[requested_path].absolute?
          raise Error.new("An absolute worktree path is required.")
        end

        original_workdir = chat.original_workdir
        unless original_workdir
          raise Error.new("That chat is not using a removable worktree.")
        end
        selected_workdir = chat.workdir
        unless selected_workdir
          raise Error.new("That chat is not using a removable worktree.")
        end
        if chat.new_worktree
          raise Error.new("The chat has not selected an existing worktree.")
        end
        if @store.last_message_id(chat.id) > 0
          raise Error.new(
            "A worktree cannot be removed after the first message."
          )
        end

        requested = normalize(requested_path)
        unless same_path?(selected_workdir, requested)
          raise Error.new("That worktree is no longer selected.")
        end

        target = list(original_workdir).find do |item|
          same_path?(item.path, requested)
        end
        unless target
          raise Error.new(
            "That path is not a worktree of this repository."
          )
        end
        if target.main
          raise Error.new("The main checkout cannot be removed.")
        end
        if target.current
          raise Error.new("The currently active worktree cannot be removed.")
        end
        if @store.worktree_referenced_by_other_chat?(chat.id, target.path)
          raise Error.new("Another chat is still using that worktree.")
        end

        output, status, error = git(
          target.path,
          ["status", "--porcelain", "--untracked-files=all"]
        )
        unless status.success?
          message = error.strip
          raise Error.new(
            message.empty? ? "Cannot inspect the worktree." : message
          )
        end
        validate_output(output)
        unless output.empty?
          raise Error.new("The worktree must be clean before it is removed.")
        end

        _, status, error = git(
          original_workdir,
          ["worktree", "remove", target.path]
        )
        unless status.success?
          message = error.strip
          raise Error.new(
            message.empty? ? "git worktree remove failed" : message
          )
        end

        @store.restore_selected_worktree(
          chat.id,
          selected_workdir,
          original_workdir
        )
      rescue error : Storage::Error
        raise Error.new(error.message || "Cannot remove the worktree.")
      end

      # Resolves only a checkout already registered with the same repository.
      # Agent-reported paths may move future turns, but never widen their
      # sandbox to an unrelated directory.
      def registered_path(workdir : String, requested : String) : String?
        return nil unless Path[requested].absolute?
        return nil unless File.directory?(requested)

        list(workdir).find do |item|
          same_path?(item.path, requested)
        end.try(&.path)
      rescue Error
        nil
      end

      def list(workdir : String) : Array(Worktree)
        root = repository_root(workdir)
        output, status, error = git(
          root,
          ["worktree", "list", "--porcelain", "-z"]
        )
        unless status.success?
          message = error.strip
          raise Error.new(
            message.empty? ? "git worktree list failed" : message
          )
        end
        validate_output(output)

        current = repository_root(root)
        result = [] of Worktree
        path : String? = nil
        branch : String? = nil
        detached = false
        prunable = false

        finish = -> {
          if item_path = path
            unless prunable
              normalized = normalize(item_path)
              result << Worktree.new(
                normalized,
                branch,
                detached,
                result.empty?,
                same_path?(normalized, current)
              )
            end
          end
          path = nil
          branch = nil
          detached = false
          prunable = false
        }

        output.split('\0').each do |token|
          if token.empty?
            finish.call
          elsif token.starts_with?("worktree ")
            path = token["worktree ".size..]
          elsif token.starts_with?("branch refs/heads/")
            branch = token["branch refs/heads/".size..]
          elsif token.starts_with?("HEAD ") && !branch
            commit = token["HEAD ".size..]
            branch = commit[0, Math.min(8, commit.size)]
          elsif token == "detached"
            detached = true
          elsif token.starts_with?("prunable")
            prunable = true
          end
        end
        finish.call if path

        if result.empty?
          raise Error.new("Git returned no worktrees.")
        end
        result
      end

      def create(
        workdir : String,
        chat_id : String,
        name_hint : String?,
      ) : String
        raise Error.new("A chat id is required.") if chat_id.empty?

        root = repository_root(workdir)
        worktrees = list(root)
        slug = slug(name_hint)
        branch = "xd/#{slug}-#{glib_hash(chat_id)}"
        legacy_branch = "xd/#{chat_id}"
        main = worktrees.first.path
        repository_parent = File.dirname(main)
        repository_name = File.basename(main)
        container = File.join(repository_parent, "worktrees")

        if existing = worktrees.find do |item|
             !item.detached &&
             (item.branch == branch || item.branch == legacy_branch)
           end
          register_container(container) if within?(existing.path, container)
          return existing.path
        end

        register_container(container)
        worktree_name = slug
        suffix = 2

        loop do
          parent = File.join(
            container,
            repository_name,
            worktree_name
          )
          target = File.join(parent, repository_name)
          unless File.exists?(target)
            Dir.mkdir_p(parent, 0o700)
            reference = "refs/heads/#{branch}"
            _, branch_status, _ = git(
              root,
              ["show-ref", "--verify", "--quiet", reference]
            )
            arguments = if branch_status.success?
                          ["worktree", "add", target, branch]
                        else
                          [
                            "worktree", "add", "-b", branch,
                            target, "HEAD",
                          ]
                        end
            _, status, error = git(root, arguments)
            unless status.success?
              message = error.strip
              raise Error.new(
                message.empty? ? "git worktree add failed" : message
              )
            end
            return normalize(target)
          end

          worktree_name = "#{slug}-#{suffix}"
          suffix += 1
        end
      rescue error : File::Error
        raise Error.new(error.message || "Cannot create worktree.")
      end

      private def repository_root(workdir : String) : String
        output, status, _ = git(
          workdir,
          ["rev-parse", "--show-toplevel"]
        )
        unless status.success?
          raise Error.new(
            "Worktree selection needs a Git working directory."
          )
        end
        validate_output(output)
        root = GitPath.native(output.strip)
        if root.empty?
          raise Error.new(
            "Worktree selection needs a Git working directory."
          )
        end
        normalize(root)
      rescue error : File::Error
        raise Error.new(
          "Worktree selection needs a Git working directory."
        )
      end

      private def slug(hint : String?) : String
        result = String.build do |value|
          separator = false
          characters = 0
          (hint || "").each_char do |character|
            unless character.alphanumeric?
              separator = !value.empty?
              next
            end
            value << '-' if separator
            separator = false
            value << character.downcase
            characters += 1
            break if characters == 40
          end
        end
        result.empty? ? "worktree" : result
      end

      # GLib's g_str_hash, preserving branch names made by old xd builds.
      private def glib_hash(value : String) : String
        hash = 5381_u32
        value.each_byte do |byte|
          hash = hash &* 33_u32 &+ byte.to_u32
        end
        hash.to_s(16).rjust(8, '0')
      end

      private def same_path?(left : String, right : String) : Bool
        normalize(left) == normalize(right)
      end

      private def within?(path : String, parent : String) : Bool
        normalized_path = File.expand_path(path)
        normalized_parent = File.expand_path(parent)
        normalized_path.starts_with?(normalized_parent + File::SEPARATOR)
      end

      private def register_container(path : String) : Nil
        Dir.mkdir_p(path, 0o700)
        @worktree_containers.register(path)
      end

      # Older builds created this layout without registering its top-level
      # container. Recognize only XD branches in XD's exact path convention,
      # then persist the container so workspace scans keep excluding it.
      private def register_generated_container(items : Array(Worktree)) : Nil
        main = items.find(&.main) || return
        repository_name = File.basename(main.path)
        container = File.join(File.dirname(main.path), "worktrees")
        generated = items.any? do |item|
          !item.main &&
            item.branch.try(&.starts_with?("xd/")) == true &&
            File.basename(item.path) == repository_name &&
            within?(item.path, container)
        end
        register_container(container) if generated
      end

      private def normalize(path : String) : String
        File.realpath(path)
      rescue File::Error
        File.expand_path(path)
      end

      private def validate_output(output : String) : Nil
        if output.bytesize > MAX_OUTPUT_BYTES
          raise Error.new("Git returned too much worktree data.")
        end
        unless output.valid_encoding?
          raise Error.new("Git returned text with an invalid encoding.")
        end
      end

      private def git(
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
