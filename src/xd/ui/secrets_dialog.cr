require "json"
require "set"
require "gtk4"
require "../agent/secrets"
require "./panel_call"
require "./panel_dialog"

module Xd
  module UI
    class SecretsDialog
      private class Row
        getter box : Gtk::Box
        getter name : Gtk::Entry
        getter value : Gtk::PasswordEntry
        getter existing : Bool

        def initialize(@box, @name, @value, @existing)
        end
      end

      def initialize(
        @parent : Gtk::Window,
        @request : PanelCall,
        @remote : Bool,
        @folder_id : String?,
        folder_name : String?,
      )
        @closed = false
        @busy = false
        @sequence = 0_i64
        @rows = [] of Row

        title = Gtk::Label.new(title_text(folder_name))
        title.xalign = 0_f32
        title.add_css_class("title-3")

        description = Gtk::Label.new(description_text)
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class("dim-label")

        header = Gtk::Box.new(:vertical, 5)
        header.append(title)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        field_label = Gtk::Label.new("Environment variables")
        field_label.xalign = 0_f32
        field_label.hexpand = true
        field_label.add_css_class("caption")
        field_label.add_css_class("dim-label")

        @add = Gtk::Button.new_with_label("Add Secret")
        @add.add_css_class("flat")
        @add.sensitive = false
        @add.clicked_signal.connect do
          row = append_row("", false)
          row.name.grab_focus
        end

        label_row = Gtk::Box.new(:horizontal, 8)
        label_row.append(field_label)
        label_row.append(@add)

        @rows_box = Gtk::Box.new(:vertical, 8)
        @rows_box.sensitive = false
        scroller = Gtk::ScrolledWindow.new
        scroller.set_policy(:never, :automatic)
        scroller.vexpand = true
        scroller.child = @rows_box

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
        body.append(label_row)
        body.append(scroller)
        body.append(@status)

        footer = Gtk::Box.new(:horizontal, 12)
        footer.append(hint("Esc", "Cancel"))
        footer.append(hint("Ctrl Enter", "Save"))
        spacer = Gtk::Box.new(:horizontal, 0)
        spacer.hexpand = true
        footer.append(spacer)

        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.add_css_class("flat")
        cancel.clicked_signal.connect { close }
        footer.append(cancel)

        @save = Gtk::Button.new_with_label("Save")
        @save.add_css_class("xd-panel-action")
        @save.sensitive = false
        @save.clicked_signal.connect { save }
        footer.append(@save)
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")

        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(body)
        column.append(footer)

        @window = PanelDialog.new(@parent, 700, 500)
        @window.title = "Agent Secrets"
        @window.add_css_class("xd-panel")
        @window.child = column
        @window.destroy_signal.connect { closed }

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if keyval == Gdk::KEY_Escape
            close
            true
          elsif (keyval == Gdk::KEY_Return ||
                keyval == Gdk::KEY_KP_Enter) &&
                state.includes?(Gdk::ModifierType::ControlMask)
            # Let GtkEntry's internal GtkText finish this event before the
            # busy state makes its row insensitive.
            GLib.idle_add do
              save
              false
            end
            false
          else
            false
          end
        end
        @window.add_controller(keys)
      end

      def present : Nil
        @window.present
        load
      end

      private def title_text(folder_name : String?) : String
        if folder_name
          "Agent Secrets · #{folder_name}"
        elsif @remote
          "Agent Secrets · Remote Machine"
        else
          "Agent Secrets · This Machine"
        end
      end

      private def description_text : String
        if @folder_id
          machine = @remote ? "remote machine" : "this machine"
          "Stored privately on #{machine}, outside the workspace. This " \
          "folder inherits global and parent secrets; values set here " \
          "override them for this folder and its children."
        elsif @remote
          "Stored in a private per-user file on the remote machine. Values " \
          "never enter the prompt; remote agent processes receive them as " \
          "environment variables."
        else
          "Stored in a private per-user file on this machine. Values never " \
          "enter the prompt; agent processes receive them as environment " \
          "variables."
        end
      end

      private def load : Nil
        token = next_token
        show_status("Loading secret names…", false)
        request = {"op" => JSON::Any.new("agent-secrets")}
        if folder_id = @folder_id
          request["folder"] = JSON::Any.new(folder_id)
        end

        spawn do
          result = @request.call(request)
          GLib.idle_add do
            if active?(token)
              if error = result.error
                show_status(error, true)
              elsif body = result.body
                names = body["names"]?.try(&.as_a?).try do |values|
                  values.compact_map(&.as_s?)
                end
                if names
                  show_names(names)
                else
                  show_status("Daemon returned no secret names.", true)
                end
              else
                show_status("Daemon returned no secret names.", true)
              end
            end
            false
          end
        end
      end

      private def show_names(names : Array(String)) : Nil
        names.each { |name| append_row(name, true) }
        if @rows.empty?
          row = append_row("", false)
          row.name.grab_focus
        end
        @rows_box.sensitive = true
        @add.sensitive = true
        @save.sensitive = true
        show_status(nil, false)
      end

      private def append_row(name : String, existing : Bool) : Row
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
        row = Row.new(box, name_entry, value_entry, existing)
        remove.clicked_signal.connect do
          @rows_box.remove(box)
          @rows.delete(row)
          show_status(nil, false)
        end
        @rows << row
        @rows_box.append(box)
        row
      end

      private def save : Nil
        return if @closed || @busy || !@save.sensitive?
        entries = prepare_entries
        return unless entries

        request = {
          "op"      => JSON::Any.new("set-agent-secrets"),
          "entries" => JSON::Any.new(entries),
        }
        if folder_id = @folder_id
          request["folder"] = JSON::Any.new(folder_id)
        end

        token = next_token
        show_status(nil, false)
        set_busy(true)
        spawn do
          result = @request.call(request)
          GLib.idle_add do
            if active?(token)
              if error = result.error
                show_status(error, true)
                set_busy(false)
              else
                close
              end
            end
            false
          end
        end
      end

      private def prepare_entries : Array(JSON::Any)?
        seen = Set(String).new
        entries = [] of JSON::Any
        @rows.each do |row|
          name = row.name.text.strip
          unless Agent::Secrets.valid_name?(name)
            show_status(
              "Names must use letters, numbers and underscores, and cannot " \
              "start with a number.",
              true
            )
            row.name.grab_focus
            return nil
          end
          if seen.includes?(name)
            show_status("Secret names must be unique.", true)
            row.name.grab_focus
            return nil
          end

          value = row.value.text
          if value.empty? && !row.existing
            show_status("A new secret needs a value.", true)
            row.value.grab_focus
            return nil
          end

          fields = {"name" => JSON::Any.new(name)}
          fields["value"] = JSON::Any.new(value) unless value.empty?
          entries << JSON::Any.new(fields)
          seen << name
        end
        entries
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

      private def set_busy(busy : Bool) : Nil
        return if @closed

        @busy = busy
        @save.grab_focus if busy
        @rows_box.sensitive = !busy
        @add.sensitive = !busy
        @save.sensitive = !busy
        @save.label = busy ? "Saving…" : "Save"
      end

      private def close : Nil
        @window.destroy unless @closed
      end

      private def closed : Nil
        return if @closed

        @closed = true
        @sequence += 1
      end

      private def active?(token : Int64) : Bool
        !@closed && token == @sequence
      end

      private def next_token : Int64
        @sequence += 1
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
