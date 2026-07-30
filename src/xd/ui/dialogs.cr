require "gtk4"
require "./adw"
require "./panel_dialog"

module Xd
  module UI
    module Dialogs
      extend self

      def prompt(
        parent : Gtk::Window,
        title : String,
        description : String,
        initial : String = "",
        &on_accept : String -> Nil
      ) : Nil
        window, content, actions = shell(parent, title)

        description_label = Gtk::Label.new(description)
        description_label.xalign = 0_f32
        description_label.wrap = true

        entry = Gtk::Entry.new
        entry.text = initial
        entry.hexpand = true

        cancel = Gtk::Button.new_with_label("Cancel")
        cancel.clicked_signal.connect { window.destroy }
        save = Gtk::Button.new_with_label("Save")
        save.add_css_class("suggested-action")

        submit = -> {
          value = entry.text.strip
          unless value.empty?
            on_accept.call(value)
            window.destroy
          end
        }
        entry.activate_signal.connect { submit.call }
        save.clicked_signal.connect { submit.call }

        content.append(description_label)
        content.append(entry)
        actions.append(cancel)
        actions.append(save)
        window.present
        entry.grab_focus
        entry.select_region(0, -1)
      end

      def confirm(
        parent : Gtk::Window,
        title : String,
        description : String,
        accept_label : String,
        &on_accept : -> Nil
      ) : Nil
        dialog = Adw::AlertDialog.new(
          heading: title,
          body: description
        )
        dialog.add_response("cancel", "Cancel")
        dialog.add_response("accept", accept_label)
        dialog.set_response_appearance("accept", :destructive)
        dialog.default_response = "cancel"
        dialog.close_response = "cancel"
        dialog.choose(parent, nil) do |_source, result|
          on_accept.call if dialog.choose_finish(result) == "accept"
        end
      end

      def shell(
        parent : Gtk::Window,
        title : String,
      ) : {PanelDialog, Gtk::Box, Gtk::Box}
        window = PanelDialog.new(parent, 420, -1)
        window.title = title

        heading = Gtk::Label.new(title)
        heading.xalign = 0_f32
        heading.add_css_class("title-2")

        content = Gtk::Box.new(:vertical, 10)
        content.margin_top = 18
        content.margin_start = 18
        content.margin_end = 18
        content.append(heading)

        actions = Gtk::Box.new(:horizontal, 8)
        actions.halign = :end
        actions.margin_top = 18
        actions.margin_bottom = 18
        actions.margin_start = 18
        actions.margin_end = 18

        root = Gtk::Box.new(:vertical, 0)
        root.append(content)
        root.append(actions)
        window.child = root
        {window, content, actions}
      end
    end
  end
end
