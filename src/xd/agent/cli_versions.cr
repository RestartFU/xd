require "json"
require "./catalog"
require "./environment"
require "./executable"

module Xd
  module Agent
    # Daemon-owned version reader for bundled assistant CLIs.
    class CliVersions
      OUTPUT_LIMIT = 4096

      class Error < Exception
      end

      enum State
        Idle
        Checking
        Failed

        def wire_name : String
          case self
          when Idle     then "idle"
          when Checking then "checking"
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

      @entries : Hash(String, Entry)
      @mutex = Mutex.new
      @closed = false

      def initialize(
        @publisher : Publisher = ->(_name : String, _fields : Hash(String, JSON::Any)) { },
        resolver : Resolver? = nil,
        environment : Hash(String, String)? = nil,
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
        Catalog.all.each { |backend| begin_check(backend.id) }
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

      private def begin_check(provider : String) : Nil
        serial = @mutex.synchronize do
          raise Error.new("Assistant version reader is stopping.") if @closed

          entry = entry!(provider)
          next nil if entry.process || entry.state.checking?

          entry.serial += 1
          entry.state = State::Checking
          entry.detail = nil
          publish_state(entry.snapshot)
          entry.serial
        end
        spawn run_check(provider, serial.not_nil!) if serial
      rescue error
        raise Error.new(error.message)
      end

      private def run_check(
        provider : String,
        serial : Int64,
      ) : Nil
        backend = @mutex.synchronize { entry!(provider).backend }
        executable = @resolver.call(backend.program)
        status, output = run(executable, ["--version"], provider, serial)
        unless status.success?
          return finish_failed(
            provider,
            serial,
            version_error(output, status)
          )
        end
        version = clean(output).lines.map(&.strip).reject(&.empty?).first?
        finish_success(provider, serial, version)
      rescue error : File::Error | IO::Error
        finish_failed(
          provider,
          serial,
          "Cannot read #{provider} version: #{error.message}"
        )
      rescue error
        finish_failed(
          provider,
          serial,
          error.message || "Cannot read assistant version."
        )
      end

      private def run(
        executable : String,
        arguments : Array(String),
        provider : String?,
        serial : Int64?,
      ) : {Process::Status, String}
        environment = @environment.dup
        environment["DISABLE_AUTOUPDATER"] = "1"
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
      ) : Nil
        snapshot = @mutex.synchronize do
          entry = entry!(provider)
          next nil unless entry.serial == serial

          entry.process = nil
          entry.version = version
          entry.detail = nil
          entry.state = State::Idle
          entry.snapshot
        end
        publish_state(snapshot) if snapshot
      end

      private def finish_failed(
        provider : String,
        serial : Int64,
        message : String,
      ) : Nil
        snapshot = @mutex.synchronize do
          entry = entry!(provider)
          next nil unless entry.serial == serial

          entry.process = nil
          entry.detail = message
          entry.state = State::Failed
          entry.snapshot
        end
        publish_state(snapshot) if snapshot
      end

      private def version_error(
        output : String,
        status : Process::Status,
      ) : String
        text = clean(output).strip
        text.empty? ? "Version check exited with status #{status.exit_code}." : text
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
