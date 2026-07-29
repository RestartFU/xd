require "./parser"

module Xd
  module Agent
    class ExecSession
      STDERR_LIMIT = 8 * 1024

      class Error < Exception
      end

      getter backend : Backend

      @process : Process?
      @mutex = Mutex.new
      @finished = false
      @stopping = false
      @backend_error : String?
      @stderr_text = ""

      def initialize(
        @backend : Backend,
        @spec : RunSpec,
        @environment : Hash(String, String),
        @on_event : Proc(Event, Nil),
        @on_finished : Proc(Bool, String?, Nil),
        @arguments : Array(String)? = nil,
      )
        @parser = Parser.new(@backend)
        @parser.model = @spec.model || @backend.default_model
      end

      def start : Nil
        arguments = @arguments || @backend.build_argv(@spec)
        process = Process.new(
          arguments,
          env: @environment,
          clear_env: true,
          input: Process::Redirect::Pipe,
          output: Process::Redirect::Pipe,
          error: Process::Redirect::Pipe,
          chdir: @spec.workdir
        )
        process.input.close

        @mutex.synchronize do
          raise Error.new("Session is already running") if @process
          @process = process
        end

        output_done = Channel(Nil).new
        error_done = Channel(Nil).new
        spawn read_output(process.output, output_done)
        spawn read_error(process.error, error_done)
        spawn finish_process(process, output_done, error_done)
      rescue error : File::Error | IO::Error
        raise Error.new(
          "Cannot start #{@backend.display_name}: #{error.message}"
        )
      end

      def running? : Bool
        @mutex.synchronize { !!@process && !@finished }
      end

      def cancel : Nil
        process = @mutex.synchronize do
          return if @finished || @stopping
          @stopping = true
          @process
        end
        return unless process

        {% if flag?(:win32) %}
          process.terminate(graceful: false)
        {% else %}
          process.signal(Signal::INT)
          spawn do
            sleep 2.seconds
            kill = @mutex.synchronize { !@finished }
            process.terminate(graceful: false) if kill
          rescue RuntimeError
          end
        {% end %}
      rescue RuntimeError
        # Process crossed the exit boundary while cancellation was sent.
      end

      private def read_output(
        output : IO,
        done : Channel(Nil),
      ) : Nil
        while line = output.gets
          @parser.feed_line(line.chomp).each do |event|
            handle_event(event)
          end
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
          @mutex.synchronize do
            @stderr_text += chunk
            if @stderr_text.bytesize > STDERR_LIMIT
              start = @stderr_text.bytesize - STDERR_LIMIT
              @stderr_text = @stderr_text.byte_slice(start, STDERR_LIMIT)
            end
          end
        end
      rescue IO::Error
      ensure
        done.send(nil)
      end

      private def handle_event(event : Event) : Nil
        @mutex.synchronize do
          case event.type
          when EventType::Error
            @backend_error = event.text unless @stopping
          when EventType::Result
            @backend_error = nil
          else
          end
        end
        @on_event.call(event)
      end

      private def finish_process(
        process : Process,
        output_done : Channel(Nil),
        error_done : Channel(Nil),
      ) : Nil
        output_done.receive
        error_done.receive
        status = process.wait

        stopping, backend_error, stderr = @mutex.synchronize do
          {@stopping, @backend_error, @stderr_text.strip}
        end
        if stopping
          finish(true, nil)
        elsif status.success? && !backend_error
          finish(true, nil)
        else
          message = stderr.empty? ? backend_error : stderr
          message ||= "Agent exited with status #{status.exit_code}"
          finish(false, message)
        end
      rescue error
        finish(false, error.message || "Agent process failed")
      end

      private def finish(success : Bool, message : String?) : Nil
        callback = @mutex.synchronize do
          return if @finished
          @finished = true
          @on_finished
        end
        callback.call(success, message)
      end
    end
  end
end
