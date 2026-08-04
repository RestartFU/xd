require "json"
require "./catalog"
require "./environment"
require "./executable"

module Xd
  module Agent
    # Daemon-owned authentication for the bundled agent CLIs.
    #
    # Login can wait minutes for a browser or a pasted code, so no protocol
    # request waits for the process. State and output return over the same
    # event bus used by local Unix and remote TLS clients.
    class Authentication
      OUTPUT_LIMIT    = 64 * 1024
      DETAIL_LIMIT    = 2048
      COMMAND_TIMEOUT = 10.seconds
      ANSI            = /\e\[[0-?]*[ -\/]*[@-~]/
      URL             = %r{https://[^\s<>"']+}
      DEVICE_CODE     = /\b[A-Z0-9]{4,}(?:-[A-Z0-9]{4,})+\b/

      class Error < Exception
      end

      enum State
        Unknown
        Checking
        SignedOut
        SignedIn
        SigningIn
        SigningOut
        Failed

        def wire_name : String
          case self
          when Unknown    then "unknown"
          when Checking   then "checking"
          when SignedOut  then "signed-out"
          when SignedIn   then "signed-in"
          when SigningIn  then "signing-in"
          when SigningOut then "signing-out"
          when Failed     then "failed"
          else
            "unknown"
          end
        end
      end

      record Snapshot,
        provider : String,
        display_name : String,
        state : State,
        detail : String?,
        login_url : String?,
        device_code : String?,
        needs_input : Bool do
        def wire_fields : Hash(String, JSON::Any)
          fields = {
            "provider"     => JSON::Any.new(provider),
            "display_name" => JSON::Any.new(display_name),
            "state"        => JSON::Any.new(state.wire_name),
            "needs_input"  => JSON::Any.new(needs_input),
          }
          fields["detail"] = JSON::Any.new(value) if value = detail
          fields["login_url"] = JSON::Any.new(value) if value = login_url
          if value = device_code
            fields["device_code"] = JSON::Any.new(value)
          end
          fields
        end
      end

      private enum Command
        Check
        Login
        Logout
      end

      private class Entry
        getter backend : Backend
        property state = State::Unknown
        property detail : String?
        property output = ""
        property login_url : String?
        property device_code : String?
        property needs_input = false
        property process : Process?
        property input : IO?
        property serial = 0_i64
        property starting = false
        property cancel_requested = false

        def initialize(@backend : Backend)
          @detail = nil
          @login_url = nil
          @device_code = nil
          @process = nil
          @input = nil
        end

        def snapshot : Snapshot
          Snapshot.new(
            @backend.id,
            @backend.display_name,
            @state,
            @detail,
            @login_url,
            @device_code,
            @needs_input
          )
        end
      end

      alias Publisher = Proc(String, Hash(String, JSON::Any), Nil)
      alias Resolver = Proc(String, String)

      @entries : Hash(String, Entry)
      @mutex = Mutex.new
      @closed = false

      def initialize(
        @publisher : Publisher = ->(_name : String, _fields : Hash(String, JSON::Any)) { },
        resolver : Resolver? = nil,
        environment : Hash(String, String)? = nil,
        @command_timeout : Time::Span = COMMAND_TIMEOUT,
      )
        @resolver = resolver || ->(name : String) { Executable.resolve(name) }
        @environment = environment || Environment.host
        @entries = Catalog.authenticatable.to_h do |backend|
          {backend.id, Entry.new(backend)}
        end
      end

      def snapshots : Array(Snapshot)
        @mutex.synchronize do
          Catalog.authenticatable.compact_map do |backend|
            @entries[backend.id]?.try(&.snapshot)
          end
        end
      end

      def snapshot(provider : String) : Snapshot
        @mutex.synchronize { entry!(provider).snapshot }
      end

      def authorization_error(provider : String) : String?
        current = snapshot(provider)
        case current.state
        when .signed_in?
          nil
        when .signed_out?
          "Sign in to #{current.display_name} before starting a turn."
        when .checking?, .unknown?
          "Still checking #{current.display_name} sign-in status."
        when .signing_in?
          "Finish signing in to #{current.display_name} before starting a turn."
        when .signing_out?
          "#{current.display_name} is signing out."
        when .failed?
          detail = current.detail.try(&.strip)
          if detail && !detail.empty?
            "Cannot verify #{current.display_name} sign-in: #{detail}"
          else
            "Cannot verify #{current.display_name} sign-in."
          end
        else
          "Cannot verify #{current.display_name} sign-in."
        end
      end

      def refresh : Nil
        Catalog.authenticatable.each do |backend|
          refresh(backend.id)
        end
      end

      def refresh(provider : String) : Nil
        begin_command(provider, Command::Check, required: false)
      end

      def login(provider : String) : Nil
        begin_command(provider, Command::Login)
      end

      def logout(provider : String) : Nil
        begin_command(provider, Command::Logout)
      end

      def input(provider : String, text : String) : Nil
        value = text.strip
        raise Error.new("Authentication input cannot be empty.") if value.empty?
        if value.bytesize > 4096
          raise Error.new("Authentication input is too long.")
        end

        stream = @mutex.synchronize do
          entry = entry!(provider)
          unless entry.state.signing_in? && entry.input
            raise Error.new(
              "#{entry.backend.display_name} is not waiting for input."
            )
          end
          entry.input
        end
        stream.not_nil! << value << '\n'
        stream.not_nil!.flush
      rescue error : IO::Error
        raise Error.new("Cannot send authentication input: #{error.message}")
      end

      def cancel(provider : String) : Nil
        process : Process? = nil
        serial = 0_i64
        @mutex.synchronize do
          entry = entry!(provider)
          unless entry.state.signing_in?
            raise Error.new(
              "#{entry.backend.display_name} is not signing in."
            )
          end
          entry.cancel_requested = true
          process = entry.process
          serial = entry.serial
        end
        stop_process(provider, serial, process) if process
      end

      def close : Nil
        processes = @mutex.synchronize do
          return if @closed

          @closed = true
          @entries.values.compact_map(&.process)
        end
        processes.each do |process|
          process.terminate(graceful: false)
        rescue RuntimeError
        end
      end

      private def begin_command(
        provider : String,
        command : Command,
        required : Bool = true,
      ) : Nil
        snapshot : Snapshot? = nil
        serial = @mutex.synchronize do
          raise Error.new("Authentication service is stopping.") if @closed

          entry = entry!(provider)
          if entry.starting || entry.process
            if required
              raise Error.new(
                "#{entry.backend.display_name} authentication is already busy."
              )
            end
            next nil
          end

          entry.serial += 1
          entry.starting = true
          entry.cancel_requested = false
          entry.output = "" if command.login?
          entry.detail = nil
          clear_instructions(entry)
          entry.state = case command
                        when .check?  then State::Checking
                        when .login?  then State::SigningIn
                        when .logout? then State::SigningOut
                        else
                          raise Error.new("Unknown authentication command.")
                        end
          snapshot = entry.snapshot
          entry.serial
        end
        return unless serial

        publish_state(snapshot.not_nil!)
        spawn run_command(provider, serial, command)
      end

      private def run_command(
        provider : String,
        serial : Int64,
        command : Command,
      ) : Nil
        backend = @mutex.synchronize { entry!(provider).backend }
        arguments = command_arguments(backend, command)
        environment = @environment.dup
        environment["NO_COLOR"] = "1"
        environment["TERM"] = "dumb"

        process = Process.new(
          arguments,
          env: environment,
          clear_env: true,
          input: command.login? ? Process::Redirect::Pipe : Process::Redirect::Close,
          output: Process::Redirect::Pipe,
          error: Process::Redirect::Pipe
        )
        canceled = register_process(provider, serial, command, process)
        if canceled
          stop_process(provider, serial, process)
        end

        output = IO::Memory.new
        errors = IO::Memory.new
        output_done = Channel(Nil).new(1)
        error_done = Channel(Nil).new(1)
        status_done = Channel(Process::Status).new(1)
        spawn read_stream(
          provider,
          serial,
          command,
          process.output,
          output,
          output_done
        )
        spawn read_stream(
          provider,
          serial,
          command,
          process.error,
          errors,
          error_done
        )
        spawn { status_done.send(process.wait) }

        status : Process::Status? = nil
        timed_out = false
        if command.login?
          status = status_done.receive
        else
          select
          when result = status_done.receive
            status = result
          when timeout(@command_timeout)
            timed_out = true
            terminate(process)
            select
            when result = status_done.receive
              status = result
            when timeout(1.second)
            end
            process.output.close unless process.output.closed?
            process.error.close unless process.error.closed?
          end
        end
        output_done.receive
        error_done.receive
        if timed_out
          return fail_command(
            provider,
            serial,
            "#{backend.display_name} authentication check timed out."
          )
        end
        complete_command(
          provider,
          serial,
          command,
          status.not_nil!,
          output.to_s,
          errors.to_s
        )
      rescue error : File::Error | IO::Error
        fail_command(
          provider,
          serial,
          "Cannot start #{provider}: #{error.message}"
        )
      rescue error
        fail_command(
          provider,
          serial,
          error.message || "Authentication process failed."
        )
      end

      private def register_process(
        provider : String,
        serial : Int64,
        command : Command,
        process : Process,
      ) : Bool
        @mutex.synchronize do
          entry = entry!(provider)
          return true if @closed || entry.serial != serial

          entry.starting = false
          entry.process = process
          if command.login?
            entry.input = process.input
          end
          entry.cancel_requested
        end
      end

      private def read_stream(
        provider : String,
        serial : Int64,
        command : Command,
        stream : IO,
        collected : IO::Memory,
        done : Channel(Nil),
      ) : Nil
        buffer = Bytes.new(1024)
        loop do
          count = stream.read(buffer)
          break if count == 0

          text = String.new(buffer[0, count])
          remaining = OUTPUT_LIMIT - collected.bytesize
          collected << text.byte_slice(0, Math.min(text.bytesize, remaining)) if remaining > 0
          append_output(provider, serial, text) if command.login?
          # Pipes can remain continuously readable while a CLI draws a
          # spinner. Crystal fibers are cooperative, so yield even when the
          # descriptor never reaches EAGAIN or the desktop main loop starves.
          Fiber.yield
        end
      rescue IO::Error
      ensure
        done.send(nil)
      end

      private def append_output(
        provider : String,
        serial : Int64,
        text : String,
      ) : Nil
        snapshot = @mutex.synchronize do
          entry = entry!(provider)
          next nil unless entry.serial == serial

          entry.output += text
          if entry.output.bytesize > OUTPUT_LIMIT
            start = entry.output.bytesize - OUTPUT_LIMIT
            entry.output = entry.output.byte_slice(start, OUTPUT_LIMIT)
          end
          previous = {
            entry.login_url,
            entry.device_code,
            entry.needs_input,
          }
          parse_instructions(entry)
          current = {
            entry.login_url,
            entry.device_code,
            entry.needs_input,
          }
          # Raw CLI output is deliberately absent from the product UI. Emit
          # only when structured instructions change; otherwise animated CLI
          # output can enqueue thousands of redundant GTK updates.
          next nil if current == previous
          entry.snapshot
        end
        publish_state(snapshot) if snapshot
      end

      private def complete_command(
        provider : String,
        serial : Int64,
        command : Command,
        status : Process::Status,
        output : String,
        errors : String,
      ) : Nil
        refresh_after = false
        snapshot = @mutex.synchronize do
          entry = entry!(provider)
          next nil unless entry.serial == serial

          entry.process = nil
          entry.input = nil
          entry.starting = false
          canceled = entry.cancel_requested
          entry.cancel_requested = false

          if command.check?
            apply_status(entry, status, output, errors)
          elsif canceled
            entry.state = State::Checking
            entry.detail = "Sign-in canceled."
            refresh_after = true
          elsif status.success?
            entry.state = State::Checking
            entry.detail = nil
            refresh_after = true
          else
            entry.state = State::Failed
            entry.detail = command_error(output, errors, status)
            clear_instructions(entry)
          end
          entry.snapshot
        end
        return unless snapshot

        publish_state(snapshot)
        if refresh_after
          begin_command(provider, Command::Check, required: false)
        end
      end

      private def fail_command(
        provider : String,
        serial : Int64,
        message : String,
      ) : Nil
        snapshot = @mutex.synchronize do
          entry = entry!(provider)
          next nil unless entry.serial == serial

          entry.process = nil
          entry.input = nil
          entry.starting = false
          entry.cancel_requested = false
          entry.state = State::Failed
          entry.detail = message
          clear_instructions(entry)
          entry.snapshot
        end
        publish_state(snapshot) if snapshot
      end

      private def apply_status(
        entry : Entry,
        status : Process::Status,
        output : String,
        errors : String,
      ) : Nil
        case entry.backend.id
        when "claude"
          apply_claude_status(entry, status, output, errors)
        when "codex"
          apply_codex_status(entry, status, output, errors)
        when "claude-mode"
          apply_proxy_status(entry, status, output, errors)
        else
          entry.state = State::Failed
          entry.detail = "Unknown assistant."
        end
      end

      private def apply_claude_status(
        entry : Entry,
        status : Process::Status,
        output : String,
        errors : String,
      ) : Nil
        fields = JSON.parse(output).as_h?
        logged_in = fields.try(&.["loggedIn"]?.try(&.as_bool?)) == true
        entry.state = logged_in ? State::SignedIn : State::SignedOut
        method = fields.try(&.["authMethod"]?.try(&.as_s?))
        entry.detail = if logged_in
                         method && method != "none" ? "Signed in with #{method}." : "Signed in."
                       else
                         "Not signed in."
                       end
      rescue JSON::ParseException
        entry.state = State::Failed
        entry.detail = command_error(output, errors, status)
      end

      private def apply_codex_status(
        entry : Entry,
        status : Process::Status,
        output : String,
        errors : String,
      ) : Nil
        text = [output, errors].reject(&.empty?).join("\n")
        lines = text.lines.map(&.strip).reject do |line|
          line.empty? || line.starts_with?("WARNING:")
        end
        detail = lines.last? || "Could not read Codex login status."
        if detail.downcase.includes?("not logged in")
          entry.state = State::SignedOut
          entry.detail = "Not signed in."
        elsif status.success?
          entry.state = State::SignedIn
          entry.detail = detail
        else
          entry.state = State::Failed
          entry.detail = detail
        end
      end

      private def apply_proxy_status(
        entry : Entry,
        status : Process::Status,
        output : String,
        errors : String,
      ) : Nil
        text = [output, errors].reject(&.empty?).join("\n")
        lines = text.lines.map(&.strip).reject(&.empty?)
        detail = lines.last? || "Could not read Claude mode login status."
        if detail.downcase.includes?("not authenticated") ||
           detail.downcase.includes?("not logged in")
          entry.state = State::SignedOut
          entry.detail = "Not signed in."
        elsif status.success?
          entry.state = State::SignedIn
          entry.detail = detail
        else
          entry.state = State::Failed
          entry.detail = detail
        end
      end

      private def command_error(
        output : String,
        errors : String,
        status : Process::Status,
      ) : String
        text = errors.strip
        text = output.strip if text.empty?
        text = clean_output(text)
        if text.empty?
          "Authentication exited with status #{status}."
        elsif text.bytesize > DETAIL_LIMIT
          text.byte_slice(0, DETAIL_LIMIT)
        else
          text
        end
      end

      private def parse_instructions(entry : Entry) : Nil
        text = clean_output(entry.output)
        entry.login_url = extract_url(text)
        case entry.backend.id
        when "codex", "claude-mode"
          entry.device_code = text.match(DEVICE_CODE).try(&.[0])
          entry.needs_input = false
        when "claude"
          entry.device_code = nil
          entry.needs_input = text.downcase.includes?("paste code")
        end
      end

      private def extract_url(text : String) : String?
        text.match(URL).try(&.[0]).try do |url|
          url.rstrip(".,;:)]}")
        end
      end

      private def clean_output(text : String) : String
        text.gsub(ANSI, "")
      end

      private def clear_instructions(entry : Entry) : Nil
        entry.login_url = nil
        entry.device_code = nil
        entry.needs_input = false
      end

      private def command_arguments(
        backend : Backend,
        command : Command,
      ) : Array(String)
        executable = @resolver.call(backend.program)
        case {backend.id, command}
        when {"codex", Command::Check}
          [executable, "login", "status"]
        when {"codex", Command::Login}
          [executable, "login", "--device-auth"]
        when {"codex", Command::Logout}
          [executable, "logout"]
        when {"claude", Command::Check}
          [executable, "auth", "status", "--json"]
        when {"claude", Command::Login}
          [executable, "auth", "login"]
        when {"claude", Command::Logout}
          [executable, "auth", "logout"]
        when {"claude-mode", Command::Check}
          [executable, "codex", "auth", "status"]
        when {"claude-mode", Command::Login}
          [executable, "codex", "auth", "device"]
        when {"claude-mode", Command::Logout}
          [executable, "codex", "auth", "logout"]
        else
          raise Error.new("Unknown assistant: #{backend.id}")
        end
      end

      private def stop_process(
        provider : String,
        serial : Int64,
        process : Process,
      ) : Nil
        {% if flag?(:win32) %}
          process.terminate(graceful: false)
        {% else %}
          process.signal(Signal::INT)
          spawn do
            sleep 2.seconds
            still_running = @mutex.synchronize do
              entry = @entries[provider]?
              entry && entry.serial == serial && entry.process.same?(process)
            end
            process.terminate(graceful: false) if still_running
          rescue RuntimeError
          end
        {% end %}
      rescue RuntimeError
      end

      private def terminate(process : Process) : Nil
        process.terminate(graceful: false)
      rescue RuntimeError
      end

      private def publish_state(snapshot : Snapshot) : Nil
        @publisher.call("agent-auth-changed", snapshot.wire_fields)
      end

      private def entry!(provider : String) : Entry
        @entries[provider]? ||
          raise Error.new("Unknown assistant: #{provider}")
      end
    end
  end
end
