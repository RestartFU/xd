require "http/client"
require "json"
require "uri"

module Xd
  module Agent
    # Stable tool record for a GitHub Actions run observed by an agent.
    module WorkflowRun
      extend self

      PREFIX = "workflow_run\n"

      record Run, id : String, repository : String, url : String

      MAX_JOBS = 100

      class StatusError < Exception
      end

      record Job,
        id : String,
        name : String,
        state : String,
        conclusion : String?,
        log : String? do
        def terminal? : Bool
          state == "completed"
        end

        def label : String
          return "" unless terminal?

          case conclusion
          when "success"         then "Passed"
          when "failure"         then "Failed"
          when "cancelled"       then "Cancelled"
          when "timed_out"       then "Timed out"
          when "action_required" then "Action required"
          when "startup_failure" then "Startup failed"
          when "skipped"         then "Skipped"
          when "stale"           then "Stale"
          when "neutral"         then "Neutral"
          else                        "Completed"
          end
        end

        def css_class : String
          return "xd-workflow-running" unless terminal?
          case conclusion
          when "success" then "xd-workflow-success"
          when "failure", "timed_out", "startup_failure"
            "xd-workflow-failure"
          else
            "xd-workflow-finished"
          end
        end
      end

      record Status,
        name : String,
        state : String,
        conclusion : String?,
        jobs : Array(Job) do
        def terminal? : Bool
          state == "completed"
        end

        def label : String
          result = case state
                   when "queued", "in_progress" then ""
                   when "completed"
                     case conclusion
                     when "success"         then "Passed"
                     when "failure"         then "Failed"
                     when "cancelled"       then "Cancelled"
                     when "timed_out"       then "Timed out"
                     when "action_required" then "Action required"
                     when "startup_failure" then "Startup failed"
                     when "skipped"         then "Skipped"
                     when "stale"           then "Stale"
                     when "neutral"         then "Neutral"
                     else                        "Completed"
                     end
                   else
                     state.split('_').map(&.capitalize).join(' ')
                   end
          return name if result.empty? && !name.empty?
          name.empty? ? result : "#{name} · #{result}"
        end

        def result_label : String
          name.empty? ? label : label.sub("#{name} · ", "")
        end

        def css_class : String
          return "xd-workflow-running" unless terminal?
          case conclusion
          when "success" then "xd-workflow-success"
          when "failure", "timed_out", "startup_failure"
            "xd-workflow-failure"
          else
            "xd-workflow-finished"
          end
        end
      end

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

      def fetch_status(
        run : Run,
        token : String? = ENV["GH_TOKEN"]? || ENV["GITHUB_TOKEN"]?,
      ) : Status
        uri = URI.parse(
          "https://api.github.com/repos/#{run.repository}/actions/runs/#{run.id}"
        )
        response = request_body(uri, api_headers(token))
        unless response[0].in?(200..299)
          raise StatusError.new(
            "GitHub returned HTTP #{response[0]}."
          )
        end
        status = parse_status(response[1]) || raise StatusError.new(
          "GitHub returned an invalid workflow status."
        )
        jobs = begin
          fetch_jobs(run, token)
        rescue StatusError
          [] of Job
        end
        Status.new(status.name, status.state, status.conclusion, jobs)
      rescue error : IO::Error | Socket::Error | URI::Error
        raise StatusError.new(error.message || "Cannot read workflow status.")
      end

      def parse_status(body : String?) : Status?
        return unless body
        record = JSON.parse(body).as_h?
        return unless record
        name = record["name"]?.try(&.as_s?) || ""
        state = record["status"]?.try(&.as_s?)
        return unless state && !state.empty?
        return if name.bytesize > 160 || state.bytesize > 40
        conclusion = record["conclusion"]?.try(&.as_s?)
        return if conclusion && conclusion.bytesize > 40
        Status.new(name, state, conclusion, [] of Job)
      rescue JSON::ParseException
        nil
      end

      def parse_jobs(body : String?) : Array(Job)?
        return unless body
        record = JSON.parse(body).as_h?
        return unless record
        values = record["jobs"]?.try(&.as_a?)
        return unless values

        result = [] of Job
        values.each do |value|
          item = value.as_h?
          next unless item
          id_value = item["id"]?.try(&.as_i64?).try(&.to_s)
          name_value = item["name"]?.try(&.as_s?)
          state_value = item["status"]?.try(&.as_s?)
          next unless id_value && name_value && state_value
          name = name_value.not_nil!
          state = state_value.not_nil!
          next if name.empty? || name.bytesize > 160 || state.empty? || state.bytesize > 40
          conclusion = item["conclusion"]?.try(&.as_s?)
          next if conclusion && conclusion.bytesize > 40
          result << Job.new(
            id_value.not_nil!,
            name,
            state,
            conclusion,
            latest_job_activity(item, state)
          )
          break if result.size >= MAX_JOBS
        end
        result
      rescue JSON::ParseException
        nil
      end

      private def fetch_jobs(run : Run, token : String?) : Array(Job)
        uri = URI.parse(
          "https://api.github.com/repos/#{run.repository}/actions/runs/" \
          "#{run.id}/jobs?per_page=#{MAX_JOBS}"
        )
        response = request_body(uri, api_headers(token))
        unless response[0].in?(200..299)
          raise StatusError.new(
            "GitHub returned HTTP #{response[0]} while reading jobs."
          )
        end
        parse_jobs(response[1]) || raise StatusError.new(
          "GitHub returned invalid workflow jobs."
        )
      end

      private def latest_job_activity(
        item : Hash(String, JSON::Any),
        job_state : String,
      ) : String?
        steps = item["steps"]?.try(&.as_a?) || return nil
        selected : JSON::Any? = nil
        if job_state == "in_progress"
          selected = steps.reverse.find do |value|
            step = value.as_h?
            next false unless step
            step["status"]?.try(&.as_s?) == "in_progress"
          end
        end
        selected ||= steps.reverse.find do |value|
          step = value.as_h?
          next false unless step
          status = step["status"]?.try(&.as_s?)
          next false unless status
          !{"queued", "pending", "requested"}.includes?(status)
        end
        step = selected.try(&.as_h?) || return nil
        name = step["name"]?.try(&.as_s?)
        return if name.nil? || name.empty?
        name
      end

      private def api_headers(token : String?) : HTTP::Headers
        headers = HTTP::Headers{
          "Accept"               => "application/vnd.github+json",
          "User-Agent"           => "xd",
          "X-GitHub-Api-Version" => "2022-11-28",
        }
        headers["Authorization"] = "Bearer #{token}" if token && !token.empty?
        headers
      end

      private def request_body(
        uri : URI,
        headers : HTTP::Headers,
      ) : Tuple(Int32, String)
        client = HTTP::Client.new(uri)
        client.connect_timeout = 5.seconds
        client.read_timeout = 10.seconds
        begin
          response = client.get(uri.request_target, headers)
          {response.status_code, response.body}
        ensure
          client.close
        end
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
