require "json"
require "gtk4"
require "./adw"
require "./dialogs"
require "./host_launch"
require "./panel_call"
require "../version"

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
      @draft_token : String?

      def initialize(
        @parent : Gtk::Window,
        @request : PanelCall,
      )
        @chat_id = nil
        @state_token = nil
        @action_token = nil
        @draft_token = nil
        @sequence = 0_i64
        @suggested = "none"
        @state_label = "Up to date"
        @enabled = false
        @busy = false
        @settings = Gio::Settings.new(APP_ID)

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
        @draft_token = nil
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
        when "git-draft-finished"
          return unless event["request"]?.try(&.as_s?) == @draft_token

          @draft_token = nil
          set_busy(false)
          action = event["kind"]?.try(&.as_s?) == "pull-request" ? "create-pr" : "commit"
          if event["success"]?.try(&.as_bool?) == true
            present_review(
              action,
              event["title"]?.try(&.as_s?) || "",
              event["body"]?.try(&.as_s?) || ""
            )
          else
            present_review(
              action,
              "",
              "",
              event["error"]?.try(&.as_s?) ||
              "Assistant could not write this draft."
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
        @draft_token = nil
        set_busy(false)
        if connected
          refresh
        else
          @widget.visible = false
        end
      end

      private def clicked : Nil
        return if @busy || !@enabled

        if {"commit", "create-pr"}.includes?(@suggested)
          request_draft(@suggested)
        else
          perform(@suggested)
        end
      end

      private def request_draft(action : String) : Nil
        chat_id = @chat_id
        return unless chat_id

        token = next_token("draft")
        @draft_token = token
        set_busy(true)
        request = {
          "op"   => JSON::Any.new("git-draft"),
          "chat" => JSON::Any.new(chat_id),
          "kind" => JSON::Any.new(
            action == "create-pr" ? "pull-request" : "commit"
          ),
          "request" => JSON::Any.new(token),
        }
        backend = @settings.string("git-writing-backend")
        model = @settings.string("git-writing-model")
        request["backend"] = JSON::Any.new(backend) unless backend.empty?
        request["model"] = JSON::Any.new(model) unless model.empty?

        spawn do
          result = @request.call(request)
          GLib.idle_add do
            if @draft_token == token && (error = result.error)
              @draft_token = nil
              set_busy(false)
              present_review(action, "", "", error)
            end
            false
          end
        end
      end

      private def present_review(
        action : String,
        title : String,
        body : String,
        warning : String? = nil,
      ) : Nil
        pull_request = action == "create-pr"
        heading = pull_request ? "Review Pull Request" : "Review Commit"
        title_label = Gtk::Label.new(heading)
        title_label.xalign = 0_f32
        title_label.add_css_class("title-3")

        description = Gtk::Label.new(
          warning || "Review and edit the assistant's draft before continuing."
        )
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class(warning ? "error" : "dim-label")

        header = Gtk::Box.new(:vertical, 5)
        header.append(title_label)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        group = Adw::PreferencesGroup.new
        group.title = "Draft"
        title_row = Adw::EntryRow.new(title: "Title")
        title_row.text = title
        group.add(title_row)

        body_label = Gtk::Label.new(
          pull_request ? "Description" : "Details (optional)"
        )
        body_label.halign = :start
        body_label.add_css_class("dim-label")
        body_view = Gtk::TextView.new
        body_view.wrap_mode = :word_char
        body_view.buffer.text = body
        body_view.left_margin = 8
        body_view.right_margin = 8
        body_view.top_margin = 8
        body_view.bottom_margin = 8
        body_scroll = Gtk::ScrolledWindow.new
        body_scroll.min_content_height = 150
        body_scroll.max_content_height = 240
        body_scroll.set_policy(:never, :automatic)
        body_scroll.child = body_view
        body_scroll.add_css_class("card")

        status = Gtk::Label.new("")
        status.xalign = 0_f32
        status.wrap = true
        status.visible = false
        status.add_css_class("error")

        body_box = Gtk::Box.new(:vertical, 8)
        body_box.margin_top = 22
        body_box.margin_bottom = 22
        body_box.margin_start = 22
        body_box.margin_end = 22
        body_box.append(group)
        body_box.append(body_label)
        body_box.append(body_scroll)
        body_box.append(status)

        footer = Gtk::Box.new(:horizontal, 12)
        footer.append(hint("Esc", "Cancel"))
        footer.append(hint("Ctrl Enter", pull_request ? "Open PR" : "Commit"))
        spacer = Gtk::Box.new(:horizontal, 0)
        spacer.hexpand = true
        footer.append(spacer)

        window = Gtk::Window.new
        submit = -> {
          clean_title = title_row.text.strip
          if clean_title.empty?
            status.label = pull_request ? "Write a pull request title first." : "Write a commit title first."
            status.visible = true
          else
            clean_body = body_view.buffer.text.strip
            if pull_request
              perform("create-pr", title: clean_title, body: clean_body)
            else
              message = clean_body.empty? ? clean_title : "#{clean_title}\n\n#{clean_body}"
              perform("commit", message: message)
            end
            window.destroy
          end
        }

        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.add_css_class("flat")
        cancel.clicked_signal.connect { window.destroy }
        footer.append(cancel)

        confirm = Gtk::Button.new_with_label(
          pull_request ? "Create Pull Request" : "Commit"
        )
        confirm.add_css_class("xd-panel-action")
        confirm.clicked_signal.connect { submit.call }
        footer.append(confirm)
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")

        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(body_box)
        column.append(footer)

        window.title = heading
        window.transient_for = @parent
        window.application = @parent.application
        window.destroy_with_parent = true
        window.modal = true
        window.decorated = false
        window.resizable = false
        window.set_default_size(700, -1)
        window.add_css_class("xd-panel")
        window.child = column
        window.close_request_signal.connect do
          window.destroy
          true
        end

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if keyval == Gdk::KEY_Escape
            window.destroy
            true
          elsif (keyval == Gdk::KEY_Return ||
                keyval == Gdk::KEY_KP_Enter) &&
                state.includes?(Gdk::ModifierType::ControlMask)
            submit.call
            true
          else
            false
          end
        end
        window.add_controller(keys)
        window.present
        title_row.grab_focus
        title_row.select_region(0, -1)
      end

      private def perform(
        action : String,
        message : String? = nil,
        title : String? = nil,
        body : String? = nil,
      ) : Nil
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
        if text = title
          request["title"] = JSON::Any.new(text)
        end
        if text = body
          request["body"] = JSON::Any.new(text)
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
        @state_label = event["label"]?.try(&.as_s?) || "Up to date"
        @button.label = @busy ? "Writing…" : @state_label
        @button.sensitive = @enabled && !@busy
        @widget.visible = event["visible"]?.try(&.as_bool?) || false
      end

      private def set_busy(busy : Bool) : Nil
        @busy = busy
        @button.label = busy ? "Writing…" : @state_label
        @button.sensitive = @enabled && !busy
      end

      private def show_error(message : String) : Nil
        Dialogs.alert(@parent, "Git Refused", message)
      end

      private def hint(key : String, what : String) : Gtk::Box
        label = Gtk::Label.new(key)
        label.add_css_class("xd-key")
        text = Gtk::Label.new(what)
        text.add_css_class("dim-label")
        text.add_css_class("caption")

        box = Gtk::Box.new(:horizontal, 6)
        box.append(label)
        box.append(text)
        box
      end

      private def next_token(kind : String) : String
        @sequence += 1
        "#{kind}:#{object_id}:#{@sequence}"
      end
    end
  end
end
