require "json"
require "gtk4"
require "./background_work"
require "./panel_call"

module Xd
  module UI
    # C-shaped daemon-side directory picker.
    #
    # Local Unix and remote TLS sources both enter through the same list-dir
    # operation. The widget never reads the client filesystem itself.
    class DirectoryBrowser
      ENTRY_BATCH = 80

      @entries : Gtk::StringList

      def self.present(
        parent : Gtk::Window,
        request : PanelCall,
        start : String?,
        &chosen : String? -> Nil
      ) : Nil
        new(parent, request, start, chosen).present
      end

      def initialize(
        @parent : Gtk::Window,
        @request : PanelCall,
        @start : String?,
        @chosen : Proc(String?, Nil),
      )
        @path = nil
        @sequence = 0_i64
        @closed = false
        @answered = false
        @labels = {} of UInt64 => Gtk::Label

        @entries = Gtk::StringList.new([] of String)
        @selection = Gtk::SingleSelection.new(@entries)

        factory = Gtk::SignalListItemFactory.new
        factory.setup_signal.connect { |object| setup_item(object) }
        factory.bind_signal.connect { |object| bind_item(object) }
        factory.teardown_signal.connect { |object| teardown_item(object) }

        @list = Gtk::ListView.new(@selection, factory)
        @list.single_click_activate = false
        @list.add_css_class("navigation-sidebar")
        @list.activate_signal.connect { |_position| descend }

        @path_label = Gtk::Label.new("")
        @path_label.ellipsize = :start
        @path_label.xalign = 0_f32
        @path_label.hexpand = true
        @path_label.add_css_class("xd-browser-path")

        use = Gtk::Button.new_with_label("Work here")
        use.add_css_class("xd-panel-action")
        use.clicked_signal.connect { answer(@path) }

        header = Gtk::Box.new(:horizontal, 10)
        header.append(@path_label)
        header.append(use)
        header.add_css_class("xd-panel-bar")
        header.add_css_class("xd-panel-head")

        @trouble = Gtk::Label.new("")
        @trouble.wrap = true
        @trouble.xalign = 0_f32
        @trouble.visible = false
        @trouble.add_css_class("error")
        @trouble.add_css_class("xd-panel-bar")

        scrolled = Gtk::ScrolledWindow.new
        scrolled.vexpand = true
        scrolled.child = @list

        footer = Gtk::Box.new(:horizontal, 16)
        footer.append(hint("↑↓", "Move"))
        footer.append(hint("Enter", "Open"))
        footer.append(hint("Backspace", "Back"))
        footer.append(hint("Esc", "Use the folder's"))
        footer.add_css_class("xd-panel-bar")
        footer.add_css_class("xd-panel-foot")

        column = Gtk::Box.new(:vertical, 0)
        column.append(header)
        column.append(@trouble)
        column.append(scrolled)
        column.append(footer)

        @window = Gtk::Window.new
        @window.title = "Choose a Folder"
        @window.transient_for = @parent
        @window.application = @parent.application
        @window.modal = true
        @window.destroy_with_parent = true
        @window.decorated = false
        @window.set_default_size(620, 460)
        @window.add_css_class("xd-panel")
        @window.add_css_class("xd-browser")
        @window.child = column
        @window.destroy_signal.connect { closed }
        @window.close_request_signal.connect do
          answer(nil)
          true
        end

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          handle_key(keyval, state)
        end
        @window.add_controller(keys)
      end

      def present : Nil
        show_directory(@start)
        @window.present
        @list.grab_focus
      end

      private def show_directory(path : String?) : Nil
        return if @closed

        @sequence += 1
        sequence = @sequence
        fields = {"op" => JSON::Any.new("list-dir")}
        fields["path"] = JSON::Any.new(path) if path

        spawn do
          result = @request.call(fields)
          queued = BackgroundWork.submit do
            prepare_directory(result, sequence)
          end
          unless queued
            GLib.idle_add do
              if !@closed && sequence == @sequence
                show_trouble(
                  "Too many folders are being prepared. Try again shortly."
                )
              end
              false
            end
          end
        end
      end

      def self.prepare_entries(values : Array(JSON::Any)) : Array(String)
        values.compact_map(&.as_s?)
      end

      def self.entry_batch_finish(start : Int, total : Int) : Int32
        Math.min(start.to_i64 + ENTRY_BATCH, total.to_i64).to_i32
      end

      private def prepare_directory(
        result : PanelCallResult,
        sequence : Int64,
      ) : Nil
        listed_path : String? = nil
        entries = [] of String
        trouble = result.error
        begin
          if body = result.body
            listed_path = body["path"]?.try(&.as_s?)
            if values = body["entries"]?.try(&.as_a?)
              entries = self.class.prepare_entries(values)
            else
              trouble ||= "Daemon returned no directory entries."
            end
            trouble ||= "Daemon returned no directory path." unless listed_path
          end
        rescue error : TypeCastError
          trouble = error.message || "Daemon returned an invalid directory."
        end

        GLib.idle_add do
          if !@closed && sequence == @sequence
            if listed_path && trouble.nil?
              fill(listed_path.not_nil!, entries)
            else
              show_trouble(
                trouble || "Cannot read that directory."
              )
            end
          end
          false
        end
      end

      private def fill(path : String, entries : Array(String)) : Nil
        @trouble.visible = false
        @entries = Gtk::StringList.new([] of String)
        @selection.model = @entries
        @path = path
        @path_label.label = path
        append_entries(entries, 0, @sequence)
      end

      private def append_entries(
        entries : Array(String),
        start : Int32,
        sequence : Int64,
      ) : Nil
        return if @closed || sequence != @sequence

        finish = self.class.entry_batch_finish(start, entries.size)
        @entries.splice(
          @entries.n_items,
          0_u32,
          entries[start...finish]
        )
        if finish < entries.size
          GLib.idle_add do
            append_entries(entries, finish, sequence)
            false
          end
        elsif !entries.empty?
          @selection.selected = 0_u32
        end
      end

      private def show_trouble(message : String) : Nil
        @trouble.label = message
        @trouble.visible = true
      end

      private def selected_path : String?
        path = @path
        object = @selection.selected_item
        return unless path && object

        entry = Gtk::StringObject.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        File.join(path, entry.string)
      end

      private def descend : Nil
        show_directory(selected_path)
      end

      private def ascend : Nil
        path = @path
        return unless path

        parent = File.dirname(path)
        show_directory(parent) unless parent == path
      end

      private def handle_key(
        keyval : UInt32,
        state : Gdk::ModifierType,
      ) : Bool
        case keyval
        when Gdk::KEY_Escape
          answer(nil)
          true
        when Gdk::KEY_BackSpace, Gdk::KEY_Left
          ascend
          true
        when Gdk::KEY_Return, Gdk::KEY_KP_Enter, Gdk::KEY_Right
          if state.includes?(Gdk::ModifierType::ControlMask)
            answer(@path)
          else
            descend
          end
          true
        else
          false
        end
      end

      private def answer(path : String?) : Nil
        return if @answered

        @answered = true
        @chosen.call(path)
        @window.destroy unless @closed
      end

      private def closed : Nil
        return if @closed

        @closed = true
        @sequence += 1
        unless @answered
          @answered = true
          @chosen.call(nil)
        end
      end

      private def setup_item(object : GObject::Object) : Nil
        item = list_item(object)
        icon = Gtk::Image.new_from_icon_name("folder-symbolic")
        icon.add_css_class("dim-label")

        label = Gtk::Label.new("")
        label.xalign = 0_f32
        label.ellipsize = :middle
        label.hexpand = true

        box = Gtk::Box.new(:horizontal, 12)
        box.margin_top = 2
        box.margin_bottom = 2
        box.append(icon)
        box.append(label)
        item.child = box
        @labels[pointer_key(item)] = label
      end

      private def bind_item(object : GObject::Object) : Nil
        item = list_item(object)
        model_item = item.item
        label = @labels[pointer_key(item)]?
        return unless model_item && label

        entry = Gtk::StringObject.new(
          model_item.to_unsafe,
          GICrystal::Transfer::None
        )
        label.label = entry.string
      end

      private def teardown_item(object : GObject::Object) : Nil
        item = list_item(object)
        @labels.delete(pointer_key(item))
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

      private def list_item(object : GObject::Object) : Gtk::ListItem
        Gtk::ListItem.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
      end

      private def pointer_key(object : GObject::Object) : UInt64
        object.to_unsafe.address
      end
    end
  end
end
