require "json"
require "gtk4"
require "../agent/catalog"
require "./dialogs"

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
        @call : Proc(
          Hash(String, JSON::Any),
          Hash(String, JSON::Any)?
        ),
      )
      end

      def settings(folder_id : String, folder_name : String) : Nil
        state = @call.call({
          "op"     => JSON::Any.new("folder-settings"),
          "folder" => JSON::Any.new(folder_id),
        })
        return unless state

        window, content, actions = Dialogs.shell(
          @parent,
          "#{folder_name} Settings"
        )
        window.resizable = true
        window.set_default_size(560, -1)

        backend_ids = [nil, "claude", "codex"] of String?
        inherited = state["effective_backend"].as_s
        backend = Gtk::DropDown.new_from_strings([
          "Inherit (#{inherited})",
          "Claude Code",
          "Codex",
        ])
        backend.selected = (
          backend_ids.index(state["backend"].as_s?) || 0
        ).to_u32
        backend.hexpand = true

        model = Gtk::Entry.new
        model.text = state["model"].as_s? || ""
        if effective = state["effective_model"].as_s?
          model.placeholder_text = "Inherit (#{effective})"
        else
          model.placeholder_text = "CLI default"
        end

        workdir = Gtk::Entry.new
        workdir.text = state["workdir"].as_s? || ""
        workdir.placeholder_text = state["effective_workdir"].as_s

        repo = Gtk::Entry.new
        repo.text = state["repo"].as_s? || ""
        repo.placeholder_text = state["effective_repo"].as_s? || "Not set"

        content.append(field("Backend", backend))
        content.append(field("Model", model))
        content.append(field("Working Directory", workdir))
        content.append(field("Repository", repo))

        help = Gtk::Label.new(
          "Blank values inherit from parent folders. Paths belong to the " \
          "daemon machine and must already exist."
        )
        help.xalign = 0_f32
        help.wrap = true
        help.add_css_class("dim-label")
        content.append(help)

        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.clicked_signal.connect { window.destroy }
        save = Gtk::Button.new_with_label("Save")
        save.add_css_class("suggested-action")
        save.clicked_signal.connect do
          request = {
            "op"      => JSON::Any.new("set-folder-settings"),
            "folder"  => JSON::Any.new(folder_id),
            "backend" => nullable(backend_ids[backend.selected.to_i]),
            "model"   => nullable(clean(model.text)),
            "workdir" => nullable(clean(workdir.text)),
            "repo"    => nullable(clean(repo.text)),
          }
          window.destroy if @call.call(request)
        end
        actions.append(cancel)
        actions.append(save)
        window.present
      end

      def context(folder_id : String, folder_name : String) : Nil
        response = @call.call({
          "op"     => JSON::Any.new("folder-context"),
          "folder" => JSON::Any.new(folder_id),
        })
        return unless response

        window, content, actions = Dialogs.shell(
          @parent,
          "#{folder_name} Agent Context"
        )
        window.resizable = true
        window.set_default_size(620, 440)

        description = Gtk::Label.new(
          "Instructions accumulate from parent folders and are appended to " \
          "every agent turn in this workspace."
        )
        description.xalign = 0_f32
        description.wrap = true

        editor = Gtk::TextView.new
        editor.buffer.text = response["context"].as_s? || ""
        editor.wrap_mode = :word_char
        editor.monospace = true
        editor.vexpand = true
        scroll = Gtk::ScrolledWindow.new
        scroll.vexpand = true
        scroll.min_content_height = 260
        scroll.child = editor

        content.append(description)
        content.append(scroll)

        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.clicked_signal.connect { window.destroy }
        save = Gtk::Button.new_with_label("Save")
        save.add_css_class("suggested-action")
        save.clicked_signal.connect do
          text = clean(editor.buffer.text)
          request = {
            "op"      => JSON::Any.new("set-folder-context"),
            "folder"  => JSON::Any.new(folder_id),
            "context" => nullable(text),
          }
          window.destroy if @call.call(request)
        end
        actions.append(cancel)
        actions.append(save)
        window.present
        editor.grab_focus
      end

      def secrets(
        folder_id : String? = nil,
        title : String = "Agent Secrets",
      ) : Nil
        request = {"op" => JSON::Any.new("agent-secrets")}
        request["folder"] = JSON::Any.new(folder_id) if folder_id
        response = @call.call(request)
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
          window.destroy if @call.call(save_request)
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
