require "gtk4"
require "../voice/model"
require "../voice/recorder"
require "../voice/transcriber"
require "./adw"

module Xd
  module UI
    # Composer-owned local voice input.
    #
    # Recording and transcription always happen on the client machine, even
    # when the selected chat belongs to a paired daemon. Only the resulting
    # text enters the normal composer/send path.
    class VoiceInput
      enum State
        Idle
        Confirming
        Downloading
        Recording
        Transcribing
      end

      getter button : Gtk::Button

      @model : Voice::Model?
      @recorder : Voice::Recorder?
      @transcriber : Voice::Transcriber?
      @model_path : String?

      def initialize(
        @parent : Gtk::Window,
        @composer : Gtk::TextView,
      )
        @state = State::Idle
        @available = false
        @closed = false
        @generation = 0_u64
        @timer = 0_u32
        @model = nil
        @recorder = nil
        @transcriber = nil
        @model_path = nil
        @started_at = Time.instant

        @button = Gtk::Button.new_from_icon_name(
          "audio-input-microphone-symbolic"
        )
        @button.add_css_class("circular")
        @button.tooltip_text = "Record voice prompt locally"
        @button.clicked_signal.connect { clicked }
        update_button
      end

      def available=(available : Bool) : Bool
        cancel if !available && !@state.idle?
        @available = available
        update_button
        available
      end

      def cancel : Nil
        @generation &+= 1
        @model.try(&.cancel)
        @recorder.try(&.cancel)
        @transcriber.try(&.cancel)
        reset
      end

      def close : Nil
        return if @closed

        @closed = true
        cancel
      end

      private def clicked : Nil
        return if @closed

        case @state
        when .recording?
          @state = State::Transcribing
          @recorder.try(&.stop)
          update_button
        when .downloading?
          cancel
        when .idle?
          begin_voice if @available
        else
        end
      end

      private def begin_voice : Nil
        @generation &+= 1
        generation = @generation
        model = Voice::Model.new
        @model = model
        if path = model.find
          start_recording(path, generation)
        else
          @state = State::Confirming
          update_button
          confirm_download(generation)
        end
      end

      private def confirm_download(generation : UInt64) : Nil
        dialog = Adw::AlertDialog.new(
          heading: "Download Local Speech Model?",
          body: "Voice input runs entirely on this device. xd needs to " \
                "download the 548 MiB Whisper large-v3-turbo model once."
        )
        dialog.add_response("cancel", "Cancel")
        dialog.add_response("download", "Download")
        dialog.set_response_appearance("download", :suggested)
        dialog.default_response = "download"
        dialog.close_response = "cancel"
        dialog.choose(@parent, nil) do |_source, result|
          response = dialog.choose_finish(result)
          next unless current?(generation)

          if response == "download"
            start_download(generation)
          else
            reset
          end
        end
      end

      private def start_download(generation : UInt64) : Nil
        model = @model || return reset
        @state = State::Downloading
        update_button

        spawn do
          path : String? = nil
          message : String? = nil
          cancelled = false
          last_progress = -1
          begin
            path = model.ensure_available do |progress|
              next if progress == last_progress

              last_progress = progress
              GLib.idle_add do
                update_download_progress(progress) if current?(generation)
                false
              end
            end
          rescue error : Voice::Cancelled
            cancelled = true
          rescue error : Voice::Error
            message = error.message || "Speech model download failed."
          end

          GLib.idle_add do
            if current?(generation)
              if ready = path
                start_recording(ready, generation)
              else
                reset
                show_error(message) unless cancelled
              end
            end
            false
          end
        end
      end

      private def update_download_progress(progress : Int32) : Nil
        return unless @state.downloading?

        @button.tooltip_text =
          "Downloading local speech model… #{progress}% — click to cancel"
      end

      private def start_recording(
        model_path : String,
        generation : UInt64,
      ) : Nil
        return unless current?(generation)

        recorder = Voice::Recorder.new
        @model = nil
        @model_path = model_path
        @recorder = recorder
        @state = State::Recording
        @started_at = Time.instant
        update_button
        start_recording_timer

        recorder.record do |result|
          GLib.idle_add do
            handle_recording(result, generation) if current?(generation)
            false
          end
        end
      rescue error : Voice::Error
        reset
        show_error(error.message || "Cannot record microphone.")
      end

      private def handle_recording(
        result : Voice::Recording,
        generation : UInt64,
      ) : Nil
        @recorder = nil
        stop_timer
        if result.cancelled
          reset
        elsif wav = result.wav
          @state = State::Transcribing
          update_button
          start_transcription(wav, generation)
        else
          reset
          show_error(result.error || "Cannot record microphone.")
        end
      end

      private def start_transcription(
        wav : Bytes,
        generation : UInt64,
      ) : Nil
        model_path = @model_path || return reset
        transcriber = Voice::Transcriber.new
        @transcriber = transcriber
        transcriber.transcribe(wav, model_path) do |result|
          GLib.idle_add do
            handle_transcription(result, generation) if current?(generation)
            false
          end
        end
      end

      private def handle_transcription(
        result : Voice::Transcription,
        _generation : UInt64,
      ) : Nil
        @transcriber = nil
        reset
        if text = result.text
          insert_transcript(text)
        elsif !result.cancelled
          show_error(result.error || "Local voice transcription failed.")
        end
      end

      private def insert_transcript(transcript : String) : Nil
        existing = @composer.buffer.text
        separator =
          existing.empty? || existing[-1].whitespace? ? "" : " "
        @composer.buffer.text = existing + separator + transcript
        @composer.buffer.place_cursor(@composer.buffer.end_iter)
        @composer.grab_focus
      end

      private def start_recording_timer : Nil
        stop_timer
        @timer = GLib.timeout(1.second) do
          if @state.recording?
            elapsed = (Time.instant - @started_at).total_seconds.to_i64
            @button.tooltip_text = "Recording voice… " \
                                   "%02d:%02d — click to transcribe" % \
                                   {elapsed // 60, elapsed % 60}
            true
          else
            @timer = 0_u32
            false
          end
        end
      end

      private def stop_timer : Nil
        return if @timer == 0

        GLib.source_remove(@timer)
        @timer = 0_u32
      end

      private def reset : Nil
        stop_timer
        @state = State::Idle
        @model = nil
        @recorder = nil
        @transcriber = nil
        @model_path = nil
        update_button
      end

      private def current?(generation : UInt64) : Bool
        !@closed && @generation == generation
      end

      private def update_button : Nil
        @button.remove_css_class("destructive-action")
        case @state
        when .confirming?
          spinner = Gtk::Spinner.new
          spinner.start
          @button.child = spinner
          @button.sensitive = false
          @button.tooltip_text = "Preparing local voice input…"
        when .downloading?
          @button.icon_name = "media-playback-stop-symbolic"
          @button.add_css_class("destructive-action")
          @button.sensitive = true
          @button.tooltip_text =
            "Downloading local speech model… 0% — click to cancel"
        when .recording?
          @button.icon_name = "media-playback-stop-symbolic"
          @button.add_css_class("destructive-action")
          @button.sensitive = true
          @button.tooltip_text =
            "Recording voice… click to transcribe"
        when .transcribing?
          spinner = Gtk::Spinner.new
          spinner.start
          @button.child = spinner
          @button.sensitive = false
          @button.tooltip_text = "Transcribing voice…"
        else
          @button.icon_name = "audio-input-microphone-symbolic"
          @button.sensitive = @available && !@closed
          @button.tooltip_text = "Record voice prompt locally"
        end
      end

      private def show_error(message : String?) : Nil
        return if @closed

        dialog = Adw::AlertDialog.new(
          heading: "Voice Input Failed",
          body: message || "Local voice input failed."
        )
        dialog.add_response("close", "Close")
        dialog.default_response = "close"
        dialog.close_response = "close"
        dialog.present(@parent)
      end
    end
  end
end
