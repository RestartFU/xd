require "http"
require "socket"
require "../agent/environment"
require "../agent/executable"
require "./data"

module Xd
  module Voice
    record Transcription,
      text : String?,
      error : String?,
      cancelled : Bool

    # One daemon-wide whisper.cpp server. The model remains mapped between
    # requests, so partial and final passes pay only inference cost.
    class Transcriber
      MAX_THREADS   = 4
      START_TIMEOUT = 30.seconds
      READ_TIMEOUT  = 2.minutes
      PROMPT        =
        "Software engineering, source code, commands, file paths, APIs, " \
        "libraries, acronyms, capitalization, and punctuation."

      alias Resolver = Proc(String)

      @process : Process?
      @port : Int32?
      @model_path : String?

      def initialize(
        @resolver : Resolver = -> { Agent::Executable.resolve("whisper-server") },
        @environment : Hash(String, String) = Agent::Environment.host,
      )
        @process = nil
        @port = nil
        @model_path = nil
        @closed = Atomic(Bool).new(false)
        @state_mutex = Mutex.new
        @request_mutex = Mutex.new
      end

      # Start loading before the first audio window arrives. Repeated calls
      # are cheap and collapse behind the same request mutex.
      def warm(model_path : String) : Nil
        spawn do
          @request_mutex.synchronize { ensure_server(model_path) }
        rescue
          # A real transcription reports startup failures to the client.
        end
      end

      def transcribe(
        wav : Bytes,
        model_path : String,
        &finished : Transcription -> Nil
      ) : Nil
        spawn do
          result = @request_mutex.synchronize do
            run(wav, model_path)
          end
          finished.call(result)
        end
      end

      def close : Nil
        return if @closed.swap(true)
        stop_server
      end

      # Individual jobs are abandoned by VoiceJobs without tearing down the
      # shared model. Their late callback is ignored by job identity.
      def cancel : Nil
      end

      def self.thread_count(cpu_count : Int32 = System.cpu_count) : Int32
        Math.min(Math.max(cpu_count // 2, 1), MAX_THREADS)
      end

      def self.server_arguments(
        executable : String,
        model_path : String,
        port : Int32,
      ) : Array(String)
        [
          executable,
          "--model", model_path,
          "--host", "127.0.0.1",
          "--port", port.to_s,
          "--threads", thread_count.to_s,
          "--best-of", "1",
          "--beam-size", "-1",
          "--language", "en",
          "--no-timestamps",
          "--no-gpu",
          "--flash-attn",
          "--prompt", PROMPT,
        ]
      end

      private def run(wav : Bytes, model_path : String) : Transcription
        port = ensure_server(model_path)
        text = request(port, wav).strip
        if text.empty?
          Transcription.new(nil, "No speech was detected.", false)
        else
          Transcription.new(text, nil, false)
        end
      rescue error
        stop_server
        Transcription.new(
          nil,
          error.message || "Local voice transcription failed.",
          false
        )
      end

      private def ensure_server(model_path : String) : Int32
        raise Error.new("Voice service is closed.") if @closed.get

        existing = @state_mutex.synchronize do
          @port if @process && @model_path == model_path
        end
        return existing if existing

        stop_server
        port = available_port
        arguments = self.class.server_arguments(
          @resolver.call,
          model_path,
          port
        )
        process = Process.new(
          arguments.shift,
          arguments,
          env: @environment,
          clear_env: true,
          input: Process::Redirect::Close,
          output: Process::Redirect::Pipe,
          error: Process::Redirect::Pipe
        )
        spawn drain(process.output)
        spawn drain(process.error)
        @state_mutex.synchronize do
          @process = process
          @port = port
          @model_path = nil
        end

        deadline = Time.instant + START_TIMEOUT
        loop do
          raise Error.new("Local voice recognizer took too long to start.") \
            if Time.instant >= deadline
          begin
            socket = TCPSocket.new("127.0.0.1", port)
            socket.close
            @state_mutex.synchronize { @model_path = model_path }
            return port
          rescue IO::Error | Socket::Error
          end
          sleep 50.milliseconds
        end
      rescue error
        stop_server
        raise error
      end

      private def request(port : Int32, wav : Bytes) : String
        body = IO::Memory.new
        content_type = ""
        HTTP::FormData.build(body) do |form|
          content_type = form.content_type
          form.field("response_format", "text")
          form.field("temperature", "0.0")
          form.field("temperature_inc", "0.0")
          metadata = HTTP::FormData::FileMetadata.new(
            filename: "speech.wav",
            size: wav.size.to_u64
          )
          form.file(
            "file",
            IO::Memory.new(wav),
            metadata,
            HTTP::Headers{"Content-Type" => "audio/wav"}
          )
        end
        body.rewind

        client = HTTP::Client.new("127.0.0.1", port)
        client.connect_timeout = 2.seconds
        client.read_timeout = READ_TIMEOUT
        response = client.post(
          "/inference",
          HTTP::Headers{"Content-Type" => content_type},
          body
        )
        unless response.status.success?
          raise Error.new(
            "Local voice recognizer returned HTTP #{response.status_code}."
          )
        end
        response.body
      ensure
        client.try(&.close)
      end

      private def available_port : Int32
        socket = TCPServer.new("127.0.0.1", 0)
        socket.local_address.as(Socket::IPAddress).port
      ensure
        socket.try(&.close)
      end

      private def stop_server : Nil
        process = @state_mutex.synchronize do
          current = @process
          @process = nil
          @port = nil
          @model_path = nil
          current
        end
        return unless process
        begin
          process.terminate(graceful: false)
        rescue RuntimeError
        end
        spawn do
          process.wait
        rescue RuntimeError
        end
      end

      private def drain(stream : IO) : Nil
        buffer = Bytes.new(16 * 1024)
        loop do
          count = stream.read(buffer)
          break if count == 0
          Fiber.yield
        end
      rescue IO::Error
      ensure
        stream.close
      end
    end
  end
end
