require "gtk4"
require "../agent/environment"
require "../version"
require "./branch_build_dialog"
require "./update_channel"

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

      @process : Process?
      @install_dir : String?
      @trouble : String?

      def initialize(@parent : Gtk::Window)
        @state = State::Quiet
        @trouble = nil
        @process = nil
        @checking = false
        @closed = false
        @check_id = 0_u32
        @install_dir = find_install_dir

        @button = Gtk::Button.new
        @button.add_css_class("suggested-action")
        @button.add_css_class("xd-update")
        @button.visible = false
        @button.clicked_signal.connect { clicked }

        @branch = Gtk::Button.new_from_icon_name(
          "applications-engineering-symbolic"
        )
        @branch.add_css_class("flat")
        @branch.tooltip_text =
          "Build a pull request or a branch and install it"
        @branch.visible = !!@install_dir &&
                          UpdateChannel.current.nightly?
        @branch.clicked_signal.connect do
          BranchBuildDialog.present(@parent, -> { set_state(State::Done) })
        end

        @widget = Gtk::Box.new(:horizontal, 6)
        @widget.halign = :start
        @widget.margin_start = 6
        @widget.margin_end = 6
        @widget.margin_top = 6
        @widget.margin_bottom = 6
        @widget.append(@button)
        @widget.append(@branch)
        @widget.visible = @branch.visible?

        schedule_checks if @install_dir
      end

      def close : Nil
        return if @closed

        @closed = true
        GLib.source_remove(@check_id) unless @check_id == 0
        @check_id = 0_u32
        @process.try(&.terminate(graceful: false))
        @process = nil
      rescue RuntimeError
      end

      private def find_install_dir : String?
        executable = Process.executable_path
        return unless executable

        directory = File.dirname(File.dirname(executable))
        launcher = File.join(directory, "xd.sh")
        return unless File::Info.executable?(launcher)

        expected = File.join(
          Path.home.to_s,
          ".local",
          "opt",
          DATA_NAME
        )
        directory == expected ? directory : nil
      rescue File::Error
        nil
      end

      private def schedule_checks : Nil
        @check_id = GLib.timeout(FIRST_CHECK) do
          look
          @check_id = GLib.timeout(POLL) do
            look
            !@closed
          end
          false
        end
      end

      private def look : Nil
        return if @closed || @checking || !@state.quiet?
        return unless @install_dir

        @checking = true
        spawn do
          output = IO::Memory.new
          status = Process.run(
            "curl",
            [
              "-fsSL",
              "--max-time", "20",
              "-H", "Accept: application/vnd.github+json",
              UpdateChannel.check_url(UpdateChannel.current),
            ],
            env: Agent::Environment.host,
            clear_env: true,
            input: Process::Redirect::Close,
            output: output,
            error: Process::Redirect::Close
          )
          latest = status.success? ? UpdateChannel.latest_from_reply(
            UpdateChannel.current,
            output.to_s
          ) : nil
          GLib.idle_add do
            @checking = false
            if !@closed &&
               @state.quiet? &&
               UpdateChannel.newer?(UpdateChannel.current, latest)
              set_state(State::Available)
            end
            false
          end
        rescue File::Error | IO::Error
          GLib.idle_add do
            @checking = false
            false
          end
        end
      end

      private def clicked : Nil
        case @state
        when .available?, .failed?
          install
        when .done?
          restart
        end
      end

      private def install : Nil
        return if @closed || @state.updating?

        set_state(State::Updating)
        spawn do
          error_output = IO::Memory.new
          process = Process.new(
            [
              "sh",
              "-c",
              UpdateChannel.install_command(UpdateChannel.current),
            ],
            env: Agent::Environment.host,
            clear_env: true,
            input: Process::Redirect::Close,
            output: Process::Redirect::Close,
            error: error_output
          )
          @process = process
          status = process.wait
          GLib.idle_add do
            @process = nil
            unless @closed
              if status.success?
                set_state(State::Done)
              else
                message = error_output.to_s.strip
                @trouble = message.empty? ? "The update did not install." : message
                set_state(State::Failed)
              end
            end
            false
          end
        rescue error : File::Error | IO::Error
          GLib.idle_add do
            @process = nil
            unless @closed
              @trouble = error.message || "Cannot start the installer."
              set_state(State::Failed)
            end
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
        @trouble = error.message || "Cannot restart xd."
        set_state(State::Failed)
      end

      private def set_state(state : State) : Nil
        @state = state
        icon = nil
        tip = nil
        fades = false

        case state
        when .available?
          icon = "document-save-symbolic"
          tip = "A newer build is available. Click to install it."
          fades = true
        when .updating?
          icon = "document-save-symbolic"
          tip = "Downloading and installing."
        when .done?
          icon = "view-refresh-symbolic"
          tip = "The new build is installed. Click to restart into it."
        when .failed?
          icon = "document-save-symbolic"
          tip = @trouble
          fades = true
        end

        @button.visible = !!icon
        @widget.visible = !!icon || @branch.visible?
        return unless icon

        @button.icon_name = icon
        @button.tooltip_text = tip
        @button.sensitive = !state.updating?
        @button.can_target = !state.updating?
        if fades
          @button.add_css_class("xd-update-fade")
        else
          @button.remove_css_class("xd-update-fade")
        end
      end
    end
  end
end
