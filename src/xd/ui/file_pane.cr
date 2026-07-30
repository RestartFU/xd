require "json"
require "gtk4"
require "../syntax"
require "./adw"
require "./panel_call"

module Xd
  module UI
    class FilePane
      FILE_LIMIT           = 1024 * 1024
      HIGHLIGHT_LINE_LIMIT = 8000

      record Entry, name : String, directory : Bool

      getter widget : Adw::Bin

      @chat_id : String?
      @workdir : String?
      @path = ""
      @file_path : String?
      @showing_preview = false
      @saving = false
      @sequence = 0_i64
      @entries_data = [] of Entry
      @tags = {} of SyntaxToken => Gtk::TextTag

      @back : Gtk::Button
      @save : Gtk::Button
      @refresh : Gtk::Button
      @path_label : Gtk::Label
      @entries : Gtk::ListBox
      @stack : Gtk::Stack
      @status : Adw::StatusPage
      @toasts : Adw::ToastOverlay
      @editor : Gtk::TextView
      @preview : Gtk::TextBuffer

      def initialize(@request : PanelCall)
        @chat_id = nil
        @workdir = nil
        @file_path = nil
        @toasts = Adw::ToastOverlay.new

        @back = Gtk::Button.new_from_icon_name(
          "go-previous-symbolic"
        )
        @back.add_css_class("flat")
        @back.tooltip_text = "Back"
        @back.sensitive = false
        @back.clicked_signal.connect { go_back }

        @path_label = Gtk::Label.new("Files")
        @path_label.xalign = 0_f32
        @path_label.ellipsize = :start
        @path_label.hexpand = true
        @path_label.add_css_class("heading")

        @save = Gtk::Button.new_from_icon_name(
          "document-save-symbolic"
        )
        @save.add_css_class("flat")
        @save.tooltip_text = "Save (Ctrl+S)"
        @save.visible = false
        @save.sensitive = false
        @save.clicked_signal.connect { save_file }

        @refresh = Gtk::Button.new_from_icon_name(
          "view-refresh-symbolic"
        )
        @refresh.add_css_class("flat")
        @refresh.tooltip_text = "Read again"
        @refresh.clicked_signal.connect { refresh }

        header = Gtk::Box.new(:horizontal, 6)
        header.margin_start = 6
        header.margin_end = 6
        header.margin_top = 6
        header.margin_bottom = 6
        header.append(@back)
        header.append(@path_label)
        header.append(@save)
        header.append(@refresh)

        @entries = Gtk::ListBox.new
        @entries.selection_mode = :single
        @entries.add_css_class("xd-file-list")
        @entries.row_activated_signal.connect do |row|
          activate_entry(row.index)
        end

        entries_window = Gtk::ScrolledWindow.new
        entries_window.hscrollbar_policy = :never
        entries_window.vscrollbar_policy = :automatic
        entries_window.child = @entries

        @editor = Gtk::TextView.new
        @editor.editable = true
        @editor.cursor_visible = true
        @editor.monospace = true
        @editor.wrap_mode = :none
        @editor.left_margin = 12
        @editor.right_margin = 12
        @editor.top_margin = 10
        @editor.bottom_margin = 10
        @editor.add_css_class("xd-file-preview")
        @preview = @editor.buffer
        @preview.modified_changed_signal.connect { update_actions }
        build_syntax_tags
        capture_save_shortcut

        preview_window = Gtk::ScrolledWindow.new
        preview_window.hscrollbar_policy = :automatic
        preview_window.vscrollbar_policy = :automatic
        preview_window.child = @editor

        @status = Adw::StatusPage.new(
          icon_name: "folder-symbolic",
          title: "Files"
        )

        @stack = Gtk::Stack.new
        @stack.add_named(entries_window, "entries")
        @stack.add_named(preview_window, "preview")
        @stack.add_named(@status, "status")
        @stack.vexpand = true

        box = Gtk::Box.new(:vertical, 0)
        box.append(header)
        box.append(Gtk::Separator.new(:horizontal))
        box.append(@stack)

        @toasts.child = box
        @widget = Adw::Bin.new(child: @toasts)
      end

      def select_chat(chat_id : String?) : Nil
        return if @chat_id == chat_id

        @chat_id = chat_id
        @sequence += 1
        @workdir = nil
        @path = ""
        @file_path = nil
        @showing_preview = false
        @saving = false
        @entries_data.clear
        @entries.remove_all
        @preview.text = ""
        @preview.modified = false
        set_header_path("")
      end

      def refresh : Nil
        chat_id = @chat_id
        unless chat_id
          show_status(
            "No Working Directory",
            "This chat has no files to browse."
          )
          return
        end

        token = next_token
        show_status("Loading…", nil)
        request_async({
          "op"   => JSON::Any.new("chat"),
          "chat" => JSON::Any.new(chat_id),
        }) do |result|
          next unless active?(token, chat_id)
          unless state = result.body
            show_call_error(result)
            next
          end

          workdir = state["workdir"]?.try(&.as_s?)
          unless workdir
            @workdir = nil
            show_status(
              "No Working Directory",
              "This chat has no files to browse."
            )
            next
          end
          if @workdir != workdir
            @workdir = workdir
            @path = ""
            @file_path = nil
            @showing_preview = false
          end

          if @showing_preview
            if @preview.modified
              show_toast("Save or undo changes before reloading.")
              next
            end
            show_file(@file_path)
          else
            show_directory(@path)
          end
        end
      end

      private def show_directory(path : String) : Nil
        chat_id = @chat_id
        return unless chat_id

        @showing_preview = false
        @file_path = nil
        @path = path
        set_header_path(path)
        show_status("Loading…", nil)
        token = next_token

        request_async({
          "op"     => JSON::Any.new("file-browse"),
          "chat"   => JSON::Any.new(chat_id),
          "action" => JSON::Any.new("list"),
          "path"   => JSON::Any.new(path),
        }) do |result|
          next unless active?(token, chat_id)
          unless response = result.body
            show_call_error(result)
            next
          end

          entries = response["entries"]?.try(&.as_a?).try do |nodes|
            nodes.compact_map do |node|
              value = node.as_h?
              next unless value
              name = value["name"]?.try(&.as_s?)
              next unless name
              Entry.new(
                name,
                value["directory"]?.try(&.as_bool?) || false
              )
            end
          end || [] of Entry

          fill_entries(path, entries)
        end
      end

      private def fill_entries(
        path : String,
        entries : Array(Entry),
      ) : Nil
        @entries.remove_all
        @entries_data = entries.sort do |left, right|
          if left.directory != right.directory
            left.directory ? -1 : 1
          else
            LibGLib.g_utf8_collate(left.name, right.name)
          end
        end
        @entries_data.each { |entry| add_entry_row(entry) }

        @path = path
        @file_path = nil
        @showing_preview = false
        set_header_path(path)
        if entries.empty?
          show_status(
            "Empty Folder",
            "This folder has no visible files."
          )
        else
          @stack.visible_child_name = "entries"
        end
      end

      private def add_entry_row(entry : Entry) : Nil
        icon = Gtk::Image.new_from_icon_name(
          entry.directory ? "folder-symbolic" : "text-x-generic-symbolic"
        )
        icon.add_css_class("dim-label")

        label = Gtk::Label.new(entry.name)
        label.xalign = 0_f32
        label.ellipsize = :middle
        label.hexpand = true

        box = Gtk::Box.new(:horizontal, 10)
        box.margin_start = 10
        box.margin_end = 10
        box.margin_top = 7
        box.margin_bottom = 7
        box.append(icon)
        box.append(label)
        if entry.directory
          box.append(
            Gtk::Image.new_from_icon_name("go-next-symbolic")
          )
        end

        row = Gtk::ListBoxRow.new
        row.child = box
        @entries.append(row)
      end

      private def activate_entry(index : Int32) : Nil
        entry = @entries_data[index]?
        return unless entry

        child = child_path(@path, entry.name)
        if entry.directory
          show_directory(child)
        else
          show_file(child)
        end
      end

      private def show_file(path : String?) : Nil
        chat_id = @chat_id
        return unless chat_id && path

        @showing_preview = true
        @file_path = path
        set_header_path(path)
        show_status("Loading…", nil)
        token = next_token

        request_async({
          "op"     => JSON::Any.new("file-browse"),
          "chat"   => JSON::Any.new(chat_id),
          "action" => JSON::Any.new("read"),
          "path"   => JSON::Any.new(path),
        }) do |result|
          next unless active?(token, chat_id)
          unless response = result.body
            show_read_error(result.error)
            next
          end

          content = response["content"]?.try(&.as_s?)
          unless content
            show_status(
              "Could Not Open File",
              "The daemon returned an invalid file."
            )
            next
          end
          show_preview_text(path, content)
        end
      end

      private def show_preview_text(path : String, text : String) : Nil
        @preview.text = text
        highlight_preview(path, text)
        @preview.modified = false
        @file_path = path
        @showing_preview = true
        set_header_path(path)
        @stack.visible_child_name = "preview"
        @editor.grab_focus
      end

      private def save_file : Nil
        chat_id = @chat_id
        path = @file_path
        return unless chat_id && path
        return unless @showing_preview && !@saving && @preview.modified

        content = @preview.text
        if content.bytesize > FILE_LIMIT
          show_toast("Files larger than 1 MB cannot be saved here.")
          return
        end

        @saving = true
        update_actions
        request_async({
          "op"      => JSON::Any.new("file-browse"),
          "chat"    => JSON::Any.new(chat_id),
          "action"  => JSON::Any.new("write"),
          "path"    => JSON::Any.new(path),
          "content" => JSON::Any.new(content),
        }) do |result|
          next unless @chat_id == chat_id && @file_path == path

          @saving = false
          if result.body
            @preview.modified = false
            show_toast("File saved")
          elsif message = result.error
            show_toast(message)
          end
          update_actions
        end
      end

      private def go_back : Nil
        if @showing_preview
          if @preview.modified
            show_toast("Save or undo changes before going back.")
            return
          end

          @showing_preview = false
          @file_path = nil
          set_header_path(@path)
          @stack.visible_child_name = "entries"
          return
        end

        show_directory(parent_path(@path))
      end

      private def show_status(
        title : String,
        description : String?,
      ) : Nil
        @status.title = title
        @status.description = description
        @stack.visible_child_name = "status"
      end

      private def show_read_error(message : String?) : Nil
        case message
        when "Files larger than 1 MB are not previewed."
          show_status(
            "File Too Large",
            "Files larger than 1 MB are not previewed."
          )
        when "Binary files cannot be previewed as text."
          show_status(
            "Binary File",
            "Binary files cannot be previewed as text."
          )
        else
          show_status(
            "Could Not Open Files",
            message || "The file could not be read."
          )
        end
      end

      private def show_call_error(result : PanelCallResult) : Nil
        show_status(
          "Could Not Open Files",
          result.error || "The directory could not be read."
        )
      end

      private def request_async(
        fields : Hash(String, JSON::Any),
        &complete : PanelCallResult -> Nil
      ) : Nil
        spawn do
          result = @request.call(fields)
          GLib.idle_add do
            complete.call(result)
            false
          end
        end
      end

      private def active?(token : Int64, chat_id : String) : Bool
        token == @sequence && @chat_id == chat_id
      end

      private def next_token : Int64
        @sequence += 1
      end

      private def show_toast(message : String) : Nil
        @toasts.add_toast(Adw::Toast.new(message))
      end

      private def set_header_path(path : String) : Nil
        label = if path.empty?
                  @workdir.try { |root| Path[root].basename.to_s } ||
                    "Files"
                else
                  path
                end
        @path_label.text = label
        @path_label.tooltip_text = label
        update_actions
      end

      private def update_actions : Nil
        modified = @preview.modified
        @save.visible = @showing_preview
        @save.sensitive = @showing_preview && modified && !@saving
        @editor.sensitive = !@saving
        @back.sensitive = !@saving &&
                          (@showing_preview || !@path.empty?)
        @refresh.sensitive = !@saving
      end

      private def build_syntax_tags : Nil
        table = @preview.tag_table
        return unless table

        SyntaxToken.values.each do |token|
          next unless colour = token.colour

          tag = Gtk::TextTag.new(foreground: colour)
          table.add(tag)
          @tags[token] = tag
        end
      end

      private def highlight_preview(path : String, text : String) : Nil
        @preview.remove_all_tags(
          @preview.start_iter,
          @preview.end_iter
        )
        language = Syntax.language_for_path(path)
        return if language.none?

        state = SyntaxState.new
        offset = 0
        lines = text.split('\n', remove_empty: false)
        lines.first(HIGHLIGHT_LINE_LIMIT)
          .each_with_index do |line, index|
            Syntax.scan_line(language, line, state).each do |piece|
              finish = offset + piece.text.size
              if tag = @tags[piece.token]?
                @preview.apply_tag(
                  tag,
                  @preview.iter_at_offset(offset),
                  @preview.iter_at_offset(finish)
                )
              end
              offset = finish
            end
            offset += 1 if index < lines.size - 1
          end
      end

      private def capture_save_shortcut : Nil
        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if state.includes?(Gdk::ModifierType::ControlMask) &&
             Gdk.keyval_to_lower(keyval) == Gdk::KEY_s
            save_file
            true
          else
            false
          end
        end
        @editor.add_controller(keys)
      end

      private def child_path(parent : String, name : String) : String
        parent.empty? ? name : "#{parent}/#{name}"
      end

      private def parent_path(path : String) : String
        path.rpartition('/')[0]
      end
    end
  end
end
