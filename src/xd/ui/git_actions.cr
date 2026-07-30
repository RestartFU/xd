require "json"
require "gtk4"
require "./adw"
require "./host_launch"
require "./panel_call"

module Xd
  module UI
    # One C-shaped button for the next repository action.
    #
    # Repository decisions and mutations stay on the daemon. This widget only
    # presents that shared state, so local Unix and remote TLS chats cannot
    # grow separate Git behavior.
    class GitActions
      getter widget : Adw::Bin

      @chat_id : String?
      @state_token : String?
      @action_token : String?

      def initialize(
        @parent : Gtk::Widget,
        @request : PanelCall,
      )
        @chat_id = nil
        @state_token = nil
        @action_token = nil
        @sequence = 0_i64
        @suggested = "none"
        @enabled = false
        @busy = false

        @button = Gtk::Button.new_with_label("Up to date")
        @button.add_css_class("flat")
        @button.clicked_signal.connect { clicked }

        @widget = Adw::Bin.new(child: @button)
        @widget.visible = false
      end

      def select_chat(chat_id : String?) : Nil
        @chat_id = chat_id
        @state_token = nil
        @action_token = nil
        @busy = false
        @widget.visible = false
        refresh if chat_id
      end

      def refresh : Nil
        chat_id = @chat_id
        return unless chat_id

        token = next_token("state")
        @state_token = token
        spawn do
          result = @request.call({
            "op"      => JSON::Any.new("git-state"),
            "chat"    => JSON::Any.new(chat_id),
            "request" => JSON::Any.new(token),
          })
          GLib.idle_add do
            if @state_token == token && result.error
              @state_token = nil
              @widget.visible = false
            end
            false
          end
        end
      end

      def handle_event(event : Hash(String, JSON::Any)) : Nil
        return unless event["chat"]?.try(&.as_s?) == @chat_id

        case event["event"]?.try(&.as_s?)
        when "git-state"
          return unless event["request"]?.try(&.as_s?) == @state_token

          @state_token = nil
          apply_state(event)
        when "git-action-finished"
          apply_state(event) if event.has_key?("visible")
          return unless event["request"]?.try(&.as_s?) == @action_token

          @action_token = nil
          set_busy(false)
          if event["success"]?.try(&.as_bool?) == true
            if url = event["url"]?.try(&.as_s?)
              HostLaunch.open_uri(url)
            end
          else
            show_error(
              event["error"]?.try(&.as_s?) || "Git refused the request."
            )
          end
        when "turn-finished", "changed", "repository-changed"
          refresh
        end
      end

      def connection_changed(connected : Bool) : Nil
        return unless @chat_id

        @state_token = nil
        @action_token = nil
        set_busy(false)
        if connected
          refresh
        else
          @widget.visible = false
        end
      end

      private def clicked : Nil
        return if @busy || !@enabled

        if @suggested == "commit"
          present_commit
        else
          perform(@suggested, nil)
        end
      end

      private def present_commit : Nil
        dialog = Adw::AlertDialog.new(
          heading: "Commit Everything Changed"
        )
        group = Adw::PreferencesGroup.new
        row = Adw::EntryRow.new(title: "Message")
        group.add(row)
        dialog.extra_child = group
        dialog.add_response("cancel", "Cancel")
        dialog.add_response("commit", "Commit")
        dialog.set_response_appearance("commit", :suggested)
        dialog.default_response = "commit"
        dialog.close_response = "cancel"
        dialog.choose(@parent, nil) do |_source, result|
          response = dialog.choose_finish(result)
          text = row.text.strip
          perform("commit", text) if response == "commit" && !text.empty?
        end
      end

      private def perform(action : String, message : String?) : Nil
        chat_id = @chat_id
        return unless chat_id

        token = next_token("action")
        @action_token = token
        set_busy(true)
        request = {
          "op"      => JSON::Any.new("git-action"),
          "chat"    => JSON::Any.new(chat_id),
          "action"  => JSON::Any.new(action),
          "request" => JSON::Any.new(token),
        }
        if text = message
          request["message"] = JSON::Any.new(text)
        end

        spawn do
          result = @request.call(request)
          GLib.idle_add do
            if @action_token == token && (error = result.error)
              @action_token = nil
              set_busy(false)
              show_error(error)
            end
            false
          end
        end
      end

      private def apply_state(event : Hash(String, JSON::Any)) : Nil
        @suggested = event["action"]?.try(&.as_s?) || "none"
        @enabled = event["enabled"]?.try(&.as_bool?) || false
        @button.label = event["label"]?.try(&.as_s?) || "Up to date"
        @button.sensitive = @enabled && !@busy
        @widget.visible = event["visible"]?.try(&.as_bool?) || false
      end

      private def set_busy(busy : Bool) : Nil
        @busy = busy
        @button.sensitive = @enabled && !busy
      end

      private def show_error(message : String) : Nil
        dialog = Adw::AlertDialog.new(
          heading: "Git Refused",
          body: message
        )
        dialog.add_response("close", "Close")
        dialog.default_response = "close"
        dialog.present(@parent)
      end

      private def next_token(kind : String) : String
        @sequence += 1
        "#{kind}:#{object_id}:#{@sequence}"
      end
    end
  end
end
