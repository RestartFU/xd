require "../agent/environment"
require "../agent/executable"
require "./data"

module Xd
  module Voice
    record Transcription,
      text : String?,
      error : String?,
      cancelled : Bool

    class Transcriber
      OUTPUT_LIMIT = 1024 * 1024
      PROMPT       =
        "Software engineering, source code, commands, file paths, APIs, " \
        "libraries, acronyms, capitalization, and punctuation."

      alias Resolver = Proc(String)

      @process : Process?

      def initialize(
        @resolver : Resolver = -> { Agent::Executable.resolve("whisper") },
        @environment : Hash(String, String) = Agent::Environment.host,
      )
        @process = nil
        @mutex = Mutex.new
        @cancelled = Atomic(Bool).new(false)
      end

      def transcribe(
        wav : Bytes,
        model_path : String,
        &finished : Transcription -> Nil
      ) : Nil
        @cancelled.set(false)
        spawn do
          result = run(wav, model_path)
          finished.call(result)
        end
      end

      def cancel : Nil
        @cancelled.set(true)
        process = @mutex.synchronize { @process }
        return unless process

        {% if flag?(:win32) %}
          process.terminate(graceful: false)
        {% else %}
          process.signal(Signal::INT)
          spawn do
            sleep 2.seconds
            current = @mutex.synchronize { @process }
            process.terminate(graceful: false) if current.same?(process)
          rescue RuntimeError
          end
        {% end %}
      rescue RuntimeError
      end

      private def run(
        wav : Bytes,
        model_path : String,
      ) : Transcription
        recording : File? = nil
        recording_path : String? = nil
        recording = File.tempfile("xd-voice-", suffix: ".wav")
        recording_path = recording.path
        recording.write(wav)
        recording.close

        output = IO::Memory.new
        errors = IO::Memory.new
        process = Process.new(
          [
            @resolver.call,
            "--model", model_path,
            "--file", recording_path,
            "--threads", Math.min(System.cpu_count, 8).to_s,
            "--beam-size", "5",
            "--language", "auto",
            "--no-timestamps",
            "--no-gpu",
            "--flash-attn",
            "--no-prints",
            "--prompt", PROMPT,
          ],
          env: @environment,
          clear_env: true,
          input: Process::Redirect::Close,
          output: Process::Redirect::Pipe,
          error: Process::Redirect::Pipe
        )
        @mutex.synchronize { @process = process }

        output_done = Channel(Nil).new
        error_done = Channel(Nil).new
        spawn read_stream(process.output, output, output_done)
        spawn read_stream(process.error, errors, error_done)
        status = process.wait
        output_done.receive
        error_done.receive
        @mutex.synchronize { @process = nil }

        if @cancelled.get
          Transcription.new(nil, nil, true)
        elsif !status.success?
          message = errors.to_s.strip
          message = output.to_s.strip if message.empty?
          message = "Local voice transcription failed." if message.empty?
          Transcription.new(nil, message, false)
        else
          text = output.to_s.strip
          if text.empty?
            Transcription.new(nil, "No speech was detected.", false)
          else
            Transcription.new(text, nil, false)
          end
        end
      rescue error : File::Error | IO::Error
        if @cancelled.get
          Transcription.new(nil, nil, true)
        else
          Transcription.new(
            nil,
            error.message || "Local voice transcription failed.",
            false
          )
        end
      ensure
        @mutex.synchronize { @process = nil }
        recording.try(&.close)
        if path = recording_path
          File.delete(path) if File.exists?(path)
        end
      end

      private def read_stream(
        stream : IO,
        target : IO::Memory,
        done : Channel(Nil),
      ) : Nil
        buffer = Bytes.new(16 * 1024)
        loop do
          count = stream.read(buffer)
          break if count == 0
          remaining = OUTPUT_LIMIT - target.size
          target.write(buffer[0, Math.min(count, remaining)]) if remaining > 0
        end
      rescue IO::Error
      ensure
        stream.close
        done.send(nil)
      end
    end
  end
end
