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
      @active_provider : String?

      private class ProviderRow
        getter provider : String
        getter row : Adw::ActionRow
        getter action : Gtk::Button
        property state = "unknown"
        property detail : String?
        property login_url : String?
        property device_code : String?
        property needs_input = false

        def initialize(
          @provider : String,
          @row : Adw::ActionRow,
          @action : Gtk::Button,
        )
          @detail = nil
          @login_url = nil
          @device_code = nil
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
        @cli_states = {} of String => String
        @cli_versions = {} of String => String?
        @cli_details = {} of String => String?

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

        cli_versions = Adw::PreferencesGroup.new
        cli_versions.title = "Bundled CLI versions"
        cli_versions.description =
          "Bundled with xd and updated only when xd updates."
        @cli_version_row = Adw::ActionRow.new
        @cli_version_row.title = "Codex and Claude Code"
        @cli_version_row.subtitle = "Checking installed versions…"
        cli_versions.add(@cli_version_row)

        @open = Gtk::Button.new_with_label("Open Sign-In Page")
        @open.add_css_class("suggested-action")
        @open.valign = :center
        @open.clicked_signal.connect do
          if uri = current_uri
            HostLaunch.open_uri(uri)
          end
        end

        @open_row = Adw::ActionRow.new
        @open_row.title = "Continue in your browser"
        @open_row.subtitle =
          "Open the official sign-in page for this assistant."
        @open_row.add_suffix(@open)

        @device_code = Gtk::Label.new("")
        @device_code.selectable = true
        @device_code.add_css_class("xd-auth-code")
        @device_code.valign = :center

        copy = Gtk::Button.new_with_label("Copy")
        copy.add_css_class("flat")
        copy.valign = :center
        copy.clicked_signal.connect { copy_device_code }

        @device_row = Adw::ActionRow.new
        @device_row.title = "One-time code"
        @device_row.subtitle = "Enter this code on the sign-in page."
        @device_row.add_suffix(@device_code)
        @device_row.add_suffix(copy)

        @code = Adw::EntryRow.new
        @code.title = "Paste authorization code"
        @code.apply_signal.connect { send_input }

        @send = Gtk::Button.new_with_label("Finish Sign-In")
        @send.valign = :center
        @send.add_css_class("suggested-action")
        @send.clicked_signal.connect { send_input }
        @code.add_suffix(@send)

        @instructions = Adw::PreferencesGroup.new
        @instructions.title = "Sign in"
        @instructions.description =
          "Complete authentication with the official assistant service."
        @instructions.add(@open_row)
        @instructions.add(@device_row)
        @instructions.add(@code)
        @instructions.visible = false

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
        body.append(cli_versions)
        body.append(@instructions)
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
        @window.title = "Assistant Accounts"
        @window.transient_for = @parent
        @window.application = @parent.application
        @window.destroy_with_parent = true
        @window.modal = true
        @window.decorated = false
        @window.resizable = false
        @window.set_default_size(700, -1)
        @window.add_css_class("xd-panel")
        @window.child = column
        @window.destroy_signal.connect { closed }
        @window.close_request_signal.connect do
          close
          true
        end

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if @code.visible? &&
             state.includes?(Gdk::ModifierType::ControlMask) &&
             Gdk.keyval_to_lower(keyval) == Gdk::KEY_v
            paste_code
            true
          elsif keyval == Gdk::KEY_Escape
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
          next unless {
                        "agent-auth-changed",
                        "agent-cli-changed",
                      }.includes?(name)

          GLib.idle_add do
            if name == "agent-auth-changed"
              handle_event(event) unless @closed
            else
              apply_cli_snapshot(event) unless @closed
            end
            false
          end
        end
        @window.present
        load
        load_clis
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

      private def load_clis : Nil
        return if @closed

        request_async({"op" => JSON::Any.new("agent-clis")}) do |body|
          providers = body["providers"]?.try(&.as_a?) || [] of JSON::Any
          providers.each do |provider|
            if fields = provider.as_h?
              apply_cli_snapshot(fields)
            end
          end
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
          row.state = "signing-in"
          row.detail = nil
          row.login_url = nil
          row.device_code = nil
          row.needs_input = false
          update_provider(row)
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

      private def paste_code : Nil
        @code.clipboard.read_text_async(nil) do |source, result|
          begin
            text = source.as(Gdk::Clipboard).read_text_finish(result)
            unless @closed || !@code.visible? || text.nil?
              @code.text = text
              @code.grab_focus
            end
          rescue error
            show_status(
              error.message || "Cannot paste the authorization code.",
              true
            )
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
                show_status(message, true)
                @code.sensitive = true
                @send.sensitive = true
                update_cli_version_row
              elsif response = body
                on_success.call(response)
              end
            end
            false
          end
        end
      end

      private def handle_event(event : Hash(String, JSON::Any)) : Nil
        apply_snapshot(event)
      end

      private def apply_cli_snapshot(
        fields : Hash(String, JSON::Any),
      ) : Nil
        provider = fields["provider"]?.try(&.as_s?) || return
        return unless Agent::Catalog.lookup(provider)

        @cli_states[provider] =
          fields["state"]?.try(&.as_s?) || "idle"
        @cli_versions[provider] = fields["version"]?.try(&.as_s?)
        @cli_details[provider] = fields["detail"]?.try(&.as_s?)
        update_cli_version_row
      end

      private def update_cli_version_row : Nil
        failed = @cli_states.find { |_provider, state| state == "failed" }
        if failed
          provider = failed[0]
          backend = Agent::Catalog.lookup(provider)
          @cli_version_row.subtitle =
            @cli_details[provider] ||
              "#{backend.try(&.display_name) || provider} version check failed."
        else
          versions = Agent::Catalog.all.map do |backend|
            version = @cli_versions[backend.id]?
            version ? version : "#{backend.display_name}: checking…"
          end
          @cli_version_row.subtitle = versions.join(" · ")
        end
      end

      private def apply_snapshot(
        fields : Hash(String, JSON::Any),
      ) : Nil
        provider = fields["provider"]?.try(&.as_s?) || return
        row = @rows[provider]? || return
        row.state = fields["state"]?.try(&.as_s?) || "unknown"
        row.detail = fields["detail"]?.try(&.as_s?)
        row.login_url = fields["login_url"]?.try(&.as_s?)
        row.device_code = fields["device_code"]?.try(&.as_s?)
        row.needs_input =
          fields["needs_input"]?.try(&.as_bool?) || false
        update_provider(row)

        if row.state == "signing-in"
          @active_provider = provider
          update_instructions(row)
        elsif @active_provider == provider
          @active_provider = nil
          hide_instructions
        end
      end

      private def update_provider(row : ProviderRow) : Nil
        row.row.subtitle = row.detail || state_label(row.state)
        label, sensitive = action_state(row.state)
        row.action.label = label
        row.action.sensitive = sensitive
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
        @instructions.title = "Sign in to #{row.row.title}"
        @instructions.description = if row.provider == "codex"
                                      "Open the page and enter the one-time code."
                                    else
                                      "Open the page, authorize Claude Code, then paste the code."
                                    end
        @instructions.visible = row.state == "signing-in"
        @open_row.visible = !row.login_url.nil?
        @device_row.visible = !row.device_code.nil?
        @device_code.label = row.device_code || ""
        @code.visible = row.needs_input
        if row.needs_input
          @code.grab_focus
        end
      end

      private def hide_instructions : Nil
        @instructions.visible = false
        @open_row.visible = false
        @device_row.visible = false
        @code.visible = false
      end

      private def current_uri : String?
        provider = @active_provider
        return unless provider
        @rows[provider]?.try(&.login_url)
      end

      private def copy_device_code : Nil
        code = @device_code.label
        return if code.empty?

        @device_code.clipboard.set(code)
        show_status("One-time code copied.", false)
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
