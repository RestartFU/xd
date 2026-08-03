require "gtk4"
require "./directory_browser"
require "./panel_call"

module Xd
  module UI
    class WorkspaceCreateDialog
      @repository : String?

      def initialize(
        @parent : Gtk::Window,
        @request : PanelCall,
        @on_create : Proc(String, String?, Nil),
      )
        @repository = nil
        @closed = false

        title = Gtk::Label.new("New Workspace")
        title.xalign = 0_f32
        title.add_css_class("title-3")

        description = Gtk::Label.new(
          "Create a workspace folder and optionally connect it to a Git " \
          "repository."
        )
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class("dim-label")

        header = Gtk::Box.new(:vertical, 5)
        header.append(title)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        name_label = Gtk::Label.new("Workspace name")
        name_label.xalign = 0_f32
        name_label.add_css_class("caption")
        name_label.add_css_class("dim-label")

        @name = Gtk::Entry.new
        @name.placeholder_text = "Workspace name"
        @name.activates_default = true
        @name.changed_signal.connect { update_state }

        name_box = Gtk::Box.new(:vertical, 5)
        name_box.append(name_label)
        name_box.append(@name)

        repository_label = Gtk::Label.new("No repository selected")
        repository_label.xalign = 0_f32
        repository_label.ellipsize = :start
        repository_label.hexpand = true
        repository_label.add_css_class("dim-label")

        choose = Gtk::Button.new_with_label("Choose Git repository…")
        choose.add_css_class("flat")
        choose.clicked_signal.connect do
          DirectoryBrowser.present(@parent, @request, @repository) do |path|
            @repository = path
            repository_label.text = path || "No repository selected"
          end
        end

        clear = Gtk::Button.new_from_icon_name("edit-clear-symbolic")
        clear.add_css_class("flat")
        clear.tooltip_text = "Clear repository"
        clear.clicked_signal.connect do
          @repository = nil
          repository_label.text = "No repository selected"
        end

        repository_box = Gtk::Box.new(:horizontal, 8)
        repository_box.append(repository_label)
        repository_box.append(choose)
        repository_box.append(clear)

        repository_hint = Gtk::Label.new(
          "The assistant will run in this repository."
        )
        repository_hint.xalign = 0_f32
        repository_hint.add_css_class("caption")
        repository_hint.add_css_class("dim-label")

        project_box = Gtk::Box.new(:vertical, 5)
        project_box.append(repository_box)
        project_box.append(repository_hint)

        @status = Gtk::Label.new("")
        @status.xalign = 0_f32
        @status.wrap = true
        @status.visible = false
        @status.add_css_class("error")

        body = Gtk::Box.new(:vertical, 18)
        body.margin_top = 22
        body.margin_bottom = 22
        body.margin_start = 22
        body.margin_end = 22
        body.append(name_box)
        body.append(project_box)
        body.append(@status)

        footer = Gtk::Box.new(:horizontal, 12)
        footer.append(hint("Esc", "Cancel"))
        footer.append(hint("Ctrl Enter", "Create"))
        spacer = Gtk::Box.new(:horizontal, 0)
        spacer.hexpand = true
        footer.append(spacer)

        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.add_css_class("flat")
        cancel.clicked_signal.connect { close }
        footer.append(cancel)

        @create = Gtk::Button.new_with_label("Create")
        @create.add_css_class("xd-panel-action")
        @create.sensitive = false
        @create.clicked_signal.connect { create }
        footer.append(@create)
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")

        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(body)
        column.append(footer)

        @window = Gtk::Window.new
        @window.title = "New Workspace"
        @window.transient_for = @parent
        @window.application = @parent.application
        @window.destroy_with_parent = true
        @window.modal = true
        @window.decorated = false
        @window.set_default_size(620, 360)
        @window.add_css_class("xd-panel")
        @window.child = column
        @window.default_widget = @create
        @window.destroy_signal.connect { closed }
        @window.close_request_signal.connect do
          close
          true
        end

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if keyval == Gdk::KEY_Escape
            close
            true
          elsif (keyval == Gdk::KEY_Return ||
                keyval == Gdk::KEY_KP_Enter) &&
                state.includes?(Gdk::ModifierType::ControlMask)
            create
            true
          else
            false
          end
        end
        @window.add_controller(keys)
      end

      def present : Nil
        @window.present
        @name.grab_focus
      end

      private def create : Nil
        return if @closed || !@create.sensitive?

        name = @name.text.strip
        @on_create.call(name, @repository)
        close
      end

      private def update_state : Nil
        return if @closed

        name = @name.text.strip
        valid = !name.empty? &&
                !name.starts_with?('.') &&
                !name.includes?('/') &&
                !name.includes?('\\')
        @create.sensitive = valid
        @status.visible = !name.empty? && !valid
        @status.text = "A workspace name cannot be hidden or contain a path separator." if !valid && !name.empty?
      end

      private def close : Nil
        @window.destroy unless @closed
      end

      private def closed : Nil
        return if @closed

        @closed = true
      end

      private def hint(key : String, label : String) : Gtk::Label
        item = Gtk::Label.new("#{key}  #{label}")
        item.add_css_class("dim-label")
        item
      end
    end
  end
end
