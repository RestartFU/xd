require "gtk4"
require "../version"
require "./branch_build"
require "./branch_build_run"

module Xd
  module UI
    class BranchBuildDialog
      @@run = BranchBuildRun.new
      @@open : BranchBuildDialog?

      def self.present(parent : Gtk::Window, on_installed : Proc(Nil)) : Nil
        @@run.on_installed = -> { GLib.idle_add { on_installed.call; false } }
        if dialog = @@open
          return dialog.present
        end
        @@open = new(parent, @@run)
        @@open.not_nil!.present
      end

      def initialize(@parent : Gtk::Window, @run : BranchBuildRun)
        @settings = Gio::Settings.new(APP_ID)
        @closed = false
        @window = Gtk::Window.new
        @window.title = "Build XD Source"
        @window.transient_for = @parent
        @window.application = @parent.application
        @window.destroy_with_parent = true
        @window.modal = true
        @window.decorated = false
        @window.set_default_size(680, -1)
        @window.add_css_class("xd-panel")

        title = Gtk::Label.new("Build XD Source")
        title.xalign = 0_f32
        title.add_css_class("title-3")
        description = Gtk::Label.new(
          "Fetch a branch, pull request, or commit; build the same Linux " \
          "nightly bundle; then install it over this nightly. Requires Docker."
        )
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class("dim-label")
        header = Gtk::Box.new(:vertical, 5)
        header.append(title)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        @entry = Gtk::Entry.new
        @entry.text = @settings.string("build-source")
        @entry.placeholder_text = "main, #128, GitHub URL, or commit SHA"
        @entry.changed_signal.connect { @run.clear_trouble; show_state }
        @entry.activate_signal.connect { start_build }
        @status = Gtk::Label.new("")
        @status.xalign = 0_f32
        @status.wrap = true
        @activity = Gtk::Label.new("")
        @activity.xalign = 0_f32
        @activity.ellipsize = :end
        @activity.selectable = true
        @activity.add_css_class("xd-workflow-log")
        @activity.visible = false
        body = Gtk::Box.new(:vertical, 12)
        body.margin_top = 22
        body.margin_bottom = 22
        body.margin_start = 22
        body.margin_end = 22
        body.append(@entry)
        body.append(@status)
        body.append(@activity)

        footer = Gtk::Box.new(:horizontal, 12)
        spacer = Gtk::Box.new(:horizontal, 0)
        spacer.hexpand = true
        footer.append(spacer)
        @spinner = Gtk::Spinner.new
        @spinner.visible = false
        footer.append(@spinner)
        close_button = Gtk::Button.new_with_label("Close")
        close_button.add_css_class("flat")
        close_button.clicked_signal.connect { dismiss }
        footer.append(close_button)
        @action = Gtk::Button.new_with_label("Build and Install")
        @action.add_css_class("xd-panel-action")
        @action.clicked_signal.connect { @run.running ? @run.stop : start_build }
        footer.append(@action)
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")
        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(body)
        column.append(footer)
        @window.child = column
        @window.default_widget = @action

        keys = Gtk::EventControllerKey.new
        keys.key_pressed_signal.connect do |keyval, _keycode, _state|
          keyval == Gdk::KEY_Escape ? (dismiss; true) : false
        end
        @window.add_controller(keys)
        @window.close_request_signal.connect { dismiss; true }
        @window.destroy_signal.connect { cleanup }
        @run.on_change = ->(installed : Bool) {
          GLib.idle_add { show_state unless @closed; dismiss if installed && !@closed; false }
        }
        GLib.timeout(500.milliseconds) do
          show_state if @run.running && !@closed
          !@closed
        end
        show_state
      end

      def present : Nil
        @window.present
        @entry.grab_focus
      end

      private def start_build : Nil
        return if @run.running
        target = BranchBuild.parse(@entry.text)
        return unless target
        @settings.set_string("build-source", @entry.text)
        @run.start(target)
        show_state
      end

      private def show_state : Nil
        target = BranchBuild.parse(@entry.text)
        @entry.sensitive = !@run.running
        @spinner.visible = @run.running
        @spinner.spinning = @run.running
        if @run.running
          @status.text = "Building #{@run.label}…"
          @action.label = "Stop"
          @action.add_css_class("destructive-action")
          @action.sensitive = true
          @activity.text = @run.last_line
          @activity.visible = true
        else
          @status.text = @run.trouble || target.try(&.label) ||
                         (@entry.text.empty? ? "Enter a branch, PR, or commit." : "Source is not valid.")
          @action.label = "Build and Install"
          @action.remove_css_class("destructive-action")
          @action.sensitive = !!target && BranchBuild.supported?
          @activity.text = @run.tail
          @activity.visible = !@run.tail.empty?
        end
      end

      private def dismiss : Nil
        return if @closed
        @settings.set_string("build-source", @entry.text)
        @window.destroy
      end

      private def cleanup : Nil
        return if @closed
        @closed = true
        @run.on_change = nil
        @@open = nil if @@open.same?(self)
      end
    end
  end
end
