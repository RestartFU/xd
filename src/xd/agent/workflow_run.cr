module Xd
  module Agent
    # Stable tool record for a GitHub Actions run observed by an agent.
    module WorkflowRun
      extend self

      PREFIX = "workflow_run\n"

      record Run, id : String, repository : String, url : String

      def capture(message : String, workdir : String) : String
        return message if message.starts_with?(PREFIX)
        return message unless message.starts_with?("$ ")
        arguments = shell_words(message.byte_slice(2))
        return message unless arguments

        run_at = nil
        arguments.each_index do |index|
          next unless index + 3 < arguments.size
          if arguments[index] == "gh" &&
             arguments[index + 1] == "run" &&
             {"watch", "view"}.includes?(arguments[index + 2]) &&
             numeric?(arguments[index + 3])
            run_at = index
            break
          end
        end
        index = run_at
        return message unless index

        run_id = arguments[index + 3]
        repository : String? = nil
        cursor = index + 4
        while cursor < arguments.size
          argument = arguments[cursor]
          if {"--repo", "-R"}.includes?(argument) &&
             cursor + 1 < arguments.size
            cursor += 1
            repository = repository_from_spec(arguments[cursor])
            break
          elsif argument.starts_with?("--repo=")
            repository = repository_from_spec(
              argument.byte_slice("--repo=".bytesize)
            )
            break
          end
          cursor += 1
        end
        repository ||= repository_from_workdir(workdir)
        return message unless repository

        "#{PREFIX}#{run_id}\n" \
        "https://github.com/#{repository}/actions/runs/#{run_id}"
      end

      def parse(message : String?) : Run?
        return unless message
        return unless message.starts_with?(PREFIX)

        body = message.byte_slice(PREFIX.bytesize)
        parts = body.split('\n', 2)
        return unless parts.size == 2
        run_id, url = parts
        return unless numeric?(run_id)

        suffix = "/actions/runs/#{run_id}"
        prefix = "https://github.com/"
        return unless url.starts_with?(prefix) && url.ends_with?(suffix)
        repository_size = url.bytesize - prefix.bytesize - suffix.bytesize
        return unless repository_size > 0
        repository = url.byte_slice(prefix.bytesize, repository_size)
        return unless repository_from_spec(repository) == repository

        Run.new(run_id, repository, url)
      end

      private def repository_from_workdir(workdir : String) : String?
        remote = git(workdir, ["remote", "get-url", "origin"]).try(&.strip)
        unless remote && !remote.empty?
          name = git(workdir, ["remote"]).try(&.lines.first?.try(&.strip))
          remote = git(workdir, ["remote", "get-url", name]).try(&.strip) if name
        end
        repository_from_spec(remote)
      end

      private def repository_from_spec(spec : String?) : String?
        return unless spec
        path = spec.strip
        {
          "git@github.com:",
          "ssh://git@github.com/",
          "https://github.com/",
          "http://github.com/",
          "github.com/",
        }.each do |prefix|
          if path.starts_with?(prefix)
            path = path.byte_slice(prefix.bytesize)
            break
          end
        end
        path = path.byte_slice(0, path.bytesize - 4) if path.ends_with?(".git")
        parts = path.split('/')
        return unless parts.size == 2
        return unless parts.all? { |part| safe_component?(part) }
        path
      end

      private def safe_component?(component : String) : Bool
        return false if component.empty?
        component.each_char.all? do |character|
          character.alphanumeric? ||
            {'-', '_', '.'}.includes?(character)
        end
      end

      private def numeric?(value : String) : Bool
        !value.empty? && value.each_char.all?(&.ascii_number?)
      end

      # GLib shell parsing treated quoting, not operators. This equivalent
      # tokenizer is enough to find a gh invocation without running the line.
      private def shell_words(command : String) : Array(String)?
        words = [] of String
        word = String::Builder.new
        quote : Char? = nil
        escaped = false
        started = false

        command.each_char do |character|
          if escaped
            word << character
            escaped = false
            started = true
            next
          end

          if current = quote
            if character == current
              quote = nil
            elsif current == '"' && character == '\\'
              escaped = true
            else
              word << character
              started = true
            end
          elsif character.whitespace?
            if started
              words << word.to_s
              word = String::Builder.new
              started = false
            end
          elsif {'\'', '"'}.includes?(character)
            quote = character
            started = true
          elsif character == '\\'
            escaped = true
            started = true
          else
            word << character
            started = true
          end
        end

        return if quote || escaped
        words << word.to_s if started
        words
      end

      private def git(
        workdir : String,
        arguments : Array(String),
      ) : String?
        output = IO::Memory.new
        status = Process.run(
          "git",
          arguments,
          chdir: workdir,
          output: output,
          error: Process::Redirect::Close
        )
        status.success? ? output.to_s : nil
      rescue File::Error | IO::Error
        nil
      end
    end
  end
end
