require "json"
require "gtk4"
require "../agent/catalog"
require "../daemon/client"
require "../daemon/endpoint"
require "./adw"
require "./dialogs"
require "./host_launch"

module Xd
  module UI
    # Authentication UI for the CLIs installed on one daemon machine.
    #
    # The panel never launches an agent itself. Every action goes through its
    # endpoint, so local Unix and paired TLS clients operate the same daemon
    # service and credentials remain on the machine that runs the agent.
    class AuthDialog
      ANSI = /\e\[[0-?]*[ -\/]*[@-~]/
      URL  = %r{https://[^\s<>"']+}

      @active_provider : String?

      private class ProviderRow
        getter provider : String
        getter row : Adw::ActionRow
        getter action : Gtk::Button
        property state = "unknown"
        property detail : String?
        property output = ""

        def initialize(
          @provider : String,
          @row : Adw::ActionRow,
          @action : Gtk::Button,
        )
          @detail = nil
        end
      end

      def initialize(
        @parent : Gtk::Window,
        @endpoint : Daemon::Endpoint,
        machine : String? = nil,
      )
        @closed = false
        @subscription = 0_i64
        @active_provider = nil
        @rows = {} of String => ProviderRow

        title = Gtk::Label.new(
          machine ? "Assistant Accounts · #{machine}" : "Assistant Accounts · This Machine"
        )
        title.xalign = 0_f32
        title.add_css_class("title-3")

        description = Gtk::Label.new(
          machine ? "Sign in to the bundled CLIs on this machine. Credentials stay " \
                    "on the remote machine and are used only by its daemon." : "Sign in to the bundled CLIs on this machine. Credentials stay " \
                                                                               "on this machine and are used only by its daemon."
        )
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class("dim-label")

        header = Gtk::Box.new(:vertical, 5)
        header.append(title)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        accounts = Adw::PreferencesGroup.new
        accounts.title = "Bundled assistants"
        accounts.description =
          "Sign-in happens in the official Codex and Claude Code CLIs."
        Agent::Catalog.all.each do |backend|
          provider = build_provider(backend)
          @rows[backend.id] = provider
          accounts.add(provider.row)
        end

        @instructions_label = Gtk::Label.new("Sign-in instructions")
        @instructions_label.xalign = 0_f32
        @instructions_label.add_css_class("caption")
        @instructions_label.add_css_class("dim-label")
        @instructions_label.visible = false

        @output = Gtk::TextView.new
        @output.editable = false
        @output.cursor_visible = false
        @output.wrap_mode = :word_char
        @output.top_margin = 10
        @output.bottom_margin = 10
        @output.left_margin = 10
        @output.right_margin = 10
        @output.add_css_class("monospace")

        output_scroll = Gtk::ScrolledWindow.new
        output_scroll.set_policy(:never, :automatic)
        output_scroll.vexpand = true
        output_scroll.child = @output

        @output_frame = Gtk::Frame.new
        @output_frame.vexpand = true
        @output_frame.child = output_scroll
        @output_frame.visible = false

        @open = Gtk::Button.new_with_label("Open Sign-In Page")
        @open.add_css_class("flat")
        @open.visible = false
        @open.clicked_signal.connect do
          if uri = current_uri
            HostLaunch.open_uri(uri)
          end
        end

        @code = Gtk::Entry.new
        @code.placeholder_text = "Paste the authorization code"
        @code.hexpand = true
        @code.visible = false
        @code.activate_signal.connect { send_input }

        @send = Gtk::Button.new_with_label("Send Code")
        @send.add_css_class("suggested-action")
        @send.visible = false
        @send.clicked_signal.connect { send_input }

        input_row = Gtk::Box.new(:horizontal, 8)
        input_row.append(@code)
        input_row.append(@send)

        controls = Gtk::Box.new(:horizontal, 8)
        controls.append(@open)
        controls.append(input_row)

        @status = Gtk::Label.new("")
        @status.xalign = 0_f32
        @status.wrap = true
        @status.visible = false
        @status.add_css_class("dim-label")

        body = Gtk::Box.new(:vertical, 10)
        body.margin_top = 22
        body.margin_bottom = 22
        body.margin_start = 22
        body.margin_end = 22
        body.vexpand = true
        body.append(accounts)
        body.append(@instructions_label)
        body.append(@output_frame)
        body.append(controls)
        body.append(@status)

        footer = Gtk::Box.new(:horizontal, 12)
        footer.append(hint("Esc", "Close"))
        spacer = Gtk::Box.new(:horizontal, 0)
        spacer.hexpand = true
        footer.append(spacer)

        refresh = Gtk::Button.new_with_label("Refresh")
        refresh.add_css_class("flat")
        refresh.clicked_signal.connect { load }
        footer.append(refresh)

        close_button = Gtk::Button.new_with_label("Close")
        close_button.add_css_class("xd-panel-action")
        close_button.clicked_signal.connect { close }
        footer.append(close_button)
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
        @window.set_default_size(700, 560)
        @window.add_css_class("xd-panel")
        @window.child = column
        @window.destroy_signal.connect { closed }

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
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
        @subscription = @endpoint.subscribe do |event|
          name = event["event"]?.try(&.as_s?)
          next unless name == "agent-auth-changed" ||
                      name == "agent-auth-output"

          GLib.idle_add do
            handle_event(event) unless @closed
            false
          end
        end
        @window.present
        load
      end

      private def build_provider(
        backend : Agent::Backend,
      ) : ProviderRow
        row = Adw::ActionRow.new
        row.title = backend.display_name
        row.subtitle = "Checking sign-in status…"

        icon = Gtk::Image.new_from_icon_name(backend.icon_name)
        icon.pixel_size = 24
        row.add_prefix(icon)

        action = Gtk::Button.new_with_label("Check")
        action.valign = :center
        action.clicked_signal.connect { provider_action(backend.id) }
        row.add_suffix(action)
        ProviderRow.new(backend.id, row, action)
      end

      private def load : Nil
        return if @closed

        show_status("Checking assistant accounts…", false)
        request_async({"op" => JSON::Any.new("agent-auth")}) do |body|
          providers = body["providers"]?.try(&.as_a?) || [] of JSON::Any
          providers.each do |provider|
            if fields = provider.as_h?
              apply_snapshot(fields)
            end
          end
          show_status(nil, false)
        end
      end

      private def provider_action(provider : String) : Nil
        row = @rows[provider]? || return
        case row.state
        when "signed-in"
          confirm_logout(row)
        when "signing-in"
          request_provider("agent-auth-cancel", provider)
        when "checking", "signing-out"
        else
          @active_provider = provider
          row.output = ""
          update_instructions(row)
          request_provider("agent-auth-start", provider)
        end
      end

      private def confirm_logout(row : ProviderRow) : Nil
        Dialogs.confirm(
          @window,
          "Sign Out of #{row.row.title}?",
          "The bundled CLI on this machine will stop using this account.",
          "Sign Out"
        ) do
          request_provider("agent-auth-logout", row.provider)
        end
      end

      private def request_provider(operation : String, provider : String) : Nil
        request_async({
          "op"       => JSON::Any.new(operation),
          "provider" => JSON::Any.new(provider),
        }) { |_body| }
      end

      private def send_input : Nil
        provider = @active_provider
        value = @code.text.strip
        return unless provider
        return if value.empty?

        @code.sensitive = false
        @send.sensitive = false
        request_async({
          "op"       => JSON::Any.new("agent-auth-input"),
          "provider" => JSON::Any.new(provider),
          "input"    => JSON::Any.new(value),
        }) do |_body|
          @code.text = ""
          @code.sensitive = true
          @send.sensitive = true
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
                show_status(message, true)
                @code.sensitive = true
                @send.sensitive = true
              elsif response = body
                on_success.call(response)
              end
            end
            false
          end
        end
      end

      private def handle_event(event : Hash(String, JSON::Any)) : Nil
        provider = event["provider"]?.try(&.as_s?) || return
        row = @rows[provider]? || return
        case event["event"]?.try(&.as_s?)
        when "agent-auth-changed"
          apply_snapshot(event)
        when "agent-auth-output"
          row.output += event["text"]?.try(&.as_s?) || ""
          @active_provider = provider
          update_instructions(row)
        end
      end

      private def apply_snapshot(
        fields : Hash(String, JSON::Any),
      ) : Nil
        provider = fields["provider"]?.try(&.as_s?) || return
        row = @rows[provider]? || return
        row.state = fields["state"]?.try(&.as_s?) || "unknown"
        row.detail = fields["detail"]?.try(&.as_s?)
        if output = fields["output"]?.try(&.as_s?)
          row.output = output
        end

        row.row.subtitle = row.detail || state_label(row.state)
        label, sensitive = action_state(row.state)
        row.action.label = label
        row.action.sensitive = sensitive

        if row.state == "signing-in" || !row.output.empty?
          @active_provider = provider
          update_instructions(row)
        elsif @active_provider == provider
          @active_provider = nil
          hide_instructions
        end
      end

      private def state_label(state : String) : String
        case state
        when "checking"    then "Checking sign-in status…"
        when "signed-in"   then "Signed in."
        when "signed-out"  then "Not signed in."
        when "signing-in"  then "Waiting for sign-in…"
        when "signing-out" then "Signing out…"
        when "failed"      then "Authentication failed."
        else                    "Status unknown."
        end
      end

      private def action_state(state : String) : {String, Bool}
        case state
        when "signed-in"   then {"Sign Out", true}
        when "signing-in"  then {"Cancel", true}
        when "checking"    then {"Checking…", false}
        when "signing-out" then {"Signing Out…", false}
        else                    {"Sign In", true}
        end
      end

      private def update_instructions(row : ProviderRow) : Nil
        text = row.output.gsub(ANSI, "")
        @output.buffer.text = text
        visible = row.state == "signing-in" || !text.empty?
        @instructions_label.visible = visible
        @output_frame.visible = visible
        @open.visible = !!text.match(URL)

        wants_input = row.provider == "claude" &&
                      row.state == "signing-in"
        @code.visible = wants_input
        @send.visible = wants_input
        if wants_input && text.includes?("Paste code")
          @code.grab_focus
        end
      end

      private def hide_instructions : Nil
        @instructions_label.visible = false
        @output_frame.visible = false
        @open.visible = false
        @code.visible = false
        @send.visible = false
      end

      private def current_uri : String?
        provider = @active_provider
        return unless provider
        text = @rows[provider]?.try(&.output) || ""
        text.gsub(ANSI, "").match(URL).try(&.[0])
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

      private def close : Nil
        @window.destroy unless @closed
      end

      private def closed : Nil
        return if @closed

        @closed = true
        @endpoint.unsubscribe(@subscription) unless @subscription == 0
        @rows.each_value do |row|
          next unless row.state == "signing-in"

          spawn do
            @endpoint.call({
              "op"       => JSON::Any.new("agent-auth-cancel"),
              "provider" => JSON::Any.new(row.provider),
            })
          rescue Daemon::Client::Error
          end
        end
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
