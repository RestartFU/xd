require "http/client"
require "json"
require "uri"
require "./environment"
require "./executable"

module Xd
  module Agent
    # Stable tool record for a GitHub Actions run observed by an agent.
    module WorkflowRun
      extend self

      PREFIX = "workflow_run\n"

      record Run, id : String, repository : String, url : String

      MAX_JOBS         = 100
      CLI_TIMEOUT      = 5.seconds
      CLI_OUTPUT_LIMIT = 2 * 1024 * 1024
      CLI_READ_BUFFER  = 8 * 1024

      class StatusError < Exception
      end

      private class CliTimeoutError < StatusError
      end

      record Job,
        id : String,
        name : String,
        state : String,
        conclusion : String?,
        log : String?,
        started_at : Time? = nil,
        completed_at : Time? = nil do
        def terminal? : Bool
          state == "completed"
        end

        def elapsed(now : Time = Time.utc) : Time::Span?
          WorkflowRun.elapsed(started_at, completed_at, terminal?, now)
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
        jobs : Array(Job),
        started_at : Time? = nil,
        completed_at : Time? = nil do
        def terminal? : Bool
          state == "completed"
        end

        def elapsed(now : Time = Time.utc) : Time::Span?
          WorkflowRun.elapsed(started_at, completed_at, terminal?, now)
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

      private record Resolution,
        status : Status,
        authenticated : Bool

      # One daemon owns GitHub polling for every connected desktop and phone.
      # Besides preventing duplicate requests, a last-known-good snapshot is
      # more useful than replacing a running card with an outage message.
      class StatusCache
        AUTHENTICATED_TTL = 8.seconds
        ANONYMOUS_TTL     = 3.minutes
        FAILURE_TTL       = 30.seconds

        alias Resolver = Proc(Run, Status)

        private record Entry,
          status : Status,
          checked_at : Time::Instant,
          active_ttl : Time::Span

        def initialize(
          @resolver : Resolver? = nil,
          @clock : Proc(Time::Instant) = -> { Time.instant },
          @active_ttl : Time::Span? = nil,
          @failure_ttl : Time::Span = FAILURE_TTL,
        )
          @entries = {} of String => Entry
          @failures = {} of String => Time::Instant
          @mutex = Mutex.new
        end

        def fetch(run : Run) : Status
          key = "#{run.repository}/#{run.id}"
          @mutex.synchronize do
            now = @clock.call
            entry = @entries[key]?
            if entry && (
                 entry.status.terminal? ||
                 now - entry.checked_at < entry.active_ttl
               )
              return entry.status
            end
            if failed_at = @failures[key]?
              if now - failed_at < @failure_ttl
                return entry.status if entry
                raise StatusError.new(
                  "Workflow status is temporarily unavailable."
                )
              end
            end

            begin
              resolution = if resolver = @resolver
                             Resolution.new(resolver.call(run), true)
                           else
                             WorkflowRun.fetch_resolution(run)
                           end
              status = resolution.status
              ttl = @active_ttl || (
                resolution.authenticated ? AUTHENTICATED_TTL : ANONYMOUS_TTL
              )
              @entries[key] = Entry.new(status, now, ttl)
              @failures.delete(key)
              status
            rescue error : StatusError
              @failures[key] = now
              return entry.status if entry
              raise error
            end
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
        fetch_resolution(run, token).status
      end

      def fetch_resolution(
        run : Run,
        token : String? = ENV["GH_TOKEN"]? || ENV["GITHUB_TOKEN"]?,
      ) : Resolution
        if token && !token.empty?
          begin
            return Resolution.new(fetch_api_status(run, token), true)
          rescue StatusError | IO::Error | Socket::Error | URI::Error
            return Resolution.new(fetch_cli_status(run), true)
          end
        end

        # A captured `gh run` command normally means the daemon already has an
        # authenticated GitHub CLI. Prefer that single authenticated request
        # to two anonymous REST requests with a very small shared quota.
        begin
          Resolution.new(fetch_cli_status(run), true)
        rescue error : CliTimeoutError
          # Falling through to two REST requests after an already stalled CLI
          # can exceed the daemon request deadline. Let the status cache serve
          # its last good value and retry later instead.
          raise error
        rescue StatusError
          Resolution.new(fetch_api_status(run, nil), false)
        end
      end

      private def fetch_api_status(
        run : Run,
        token : String?,
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
        Status.new(
          status.name,
          status.state,
          status.conclusion,
          jobs,
          status.started_at,
          status.completed_at
        )
      rescue error : IO::Error | Socket::Error | URI::Error
        raise StatusError.new(error.message || "Cannot reach GitHub.")
      end

      def parse_wire_status(fields : Hash(String, JSON::Any)) : Status?
        body = fields.to_json
        status = parse_status(body) || return nil
        jobs = parse_jobs(body) || return nil
        Status.new(
          status.name,
          status.state,
          status.conclusion,
          jobs,
          status.started_at,
          status.completed_at
        )
      end

      def parse_status(body : String?) : Status?
        return unless body
        record = JSON.parse(body).as_h?
        return unless record
        name = record["name"]?.try(&.as_s?) || ""
        state = (record["status"]? || record["state"]?).try(&.as_s?)
        return unless state && !state.empty?
        return if name.bytesize > 160 || state.bytesize > 40
        conclusion = record["conclusion"]?.try(&.as_s?)
        return if conclusion && conclusion.bytesize > 40
        # REST names a run's clock `run_started_at`, the CLI `startedAt`, and
        # neither reports a finish time: `updated_at` is the last write, which
        # for a completed run is the moment it completed.
        started_at = timestamp(
          record["run_started_at"]? || record["started_at"]? ||
          record["startedAt"]? ||
          record["created_at"]? || record["createdAt"]?
        )
        completed_at = if state == "completed"
                         timestamp(
                           record["completed_at"]? ||
                           record["updated_at"]? || record["updatedAt"]?
                         )
                       end
        Status.new(
          name,
          state,
          conclusion,
          [] of Job,
          started_at,
          completed_at
        )
      rescue JSON::ParseException
        nil
      end

      # Time a run or job has been going, or took. Nil when it never started,
      # or when it finished without saying when — a growing count on something
      # already over reads worse than no count.
      def elapsed(
        started_at : Time?,
        completed_at : Time?,
        terminal : Bool,
        now : Time = Time.utc,
      ) : Time::Span?
        return unless started_at
        return if completed_at.nil? && terminal
        span = (completed_at || now) - started_at
        span < Time::Span.zero ? Time::Span.zero : span
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
          id = item["id"]? || item["databaseId"]?
          id_value = id.try(&.as_s?) || id.try(&.as_i64?).try(&.to_s)
          name_value = item["name"]?.try(&.as_s?)
          state_value = (item["status"]? || item["state"]?).try(&.as_s?)
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
            item["log"]?.try(&.as_s?) || latest_job_activity(item, state),
            timestamp(item["started_at"]? || item["startedAt"]?),
            timestamp(item["completed_at"]? || item["completedAt"]?)
          )
          break if result.size >= MAX_JOBS
        end
        result
      rescue JSON::ParseException
        nil
      end

      def fetch_cli_status(
        run : Run,
        timeout : Time::Span = CLI_TIMEOUT,
      ) : Status
        executable = Executable.resolve("gh")
        process = Process.new(
          executable,
          [
            "run",
            "view",
            run.id,
            "--repo",
            run.repository,
            "--json",
            "name,status,conclusion,startedAt,updatedAt,jobs",
          ],
          env: Environment.host,
          clear_env: true,
          input: Process::Redirect::Close,
          output: Process::Redirect::Pipe,
          error: Process::Redirect::Close
        )
        output_done = Channel(String).new(1)
        status_done = Channel(Process::Status).new(1)
        spawn drain_cli_output(process.output, output_done)
        spawn { status_done.send(process.wait) }

        status : Process::Status? = nil
        timed_out = false
        select
        when result = status_done.receive
          status = result
        when timeout(timeout)
          timed_out = true
          begin
            process.terminate(graceful: false)
          rescue RuntimeError
            # It exited between the timeout firing and the kill request.
          end
          select
          when result = status_done.receive
            status = result
          when timeout(1.second)
          end
          process.output.close unless process.output.closed?
        end
        body = output_done.receive
        if timed_out
          raise CliTimeoutError.new(
            "GitHub CLI timed out after #{timeout.total_seconds} seconds."
          )
        end
        unless status.try(&.success?)
          raise StatusError.new(
            "GitHub CLI returned status #{status}."
          )
        end

        parsed = parse_status(body) || raise StatusError.new(
          "GitHub CLI returned an invalid workflow status."
        )
        jobs = parse_jobs(body) || raise StatusError.new(
          "GitHub CLI returned invalid workflow jobs."
        )
        Status.new(
          parsed.name,
          parsed.state,
          parsed.conclusion,
          jobs,
          parsed.started_at,
          parsed.completed_at
        )
      rescue error : File::Error | IO::Error
        raise StatusError.new(error.message || "Cannot run GitHub CLI.")
      end

      private def drain_cli_output(
        stream : IO,
        done : Channel(String),
      ) : Nil
        output = IO::Memory.new
        buffer = Bytes.new(CLI_READ_BUFFER)
        while count = stream.read(buffer)
          break if count == 0
          remaining = CLI_OUTPUT_LIMIT - output.bytesize
          output.write(buffer[0, Math.min(count, remaining)]) if remaining > 0
          Fiber.yield
        end
      rescue IO::Error
      ensure
        done.send(output.to_s)
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

      # The CLI writes a zero time rather than null for a job that has not
      # finished, so an unset clock arrives as year one rather than as absent.
      private def timestamp(value : JSON::Any?) : Time?
        if seconds = value.try(&.as_i64?)
          return if seconds <= 0
          return Time.unix(seconds)
        end
        text = value.try(&.as_s?)
        return if text.nil? || text.empty?
        moment = Time.parse_rfc3339(text)
        moment.year < 2000 ? nil : moment
      rescue Time::Format::Error
        nil
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
