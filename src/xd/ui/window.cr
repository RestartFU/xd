require "base64"
require "gtk4"
require "../daemon/client"

module Xd
  module UI
    class Window
      getter widget : Gtk::ApplicationWindow

      @active_chat : String?
      @stream_label : Gtk::Label?
      @folders = [] of String

      def initialize(
        application : Gtk::Application,
        @client : Daemon::Client,
      )
        @active_chat = nil
        @stream_label = nil
        @widget = Gtk::ApplicationWindow.new(application)
        @widget.title = "xd"
        @widget.set_default_size(1180, 760)

        @tree_box = Gtk::Box.new(:vertical, 2)
        @tree_box.margin_top = 6
        @tree_box.margin_bottom = 6
        @tree_box.margin_start = 6
        @tree_box.margin_end = 6

        tree_scroll = Gtk::ScrolledWindow.new
        tree_scroll.vexpand = true
        tree_scroll.child = @tree_box

        tree_title = Gtk::Label.new("Workspaces")
        tree_title.xalign = 0_f32
        tree_title.hexpand = true
        tree_title.add_css_class("title")
        new_chat = Gtk::Button.new_with_label("+")
        new_chat.tooltip_text = "New chat"
        new_chat.add_css_class("flat")
        new_chat.clicked_signal.connect { create_chat }

        tree_header = Gtk::Box.new(:horizontal, 8)
        tree_header.margin_top = 8
        tree_header.margin_bottom = 8
        tree_header.margin_start = 12
        tree_header.margin_end = 8
        tree_header.append(tree_title)
        tree_header.append(new_chat)

        sidebar = Gtk::Box.new(:vertical, 0)
        sidebar.width_request = 280
        sidebar.add_css_class("xd-sidebar")
        sidebar.append(tree_header)
        sidebar.append(Gtk::Separator.new(:horizontal))
        sidebar.append(tree_scroll)

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
        @send.clicked_signal.connect { send_message }

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
        chat.append(Gtk::Separator.new(:horizontal))
        chat.append(@transcript_scroll)
        chat.append(@status)
        chat.append(composer)

        root = Gtk::Box.new(:horizontal, 0)
        root.append(sidebar)
        root.append(Gtk::Separator.new(:vertical))
        root.append(chat)
        @widget.child = root

        @client.subscribe do |event|
          GLib.idle_add do
            handle_event(event)
            false
          end
        end

        reload_tree
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

      private def reload_tree : Nil
        response = call({"op" => JSON::Any.new("tree")})
        return unless response

        clear(@tree_box)
        folders = response["folders"].as_a
        chats = response["chats"].as_a
        @folders = folders.map { |folder| folder["id"].as_s }
        names = {} of String => String
        folders.each do |folder|
          names[folder["id"].as_s] = folder["name"].as_s
        end

        @folders.each do |folder_id|
          heading = Gtk::Label.new(names[folder_id]? || "Workspace")
          heading.xalign = 0_f32
          heading.margin_top = 8
          heading.margin_start = 8
          heading.add_css_class("dim-label")
          @tree_box.append(heading)

          chats.each do |chat|
            next unless chat["folder"].as_s == folder_id
            add_chat_button(chat)
          end
        end
      end

      private def add_chat_button(chat : JSON::Any) : Nil
        id = chat["id"].as_s
        title = chat["title"].as_s? || "New Chat"
        title = "New Chat" if title.empty?
        button = Gtk::Button.new_with_label(title)
        button.hexpand = true
        button.halign = :fill
        button.add_css_class("flat")
        button.add_css_class("xd-chat-row")
        button.clicked_signal.connect { open_chat(id, title) }
        @tree_box.append(button)
      end

      private def create_chat : Nil
        folder_id = @folders.first?
        unless folder_id
          created = call({
            "op"   => JSON::Any.new("new-folder"),
            "name" => JSON::Any.new("Workspace"),
          })
          return unless created
          folder_id = created["id"].as_s
        end

        created = call({
          "op"     => JSON::Any.new("new-chat"),
          "folder" => JSON::Any.new(folder_id),
        })
        return unless created

        reload_tree
        open_chat(created["id"].as_s, "New Chat")
      end

      private def open_chat(id : String, title : String) : Nil
        @active_chat = id
        @stream_label = nil
        @chat_title.text = title
        @entry.sensitive = true
        @send.sensitive = true
        load_messages
        @entry.grab_focus
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
        response["messages"].as_a.each do |message|
          add_message(
            message["role"].as_s,
            message["content"].as_s,
            message["label"]?.try(&.as_s?)
          )
        end
        @stream_label = nil
        scroll_to_bottom
      end

      private def add_message(
        role : String,
        content : String,
        label : String? = nil,
      ) : Gtk::Label?
        if role == "duration"
          @status.text = "Finished in #{content}s"
          return
        end

        heading = case role
                  when "user"      then "You"
                  when "assistant" then label || "Assistant"
                  when "tool"      then "Tool"
                  when "error"     then "Error"
                  else                  role.capitalize
                  end
        text = content.empty? ? heading : "#{heading}\n#{content}"
        row = Gtk::Label.new(text)
        row.xalign = 0_f32
        row.wrap = true
        row.wrap_mode = :word_char
        row.selectable = true
        row.add_css_class("xd-message")
        row.add_css_class("xd-message-#{role}")
        @transcript.append(row)
        row
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
        load_messages if response
      end

      private def handle_event(event : Hash(String, JSON::Any)) : Nil
        name = event["event"]?.try(&.as_s?) || return
        case name
        when "tree"
          reload_tree
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
        when "turn-finished", "changed"
          load_messages if active_event?(event)
        when "queued"
          return unless active_event?(event)
          queue = event["queue"]?.try(&.as_a?)
          @status.text = queue && !queue.empty? ? "Message queued" : ""
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
