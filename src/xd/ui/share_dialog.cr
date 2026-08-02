require "json"
require "gtk4"
require "../daemon/endpoint"

module Xd
  module UI
    # Local-only control panel that exposes the running daemon over TLS and
    # mints a short-lived credential for one additional device.
    class ShareDialog
      @window : Gtk::Window
      @details : Gtk::Box
      @status : Gtk::Label
      @host : Gtk::Entry
      @port : Gtk::Entry
      @name : Gtk::Entry
      @code : Gtk::Entry
      @refresh : Gtk::Button
      @busy : Bool
      @closed : Bool

      def initialize(
        @parent : Gtk::Window,
        @endpoint : Daemon::Endpoint,
      )
        @busy = false
        @closed = false

        @window = Gtk::Window.new
        @window.title = "Add a Device"
        @window.transient_for = @parent
        @window.application = @parent.application
        @window.destroy_with_parent = true
        @window.modal = true
        @window.decorated = false
        @window.resizable = false
        @window.set_default_size(680, -1)
        @window.add_css_class("xd-panel")

        title = Gtk::Label.new("Add a Device")
        title.xalign = 0_f32
        title.add_css_class("title-3")

        description = Gtk::Label.new(
          "Connect another XD app to this machine. Both devices will use " \
          "this daemon, its chats, and its running agents."
        )
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class("dim-label")

        header = Gtk::Box.new(:vertical, 5)
        header.append(title)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        @status = Gtk::Label.new("Choose a device name, then create a code.")
        @status.xalign = 0_f32
        @status.wrap = true

        @details = Gtk::Box.new(:vertical, 12)
        @host = field(@details, "Machine Address")
        @port = field(@details, "Port")
        @name = field(
          @details,
          "Device name",
          "Unknown device",
          editable: true
        )

        code_row = Gtk::Box.new(:horizontal, 8)
        @code = Gtk::Entry.new
        @code.editable = false
        @code.hexpand = true
        @code.add_css_class("title-3")
        copy = Gtk::Button.new_with_label("Copy")
        copy.clicked_signal.connect do
          @code.clipboard.set(@code.text) unless @code.text.empty?
        end
        code_row.append(@code)
        code_row.append(copy)

        code_label = Gtk::Label.new("One-Time Pairing Code")
        code_label.xalign = 0_f32
        code_label.add_css_class("caption")
        code_label.add_css_class("dim-label")
        code_box = Gtk::Box.new(:vertical, 5)
        code_box.append(code_label)
        code_box.append(code_row)
        @details.append(code_box)

        help = Gtk::Label.new(
          "On the other device, choose “Connect to a Machine…”, then enter " \
          "this address, port, and code. The device name is managed here. " \
          "Code expires after five minutes and works once. Keep XD open on " \
          "this machine."
        )
        help.xalign = 0_f32
        help.wrap = true
        help.add_css_class("dim-label")
        @details.append(help)
        @details.visible = true

        body = Gtk::Box.new(:vertical, 16)
        body.margin_top = 22
        body.margin_bottom = 22
        body.margin_start = 22
        body.margin_end = 22
        body.append(@status)
        body.append(@details)

        footer = Gtk::Box.new(:horizontal, 12)
        footer.append(hint("Esc", "Close"))
        spacer = Gtk::Box.new(:horizontal, 0)
        spacer.hexpand = true
        footer.append(spacer)

        close_button = Gtk::Button.new_with_label("Close")
        close_button.add_css_class("flat")
        close_button.clicked_signal.connect { dismiss }
        footer.append(close_button)

        @refresh = Gtk::Button.new_with_label("Create Code")
        @refresh.add_css_class("xd-panel-action")
        @refresh.clicked_signal.connect { request_code }
        footer.append(@refresh)
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")

        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(body)
        column.append(footer)
        @window.child = column

        keys = Gtk::EventControllerKey.new
        keys.key_pressed_signal.connect do |keyval, _keycode, _state|
          if keyval == Gdk::KEY_Escape
            dismiss
            true
          else
            false
          end
        end
        @window.add_controller(keys)
        @window.destroy_signal.connect { @closed = true }
        @window.close_request_signal.connect do
          dismiss
          true
        end
      end

      def present : Nil
        @window.present
        @name.grab_focus
      end

      private def field(
        body : Gtk::Box,
        title : String,
        text : String = "",
        editable : Bool = false,
      ) : Gtk::Entry
        label = Gtk::Label.new(title)
        label.xalign = 0_f32
        label.add_css_class("caption")
        label.add_css_class("dim-label")

        entry = Gtk::Entry.new
        entry.text = text
        entry.editable = editable
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

      private def request_code : Nil
        return if @busy || @closed

        name = @name.text.strip
        if name.empty?
          @status.text = "Enter a name for the device."
          @status.add_css_class("error")
          @name.grab_focus
          return
        end

        set_busy(true)
        @status.text = "Opening a secure listener…"
        @status.remove_css_class("error")
        spawn do
          begin
            response = @endpoint.call({
              "op"   => JSON::Any.new("peer-pairing"),
              "name" => JSON::Any.new(name),
            })
            GLib.idle_add do
              unless @closed
                @host.text = response["host"].as_s
                @port.text = response["port"].as_i64.to_s
                @code.text = response["code"].as_s
                @status.text = "Ready for one device."
                set_busy(false)
              end
              false
            end
          rescue error
            message = error.message || "Could not create a pairing code."
            GLib.idle_add do
              unless @closed
                @status.text = message
                @status.add_css_class("error")
                @host.text = ""
                @port.text = ""
                @code.text = ""
                set_busy(false)
              end
              false
            end
          end
        end
      end

      private def set_busy(busy : Bool) : Nil
        @busy = busy
        @name.sensitive = !busy
        @refresh.sensitive = !busy
        @refresh.label = busy ? "Opening…" : "New Code"
      end

      private def dismiss : Nil
        return if @closed

        @closed = true
        @window.destroy
      end
    end
  end
end
