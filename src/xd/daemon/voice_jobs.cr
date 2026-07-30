require "base64"
require "../voice/model"
require "../voice/transcriber"

module Xd
  module Daemon
    # Daemon-owned speech model and transcription jobs.
    #
    # Microphone capture remains on the GTK client. Everything after capture
    # runs on the selected chat's daemon, regardless of Unix or TLS transport.
    class VoiceJobs
      class Error < Exception
      end

      MAX_AUDIO_BYTES = 64 * 1024 * 1024

      alias Publisher = Proc(
        String,
        Hash(String, JSON::Any),
        UInt64,
        Nil,
      )
      alias ModelFactory = Proc(Voice::Model)
      alias TranscriberFactory = Proc(Voice::Transcriber)

      private record JobKey, owner : UInt64, token : String

      @download : JobKey?

      private class Job
        def initialize(
          @model : Voice::Model? = nil,
          @transcriber : Voice::Transcriber? = nil,
        )
        end

        def cancel : Nil
          @model.try(&.cancel)
          @transcriber.try(&.cancel)
        end
      end

      def initialize(
        @publish : Publisher,
        @model_factory : ModelFactory = -> { Voice::Model.new },
        @transcriber_factory : TranscriberFactory = -> { Voice::Transcriber.new },
      )
        @jobs = {} of JobKey => Job
        @download = nil
        @closed = false
        @mutex = Mutex.new
      end

      def model_available? : Bool
        !@model_factory.call.find.nil?
      end

      def download(owner : UInt64, token : String) : Nil
        key = JobKey.new(owner, validate_token(token))
        model = @model_factory.call
        job = Job.new(model: model)

        @mutex.synchronize do
          raise Error.new("Voice service is closed.") if @closed
          raise Error.new("That voice request is already running.") if @jobs.has_key?(key)
          if @download
            raise Error.new("Speech model download is already running.")
          end
          @jobs[key] = job
          @download = key
        end

        spawn do
          begin
            model.ensure_available do |progress|
              publish_if_current(key, job, "downloading", {
                "progress" => JSON::Any.new(progress.to_i64),
              })
            end
            finish(key, job, "ready")
          rescue Voice::Cancelled
            finish(key, job, "cancelled")
          rescue error : Voice::Error
            finish(
              key,
              job,
              "error",
              error.message || "Speech model download failed."
            )
          end
        end
      end

      def transcribe(
        owner : UInt64,
        token : String,
        encoded_audio : String,
      ) : Nil
        if encoded_audio.bytesize > MAX_AUDIO_BYTES * 2
          raise Error.new("Voice recording is too large.")
        end
        audio = Base64.decode(encoded_audio)
        if audio.empty?
          raise Error.new("Voice recording is empty.")
        end
        if audio.size > MAX_AUDIO_BYTES
          raise Error.new("Voice recording is too large.")
        end

        key = JobKey.new(owner, validate_token(token))
        transcriber = @transcriber_factory.call
        job = Job.new(transcriber: transcriber)
        model_path = @model_factory.call.find ||
                     raise Error.new(
                       "Speech model is not installed on this machine."
                     )

        @mutex.synchronize do
          raise Error.new("Voice service is closed.") if @closed
          raise Error.new("That voice request is already running.") if @jobs.has_key?(key)
          @jobs[key] = job
        end

        transcriber.transcribe(audio, model_path) do |result|
          if result.cancelled
            finish(key, job, "cancelled")
          elsif text = result.text
            finish(key, job, "transcribed", fields: {
              "text" => JSON::Any.new(text),
            })
          else
            finish(
              key,
              job,
              "error",
              result.error || "Voice transcription failed."
            )
          end
        end
      rescue error : Base64::Error
        raise Error.new("Voice recording is not valid base64.")
      end

      def cancel(owner : UInt64, token : String) : Bool
        key = JobKey.new(owner, validate_token(token))
        job = @mutex.synchronize do
          found = @jobs.delete(key)
          @download = nil if @download == key
          found
        end
        job.try(&.cancel)
        !job.nil?
      end

      def close : Nil
        jobs = @mutex.synchronize do
          return if @closed

          @closed = true
          @download = nil
          values = @jobs.values
          @jobs.clear
          values
        end
        jobs.each(&.cancel)
      end

      private def validate_token(token : String) : String
        cleaned = token.strip
        unless !cleaned.empty? && cleaned.bytesize <= 128
          raise Error.new("Voice request needs a valid token.")
        end
        cleaned
      end

      private def publish_if_current(
        key : JobKey,
        job : Job,
        state : String,
        fields = {} of String => JSON::Any,
      ) : Nil
        current = @mutex.synchronize { @jobs[key]?.try(&.same?(job)) }
        publish(key, state, fields) if current
      end

      private def finish(
        key : JobKey,
        job : Job,
        state : String,
        error : String? = nil,
        fields = {} of String => JSON::Any,
      ) : Nil
        current = @mutex.synchronize do
          if @jobs[key]?.try(&.same?(job))
            @jobs.delete(key)
            @download = nil if @download == key
            true
          else
            false
          end
        end
        return unless current

        fields["error"] = JSON::Any.new(error) if error
        publish(key, state, fields)
      end

      private def publish(
        key : JobKey,
        state : String,
        fields : Hash(String, JSON::Any),
      ) : Nil
        body = {
          "request" => JSON::Any.new(key.token),
          "state"   => JSON::Any.new(state),
        }
        fields.each { |name, value| body[name] = value }
        @publish.call("voice", body, key.owner)
      end
    end
  end
end
