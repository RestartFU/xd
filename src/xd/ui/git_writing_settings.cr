require "gtk4"
require "../agent/catalog"
require "../version"
require "./adw"

module Xd
  module UI
    class GitWritingSettings
      BACKEND_IDS = [nil, "claude", "codex"] of String?

      def initialize(
        @parent : Gtk::Window,
        @on_shortcuts : Proc(Nil),
      )
        @settings = Gio::Settings.new(APP_ID)
      end

      def present : Nil
        title = Gtk::Label.new("Settings")
        title.xalign = 0_f32
        title.add_css_class("title-3")

        description = Gtk::Label.new(
          "Choose which assistant writes Git metadata. Every draft stays " \
          "editable before XD commits or opens a pull request."
        )
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class("dim-label")

        header = Gtk::Box.new(:vertical, 5)
        header.append(title)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        group = Adw::PreferencesGroup.new
        group.title = "Git Writing"
        group.description =
          "Use the active chat model, or reserve another bundled model."

        backend_names = Gtk::StringList.new([
          "Use Chat Model",
          "Claude Code",
          "Codex",
        ])
        backend_row = Adw::ComboRow.new
        backend_row.title = "Assistant"
        backend_row.model = backend_names
        stored_backend = @settings.string("git-writing-backend")
        backend_row.selected = (
          BACKEND_IDS.index(stored_backend.presence) || 0
        ).to_u32

        model_row = Adw::ComboRow.new
        model_row.title = "Model"

        update_models = -> {
          backend = BACKEND_IDS[backend_row.selected.to_i]?
          if backend_id = backend
            selected_backend = Agent::Catalog.lookup(backend_id).not_nil!
            names = selected_backend.models.map(&.display_name)
            model_row.model = Gtk::StringList.new(names)
            saved = @settings.string("git-writing-model")
            selected = selected_backend.models.index { |model| model.id == saved }
            selected ||= selected_backend.models.index { |model|
              model.id == selected_backend.default_model
            }
            model_row.selected = (selected || 0).to_u32
            model_row.sensitive = true
          else
            model_row.model = Gtk::StringList.new(["Current Chat Model"])
            model_row.selected = 0_u32
            model_row.sensitive = false
          end
        }

        backend_row.notify_signal["selected"].connect do |_property|
          update_models.call
        end
        update_models.call

        group.add(backend_row)
        group.add(model_row)

        shortcuts = Adw::PreferencesGroup.new
        shortcuts.title = "Prompt Shortcuts"
        shortcuts.description =
          "Create buttons that send frequently used prompts in every workspace."
        shortcut_row = Adw::ActionRow.new
        shortcut_row.title = "Global Shortcuts"
        shortcut_row.subtitle = "Stored by this daemon and shared with paired devices."
        edit_shortcuts = Gtk::Button.new_with_label("Edit…")
        edit_shortcuts.valign = :center
        shortcut_row.add_suffix(edit_shortcuts)
        shortcuts.add(shortcut_row)

        body = Gtk::Box.new(:vertical, 10)
        body.margin_top = 22
        body.margin_bottom = 22
        body.margin_start = 22
        body.margin_end = 22
        body.append(group)
        body.append(shortcuts)

        footer = Gtk::Box.new(:horizontal, 12)
        footer.append(hint("Esc", "Cancel"))
        footer.append(hint("Ctrl Enter", "Save"))
        spacer = Gtk::Box.new(:horizontal, 0)
        spacer.hexpand = true
        footer.append(spacer)

        window = Gtk::Window.new
        edit_shortcuts.clicked_signal.connect do
          window.destroy
          @on_shortcuts.call
        end
        save = -> {
          backend = BACKEND_IDS[backend_row.selected.to_i]? || ""
          model = ""
          if selected_backend = Agent::Catalog.lookup(backend.presence)
            model = selected_backend.models[model_row.selected.to_i]?.try(&.id) ||
                    selected_backend.default_model
          end
          @settings.set_string("git-writing-backend", backend)
          @settings.set_string("git-writing-model", model)
          window.destroy
        }

        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.add_css_class("flat")
        cancel.clicked_signal.connect { window.destroy }
        footer.append(cancel)

        save_button = Gtk::Button.new_with_label("Save")
        save_button.add_css_class("xd-panel-action")
        save_button.clicked_signal.connect { save.call }
        footer.append(save_button)
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")

        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(body)
        column.append(footer)

        window.title = "Settings"
        window.transient_for = @parent
        window.application = @parent.application
        window.destroy_with_parent = true
        window.modal = true
        window.decorated = false
        window.resizable = false
        window.set_default_size(700, -1)
        window.add_css_class("xd-panel")
        window.child = column
        window.close_request_signal.connect do
          window.destroy
          true
        end

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if keyval == Gdk::KEY_Escape
            window.destroy
            true
          elsif (keyval == Gdk::KEY_Return ||
                keyval == Gdk::KEY_KP_Enter) &&
                state.includes?(Gdk::ModifierType::ControlMask)
            save.call
            true
          else
            false
          end
        end
        window.add_controller(keys)
        window.present
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
