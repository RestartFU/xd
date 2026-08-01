require "./codex_protocol"
require "./environment"
require "./executable"

module Xd
  module Agent
    class CodexAppServer
      STDERR_LIMIT      = 8 * 1024
      OUTPUT_LINE_LIMIT = 8 * 1024 * 1024

      class Error < Exception
      end

      getter backend : Backend

      @process : Process
      @protocol : CodexProtocol
      @state_mutex = Mutex.new
      @write_mutex = Mutex.new
      @stderr_text = ""
      @closed = false
      @failed = false

      def initialize(
        @backend : Backend,
        @environment : Hash(String, String),
        version : String,
        arguments : Array(String)? = nil,
        @cancel_grace : Time::Span = 5.seconds,
        @output_line_limit : Int32 = OUTPUT_LINE_LIMIT,
      )
        command = arguments || @backend.build_argv(RunSpec.new(""))
        unless arguments
          command = command.dup
          command[0] = Executable.resolve(@backend.program)
        end

        @process = begin
          Process.new(
            command,
            env: @environment,
            clear_env: true,
            input: Process::Redirect::Pipe,
            output: Process::Redirect::Pipe,
            error: Process::Redirect::Pipe
          )
        rescue error : File::Error | IO::Error
          raise Error.new(
            "Cannot start #{@backend.display_name}: #{error.message}"
          )
        end
        @protocol = CodexProtocol.new(
          @backend,
          version,
          ->(line : String) { write_line(line) }
        )

        output_done = Channel(Nil).new
        error_done = Channel(Nil).new
        spawn read_output(@process.output, output_done)
        spawn read_error(@process.error, error_done)
        spawn monitor(output_done, error_done)
        begin
          @protocol.initialize_client
        rescue error : IO::Error
          @process.terminate(graceful: false)
          raise Error.new(
            "Cannot start #{@backend.display_name}: #{error.message}"
          )
        end
      end

      def start_turn(
        spec : RunSpec,
        allowed_environment_names : Array(String)?,
        on_event : Proc(Event, Nil),
        on_finished : Proc(Bool, String?, Nil),
      ) : CodexTurn
        turn = @protocol.start_turn(
          spec,
          allowed_environment_names,
          on_event,
          on_finished
        )
        turn.cancel_callback = -> { cancel(turn) }
        turn
      rescue error : IO::Error
        fail_server(error.message || "Cannot write to Codex app-server")
        raise Error.new(error.message)
      end

      def cancel(turn : CodexTurn) : Nil
        @protocol.cancel(turn)
        spawn do
          sleep @cancel_grace
          unless turn.finished
            @protocol.complete_cancel(turn)
            fail_server("Codex app-server ignored cancellation")
          end
        end
      rescue error : IO::Error
        fail_server(error.message || "Cannot write to Codex app-server")
      end

      def failed? : Bool
        @state_mutex.synchronize { @failed }
      end

      def close : Nil
        should_close = @state_mutex.synchronize do
          next false if @closed
          @closed = true
          true
        end
        return unless should_close

        @protocol.fail("Codex app-server stopped")
        @process.terminate(graceful: false)
      rescue RuntimeError
      end

      private def write_line(line : String) : Nil
        closed = @state_mutex.synchronize { @closed || @failed }
        raise IO::Error.new("Codex app-server is closed") if closed

        @write_mutex.synchronize do
          @process.input << line
          @process.input.flush
        end
      end

      private def read_output(
        output : IO,
        done : Channel(Nil),
      ) : Nil
        while line = output.gets('\n', @output_line_limit + 1, chomp: true)
          if line.bytesize > @output_line_limit
            fail_server("Codex sent an oversized response.")
            break
          end
          @protocol.receive_line(line)
          Fiber.yield
        end
      rescue IO::Error
      ensure
        done.send(nil)
      end

      private def read_error(
        error_stream : IO,
        done : Channel(Nil),
      ) : Nil
        buffer = Bytes.new(1024)
        while count = error_stream.read(buffer)
          break if count == 0
          chunk = String.new(buffer[0, count])
          @state_mutex.synchronize do
            @stderr_text += chunk
            if @stderr_text.bytesize > STDERR_LIMIT
              start = @stderr_text.bytesize - STDERR_LIMIT
              @stderr_text = @stderr_text.byte_slice(start, STDERR_LIMIT)
            end
          end
          Fiber.yield
        end
      rescue IO::Error
      ensure
        done.send(nil)
      end

      private def monitor(
        output_done : Channel(Nil),
        error_done : Channel(Nil),
      ) : Nil
        output_done.receive
        error_done.receive
        @process.wait

        closed, stderr = @state_mutex.synchronize do
          {@closed, @stderr_text.strip}
        end
        unless closed
          fail_server(
            stderr.empty? ? "Codex app-server closed unexpectedly" : stderr
          )
        end
      rescue error
        fail_server(error.message || "Codex app-server failed")
      end

      private def fail_server(message : String) : Nil
        should_fail = @state_mutex.synchronize do
          next false if @failed
          @failed = true
          true
        end
        return unless should_fail

        @protocol.fail(message)
        @process.terminate(graceful: false)
      rescue RuntimeError
      end
    end

    class CodexPool
      @servers = {} of String => CodexAppServer
      @mutex = Mutex.new

      def initialize(
        @backend : Backend = Catalog::CODEX,
        @version : String = "unknown",
      )
      end

      def start(
        spec : RunSpec,
        environment : Hash(String, String),
        secret_names : Array(String),
        on_event : Proc(Event, Nil),
        on_finished : Proc(Bool, String?, Nil),
        arguments : Array(String)? = nil,
      ) : CodexTurn
        executable = arguments.try(&.first?) ||
                     Executable.resolve(@backend.program)
        key = Environment.pool_key(executable, environment)

        server = @mutex.synchronize do
          current = @servers[key]?
          if !current || current.failed?
            current.try(&.close)
            current = CodexAppServer.new(
              @backend,
              environment,
              @version,
              arguments
            )
            @servers[key] = current
          end
          current
        end

        server.start_turn(
          spec,
          Environment.allowed_names(environment, secret_names),
          on_event,
          on_finished
        )
      end

      def cancel(turn : CodexTurn) : Nil
        turn.cancel
      end

      def size : Int32
        @mutex.synchronize do
          @servers.count { |_key, server| !server.failed? }
        end
      end

      def close : Nil
        servers = @mutex.synchronize do
          copy = @servers.values.dup
          @servers.clear
          copy
        end
        servers.each(&.close)
      end
    end
  end
end
