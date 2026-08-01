require "gtk4"
require "json"
require "../daemon/endpoint"
require "./adw"

module Xd
  module UI
    # Updates the machine a connection points at.
    #
    # Installing and restarting are separate buttons on purpose. Replacing the
    # files is safe while turns run; restarting drops every attached device and
    # loses whatever the agent was doing, so it is never automatic.
    class DaemonUpdateDialog
      def initialize(
        @parent : Gtk::Window,
        @endpoint : Daemon::Endpoint,
        @machine : String?,
      )
        @closed = false
        @supported = false
        @state = "idle"

        @window = Adw::Window.new
        @window.transient_for = @parent
        @window.modal = true
        @window.default_width = 460
        @window.title = title
        @window.add_css_class("xd-panel")

        @status = Gtk::Label.new("Checking for an update…")
        @status.xalign = 0_f32
        @status.wrap = true

        @detail = Gtk::Label.new("")
        @detail.xalign = 0_f32
        @detail.wrap = true
        @detail.add_css_class("dim-label")
        @detail.add_css_class("caption")

        @install = Gtk::Button.new_with_label("Install")
        @install.add_css_class("suggested-action")
        @install.sensitive = false
        @install.clicked_signal.connect { act("install") }

        @restart = Gtk::Button.new_with_label("Restart")
        @restart.sensitive = false
        @restart.tooltip_text =
          "Restarting drops every attached device and loses any running turn."
        @restart.clicked_signal.connect { act("restart") }

        close = Gtk::Button.new_with_label("Close")
        close.add_css_class("flat")
        close.clicked_signal.connect { @window.destroy }

        header = Gtk::Label.new(title)
        header.xalign = 0_f32
        header.add_css_class("title-3")

        content = Gtk::Box.new(:vertical, 12)
        content.margin_top = 20
        content.margin_bottom = 20
        content.margin_start = 20
        content.margin_end = 20
        content.append(header)
        content.append(@status)
        content.append(@detail)

        actions = Gtk::Box.new(:horizontal, 8)
        actions.halign = :end
        actions.append(close)
        actions.append(@restart)
        actions.append(@install)
        content.append(actions)

        @window.content = content
        @window.close_request_signal.connect { @closed = true; false }
      end

      def present : Nil
        @window.present
        act("check")
      end

      private def title : String
        machine = @machine
        machine && !machine.empty? ? "Update #{machine}" : "Update Daemon"
      end

      private def act(action : String) : Nil
        @install.sensitive = false
        @restart.sensitive = false
        request_async({
          "op"     => JSON::Any.new("daemon-update"),
          "action" => JSON::Any.new(action),
        }) { |body| apply(body, action) }
      end

      private def apply(
        body : Hash(String, JSON::Any),
        action : String,
      ) : Nil
        @supported = body["supported"]?.try(&.as_bool?) || false
        @state = body["state"]?.try(&.as_s?) || "idle"
        available = body["available"]?.try(&.as_bool?) || false
        version = body["version"]?.try(&.as_s?) || "unknown"
        latest = body["latest"]?.try(&.as_s?)
        trouble = body["error"]?.try(&.as_s?)

        if action == "restart"
          @status.text = "Restarting. This connection will drop and come back."
          @detail.text = ""
          return
        end

        @detail.text = String.build do |value|
          value << "Running " << version
          value << " · latest " << latest if latest
        end

        unless @supported
          @status.text =
            "This machine's installation cannot update itself. " \
            "Update it the way it was installed."
          return
        end

        case @state
        when "checking"
          @status.text = "Checking for an update…"
        when "installing"
          @status.text = "Installing. The daemon keeps running until restarted."
        when "installed"
          @status.text = "Installed. Restart to run the new build."
          @restart.sensitive = true
        when "failed"
          @status.text = trouble || "The update failed."
          @install.sensitive = true
        else
          if available
            @status.text = "An update is available."
            @install.sensitive = true
          else
            @status.text = "This machine is up to date."
          end
        end
      end

      private def request_async(
        request : Hash(String, JSON::Any),
        &on_success : Hash(String, JSON::Any) -> Nil
      ) : Nil
        spawn do
          body : Hash(String, JSON::Any)? = nil
          error_message : String? = nil
          begin
            body = @endpoint.call(request)
          rescue error : Daemon::Client::Error
            error_message = error.message || "Daemon request failed."
          end
          GLib.idle_add do
            unless @closed
              if message = error_message
                @status.text = message
                @detail.text = ""
              elsif response = body
                on_success.call(response)
              end
            end
            false
          end
        end
      end
    end
  end
end
