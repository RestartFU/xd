require "base64"
require "gtk4"
require "../agent/ask"
require "../daemon/client"
require "./chat_controls"
require "./sidebar"
require "./tool_panel"

module Xd
  module UI
    class Window
      getter widget : Gtk::ApplicationWindow

      @active_chat : String?
      @stream_label : Gtk::Label?
      @working = false

      def initialize(
        application : Gtk::Application,
        @client : Daemon::Client,
      )
        @active_chat = nil
        @stream_label = nil
        @widget = Gtk::ApplicationWindow.new(application)
        @widget.title = "xd"
        @widget.set_default_size(1380, 820)

        @sidebar = Sidebar.new(
          @widget,
          ->(request : Hash(String, JSON::Any)) { call(request) },
          ->(id : String, title : String) { open_chat(id, title) },
          ->(id : String) { chat_deleted(id) }
        )
        @tool_panel = ToolPanel.new(
          ->(request : Hash(String, JSON::Any)) { call(request) }
        )

        @chat_title = Gtk::Label.new("Select a chat")
        @chat_title.xalign = 0_f32
        @chat_title.hexpand = true
        @chat_title.add_css_class("title")

        chat_header = Gtk::Box.new(:horizontal, 8)
        chat_header.margin_top = 12
        chat_header.margin_bottom = 12
        chat_header.margin_start = 18
        chat_header.margin_end = 18
        chat_header.append(@chat_title)
        {
          "Files"    => "files",
          "Diff"     => "diff",
          "Terminal" => "terminal",
        }.each do |label, page|
          button = Gtk::Button.new_with_label(label)
          button.add_css_class("flat")
          button.clicked_signal.connect { @tool_panel.toggle(page) }
          chat_header.append(button)
        end

        @controls = ChatControls.new(
          ->(option : String, value : String?) {
            set_option(option, value)
          }
        )
        @controls.widget.margin_start = 12
        @controls.widget.margin_end = 12
        @controls.widget.margin_bottom = 6

        @transcript = Gtk::Box.new(:vertical, 10)
        @transcript.margin_top = 18
        @transcript.margin_bottom = 18
        @transcript.margin_start = 24
        @transcript.margin_end = 24
        @transcript.valign = :start

        @transcript_scroll = Gtk::ScrolledWindow.new
        @transcript_scroll.vexpand = true
        @transcript_scroll.child = @transcript

        @entry = Gtk::Entry.new
        @entry.hexpand = true
        @entry.placeholder_text = "Ask Codex or Claude…"
        @entry.sensitive = false
        @entry.activate_signal.connect { send_message }

        @send = Gtk::Button.new_with_label("Send")
        @send.sensitive = false
        @send.add_css_class("suggested-action")
        @send.clicked_signal.connect do
          @working ? cancel_turn : send_message
        end

        @queue_box = Gtk::Box.new(:vertical, 4)
        @queue_box.margin_start = 18
        @queue_box.margin_end = 18
        @queue_box.add_css_class("xd-queue")
        @queue_box.visible = false

        composer = Gtk::Box.new(:horizontal, 8)
        composer.margin_top = 10
        composer.margin_bottom = 14
        composer.margin_start = 18
        composer.margin_end = 18
        composer.add_css_class("xd-composer")
        composer.append(@entry)
        composer.append(@send)

        @status = Gtk::Label.new("")
        @status.xalign = 0_f32
        @status.margin_start = 18
        @status.margin_end = 18
        @status.add_css_class("dim-label")

        chat = Gtk::Box.new(:vertical, 0)
        chat.hexpand = true
        chat.add_css_class("xd-chat")
        chat.append(chat_header)
        chat.append(@controls.widget)
        chat.append(Gtk::Separator.new(:horizontal))
        chat.append(@transcript_scroll)
        chat.append(@queue_box)
        chat.append(@status)
        chat.append(composer)

        root = Gtk::Box.new(:horizontal, 0)
        root.append(@sidebar.widget)
        root.append(Gtk::Separator.new(:vertical))
        root.append(chat)
        root.append(Gtk::Separator.new(:vertical))
        root.append(@tool_panel.widget)
        @widget.child = root

        @client.subscribe do |event|
          GLib.idle_add do
            handle_event(event)
            false
          end
        end

        @sidebar.reload
      end

      def present : Nil
        @widget.present
      end

      private def call(fields : Hash(String, JSON::Any))
        @status.text = ""
        @client.call(fields)
      rescue error : Daemon::Client::Error
        @status.text = error.message || "Daemon request failed."
        nil
      end

      private def open_chat(id : String, title : String) : Nil
        @active_chat = id
        @stream_label = nil
        @chat_title.text = title
        @entry.sensitive = true
        @send.sensitive = true
        @controls.sensitive = true
        @tool_panel.chat = id
        load_chat_state
        load_messages
        @entry.grab_focus
      end

      private def chat_deleted(id : String) : Nil
        return unless @active_chat == id

        @active_chat = nil
        @stream_label = nil
        @working = false
        @chat_title.text = "Select a chat"
        @entry.text = ""
        @entry.sensitive = false
        @send.label = "Send"
        @send.sensitive = false
        @send.remove_css_class("destructive-action")
        @send.add_css_class("suggested-action")
        @controls.sensitive = false
        @tool_panel.chat = nil
        @tool_panel.widget.visible = false
        clear(@transcript)
        clear(@queue_box)
        @queue_box.visible = false
        @status.text = ""
      end

      private def load_messages : Nil
        chat_id = @active_chat
        return unless chat_id

        response = call({
          "op"   => JSON::Any.new("messages"),
          "chat" => JSON::Any.new(chat_id),
        })
        return unless response

        clear(@transcript)
        messages = response["messages"].as_a
        messages.each_with_index do |message, index|
          add_message(
            message["role"].as_s,
            message["content"].as_s,
            message["label"]?.try(&.as_s?),
            reply_answerable?(messages, index)
          )
        end
        @stream_label = nil
        scroll_to_bottom
      end

      private def add_message(
        role : String,
        content : String,
        label : String? = nil,
        answerable : Bool = false,
      ) : Gtk::Label?
        if role == "duration"
          @status.text = "Finished in #{content}s"
          return
        end

        parsed = role == "assistant" ? Agent::Ask.parse(content) : nil
        shown = if parsed
                  [parsed.remainder, parsed.ask.question]
                    .reject(&.empty?).join("\n\n")
                else
                  content
                end
        heading = case role
                  when "user"      then "You"
                  when "assistant" then label || "Assistant"
                  when "tool"      then "Tool"
                  when "error"     then "Error"
                  else                  role.capitalize
                  end
        text = shown.empty? ? heading : "#{heading}\n#{shown}"
        row = Gtk::Label.new(text)
        row.xalign = 0_f32
        row.wrap = true
        row.wrap_mode = :word_char
        row.selectable = true
        row.add_css_class("xd-message")
        row.add_css_class("xd-message-#{role}")
        @transcript.append(row)
        append_ask(parsed.ask) if parsed && answerable
        row
      end

      private def reply_answerable?(
        messages : Array(JSON::Any),
        position : Int,
      ) : Bool
        return false unless messages[position]["role"].as_s == "assistant"

        ((position + 1)...messages.size).all? do |index|
          messages[index]["role"].as_s == "duration"
        end
      end

      private def append_ask(ask : Agent::Ask) : Nil
        choices = Gtk::Box.new(:vertical, 5)
        choices.add_css_class("xd-ask")

        ask.options.each do |option|
          answer = option
          button = Gtk::Button.new_with_label(answer)
          button.hexpand = true
          button.halign = :fill
          button.add_css_class("xd-choice")
          button.clicked_signal.connect { answer_ask(answer) }
          choices.append(button)
        end

        if ask.accepts_input
          input = Gtk::Entry.new
          input.hexpand = true
          input.placeholder_text = "Type your answer"
          input.activate_signal.connect { answer_ask(input.text) }

          send = Gtk::Button.new_with_label("Send")
          send.add_css_class("suggested-action")
          send.clicked_signal.connect { answer_ask(input.text) }

          row = Gtk::Box.new(:horizontal, 6)
          row.append(input)
          row.append(send)
          choices.append(row)
        end

        @transcript.append(choices)
      end

      private def answer_ask(answer : String) : Nil
        text = answer.strip
        return if text.empty?

        @entry.text = text
        send_message
      end

      private def send_message : Nil
        chat_id = @active_chat
        return unless chat_id
        text = @entry.text.strip
        return if text.empty?

        @entry.text = ""
        response = call({
          "op"   => JSON::Any.new("send"),
          "chat" => JSON::Any.new(chat_id),
          "text" => JSON::Any.new(text),
        })
        if response
          if response["queued"]?.try(&.as_bool?) == true
            @status.text = "Message queued"
          end
          load_messages
          load_chat_state
        end
      end

      private def cancel_turn : Nil
        chat_id = @active_chat
        return unless chat_id

        call({
          "op"   => JSON::Any.new("cancel"),
          "chat" => JSON::Any.new(chat_id),
        })
      end

      private def set_option(option : String, value : String?) : Nil
        chat_id = @active_chat
        return unless chat_id

        request = {
          "op"     => JSON::Any.new("set-option"),
          "chat"   => JSON::Any.new(chat_id),
          "option" => JSON::Any.new(option),
        }
        request["value"] = JSON::Any.new(value) if value
        load_chat_state if call(request)
      end

      private def load_chat_state : Nil
        chat_id = @active_chat
        return unless chat_id

        state = call({
          "op"   => JSON::Any.new("chat"),
          "chat" => JSON::Any.new(chat_id),
        })
        return unless state

        @controls.update(state)
        @working = state["working"]?.try(&.as_bool?) || false
        @send.label = @working ? "Stop" : "Send"
        if @working
          @send.remove_css_class("suggested-action")
          @send.add_css_class("destructive-action")
        else
          @send.remove_css_class("destructive-action")
          @send.add_css_class("suggested-action")
        end
        queue = state["queue"]?.try(&.as_a?) || [] of JSON::Any
        render_queue(queue)
      end

      private def render_queue(queue : Array(JSON::Any)) : Nil
        clear(@queue_box)
        @queue_box.visible = !queue.empty?

        queue.each_with_index do |node, index|
          text = node.as_s
          label = Gtk::Label.new(text)
          label.xalign = 0_f32
          label.hexpand = true
          label.ellipsize = :end

          steer = Gtk::Button.new_with_label("Run next")
          steer.add_css_class("flat")
          steer.clicked_signal.connect do
            steer_queue(index, text)
          end

          remove = Gtk::Button.new_with_label("×")
          remove.add_css_class("flat")
          remove.tooltip_text = "Remove queued message"
          remove.clicked_signal.connect { drop_queue(index) }

          row = Gtk::Box.new(:horizontal, 6)
          row.add_css_class("xd-queue-row")
          row.append(label)
          row.append(steer)
          row.append(remove)
          @queue_box.append(row)
        end
      end

      private def steer_queue(index : Int, text : String) : Nil
        chat_id = @active_chat
        return unless chat_id
        call({
          "op"    => JSON::Any.new("steer-queue"),
          "chat"  => JSON::Any.new(chat_id),
          "index" => JSON::Any.new(index.to_i64),
          "text"  => JSON::Any.new(text),
        })
      end

      private def drop_queue(index : Int) : Nil
        chat_id = @active_chat
        return unless chat_id
        call({
          "op"    => JSON::Any.new("drop-queue"),
          "chat"  => JSON::Any.new(chat_id),
          "index" => JSON::Any.new(index.to_i64),
        })
      end

      private def handle_event(event : Hash(String, JSON::Any)) : Nil
        @tool_panel.handle_event(event)
        name = event["event"]?.try(&.as_s?) || return
        case name
        when "tree"
          @sidebar.reload
        when "text"
          return unless active_event?(event)
          text = event["text"]?.try(&.as_s?) || return
          label = @stream_label
          unless label
            label = add_message("assistant", "")
            @stream_label = label
          end
          if label
            current = label.text
            current = "Assistant\n" if current == "Assistant"
            label.text = current + text
          end
          scroll_to_bottom
        when "tool"
          return unless active_event?(event)
          add_message("tool", event["text"]?.try(&.as_s?) || "Used a tool")
          @stream_label = nil
          scroll_to_bottom
        when "turn-started"
          return unless active_event?(event)
          @status.text = "Working…"
          @stream_label = nil
          load_chat_state
        when "turn-finished"
          if active_event?(event)
            load_messages
            load_chat_state
            if event["waiting"]?.try(&.as_bool?) == true
              @status.text = "Waiting for your answer"
            end
          end
        when "changed"
          if active_event?(event)
            load_messages
            load_chat_state
          end
        when "queued"
          return unless active_event?(event)
          queue = event["queue"]?.try(&.as_a?)
          @status.text = queue && !queue.empty? ? "Message queued" : ""
          load_chat_state
        end
      end

      private def active_event?(event : Hash(String, JSON::Any)) : Bool
        event["chat"]?.try(&.as_s?) == @active_chat
      end

      private def clear(box : Gtk::Box) : Nil
        while child = box.first_child
          box.remove(child)
        end
      end

      private def scroll_to_bottom : Nil
        GLib.idle_add do
          adjustment = @transcript_scroll.vadjustment
          adjustment.value = adjustment.upper
          false
        end
      end
    end
  end
end
