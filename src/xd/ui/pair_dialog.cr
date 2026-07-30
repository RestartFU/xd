require "gtk4"
require "../remote/connection"

module Xd
  module UI
    # Focused pairing panel matching the original C client.
    class PairDialog
      @window : Gtk::Window
      @host : Gtk::Entry
      @port : Gtk::Entry
      @code : Gtk::Entry
      @trouble : Gtk::Label
      @cancel : Gtk::Button
      @connect : Gtk::Button
      @busy : Bool
      @closed : Bool

      def initialize(
        @parent : Gtk::Window,
        @connection : Remote::Connection,
        @on_paired : Proc(Nil),
      )
        snapshot = @connection.snapshot
        @busy = false
        @closed = false

        @window = Gtk::Window.new
        @window.title = "Connect to a Remote"
        @window.transient_for = @parent
        @window.destroy_with_parent = true
        @window.modal = true
        @window.decorated = false
        @window.set_default_size(620, 460)
        @window.add_css_class("xd-panel")

        title = Gtk::Label.new("Connect to a Remote")
        title.xalign = 0_f32
        title.add_css_class("title-3")

        description = Gtk::Label.new(
          "Run “xd serve --pair” on the other machine, then enter the " \
          "short-lived code it prints."
        )
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class("dim-label")

        header = Gtk::Box.new(:vertical, 5)
        header.append(title)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        body = Gtk::Box.new(:vertical, 14)
        body.margin_top = 22
        body.margin_bottom = 22
        body.margin_start = 22
        body.margin_end = 22
        body.vexpand = true
        body.valign = :start

        @host = field(
          body,
          "Host",
          snapshot.host || "",
          Gtk::InputPurpose::Url
        )
        @port = field(
          body,
          "Port",
          (snapshot.port || 4001).to_s,
          Gtk::InputPurpose::Digits
        )
        @code = field(
          body,
          "Pairing Code",
          "",
          Gtk::InputPurpose::Pin
        )

        @trouble = Gtk::Label.new("")
        @trouble.xalign = 0_f32
        @trouble.wrap = true
        @trouble.add_css_class("error")
        @trouble.visible = false
        body.append(@trouble)

        footer = Gtk::Box.new(:horizontal, 12)
        footer.append(hint("Esc", "Cancel"))
        footer.append(hint("Enter", "Connect"))
        spacer = Gtk::Box.new(:horizontal, 0)
        spacer.hexpand = true
        footer.append(spacer)

        @cancel = Gtk::Button.new_with_label("Cancel")
        @cancel.add_css_class("flat")
        @cancel.clicked_signal.connect { close }
        footer.append(@cancel)

        @connect = Gtk::Button.new_with_label("Connect")
        @connect.add_css_class("xd-panel-action")
        @connect.clicked_signal.connect { begin_pairing }
        footer.append(@connect)
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")

        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(body)
        column.append(footer)
        @window.child = column
        @window.default_widget = @connect

        @host.activate_signal.connect { begin_pairing }
        @port.activate_signal.connect { begin_pairing }
        @code.activate_signal.connect { begin_pairing }

        keys = Gtk::EventControllerKey.new
        keys.key_pressed_signal.connect do |keyval, _keycode, _state|
          if keyval == Gdk::KEY_Escape
            close
            true
          else
            false
          end
        end
        @window.add_controller(keys)
      end

      def present : Nil
        @window.present
        @code.grab_focus
      end

      private def field(
        body : Gtk::Box,
        title : String,
        text : String,
        purpose : Gtk::InputPurpose,
      ) : Gtk::Entry
        label = Gtk::Label.new(title)
        label.xalign = 0_f32
        label.add_css_class("caption")
        label.add_css_class("dim-label")

        entry = Gtk::Entry.new
        entry.text = text
        entry.input_purpose = purpose
        entry.activates_default = true
        entry.hexpand = true

        box = Gtk::Box.new(:vertical, 5)
        box.append(label)
        box.append(entry)
        body.append(box)
        entry
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

      private def begin_pairing : Nil
        return if @busy || @closed

        host = @host.text.strip
        port = @port.text.to_i?
        code = @code.text.gsub(/\s+/, "").upcase
        if host.empty?
          return show_error("Enter the remote machine’s address.", @host)
        end
        unless port && 1 <= port <= 65_535
          return show_error(
            "Port must be a number from 1 to 65535.",
            @port
          )
        end
        if code.empty?
          return show_error(
            "Enter the code printed by “xd serve --pair”.",
            @code
          )
        end

        @trouble.visible = false
        set_busy(true)
        spawn do
          begin
            @connection.pair(
              host,
              port,
              code,
              canceled: -> { @closed }
            )
            GLib.idle_add do
              unless @closed
                @on_paired.call
                close
              end
              false
            end
          rescue error
            message = error.message || "Pairing failed."
            GLib.idle_add do
              unless @closed
                set_busy(false)
                show_error(message, @code)
              end
              false
            end
          end
        end
      end

      private def show_error(
        message : String,
        focus : Gtk::Widget,
      ) : Nil
        @trouble.text = message
        @trouble.visible = true
        focus.grab_focus
      end

      private def set_busy(busy : Bool) : Nil
        @busy = busy
        @host.sensitive = !busy
        @port.sensitive = !busy
        @code.sensitive = !busy
        @connect.sensitive = !busy
        @connect.label = busy ? "Connecting…" : "Connect"
      end

      private def close : Nil
        return if @closed

        @closed = true
        @window.destroy
      end
    end
  end
end
