require "json"
require "gtk4"
require "../daemon/endpoint"
require "./adw"
require "./dialogs"

module Xd
  module UI
    # Local-owner control panel for the credentials accepted by this daemon.
    # Device ids are opaque token hashes and never leave this local endpoint.
    class DevicesDialog
      @dialog : Adw::Dialog
      @list : Gtk::ListBox
      @stack : Gtk::Stack
      @status : Adw::StatusPage
      @endpoint : Daemon::Endpoint
      @parent : Gtk::Window
      @closed = false

      def initialize(
        @parent : Gtk::Window,
        @endpoint : Daemon::Endpoint,
      )
        @dialog = Adw::Dialog.new
        @dialog.title = "Connected Devices"
        @dialog.content_width = 620
        @dialog.content_height = 500

        @list = Gtk::ListBox.new
        @list.selection_mode = :none
        @list.add_css_class("boxed-list")
        @list.margin_top = 12
        @list.margin_bottom = 12
        @list.margin_start = 12
        @list.margin_end = 12
        @list.valign = :start

        @status = Adw::StatusPage.new(icon_name: "computer-symbolic")
        @status.title = "No Paired Devices"
        @status.description = "Devices paired with this daemon appear here."

        @stack = Gtk::Stack.new
        @stack.vexpand = true
        @stack.add_named(@status, "empty")
        scroll = Gtk::ScrolledWindow.new
        scroll.set_policy(:never, :automatic)
        scroll.child = @list
        @stack.add_named(scroll, "list")

        header = Adw::HeaderBar.new
        title = Adw::WindowTitle.new(title: "Connected Devices")
        header.title_widget = title
        refresh = Gtk::Button.new_from_icon_name("view-refresh-symbolic")
        refresh.tooltip_text = "Refresh devices"
        refresh.clicked_signal.connect { load }
        header.pack_end(refresh)

        toolbar = Adw::ToolbarView.new
        toolbar.add_top_bar(header)
        toolbar.content = @stack
        @dialog.child = toolbar
        @dialog.closed_signal.connect { @closed = true }
      end

      def present : Nil
        @dialog.present(@parent)
        load
      end

      private def load : Nil
        return if @closed

        request({"op" => JSON::Any.new("devices")}) do |response|
          apply(response)
        end
      end

      private def apply(response : Hash(String, JSON::Any)) : Nil
        while child = @list.first_child
          @list.remove(child)
        end

        values = response["devices"]?.try(&.as_a?) || [] of JSON::Any
        if values.empty?
          @status.title = "No Paired Devices"
          @status.description = "Devices paired with this daemon appear here."
          @stack.visible_child_name = "empty"
          return
        end

        values.each do |value|
          fields = value.as_h?
          next unless fields
          id = fields["id"]?.try(&.as_s?) || next
          name = fields["name"]?.try(&.as_s?) || "Unknown device"
          connected = fields["connected"]?.try(&.as_bool?) || false
          last_seen = fields["last_seen"]?.try(&.as_i64?) || 0_i64
          add_row(id, name, connected, last_seen)
        end
        @stack.visible_child_name = @list.first_child.nil? ? "empty" : "list"
      rescue KeyError | TypeCastError
        show_error("The daemon returned an invalid device list.")
      end

      private def add_row(
        id : String,
        name : String,
        connected : Bool,
        last_seen : Int64,
      ) : Nil
        state = connected ? "Connected" : "Last seen #{last_seen}"
        row = Adw::ActionRow.new(title: name, subtitle: state)

        rename = Gtk::Button.new_from_icon_name("document-edit-symbolic")
        rename.add_css_class("flat")
        rename.tooltip_text = "Rename device"
        rename.valign = :center
        rename.clicked_signal.connect { rename_device(id, name) }
        row.add_suffix(rename)

        revoke = Gtk::Button.new_from_icon_name("user-trash-symbolic")
        revoke.add_css_class("flat")
        revoke.add_css_class("destructive-action")
        revoke.tooltip_text = "Revoke device"
        revoke.valign = :center
        revoke.clicked_signal.connect { confirm_revoke(id, name) }
        row.add_suffix(revoke)
        @list.append(row)
      end

      private def rename_device(id : String, current : String) : Nil
        Dialogs.prompt(
          @parent,
          "Rename Device",
          "This name is controlled by the daemon owner.",
          current
        ) do |name|
          request({
            "op"     => JSON::Any.new("rename-device"),
            "device" => JSON::Any.new(id),
            "name"   => JSON::Any.new(name),
          }) { |_response| load }
        end
      end

      private def confirm_revoke(id : String, name : String) : Nil
        Dialogs.confirm(
          @parent,
          "Revoke #{name}?",
          "This device will be disconnected and must pair again to regain access.",
          "Revoke"
        ) do
          request({
            "op"     => JSON::Any.new("revoke-device"),
            "device" => JSON::Any.new(id),
          }) { |_response| load }
        end
      end

      private def request(
        request : Hash(String, JSON::Any),
        &on_success : Hash(String, JSON::Any) -> Nil,
      ) : Nil
        spawn do
          response : Hash(String, JSON::Any)? = nil
          error_message : String? = nil
          begin
            response = @endpoint.call(request)
          rescue error
            error_message = error.message || "Device request failed."
          end
          GLib.idle_add do
            unless @closed
              if message = error_message
                show_error(message)
              elsif body = response
                on_success.call(body)
              end
            end
            false
          end
        end
      end

      private def show_error(message : String) : Nil
        @status.title = "Device Request Failed"
        @status.description = message
        @stack.visible_child_name = "empty"
      end
    end
  end
end
