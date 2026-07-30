require "json"
require "gtk4"
require "../agent/catalog"
require "./adw"
require "./context_dialog"
require "./directory_browser"
require "./dialogs"
require "./panel_call"

module Xd
  module UI
    class FolderDialogs
      private class SecretRow
        getter box : Gtk::Box
        getter name_entry : Gtk::Entry
        getter value_entry : Gtk::PasswordEntry
        getter existing : Bool

        def initialize(
          @box,
          @name_entry,
          @value_entry,
          @existing,
        )
        end
      end

      def initialize(
        @parent : Gtk::Window,
        @request : PanelCall,
        @on_error : Proc(String, Nil),
      )
      end

      def settings(folder_id : String, folder_name : String) : Nil
        state = call({
          "op"     => JSON::Any.new("folder-settings"),
          "folder" => JSON::Any.new(folder_id),
        })
        return unless state

        inherited_backend =
          state["inherited_backend"]?.try(&.as_s?) ||
            state["effective_backend"].as_s
        inherited_model =
          state["inherited_model"]?.try(&.as_s?) ||
            state["effective_model"]?.try(&.as_s?)
        inherited_workdir =
          state["inherited_workdir"]?.try(&.as_s?) ||
            state["effective_workdir"]?.try(&.as_s?)
        inherited_repo =
          state["inherited_repo"]?.try(&.as_s?) ||
            state["effective_repo"]?.try(&.as_s?)

        dialog = Adw::PreferencesDialog.new
        dialog.title = folder_name
        page = Adw::PreferencesPage.new

        assistant = Adw::PreferencesGroup.new
        assistant.title = "Assistant"
        assistant.description =
          "Chats started in this folder use these unless they say otherwise."

        backend_ids = [nil, "claude", "codex"] of String?
        backend_names = Gtk::StringList.new([
          "Inherit (#{inherited_backend})",
          "Claude Code",
          "Codex",
        ])
        backend_row = Adw::ComboRow.new
        backend_row.title = "Backend"
        backend_row.model = backend_names
        backend_row.selected = (
          backend_ids.index(state["backend"].as_s?) || 0
        ).to_u32

        model_row = Adw::EntryRow.new
        model_row.title = inherited_model ? "Model (blank inherits)" : "Model (blank: CLI default)"
        model_row.text = state["model"].as_s? || ""

        assistant.add(backend_row)
        assistant.add(model_row)
        page.add(assistant)

        project = Adw::PreferencesGroup.new
        project.title = "Project"
        project.description =
          "Where the assistant runs. The code does not have to live inside " \
          "the workspace tree."

        workdir = state["workdir"].as_s?
        repo = state["repo"].as_s?
        workdir_row, choose_workdir, clear_workdir =
          build_path_row("Working Directory")
        repo_row, choose_repo, clear_repo =
          build_path_row("Repository")

        refresh_paths = -> {
          update_path_row(
            workdir_row,
            workdir,
            inherited_workdir,
            state["inherited_workdir_from"]?.try(&.as_s?)
          )
          update_path_row(
            repo_row,
            repo,
            inherited_repo,
            state["inherited_repo_from"]?.try(&.as_s?)
          )
        }
        choose_workdir.clicked_signal.connect do
          DirectoryBrowser.present(
            @parent,
            @request,
            workdir || inherited_workdir
          ) do |chosen|
            if chosen
              workdir = chosen
              refresh_paths.call
            end
          end
        end
        choose_repo.clicked_signal.connect do
          DirectoryBrowser.present(
            @parent,
            @request,
            repo || inherited_repo || inherited_workdir
          ) do |chosen|
            if chosen
              repo = chosen
              refresh_paths.call
            end
          end
        end
        clear_workdir.clicked_signal.connect do
          workdir = nil
          refresh_paths.call
        end
        clear_repo.clicked_signal.connect do
          repo = nil
          refresh_paths.call
        end

        project.add(workdir_row)
        project.add(repo_row)
        page.add(project)
        refresh_paths.call

        dialog.add(page)
        dialog.closed_signal.connect do
          save_async({
            "op"      => JSON::Any.new("set-folder-settings"),
            "folder"  => JSON::Any.new(folder_id),
            "backend" => nullable(backend_ids[backend_row.selected.to_i]),
            "model"   => nullable(clean(model_row.text)),
            "workdir" => nullable(workdir),
            "repo"    => nullable(repo),
          })
        end
        dialog.present(@parent)
      end

      private def build_path_row(
        title : String,
      ) : {Adw::ActionRow, Gtk::Button, Gtk::Button}
        row = Adw::ActionRow.new
        row.title = title
        row.subtitle_lines = 2

        choose = Gtk::Button.new_from_icon_name("folder-open-symbolic")
        choose.add_css_class("flat")
        choose.valign = :center
        choose.tooltip_text = "Choose…"

        clear = Gtk::Button.new_from_icon_name("edit-clear-symbolic")
        clear.add_css_class("flat")
        clear.valign = :center
        clear.tooltip_text = "Inherit from the parent folder"

        buttons = Gtk::Box.new(:horizontal, 6)
        buttons.append(choose)
        buttons.append(clear)
        row.add_suffix(buttons)
        {row, choose, clear}
      end

      private def update_path_row(
        row : Adw::ActionRow,
        value : String?,
        inherited : String?,
        inherited_from : String?,
      ) : Nil
        row.subtitle = if value && !value.empty?
                         value
                       elsif inherited && inherited_from
                         "#{inherited} — inherited from #{inherited_from}"
                       else
                         inherited || "Not set"
                       end
      end

      private def save_async(
        request : Hash(String, JSON::Any),
      ) : Nil
        spawn do
          result = @request.call(request)
          if error = result.error
            GLib.idle_add do
              @on_error.call(error)
              false
            end
          end
        end
      end

      private def call(
        request : Hash(String, JSON::Any),
      ) : Hash(String, JSON::Any)?
        result = @request.call(request)
        if error = result.error
          @on_error.call(error)
          return nil
        end
        result.body
      end

      def context(folder_id : String, folder_name : String) : Nil
        ContextDialog.new(
          @parent,
          @request,
          folder_id,
          folder_name
        ).present
      end

      def secrets(
        folder_id : String? = nil,
        title : String = "Agent Secrets",
      ) : Nil
        request = {"op" => JSON::Any.new("agent-secrets")}
        request["folder"] = JSON::Any.new(folder_id) if folder_id
        response = call(request)
        return unless response

        window, content, actions = Dialogs.shell(@parent, title)
        window.resizable = true
        window.set_default_size(680, 420)

        description = Gtk::Label.new(
          "Values enter agent process environment. Existing values never " \
          "cross daemon connection; leave them blank to keep unchanged."
        )
        description.xalign = 0_f32
        description.wrap = true
        content.append(description)

        rows = [] of SecretRow
        rows_box = Gtk::Box.new(:vertical, 8)
        response["names"].as_a.each do |node|
          append_secret_row(rows_box, rows, node.as_s, true)
        end

        scroll = Gtk::ScrolledWindow.new
        scroll.vexpand = true
        scroll.min_content_height = 220
        scroll.child = rows_box
        content.append(scroll)

        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.clicked_signal.connect { window.destroy }
        add = Gtk::Button.new_with_label("Add")
        add.clicked_signal.connect do
          row = append_secret_row(rows_box, rows, "", false)
          row.name_entry.grab_focus
        end
        save = Gtk::Button.new_with_label("Save")
        save.add_css_class("suggested-action")
        save.clicked_signal.connect do
          entries = rows.map do |row|
            fields = {
              "name" => JSON::Any.new(row.name_entry.text.strip),
            }
            value = row.value_entry.text
            fields["value"] = JSON::Any.new(value) unless value.empty?
            JSON::Any.new(fields)
          end
          save_request = {
            "op"      => JSON::Any.new("set-agent-secrets"),
            "entries" => JSON::Any.new(entries),
          }
          if folder_id
            save_request["folder"] = JSON::Any.new(folder_id)
          end
          window.destroy if call(save_request)
        end
        actions.append(cancel)
        actions.append(add)
        actions.append(save)
        window.present
      end

      private def field(
        label_text : String,
        input : Gtk::Widget,
      ) : Gtk::Box
        label = Gtk::Label.new(label_text)
        label.xalign = 0_f32
        label.width_chars = 18
        row = Gtk::Box.new(:horizontal, 10)
        input.hexpand = true
        row.append(label)
        row.append(input)
        row
      end

      private def append_secret_row(
        container : Gtk::Box,
        rows : Array(SecretRow),
        name : String,
        existing : Bool,
      ) : SecretRow
        name_entry = Gtk::Entry.new
        name_entry.text = name
        name_entry.placeholder_text = "ENVIRONMENT_VARIABLE"
        name_entry.editable = !existing
        name_entry.hexpand = true

        value_entry = Gtk::PasswordEntry.new
        value_entry.placeholder_text = existing ? "Unchanged" : "Secret value"
        value_entry.show_peek_icon = true
        value_entry.hexpand = true

        remove = Gtk::Button.new_from_icon_name("user-trash-symbolic")
        remove.add_css_class("flat")
        remove.tooltip_text = "Remove secret"

        box = Gtk::Box.new(:horizontal, 8)
        box.append(name_entry)
        box.append(value_entry)
        box.append(remove)
        row = SecretRow.new(box, name_entry, value_entry, existing)
        remove.clicked_signal.connect do
          container.remove(box)
          rows.delete(row)
        end
        rows << row
        container.append(box)
        row
      end

      private def clean(value : String) : String?
        stripped = value.strip
        stripped unless stripped.empty?
      end

      private def nullable(value : String?) : JSON::Any
        JSON::Any.new(value)
      end
    end
  end
end
