require "gtk4"
require "../agent/catalog"
require "../version"
require "./adw"

module Xd
  module UI
    class GitWritingSettings
      BACKEND_IDS = [nil, "claude", "codex"] of String?

      def initialize(@parent : Gtk::Window)
        @settings = Gio::Settings.new(APP_ID)
      end

      def present : Nil
        dialog = Adw::PreferencesDialog.new
        dialog.title = "Settings"
        page = Adw::PreferencesPage.new
        group = Adw::PreferencesGroup.new
        group.title = "Git Writing"
        group.description =
          "Choose which assistant drafts commit messages and pull request " \
          "details. XD always shows an editable preview before changing Git."

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
        page.add(group)
        dialog.add(page)
        dialog.closed_signal.connect do
          backend = BACKEND_IDS[backend_row.selected.to_i]? || ""
          model = ""
          if selected_backend = Agent::Catalog.lookup(backend.presence)
            model = selected_backend.models[model_row.selected.to_i]?.try(&.id) ||
                    selected_backend.default_model
          end
          @settings.set_string("git-writing-backend", backend)
          @settings.set_string("git-writing-model", model)
        end
        dialog.present(@parent)
      end
    end
  end
end
