require "socket"
require "./environment"
require "./executable"

module Xd
  module Agent
    # One daemon-owned proxy serves every Claude-mode turn. It binds only to
    # loopback and starts lazily, so normal Codex users pay no process cost.
    class ClaudeProxy
      # The proxy discovers provider/model aliases before accepting requests.
      # A cold Codex installation can make that take longer than ten seconds.
      # Keep this below the daemon client's 30-second request timeout so a
      # failed start still returns a useful error instead of timing out.
      START_TIMEOUT = 25.seconds
      OUTPUT_LIMIT  = 8 * 1024

      class Error < Exception
      end

      @process : Process?
      @port : Int32?
      @output = ""
      @mutex = Mutex.new
      @output_mutex = Mutex.new

      def initialize(@environment : Hash(String, String) = Environment.host)
        @process = nil
        @port = nil
      end

      def endpoint : String
        port = @mutex.synchronize do
          if current = @port
            next current if reachable?(current)
            terminate(@process)
            @process = nil
            @port = nil
          end
          start_locked
        end
        "http://127.0.0.1:#{port}"
      end

      def close : Nil
        process = @mutex.synchronize do
          current = @process
          @process = nil
          @port = nil
          current
        end
        terminate(process)
      end

      private def start_locked : Int32
        port = free_port
        executable = Executable.resolve("claude-code-proxy")
        environment = @environment.dup
        environment["CCP_BIND_ADDRESS"] = "127.0.0.1"
        environment["CCP_ALIAS_PROVIDER"] = "codex"
        environment["NO_COLOR"] = "1"
        environment["TERM"] = "dumb"
        process = Process.new(
          [executable, "serve", "--port", port.to_s, "--no-monitor"],
          env: environment,
          clear_env: true,
          input: Process::Redirect::Close,
          output: Process::Redirect::Pipe,
          error: Process::Redirect::Pipe
        )
        @process = process
        @port = port
        @output_mutex.synchronize { @output = "" }
        spawn drain(process.output)
        spawn drain(process.error)

        deadline = Time.instant + START_TIMEOUT
        loop do
          begin
            socket = TCPSocket.new(
              "127.0.0.1",
              port,
              connect_timeout: 100.milliseconds
            )
            socket.close
            return port
          rescue Socket::Error | IO::Error
          end
          break if Time.instant >= deadline
          sleep 50.milliseconds
        end

        detail = @output_mutex.synchronize { @output.strip }
        terminate(process)
        @process = nil
        @port = nil
        suffix = detail.empty? ? "" : ": #{detail}"
        raise Error.new(
          "Claude mode proxy did not become reachable within " \
          "#{START_TIMEOUT.total_seconds.to_i} seconds#{suffix}"
        )
      rescue error : File::Error | IO::Error
        @process = nil
        @port = nil
        raise Error.new("Cannot start Claude mode proxy: #{error.message}")
      end

      private def free_port : Int32
        listener = TCPServer.new("127.0.0.1", 0)
        listener.local_address.as(Socket::IPAddress).port
      ensure
        listener.try(&.close)
      end

      private def reachable?(port : Int32) : Bool
        socket = TCPSocket.new(
          "127.0.0.1",
          port,
          connect_timeout: 100.milliseconds
        )
        socket.close
        true
      rescue Socket::Error | IO::Error
        false
      end

      private def drain(stream : IO) : Nil
        buffer = Bytes.new(1024)
        while count = stream.read(buffer)
          break if count == 0
          text = String.new(buffer[0, count])
          @output_mutex.synchronize do
            @output += text
            if @output.bytesize > OUTPUT_LIMIT
              start = @output.bytesize - OUTPUT_LIMIT
              @output = @output.byte_slice(start, OUTPUT_LIMIT)
            end
          end
          Fiber.yield
        end
      rescue IO::Error
      end

      private def terminate(process : Process?) : Nil
        process.try(&.terminate(graceful: false))
      rescue RuntimeError
      end
    end
  end
end
