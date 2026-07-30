require "json"
require "./catalog"
require "./environment"
require "./executable"

module Xd
  module Agent
    # Daemon-owned updater for official bundled assistant CLIs.
    #
    # Update commands can take time and replace their executable, so every
    # operation runs in a fiber and publishes structured state over the same
    # Unix/TLS event bus as authentication.
    class Updates
      OUTPUT_LIMIT = 4096

      class Error < Exception
      end

      enum State
        Idle
        Checking
        Updating
        Updated
        Failed

        def wire_name : String
          case self
          when Idle     then "idle"
          when Checking then "checking"
          when Updating then "updating"
          when Updated  then "updated"
          when Failed   then "failed"
          else               "idle"
          end
        end
      end

      record Snapshot,
        provider : String,
        display_name : String,
        state : State,
        version : String?,
        detail : String? do
        def wire_fields : Hash(String, JSON::Any)
          fields = {
            "provider"     => JSON::Any.new(provider),
            "display_name" => JSON::Any.new(display_name),
            "state"        => JSON::Any.new(state.wire_name),
          }
          fields["version"] = JSON::Any.new(value) if value = version
          fields["detail"] = JSON::Any.new(value) if value = detail
          fields
        end
      end

      private class Entry
        getter backend : Backend
        property state = State::Idle
        property version : String?
        property detail : String?
        property process : Process?
        property serial = 0_i64

        def initialize(@backend : Backend)
          @version = nil
          @detail = nil
          @process = nil
        end

        def snapshot : Snapshot
          Snapshot.new(
            @backend.id,
            @backend.display_name,
            @state,
            @version,
            @detail
          )
        end
      end

      alias Publisher = Proc(String, Hash(String, JSON::Any), Nil)
      alias Resolver = Proc(String, String)
      alias BeginUpdate = Proc(String, Nil)
      alias FinishUpdate = Proc(String, Bool, Nil)

      @entries : Hash(String, Entry)
      @mutex = Mutex.new
      @closed = false

      def initialize(
        @publisher : Publisher = ->(_name : String, _fields : Hash(String, JSON::Any)) { },
        resolver : Resolver? = nil,
        environment : Hash(String, String)? = nil,
        @begin_update : BeginUpdate = ->(_provider : String) { },
        @finish_update : FinishUpdate = ->(_provider : String, _success : Bool) { },
      )
        @resolver = resolver || ->(name : String) { Executable.resolve(name) }
        @environment = environment || Environment.host
        @entries = Catalog.all.to_h do |backend|
          {backend.id, Entry.new(backend)}
        end
      end

      def snapshots : Array(Snapshot)
        @mutex.synchronize do
          Catalog.all.compact_map do |backend|
            @entries[backend.id]?.try(&.snapshot)
          end
        end
      end

      def refresh : Nil
        Catalog.all.each { |backend| begin_command(backend.id, false) }
      end

      def update_all : Nil
        Catalog.all.each { |backend| begin_command(backend.id, true) }
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

      private def begin_command(provider : String, update : Bool) : Nil
        serial = @mutex.synchronize do
          raise Error.new("Assistant updater is stopping.") if @closed

          entry = entry!(provider)
          next nil if entry.process ||
                      entry.state.checking? ||
                      entry.state.updating?

          @begin_update.call(provider) if update
          entry.serial += 1
          entry.state = update ? State::Updating : State::Checking
          entry.detail = nil
          publish_state(entry.snapshot)
          entry.serial
        end
        spawn run_command(provider, serial.not_nil!, update) if serial
      rescue error
        raise Error.new(error.message)
      end

      private def run_command(
        provider : String,
        serial : Int64,
        update : Bool,
      ) : Nil
        backend = @mutex.synchronize { entry!(provider).backend }
        executable = @resolver.call(backend.program)
        before = version(executable)
        if update
          status, output = run(executable, ["update"], provider, serial)
          unless status.success?
            return finish_failed(
              provider,
              serial,
              command_error(output, status),
              update
            )
          end
        end
        after = version(executable)
        detail = if update
                   before == after ?
                     "Already up to date." :
                     "Updated from #{before || "unknown"} to #{after || "unknown"}."
                 end
        finish_success(provider, serial, after || before, detail, update)
      rescue error : File::Error | IO::Error
        finish_failed(
          provider,
          serial,
          "Cannot update #{provider}: #{error.message}",
          update
        )
      rescue error
        finish_failed(
          provider,
          serial,
          error.message || "Assistant update failed.",
          update
        )
      end

      private def version(executable : String) : String?
        status, output = run(executable, ["--version"], nil, nil)
        return unless status.success?

        clean(output).lines.map(&.strip).reject(&.empty?).first?
      end

      private def run(
        executable : String,
        arguments : Array(String),
        provider : String?,
        serial : Int64?,
      ) : {Process::Status, String}
        environment = @environment.dup
        environment.delete("DISABLE_AUTOUPDATER")
        environment["NO_COLOR"] = "1"
        environment["TERM"] = "dumb"
        output = IO::Memory.new
        errors = IO::Memory.new
        process = Process.new(
          [executable] + arguments,
          env: environment,
          clear_env: true,
          input: Process::Redirect::Close,
          output: Process::Redirect::Pipe,
          error: Process::Redirect::Pipe
        )
        if provider && serial
          @mutex.synchronize do
            entry = entry!(provider)
            entry.process = process if entry.serial == serial
          end
        end

        output_done = Channel(Nil).new
        error_done = Channel(Nil).new
        spawn read_stream(process.output, output, output_done)
        spawn read_stream(process.error, errors, error_done)
        output_done.receive
        error_done.receive
        status = process.wait
        {status, limited([output.to_s, errors.to_s].join("\n"))}
      end

      private def read_stream(
        stream : IO,
        collected : IO::Memory,
        done : Channel(Nil),
      ) : Nil
        buffer = Bytes.new(1024)
        loop do
          count = stream.read(buffer)
          break if count == 0
          collected.write(buffer[0, count])
          Fiber.yield
        end
      rescue IO::Error
      ensure
        done.send(nil)
      end

      private def finish_success(
        provider : String,
        serial : Int64,
        version : String?,
        detail : String?,
        update : Bool,
      ) : Nil
        snapshot = @mutex.synchronize do
          entry = entry!(provider)
          next nil unless entry.serial == serial

          entry.process = nil
          entry.version = version
          entry.detail = detail
          entry.state = update ? State::Updated : State::Idle
          entry.snapshot
        end
        @finish_update.call(provider, true) if update && snapshot
        publish_state(snapshot) if snapshot
      end

      private def finish_failed(
        provider : String,
        serial : Int64,
        message : String,
        update : Bool,
      ) : Nil
        snapshot = @mutex.synchronize do
          entry = entry!(provider)
          next nil unless entry.serial == serial

          entry.process = nil
          entry.detail = message
          entry.state = State::Failed
          entry.snapshot
        end
        @finish_update.call(provider, false) if update && snapshot
        publish_state(snapshot) if snapshot
      end

      private def command_error(
        output : String,
        status : Process::Status,
      ) : String
        text = clean(output).strip
        text.empty? ?
          "Update exited with status #{status.exit_code}." :
          text
      end

      private def clean(text : String) : String
        text.gsub(/\e\[[0-?]*[ -\/]*[@-~]/, "")
      end

      private def limited(text : String) : String
        return text if text.bytesize <= OUTPUT_LIMIT
        text.byte_slice(text.bytesize - OUTPUT_LIMIT, OUTPUT_LIMIT)
      end

      private def publish_state(snapshot : Snapshot) : Nil
        @publisher.call("agent-cli-changed", snapshot.wire_fields)
      end

      private def entry!(provider : String) : Entry
        @entries[provider]? ||
          raise Error.new("Unknown assistant: #{provider}")
      end
    end
  end
end
