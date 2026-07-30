require "gtk4"
require "../version"
require "./branch_build"
require "./branch_build_run"

module Xd
  module UI
    class BranchBuildDialog
      @@run = BranchBuildRun.new
      @@open : BranchBuildDialog?

      def self.present(
        parent : Gtk::Window,
        on_installed : Proc(Nil),
      ) : Nil
        @@run.on_installed = -> {
          GLib.idle_add do
            on_installed.call
            false
          end
        }
        if dialog = @@open
          dialog.present
          return
        end

        @@open = new(parent, @@run)
        @@open.not_nil!.present
      end

      def initialize(
        @parent : Gtk::Window,
        @run : BranchBuildRun,
      )
        @settings = Gio::Settings.new(APP_ID)
        @closed = false
        @focused = false

        @window = Gtk::Window.new
        @window.title = "Build a Branch"
        @window.transient_for = @parent
        @window.application = @parent.application
        @window.destroy_with_parent = true
        @window.decorated = false
        @window.set_default_size(620, -1)
        @window.add_css_class("xd-panel")

        title = Gtk::Label.new("Build a Branch")
        title.xalign = 0_f32
        title.add_css_class("title-3")

        description = Gtk::Label.new(
          "The branch is fetched, built the way the nightly is built, " \
          "and installed over this copy. It needs git and docker. " \
          "The update button puts the nightly back."
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
        @entry.placeholder_text =
          "https://github.com/RestartFU/xd/pull/128, or a branch name"
        @entry.changed_signal.connect do
          @run.clear_trouble
          show_state
        end
        @entry.activate_signal.connect { start_build }

        @status = Gtk::Label.new("")
        @status.xalign = 0_f32
        @status.wrap = true
        @status.add_css_class("dim-label")

        @activity = Gtk::Label.new("")
        @activity.xalign = 0_f32
        @activity.ellipsize = :end
        @activity.selectable = true
        @activity.add_css_class("xd-workflow-log")
        @activity.visible = false

        body = Gtk::Box.new(:vertical, 12)
        body.margin_top = 20
        body.margin_bottom = 20
        body.margin_start = 22
        body.margin_end = 22
        body.append(@entry)
        body.append(@status)
        body.append(@activity)

        footer = Gtk::Box.new(:horizontal, 12)
        footer.append(hint("Esc", "Close"))
        footer.append(hint("Enter", "Build"))
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
        @action.clicked_signal.connect do
          @run.running ? @run.stop : start_build
        end
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
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, _state|
          if keyval == Gdk::KEY_Escape
            dismiss
            true
          else
            false
          end
        end
        @window.add_controller(keys)

        @window.notify_signal["is-active"].connect do |_property|
          if @window.is_active?
            @focused = true
          elsif @focused
            # Destroying a window from inside its focus notification is
            # re-entrant in Mutter: GTK is still dispatching against widgets
            # that cleanup would release. Close on the next main-loop turn.
            GLib.idle_add do
              dismiss unless @closed
              false
            end
          end
        end
        @window.close_request_signal.connect do
          save_source
          false
        end
        @window.destroy_signal.connect { cleanup }
        @run.on_change = ->(installed : Bool) {
          GLib.idle_add do
            unless @closed
              show_state
              dismiss if installed
            end
            false
          end
        }

        GLib.timeout(500.milliseconds) do
          unless @closed
            show_state if @run.running
          end
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
          @action.remove_css_class("suggested-action")
          @action.add_css_class("destructive-action")
          @action.sensitive = true
          @activity.text = @run.last_line
          @activity.visible = true
          return
        end

        @status.text = if trouble = @run.trouble
                         trouble
                       elsif target
                         target.label
                       elsif @entry.text.empty?
                         "A pull request link, a branch link, or a branch name."
                       else
                         "Not a pull request or a branch."
                       end
        @action.label = "Build and Install"
        @action.remove_css_class("destructive-action")
        @action.add_css_class("suggested-action")
        @action.sensitive = !!target

        if @run.trouble && !@run.tail.empty?
          @activity.text = @run.tail
          @activity.visible = true
        else
          @activity.visible = false
        end
      end

      private def hint(key : String, what : String) : Gtk::Box
        name = Gtk::Label.new(key)
        name.add_css_class("xd-key")
        label = Gtk::Label.new(what)
        label.add_css_class("dim-label")

        row = Gtk::Box.new(:horizontal, 6)
        row.append(name)
        row.append(label)
        row
      end

      private def dismiss : Nil
        return if @closed

        save_source
        @window.destroy
      end

      private def save_source : Nil
        @settings.set_string("build-source", @entry.text)
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
