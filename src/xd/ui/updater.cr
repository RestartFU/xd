require "gtk4"
require "../agent/environment"
require "../bundle_environment"
require "../version"
require "./branch_build_dialog"
require "../update_channel"

module Xd
  module UI
    class Updater
      FIRST_CHECK = 8.seconds
      POLL        = 5.minutes

      private enum State
        Quiet
        Available
        Updating
        Done
        Failed
      end

      getter widget : Gtk::Box

      @install_dir : String?

      def initialize(@parent : Gtk::Window)
        @state = State::Quiet
        @process = nil.as(Process?)
        @trouble = nil.as(String?)
        @checking = false
        @closed = false
        @check_id = 0_u32
        @install_dir = find_install_dir
        @curl = BundleEnvironment.executable("curl") || "curl"

        @button = Gtk::Button.new_from_icon_name("software-update-available-symbolic")
        @button.add_css_class("suggested-action")
        @button.visible = false
        @button.clicked_signal.connect { clicked }
        @branch = Gtk::Button.new_from_icon_name("applications-engineering-symbolic")
        @branch.add_css_class("flat")
        @branch.tooltip_text = "Build XD from a branch, pull request, or commit"
        @branch.visible = !!@install_dir && Xd::UpdateChannel.current.nightly? && BranchBuild.supported?
        @branch.clicked_signal.connect do
          BranchBuildDialog.present(@parent, -> { set_state(State::Done) })
        end
        @widget = Gtk::Box.new(:horizontal, 6)
        @widget.append(@button)
        @widget.append(@branch)
        @widget.visible = @branch.visible?
        schedule_checks if @install_dir
      end

      def close : Nil
        return if @closed
        @closed = true
        GLib.source_remove(@check_id) unless @check_id == 0
        @process.try(&.terminate(graceful: false))
      rescue RuntimeError
      end

      private def find_install_dir : String?
        {% if flag?(:linux) && flag?(:x86_64) %}
          executable = Process.executable_path
          return unless executable
          directory = File.dirname(File.dirname(executable))
          expected = File.join(Path.home, ".local", "opt", DATA_NAME)
          launcher = File.join(directory, "xd.sh")
          directory == expected && File::Info.executable?(launcher) ? directory : nil
        {% else %}
          nil
        {% end %}
      rescue File::Error
        nil
      end

      private def schedule_checks : Nil
        @check_id = GLib.timeout(FIRST_CHECK) do
          look
          @check_id = GLib.timeout(POLL) { look; !@closed }
          false
        end
      end

      private def look : Nil
        return if @closed || @checking || !@state.quiet?
        @checking = true
        Fiber::ExecutionContext::Isolated.new("xd update check") do
          output = IO::Memory.new
          status = Process.run(
            @curl,
            ["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", Xd::UpdateChannel.check_url(Xd::UpdateChannel.current)],
            env: Agent::Environment.host,
            clear_env: true,
            input: Process::Redirect::Close,
            output: output,
            error: Process::Redirect::Close
          )
          latest = status.success? ? Xd::UpdateChannel.latest_from_reply(Xd::UpdateChannel.current, output.to_s) : nil
          GLib.idle_add do
            @checking = false
            set_state(State::Available) if !@closed && @state.quiet? && Xd::UpdateChannel.newer?(Xd::UpdateChannel.current, latest)
            false
          end
        rescue File::Error | IO::Error
          GLib.idle_add { @checking = false; false }
        end
      end

      private def clicked : Nil
        case @state
        when .available?, .failed? then install
        when .done?                then restart
        end
      end

      private def install : Nil
        return if @closed || @state.updating?
        set_state(State::Updating)
        spawn do
          errors = IO::Memory.new
          environment = Agent::Environment.host
          environment["XD_CURL"] = @curl
          process = Process.new(
            ["sh", "-c", Xd::UpdateChannel.install_command(Xd::UpdateChannel.current, @curl)],
            env: environment,
            clear_env: true,
            input: Process::Redirect::Close,
            output: Process::Redirect::Close,
            error: errors
          )
          @process = process
          status = process.wait
          GLib.idle_add do
            @process = nil
            unless @closed
              @trouble = errors.to_s.strip
              set_state(status.success? ? State::Done : State::Failed)
            end
            false
          end
        rescue error : File::Error | IO::Error
          GLib.idle_add do
            @process = nil
            @trouble = error.message
            set_state(State::Failed) unless @closed
            false
          end
        end
      end

      private def restart : Nil
        directory = @install_dir
        return unless directory
        Process.new(
          [File.join(directory, "xd.sh")],
          env: Agent::Environment.host,
          clear_env: true,
          input: Process::Redirect::Close,
          output: Process::Redirect::Close,
          error: Process::Redirect::Close
        )
        @parent.application.try(&.quit)
      rescue error : File::Error | IO::Error
        @trouble = error.message
        set_state(State::Failed)
      end

      private def set_state(state : State) : Nil
        @state = state
        case state
        when .available?
          @button.icon_name = "software-update-available-symbolic"
          @button.tooltip_text = "New XD build available. Click to install."
          @button.sensitive = true
          @button.visible = true
        when .updating?
          @button.tooltip_text = "Downloading and installing…"
          @button.sensitive = false
          @button.visible = true
        when .done?
          @button.icon_name = "view-refresh-symbolic"
          @button.tooltip_text = "Installed. Click to restart XD."
          @button.sensitive = true
          @button.visible = true
        when .failed?
          @button.icon_name = "dialog-error-symbolic"
          @button.tooltip_text = @trouble.presence || "Update failed. Click to retry."
          @button.sensitive = true
          @button.visible = true
        else
          @button.visible = false
        end
        @widget.visible = @button.visible? || @branch.visible?
      end
    end
  end
end
