require "json"
require "gtk4"
require "./panel_call"

module Xd
  module UI
    class ShortcutDialog
      MAX_SHORTCUTS = 24

      def initialize(
        @parent : Gtk::Window,
        @request : PanelCall,
        @on_error : Proc(String, Nil),
        @folder_id : String? = nil,
        @title : String = "Global Shortcuts",
      )
      end

      def present : Nil
        request = {
          "op" => JSON::Any.new("shortcuts"),
        }
        if folder_id = @folder_id
          request["folder"] = JSON::Any.new(folder_id)
        end

        spawn do
          result = @request.call(request)
          GLib.idle_add do
            if error = result.error
              @on_error.call(error)
            elsif body = result.body
              key = @folder_id ? "workspace" : "global"
              prompts = body[key]?.try(&.as_a?).try do |nodes|
                nodes.compact_map(&.as_s?)
              end || [] of String
              present_editor(prompts)
            else
              @on_error.call("Daemon returned no shortcuts.")
            end
            false
          end
        end
      end

      private def present_editor(prompts : Array(String)) : Nil
        title = Gtk::Label.new(@title)
        title.xalign = 0_f32
        title.add_css_class("title-3")

        description = Gtk::Label.new(
          @folder_id ? "These prompt buttons appear in this workspace and its children." : "These prompt buttons appear in every workspace on this daemon."
        )
        description.xalign = 0_f32
        description.wrap = true
        description.add_css_class("dim-label")

        header = Gtk::Box.new(:vertical, 5)
        header.append(title)
        header.append(description)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        rows = [] of {Gtk::Box, Gtk::Entry}
        list = Gtk::Box.new(:vertical, 8)
        empty = Gtk::Label.new("No shortcuts yet.")
        empty.xalign = 0_f32
        empty.add_css_class("dim-label")
        list.append(empty)

        refresh_empty = -> {
          empty.visible = rows.empty?
        }
        add_row = ->(prompt : String) {
          if rows.size < MAX_SHORTCUTS
            entry = Gtk::Entry.new
            entry.text = prompt
            entry.hexpand = true
            entry.placeholder_text = "Prompt sent when this button is pressed"

            remove = Gtk::Button.new_from_icon_name("edit-delete-symbolic")
            remove.add_css_class("flat")
            remove.tooltip_text = "Remove shortcut"

            row = Gtk::Box.new(:horizontal, 8)
            row.append(entry)
            row.append(remove)
            rows << {row, entry}
            list.append(row)
            refresh_empty.call

            remove.clicked_signal.connect do
              rows.reject! { |item| item[0].to_unsafe == row.to_unsafe }
              list.remove(row)
              refresh_empty.call
            end
          end
        }
        prompts.each { |prompt| add_row.call(prompt) }

        add = Gtk::Button.new_with_label("Add Prompt")
        add.halign = :start
        add.clicked_signal.connect do
          if rows.size < MAX_SHORTCUTS
            add_row.call("")
            rows.last[1].grab_focus
          end
        end

        body = Gtk::Box.new(:vertical, 12)
        body.margin_top = 20
        body.margin_bottom = 20
        body.margin_start = 22
        body.margin_end = 22
        body.append(list)
        body.append(add)

        scroll = Gtk::ScrolledWindow.new
        scroll.set_policy(:never, :automatic)
        scroll.min_content_height = 180
        scroll.max_content_height = 420
        scroll.propagate_natural_height = true
        scroll.child = body

        footer = Gtk::Box.new(:horizontal, 8)
        footer.halign = :end
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")

        window = Gtk::Window.new
        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.add_css_class("flat")
        cancel.clicked_signal.connect { window.destroy }
        footer.append(cancel)

        save = Gtk::Button.new_with_label("Save")
        save.add_css_class("xd-panel-action")
        footer.append(save)

        save.clicked_signal.connect do
          values = rows.map { |item| item[1].text.strip }.reject(&.empty?)
          request = {
            "op"        => JSON::Any.new("set-shortcuts"),
            "shortcuts" => JSON::Any.new(
              values.map { |value| JSON::Any.new(value) }
            ),
          }
          if folder_id = @folder_id
            request["folder"] = JSON::Any.new(folder_id)
          end
          save.sensitive = false
          spawn do
            result = @request.call(request)
            GLib.idle_add do
              if error = result.error
                save.sensitive = true
                @on_error.call(error)
              else
                window.destroy
              end
              false
            end
          end
        end

        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(scroll)
        column.append(footer)

        window.title = @title
        window.transient_for = @parent
        window.application = @parent.application
        window.destroy_with_parent = true
        window.modal = true
        window.decorated = false
        window.resizable = true
        window.set_default_size(700, -1)
        window.add_css_class("xd-panel")
        window.child = column
        window.close_request_signal.connect do
          window.destroy
          true
        end
        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, _state|
          if keyval == Gdk::KEY_Escape
            window.destroy
            true
          else
            false
          end
        end
        window.add_controller(keys)
        window.present
      end
    end
  end
end
