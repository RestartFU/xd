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

      alias Publisher = Proc(
        String,
        Hash(String, JSON::Any),
        UInt64,
        Nil,
      )
      alias ModelFactory = Proc(Voice::Model)
      alias TranscriberFactory = Proc(Voice::Transcriber)
      alias AgentTranscriber = Proc(
        String,
        String,
        String,
        Proc(Voice::Transcription, Nil),
        Proc(Nil),
      )

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
        @cancel_callback : Proc(Nil)?
        @temporary_path : String?
        @stopped = false
        @state_mutex = Mutex.new

        def initialize(
          @model : Voice::Model? = nil,
          @transcriber : Voice::Transcriber? = nil,
          @temporary_path : String? = nil,
        )
          @cancel_callback = nil
        end

        def cancel_callback=(callback : Proc(Nil)) : Proc(Nil)
          cancel_now = @state_mutex.synchronize do
            if @stopped
              true
            else
              @cancel_callback = callback
              false
            end
          end
          callback.call if cancel_now
          callback
        end

        def cancel : Nil
          callback : Proc(Nil)? = nil
          path : String? = nil
          active = @state_mutex.synchronize do
            next false if @stopped

            @stopped = true
            callback = @cancel_callback
            @cancel_callback = nil
            path = @temporary_path
            @temporary_path = nil
            true
          end
          return unless active

          @model.try(&.cancel)
          @transcriber.try(&.cancel)
          callback.try(&.call)
          cleanup(path)
          signal_done
        end

        def complete : Nil
          path : String? = nil
          active = @state_mutex.synchronize do
            next false if @stopped

            @stopped = true
            @cancel_callback = nil
            path = @temporary_path
            @temporary_path = nil
            true
          end
          return unless active

          cleanup(path)
          signal_done
        end

        private def signal_done : Nil
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

        private def cleanup(path : String?) : Nil
          File.delete?(path) if path
        rescue File::Error
        end
      end

      def initialize(
        @publish : Publisher,
        @model_factory : ModelFactory = -> { Voice::Model.new },
        @transcriber_factory : TranscriberFactory = -> { Voice::Transcriber.new },
        @agent_transcriber : AgentTranscriber? = nil,
        @transcription_timeout : Time::Span = TRANSCRIPTION_TIMEOUT,
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
        chat_id : String,
        provider : String = "local",
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

        case provider
        when "local"
          transcribe_local(owner, token, audio)
        when "codex"
          transcribe_with_agent(owner, token, chat_id, provider, audio)
        else
          raise Error.new("Unknown voice transcription provider.")
        end
      rescue error : Base64::Error
        raise Error.new("Voice recording is not valid base64.")
      end

      private def transcribe_local(
        owner : UInt64,
        token : String,
        audio : Bytes,
      ) : Nil
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
        spawn monitor_transcription(key, job)
      end

      private def transcribe_with_agent(
        owner : UInt64,
        token : String,
        chat_id : String,
        provider : String,
        audio : Bytes,
      ) : Nil
        transcribe = @agent_transcriber ||
                     raise Error.new(
                       "Cloud voice transcription is unavailable."
                     )
        recording = File.tempfile("xd-voice-agent-", suffix: ".wav")
        recording_path = recording.path
        recording.write(audio)
        recording.close

        key = JobKey.new(owner, validate_token(token))
        job = Job.new(temporary_path: recording_path)
        @mutex.synchronize do
          raise Error.new("Voice service is closed.") if @closed
          raise Error.new("That voice request is already running.") if @jobs.has_key?(key)
          @jobs[key] = job
        end

        callback = ->(result : Voice::Transcription) {
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
        }
        job.cancel_callback = transcribe.call(
          provider,
          chat_id,
          recording_path,
          callback
        )
        spawn monitor_transcription(key, job)
      rescue error
        if job && key
          @mutex.synchronize do
            @jobs.delete(key) if @jobs[key]?.try(&.same?(job))
          end
          job.cancel
        elsif recording_path
          File.delete?(recording_path)
        end
        raise Error.new(
          error.message || "Cannot start cloud voice transcription."
        )
      ensure
        recording.try(&.close)
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
