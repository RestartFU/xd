require "base64"
require "random/secure"
require "gtk4"
require "../daemon/client"
require "../voice/recorder"
require "./event_inbox"

module Xd
  module UI
    # Composer microphone capture with daemon-owned speech processing.
    #
    # Audio capture must happen on the GTK client. Model download and
    # transcription run on the selected chat's endpoint, so remote chats use
    # the remote machine's CPU, storage, and bundled whisper binary.
    class VoiceInput
      enum State
        Idle
        Confirming
        AwaitingDownload
        Downloading
        Recording
        Transcribing
      end

      getter widget : Gtk::Box
      getter button : Gtk::Button

      @endpoint : Daemon::Endpoint?
      @subscription : Int64?
      @chat_id : String?
      @request_token : String?
      @recorder : Voice::Recorder?
      @error_message : String?
      @event_inbox = EventInbox(Daemon::Endpoint).new

      def initialize(
        _parent : Gtk::Window,
        @composer : Gtk::TextView,
      )
        @state = State::Idle
        @endpoint = nil
        @subscription = nil
        @chat_id = nil
        @request_token = nil
        @recorder = nil
        @error_message = nil
        @remote_target = false
        @closed = false
        @generation = 0_u64
        @timer = 0_u32
        @started_at = Time.instant
        @download_progress = 0

        @widget = Gtk::Box.new(:horizontal, 6)
        @widget.valign = :center

        @button = Gtk::Button.new_from_icon_name(
          "audio-input-microphone-symbolic"
        )
        @button.add_css_class("circular")
        @button.clicked_signal.connect { clicked }

        @download_prompt = Gtk::Box.new(:horizontal, 6)
        @download_prompt.valign = :center
        @download_prompt.visible = false
        @download_prompt_label = Gtk::Label.new("")
        @download_prompt_label.ellipsize = :end
        @download_action = Gtk::Button.new_with_label("Download")
        @download_action.add_css_class("suggested-action")
        @download_action.clicked_signal.connect do
          start_download(@generation) if @state.awaiting_download?
        end
        @download_decline = Gtk::Button.new_with_label("Not now")
        @download_decline.clicked_signal.connect { cancel }
        @download_prompt.append(@download_prompt_label)
        @download_prompt.append(@download_decline)
        @download_prompt.append(@download_action)

        @download_status = Gtk::Box.new(:horizontal, 6)
        @download_status.valign = :center
        @download_status.visible = false
        @download_bar = Gtk::ProgressBar.new
        @download_bar.show_text = true
        @download_bar.set_size_request(160, -1)
        @download_bar.valign = :center
        @download_cancel = Gtk::Button.new_with_label("Cancel")
        @download_cancel.add_css_class("destructive-action")
        @download_cancel.clicked_signal.connect { cancel }
        @download_status.append(@download_bar)
        @download_status.append(@download_cancel)

        @error_status = Gtk::Box.new(:horizontal, 6)
        @error_status.valign = :center
        @error_status.visible = false
        @error_label = Gtk::Label.new("")
        @error_label.ellipsize = :end
        @error_label.max_width_chars = 38
        @error_dismiss = Gtk::Button.new_with_label("Dismiss")
        @error_dismiss.clicked_signal.connect do
          @error_message = nil
          update_button
        end
        @error_status.append(@error_label)
        @error_status.append(@error_dismiss)

        @widget.append(@button)
        @widget.append(@download_prompt)
        @widget.append(@download_status)
        @widget.append(@error_status)
        update_button
      end

      def select(
        endpoint : Daemon::Endpoint?,
        chat_id : String?,
        remote : Bool = false,
      ) : Nil
        previous = @endpoint
        cancel
        if previous && !previous.same?(endpoint)
          if subscription = @subscription
            previous.unsubscribe(subscription)
          end
          @subscription = nil
        end

        @endpoint = endpoint
        @chat_id = chat_id
        @remote_target = remote
        @event_inbox.clear
        if endpoint && @subscription.nil?
          @subscription = endpoint.subscribe do |event|
            next unless event["event"]?.try(&.as_s?) == "voice"

            if @event_inbox.push(endpoint, event)
              GLib.idle_add do
                drain_events
              end
            end
          end
        end
        update_button
      end

      def cancel : Nil
        endpoint = @endpoint
        token = @request_token
        daemon_job = @state.downloading? || @state.transcribing?

        @generation &+= 1
        @recorder.try(&.cancel)
        if endpoint && token && daemon_job
          spawn do
            begin
              endpoint.call({
                "op"      => JSON::Any.new("voice-cancel"),
                "request" => JSON::Any.new(token),
              })
            rescue Daemon::Client::Error
            end
          end
        end
        reset
      end

      def close : Nil
        return if @closed

        cancel
        @closed = true
        if endpoint = @endpoint
          if subscription = @subscription
            endpoint.unsubscribe(subscription)
          end
        end
        @subscription = nil
        @endpoint = nil
        @chat_id = nil
        @event_inbox.clear
        update_button
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
          begin_voice if available?
        else
        end
      end

      private def begin_voice : Nil
        endpoint = @endpoint || return
        chat_id = @chat_id || return
        @error_message = nil
        @generation &+= 1
        generation = @generation
        @request_token = Random::Secure.hex(16)
        @state = State::Confirming
        update_button

        spawn do
          response : Hash(String, JSON::Any)? = nil
          message : String? = nil
          begin
            response = endpoint.call({
              "op"   => JSON::Any.new("voice-model"),
              "chat" => JSON::Any.new(chat_id),
            })
          rescue error : Daemon::Client::Error
            message = error.message || "Cannot check the speech model."
          end

          GLib.idle_add do
            if current?(generation)
              if response.try(&.["available"]?.try(&.as_bool?))
                start_recording(generation)
              elsif message
                reset
                show_error(message)
              else
                confirm_download(generation)
              end
            end
            false
          end
        end
      end

      private def confirm_download(generation : UInt64) : Nil
        return unless current?(generation)

        @state = State::AwaitingDownload
        update_button
      end

      private def start_download(generation : UInt64) : Nil
        endpoint = @endpoint || return reset
        chat_id = @chat_id || return reset
        token = @request_token || return reset
        @state = State::Downloading
        @download_progress = 0
        update_button

        spawn do
          message : String? = nil
          begin
            endpoint.call({
              "op"      => JSON::Any.new("voice-model-download"),
              "chat"    => JSON::Any.new(chat_id),
              "request" => JSON::Any.new(token),
            })
          rescue error : Daemon::Client::Error
            message = error.message || "Speech model download failed."
          end

          GLib.idle_add do
            if current?(generation) && message
              reset
              show_error(message)
            end
            false
          end
        end
      end

      private def start_recording(generation : UInt64) : Nil
        return unless current?(generation)

        recorder = Voice::Recorder.new
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
        endpoint = @endpoint || return reset
        chat_id = @chat_id || return reset
        token = @request_token || return reset
        encoded = Base64.strict_encode(wav)

        spawn do
          message : String? = nil
          begin
            endpoint.call({
              "op"      => JSON::Any.new("voice-transcribe"),
              "chat"    => JSON::Any.new(chat_id),
              "request" => JSON::Any.new(token),
              "audio"   => JSON::Any.new(encoded),
            })
          rescue error : Daemon::Client::Error
            message = error.message || "Voice transcription failed."
          end

          GLib.idle_add do
            if current?(generation) && message
              reset
              show_error(message)
            end
            false
          end
        end
      end

      private def handle_event(
        endpoint : Daemon::Endpoint,
        event : Hash(String, JSON::Any),
      ) : Nil
        return if @closed || !endpoint.same?(@endpoint)
        return unless event["event"]?.try(&.as_s?) == "voice"
        token = @request_token
        return unless token &&
                      event["request"]?.try(&.as_s?) == token

        case event["state"]?.try(&.as_s?)
        when "downloading"
          if progress = event["progress"]?.try(&.as_i64?)
            update_download_progress(progress.to_i)
          end
        when "ready"
          start_recording(@generation) if @state.downloading?
        when "transcribed"
          if @state.transcribing?
            text = event["text"]?.try(&.as_s?)
            reset
            text ? insert_transcript(text) : show_error("Daemon returned no transcription.")
          end
        when "cancelled"
          reset
        when "error"
          message = event["error"]?.try(&.as_s?) ||
                    "Voice input failed."
          reset
          show_error(message)
        end
      end

      private def drain_events : Bool
        events, more = @event_inbox.drain
        events.each do |endpoint, event|
          handle_event(endpoint, event)
        end
        more && !@closed
      end

      private def update_download_progress(progress : Int) : Nil
        return unless @state.downloading?

        bounded = progress.clamp(0, 100)
        return if bounded < @download_progress

        @download_progress = bounded
        update_download_status
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
                                   "%02d:%02d — click to transcribe on " \
                                   "#{target_name}" % \
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
        @recorder = nil
        @request_token = nil
        @download_progress = 0
        @error_message = nil
        update_button
      end

      private def current?(generation : UInt64) : Bool
        !@closed && @generation == generation
      end

      private def available? : Bool
        !@closed && !@endpoint.nil? && !@chat_id.nil?
      end

      private def target_name : String
        @remote_target ? "remote machine" : "this machine"
      end

      private def update_button : Nil
        @button.remove_css_class("destructive-action")
        @button.add_css_class("circular")
        awaiting_download = @state.awaiting_download?
        downloading = @state.downloading?
        showing_error = !@error_message.nil?
        @button.visible =
          !awaiting_download && !downloading && !showing_error
        @download_prompt.visible = awaiting_download
        @download_status.visible = downloading
        @error_status.visible = showing_error
        @download_prompt_label.text =
          "Speech model on #{target_name} · 548 MiB"
        @download_prompt_label.tooltip_text =
          "Download Whisper large-v3-turbo onto #{target_name} once."
        if message = @error_message
          @error_label.text = message
          @error_label.tooltip_text = message
        end

        case @state
        when .confirming?
          spinner = Gtk::Spinner.new
          spinner.start
          @button.child = spinner
          @button.sensitive = false
          @button.tooltip_text =
            "Checking speech model on #{target_name}…"
        when .awaiting_download?
          @download_action.sensitive = available?
        when .downloading?
          update_download_status
        when .recording?
          @button.icon_name = "media-playback-stop-symbolic"
          @button.add_css_class("destructive-action")
          @button.sensitive = true
          @button.tooltip_text =
            "Recording voice… click to transcribe on #{target_name}"
        when .transcribing?
          spinner = Gtk::Spinner.new
          spinner.start
          @button.child = spinner
          @button.sensitive = false
          @button.tooltip_text =
            "Transcribing voice on #{target_name}…"
        else
          @button.icon_name = "audio-input-microphone-symbolic"
          @button.sensitive = available?
          @button.tooltip_text =
            "Record here and transcribe on #{target_name}"
        end
      end

      private def update_download_status : Nil
        @download_bar.fraction = @download_progress / 100.0
        @download_bar.text = "#{@download_progress}%"
        @download_bar.tooltip_text =
          "Downloading speech model on #{target_name}… " \
          "#{@download_progress}%"
      end

      private def show_error(message : String?) : Nil
        return if @closed

        @error_message = message || "Voice input failed."
        update_button
      end
    end
  end
end
