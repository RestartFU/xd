require "json"
require "gtk4"
require "./panel_call"

module Xd
  module UI
    class ContextDialog
      def initialize(
        @parent : Gtk::Window,
        @request : PanelCall,
        @folder_id : String,
        folder_name : String,
      )
        @closed = false
        @busy = false
        @sequence = 0_i64

        title = Gtk::Label.new("Agent Context · #{folder_name}")
        title.xalign = 0_f32
        title.add_css_class("title-3")

        description = Gtk::Label.new(
          "These instructions are added to every agent turn in this folder. " \
          "Context from parent folders is applied before this text."
        )
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class("dim-label")

        header = Gtk::Box.new(:vertical, 5)
        header.append(title)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        field_label = Gtk::Label.new("Context for this folder")
        field_label.xalign = 0_f32
        field_label.add_css_class("caption")
        field_label.add_css_class("dim-label")

        @context = Gtk::TextView.new
        @context.wrap_mode = :word_char
        @context.top_margin = 10
        @context.bottom_margin = 10
        @context.left_margin = 10
        @context.right_margin = 10
        @context.sensitive = false

        scroller = Gtk::ScrolledWindow.new
        scroller.set_policy(:never, :automatic)
        scroller.vexpand = true
        scroller.child = @context

        frame = Gtk::Frame.new
        frame.vexpand = true
        frame.child = scroller

        @status = Gtk::Label.new("")
        @status.xalign = 0_f32
        @status.wrap = true
        @status.visible = false
        @status.add_css_class("dim-label")

        body = Gtk::Box.new(:vertical, 8)
        body.margin_top = 22
        body.margin_bottom = 22
        body.margin_start = 22
        body.margin_end = 22
        body.vexpand = true
        body.append(field_label)
        body.append(frame)
        body.append(@status)

        footer = Gtk::Box.new(:horizontal, 12)
        footer.append(hint("Esc", "Cancel"))
        footer.append(hint("Ctrl Enter", "Save"))
        spacer = Gtk::Box.new(:horizontal, 0)
        spacer.hexpand = true
        footer.append(spacer)

        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.add_css_class("flat")
        cancel.clicked_signal.connect { close }
        footer.append(cancel)

        @save = Gtk::Button.new_with_label("Save")
        @save.add_css_class("xd-panel-action")
        @save.sensitive = false
        @save.clicked_signal.connect { save }
        footer.append(@save)
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")

        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(body)
        column.append(footer)

        @window = Gtk::Window.new
        @window.transient_for = @parent
        @window.application = @parent.application
        @window.destroy_with_parent = true
        @window.modal = true
        @window.decorated = false
        @window.set_default_size(620, 500)
        @window.add_css_class("xd-panel")
        @window.child = column
        @window.destroy_signal.connect { closed }

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if keyval == Gdk::KEY_Escape
            close
            true
          elsif (keyval == Gdk::KEY_Return ||
                keyval == Gdk::KEY_KP_Enter) &&
                state.includes?(Gdk::ModifierType::ControlMask)
            save
            true
          else
            false
          end
        end
        @window.add_controller(keys)
      end

      def present : Nil
        @window.present
        load
      end

      private def load : Nil
        token = next_token
        show_status("Loading context…", false)
        spawn do
          result = @request.call({
            "op"     => JSON::Any.new("folder-context"),
            "folder" => JSON::Any.new(@folder_id),
          })
          GLib.idle_add do
            if active?(token)
              if error = result.error
                show_status(error, true)
              elsif body = result.body
                show_context(body["context"]?.try(&.as_s?) || "")
              else
                show_status("Daemon returned no context.", true)
              end
            end
            false
          end
        end
      end

      private def save : Nil
        return if @closed || @busy || !@save.sensitive?

        text = @context.buffer.text.strip
        token = next_token
        show_status(nil, false)
        set_busy(true)
        spawn do
          result = @request.call({
            "op"      => JSON::Any.new("set-folder-context"),
            "folder"  => JSON::Any.new(@folder_id),
            "context" => JSON::Any.new(text.empty? ? nil : text),
          })
          GLib.idle_add do
            if active?(token)
              if error = result.error
                show_status(error, true)
                set_busy(false)
              else
                close
              end
            end
            false
          end
        end
      end

      private def show_context(text : String) : Nil
        @context.buffer.text = text
        @context.sensitive = true
        @save.sensitive = true
        show_status(nil, false)
        @context.grab_focus
      end

      private def show_status(message : String?, error : Bool) : Nil
        return if @closed

        @status.label = message || ""
        @status.visible = !message.nil?
        if error
          @status.add_css_class("error")
        else
          @status.remove_css_class("error")
        end
      end

      private def set_busy(busy : Bool) : Nil
        return if @closed

        @busy = busy
        @context.sensitive = !busy
        @save.sensitive = !busy
        @save.label = busy ? "Saving…" : "Save"
      end

      private def close : Nil
        @window.destroy unless @closed
      end

      private def closed : Nil
        return if @closed

        @closed = true
        @sequence += 1
      end

      private def active?(token : Int64) : Bool
        !@closed && token == @sequence
      end

      private def next_token : Int64
        @sequence += 1
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
    end
  end
end
