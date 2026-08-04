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

      MAX_AUDIO_BYTES       = 64 * 1024 * 1024
      TRANSCRIPTION_TIMEOUT = 5.minutes
      PARTIAL_MIN_BYTES     = Voice::SAMPLE_RATE.to_i * 2
      PARTIAL_STEP_BYTES    = Voice::SAMPLE_RATE.to_i

      alias Publisher = Proc(
        String,
        Hash(String, JSON::Any),
        UInt64,
        Nil,
      )
      alias ModelFactory = Proc(Voice::Model)
      alias TranscriberFactory = Proc(Voice::Transcriber)

      private record JobKey, owner : UInt64, token : String
      private record DownloadSnapshot,
        progress : Int32,
        outcome : DownloadOutcome,
        error : String?

      private enum DownloadOutcome
        Running
        Ready
        Cancelled
        Failed
      end

      @download : JobKey?

      # HTTP, hashing, and file writes run on an OS thread. Only this small
      # snapshot crosses back to the daemon scheduler, where events are
      # published. That keeps both GTK and socket fibers responsive without
      # calling scheduler-owned subscribers from a foreign thread.
      private class DownloadState
        @error : String?

        def initialize
          @progress = -1
          @outcome = DownloadOutcome::Running
          @error = nil
          @mutex = Mutex.new
        end

        def report(progress : Int32) : Nil
          @mutex.synchronize { @progress = progress }
        end

        def finish(
          outcome : DownloadOutcome,
          error : String? = nil,
        ) : Nil
          @mutex.synchronize do
            @outcome = outcome
            @error = error
          end
        end

        def snapshot : DownloadSnapshot
          @mutex.synchronize do
            DownloadSnapshot.new(@progress, @outcome, @error)
          end
        end
      end

      private class Job
        @done = Channel(Nil).new(1)

        getter pcm : IO::Memory?
        property in_flight : Bool
        property last_started_size : Int32
        property final_wav : Bytes?

        def initialize(
          @model : Voice::Model? = nil,
          streaming : Bool = false,
        )
          @pcm = streaming ? IO::Memory.new : nil
          @in_flight = false
          @last_started_size = 0
          @final_wav = nil
        end

        def cancel : Nil
          @model.try(&.cancel)
          complete
        end

        def complete : Nil
          select
          when @done.send(nil)
          else
          end
        end

        def timed_out?(duration : Time::Span) : Bool
          select
          when @done.receive
            false
          when timeout(duration)
            true
          end
        end
      end

      def initialize(
        @publish : Publisher,
        @model_factory : ModelFactory = -> { Voice::Model.new },
        @transcriber_factory : TranscriberFactory = -> { Voice::Transcriber.new },
        @transcription_timeout : Time::Span = TRANSCRIPTION_TIMEOUT,
      )
        @jobs = {} of JobKey => Job
        @download = nil
        @closed = false
        @mutex = Mutex.new
        @transcriber = @transcriber_factory.call
      end

      def model_available? : Bool
        path = @model_factory.call.find
        @transcriber.warm(path) if path
        !path.nil?
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

        state = DownloadState.new
        Fiber::ExecutionContext::Isolated.new("xd speech model") do
          begin
            model.ensure_available do |progress|
              state.report(progress)
            end
            state.finish(DownloadOutcome::Ready)
          rescue Voice::Cancelled
            state.finish(DownloadOutcome::Cancelled)
          rescue error
            state.finish(
              DownloadOutcome::Failed,
              error.message || "Speech model download failed."
            )
          end
        end
        spawn monitor_download(key, job, state)
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
        job = Job.new
        model_path = @model_factory.call.find ||
                     raise Error.new(
                       "Speech model is not installed on this machine."
                     )

        @mutex.synchronize do
          raise Error.new("Voice service is closed.") if @closed
          raise Error.new("That voice request is already running.") if @jobs.has_key?(key)
          @jobs[key] = job
        end

        @transcriber.transcribe(audio, model_path) do |result|
          transcription_finished(key, job, result)
        end
        spawn monitor_transcription(key, job)
      rescue error : Base64::Error
        raise Error.new("Voice recording is not valid base64.")
      end

      def start_stream(owner : UInt64, token : String) : Nil
        key = JobKey.new(owner, validate_token(token))
        model_path = @model_factory.call.find ||
                     raise Error.new(
                       "Speech model is not installed on this machine."
                     )
        job = Job.new(streaming: true)
        @mutex.synchronize do
          raise Error.new("Voice service is closed.") if @closed
          raise Error.new("That voice request is already running.") if @jobs.has_key?(key)
          @jobs[key] = job
        end
        @transcriber.warm(model_path)
        spawn monitor_transcription(key, job)
      end

      def append_stream(
        owner : UInt64,
        token : String,
        encoded_audio : String,
      ) : Nil
        audio = decode_audio(encoded_audio, "Voice audio chunk")
        key = JobKey.new(owner, validate_token(token))
        inference = @mutex.synchronize do
          job = stream_job(key)
          pcm = job.pcm.not_nil!
          if pcm.size + audio.size > MAX_AUDIO_BYTES
            raise Error.new("Voice recording is too large.")
          end
          pcm.write(audio)
          next_inference(job)
        end
        start_inference(key, inference) if inference
      end

      def finish_stream(
        owner : UInt64,
        token : String,
        encoded_audio : String,
      ) : Nil
        audio = decode_audio(encoded_audio, "Voice recording")
        key = JobKey.new(owner, validate_token(token))
        inference = @mutex.synchronize do
          job = stream_job(key)
          job.final_wav = audio
          next_inference(job)
        end
        start_inference(key, inference) if inference
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
        @transcriber.close
      end

      private def decode_audio(encoded : String, label : String) : Bytes
        if encoded.bytesize > MAX_AUDIO_BYTES * 2
          raise Error.new("#{label} is too large.")
        end
        audio = Base64.decode(encoded)
        raise Error.new("#{label} is empty.") if audio.empty?
        raise Error.new("#{label} is too large.") if audio.size > MAX_AUDIO_BYTES
        audio
      rescue Base64::Error
        raise Error.new("#{label} is not valid base64.")
      end

      private def stream_job(key : JobKey) : Job
        job = @jobs[key]? || raise Error.new("Voice stream is not running.")
        raise Error.new("Voice request is not a stream.") unless job.pcm
        job
      end

      private def next_inference(job : Job) : {Bytes, Bool}?
        return if job.in_flight

        if wav = job.final_wav
          job.in_flight = true
          return {wav, true}
        end

        pcm = job.pcm || return
        return if pcm.size < PARTIAL_MIN_BYTES
        return if pcm.size - job.last_started_size < PARTIAL_STEP_BYTES

        job.in_flight = true
        job.last_started_size = pcm.size
        {Voice::Data.wav_from_s16(pcm.to_slice), false}
      end

      private def start_inference(
        key : JobKey,
        inference : {Bytes, Bool},
      ) : Nil
        wav, final = inference
        model_path = @model_factory.call.find
        unless model_path
          if job = @mutex.synchronize { @jobs[key]? }
            finish(key, job, "error", "Speech model is not installed on this machine.")
          end
          return
        end

        @transcriber.transcribe(wav, model_path) do |result|
          inference_finished(key, final, result)
        end
      end

      private def inference_finished(
        key : JobKey,
        final : Bool,
        result : Voice::Transcription,
      ) : Nil
        job : Job? = nil
        next_run : {Bytes, Bool}? = nil
        @mutex.synchronize do
          job = @jobs[key]?
          if current = job
            current.in_flight = false
            next_run = next_inference(current) unless final
          end
        end
        current = job || return
        text = result.text

        if result.cancelled
          finish(key, current, "cancelled")
        elsif text
          if final
            finish(key, current, "transcribed", fields: {
              "text" => JSON::Any.new(text),
            })
          else
            publish_if_current(key, current, "partial", {
              "text" => JSON::Any.new(text),
            })
            start_inference(key, next_run) if next_run
          end
        elsif !final
          # Silence and unstable early windows are normal while someone is
          # still talking. The authoritative final pass reports real errors.
          start_inference(key, next_run) if next_run
        else
          finish(
            key,
            current,
            "error",
            result.error || "Voice transcription failed."
          )
        end
      end

      private def transcription_finished(
        key : JobKey,
        job : Job,
        result : Voice::Transcription,
      ) : Nil
        if result.cancelled
          finish(key, job, "cancelled")
        elsif result.text
          finish(key, job, "transcribed", fields: {
            "text" => JSON::Any.new(result.text.not_nil!),
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

      private def validate_token(token : String) : String
        cleaned = token.strip
        unless !cleaned.empty? && cleaned.bytesize <= 128
          raise Error.new("Voice request needs a valid token.")
        end
        cleaned
      end

      private def monitor_download(
        key : JobKey,
        job : Job,
        state : DownloadState,
      ) : Nil
        last_progress = -1
        loop do
          return unless current?(key, job)

          snapshot = state.snapshot
          if snapshot.progress != last_progress
            publish_if_current(key, job, "downloading", {
              "progress" => JSON::Any.new(snapshot.progress.to_i64),
            })
            last_progress = snapshot.progress
          end

          case snapshot.outcome
          when .running?
            sleep 50.milliseconds
          when .ready?
            finish(key, job, "ready")
            return
          when .cancelled?
            finish(key, job, "cancelled")
            return
          when .failed?
            finish(
              key,
              job,
              "error",
              snapshot.error || "Speech model download failed."
            )
            return
          end
        end
      end

      private def current?(key : JobKey, job : Job) : Bool
        @mutex.synchronize { @jobs[key]?.try(&.same?(job)) || false }
      end

      private def publish_if_current(
        key : JobKey,
        job : Job,
        state : String,
        fields = {} of String => JSON::Any,
      ) : Nil
        publish(key, state, fields) if current?(key, job)
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

        job.complete
        fields["error"] = JSON::Any.new(error) if error
        publish(key, state, fields)
      end

      private def monitor_transcription(key : JobKey, job : Job) : Nil
        return unless job.timed_out?(@transcription_timeout)

        current = @mutex.synchronize do
          if @jobs[key]?.try(&.same?(job))
            @jobs.delete(key)
            true
          else
            false
          end
        end
        return unless current

        job.cancel
        publish(key, "error", {
          "error" => JSON::Any.new(
            "Voice transcription timed out. Try a shorter recording."
          ),
        })
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
