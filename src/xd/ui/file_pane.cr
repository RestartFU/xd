require "json"
require "gtk4"
require "../syntax_highlight"
require "./adw"
require "./background_work"
require "./panel_call"

module Xd
  module UI
    class FilePane
      FILE_LIMIT      = 1024 * 1024
      HIGHLIGHT_BATCH = 256
      ENTRY_BATCH     =  80
      PREVIEW_BATCH   = 32 * 1024
      PREVIEW_CLEAR   = 8 * 1024

      record Entry, name : String, directory : Bool

      record TextChange,
        start : Int32,
        finish : Int32,
        replacement : String

      getter widget : Adw::Bin

      @chat_id : String?
      @workdir : String?
      @path = ""
      @file_path : String?
      @showing_preview = false
      @loading_preview = false
      @saving = false
      @sequence = 0_i64
      @entries_data = [] of Entry
      @entries_ready = false
      @refresh_active = false
      @refresh_pending = false
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
        @preview.modified_changed_signal.connect do
          @sequence += 1 if @preview.modified && !@loading_preview
          update_actions
        end
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
        @loading_preview = false
        @saving = false
        @entries_data.clear
        @entries_ready = false
        @entries.remove_all
        @preview.modified = false
        set_header_path("")
        if chat_id
          show_status("Loading…", nil)
        else
          show_status(
            "No Working Directory",
            "This chat has no files to browse."
          )
        end
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

        if @refresh_active
          @refresh_pending = true
          return
        end
        @refresh_active = true

        token = next_token
        show_status("Loading…", nil) unless @workdir
        request_async({
          "op"   => JSON::Any.new("chat"),
          "chat" => JSON::Any.new(chat_id),
        }) do |result|
          unless active?(token, chat_id)
            finish_refresh
            next
          end
          unless state = result.body
            show_call_error(result)
            finish_refresh
            next
          end

          workdir = state["workdir"]?.try(&.as_s?)
          unless workdir
            @workdir = nil
            show_status(
              "No Working Directory",
              "This chat has no files to browse."
            )
            finish_refresh
            next
          end
          workdir_changed = @workdir != workdir
          if workdir_changed
            @workdir = workdir
            @path = ""
            @file_path = nil
            @showing_preview = false
            @entries_ready = false
          end

          if @showing_preview
            if @preview.modified
              show_toast("Save or undo changes before reloading.")
              finish_refresh
              next
            end
            refresh_file(@file_path, auto_refresh: true)
          else
            show_directory(
              @path,
              incremental: !workdir_changed,
              auto_refresh: true
            )
          end
        end
      end

      private def show_directory(
        path : String,
        incremental : Bool = false,
        auto_refresh : Bool = false,
      ) : Nil
        chat_id = @chat_id
        unless chat_id
          finish_refresh if auto_refresh
          return
        end

        @showing_preview = false
        @loading_preview = false
        @file_path = nil
        @path = path
        @entries_ready = false unless incremental
        set_header_path(path)
        show_status("Loading…", nil) unless incremental
        token = next_token

        request_async({
          "op"     => JSON::Any.new("file-browse"),
          "chat"   => JSON::Any.new(chat_id),
          "action" => JSON::Any.new("list"),
          "path"   => JSON::Any.new(path),
        }) do |result|
          unless active?(token, chat_id)
            finish_refresh if auto_refresh
            next
          end
          unless response = result.body
            show_call_error(result)
            finish_refresh if auto_refresh
            next
          end

          nodes = response["entries"]?.try(&.as_a?) || [] of JSON::Any
          prepare_entries(path, chat_id, token, nodes, incremental)
          finish_refresh if auto_refresh
        end
      end

      def self.prepare_entries(nodes : Array(JSON::Any)) : Array(Entry)
        entries = nodes.compact_map do |node|
          value = node.as_h?
          next unless value
          name = value["name"]?.try(&.as_s?)
          next unless name
          Entry.new(
            name,
            value["directory"]?.try(&.as_bool?) || false
          )
        end
        entries.sort do |left, right|
          if left.directory != right.directory
            left.directory ? -1 : 1
          else
            LibGLib.g_utf8_collate(left.name, right.name)
          end
        end
      end

      def self.entry_batch_finish(start : Int, total : Int) : Int32
        Math.min(start.to_i64 + ENTRY_BATCH, total.to_i64).to_i32
      end

      def self.preview_chunk(text : String, start : Int) : String
        return "" if start >= text.bytesize

        finish = Math.min(start + PREVIEW_BATCH, text.bytesize)
        while finish < text.bytesize &&
              (text.byte_at(finish) & 0xc0) == 0x80
          finish -= 1
        end
        text.byte_slice(start, finish - start)
      end

      def self.text_change(old_text : String, new_text : String) : TextChange?
        return if old_text == new_text

        common = Math.min(old_text.bytesize, new_text.bytesize)
        prefix = 0
        while prefix < common &&
              old_text.byte_at(prefix) == new_text.byte_at(prefix)
          prefix += 1
        end
        while prefix > 0 && prefix < old_text.bytesize &&
              (old_text.byte_at(prefix) & 0xc0) == 0x80
          prefix -= 1
        end

        suffix = 0
        suffix_limit = common - prefix
        while suffix < suffix_limit &&
              old_text.byte_at(old_text.bytesize - suffix - 1) ==
                new_text.byte_at(new_text.bytesize - suffix - 1)
          suffix += 1
        end
        while suffix > 0 &&
              (((old_text.byte_at(old_text.bytesize - suffix) & 0xc0) == 0x80) ||
              ((new_text.byte_at(new_text.bytesize - suffix) & 0xc0) == 0x80))
          suffix -= 1
        end

        old_finish = old_text.bytesize - suffix
        new_finish = new_text.bytesize - suffix
        TextChange.new(
          old_text.byte_slice(0, prefix).size,
          old_text.byte_slice(0, old_finish).size,
          new_text.byte_slice(prefix, new_finish - prefix)
        )
      end

      private def prepare_entries(
        path : String,
        chat_id : String,
        token : Int64,
        nodes : Array(JSON::Any),
        incremental : Bool,
      ) : Nil
        queued = BackgroundWork.submit do
          entries : Array(Entry)? = nil
          message : String? = nil
          begin
            entries = self.class.prepare_entries(nodes)
          rescue error
            message = error.message || "Directory entries could not be prepared."
          end
          GLib.idle_add do
            if active?(token, chat_id)
              if result = entries
                fill_entries(path, result, chat_id, token, incremental)
              else
                show_status(
                  "Could Not Open Files",
                  message || "Directory entries could not be prepared."
                )
              end
            end
            false
          end
          nil
        end
        unless queued
          show_status(
            "Still Loading Files",
            "Too many previews are being prepared. Try again shortly."
          )
        end
      end

      private def fill_entries(
        path : String,
        entries : Array(Entry),
        chat_id : String,
        token : Int64,
        incremental : Bool,
      ) : Nil
        @path = path
        @file_path = nil
        @showing_preview = false
        set_header_path(path)
        incremental &&= @entries_ready
        if entries.empty?
          if incremental
            reconcile_entries(entries)
          else
            @entries.remove_all
            @entries_data = entries
          end
          show_status(
            "Empty Folder",
            "This folder has no visible files."
          )
          @entries_ready = true
        elsif incremental
          reconcile_entries(entries)
          @stack.visible_child_name = "entries"
        else
          @entries_ready = false
          @entries.remove_all
          @entries_data = entries
          show_status(
            "Loading Files…",
            "Preparing #{entries.size} entries."
          )
          append_entry_batch(path, entries, chat_id, token, 0)
        end
      end

      private def append_entry_batch(
        path : String,
        entries : Array(Entry),
        chat_id : String,
        token : Int64,
        start : Int,
      ) : Nil
        return unless active?(token, chat_id) && @path == path

        finish = self.class.entry_batch_finish(start, entries.size)
        entries[start...finish].each { |entry| add_entry_row(entry) }
        if finish < entries.size
          @status.description = "#{finish} of #{entries.size} entries"
          GLib.idle_add do
            append_entry_batch(
              path,
              entries,
              chat_id,
              token,
              finish
            )
            false
          end
        else
          @entries_ready = true
          @stack.visible_child_name = "entries"
        end
      end

      private def add_entry_row(entry : Entry) : Nil
        @entries.append(build_entry_row(entry))
      end

      private def build_entry_row(entry : Entry) : Gtk::ListBoxRow
        row = Gtk::ListBoxRow.new
        row.child = build_entry_content(entry)
        row
      end

      private def build_entry_content(entry : Entry) : Gtk::Box
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

        box
      end

      private def reconcile_entries(entries : Array(Entry)) : Nil
        wanted = entries.to_h { |entry| {entry.name, entry} }
        index = @entries_data.size - 1
        while index >= 0
          unless wanted.has_key?(@entries_data[index].name)
            @entries.row_at_index(index).try { |row| @entries.remove(row) }
            @entries_data.delete_at(index)
          end
          index -= 1
        end

        entries.each_with_index do |entry, target|
          if current = @entries_data[target]?
            if current.name == entry.name
              if current != entry
                @entries.row_at_index(target).try do |row|
                  row.child = build_entry_content(entry)
                end
                @entries_data[target] = entry
              end
              next
            end
          end

          if current = @entries_data.index { |value| value.name == entry.name }
            @entries.row_at_index(current).try { |row| @entries.remove(row) }
            @entries.insert(build_entry_row(entry), target)
            @entries_data.delete_at(current)
            @entries_data.insert(target, entry)
          else
            @entries.insert(build_entry_row(entry), target)
            @entries_data.insert(target, entry)
          end
        end
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
        @loading_preview = true
        @file_path = path
        set_header_path(path)
        show_status("Loading…", nil)
        update_actions
        token = next_token

        request_async({
          "op"     => JSON::Any.new("file-browse"),
          "chat"   => JSON::Any.new(chat_id),
          "action" => JSON::Any.new("read"),
          "path"   => JSON::Any.new(path),
        }) do |result|
          next unless active?(token, chat_id)
          unless response = result.body
            @loading_preview = false
            show_read_error(result.error)
            next
          end

          content = response["content"]?.try(&.as_s?)
          unless content
            @loading_preview = false
            show_status(
              "Could Not Open File",
              "The daemon returned an invalid file."
            )
            update_actions
            next
          end
          start_preview_text(path, content, chat_id, token)
        end
      end

      private def refresh_file(
        path : String?,
        auto_refresh : Bool = false,
      ) : Nil
        chat_id = @chat_id
        unless chat_id && path
          finish_refresh if auto_refresh
          return
        end

        token = next_token
        request_async({
          "op"     => JSON::Any.new("file-browse"),
          "chat"   => JSON::Any.new(chat_id),
          "action" => JSON::Any.new("read"),
          "path"   => JSON::Any.new(path),
        }) do |result|
          unless active?(token, chat_id) && @file_path == path
            finish_refresh if auto_refresh
            next
          end
          unless response = result.body
            show_toast(result.error || "The file could not be refreshed.")
            finish_refresh if auto_refresh
            next
          end
          text = response["content"]?.try(&.as_s?)
          unless text
            show_toast("The daemon returned an invalid file.")
            finish_refresh if auto_refresh
            next
          end
          if @preview.text == text
            finish_refresh if auto_refresh
            next
          end

          queued = BackgroundWork.submit do
            spans = SyntaxHighlight.prepare(path, text)
            GLib.idle_add do
              apply_refreshed_preview(path, text, chat_id, token, spans)
              false
            end
            nil
          end
          show_toast("Still refreshing file. Try again shortly.") unless queued
          finish_refresh if auto_refresh
        end
      end

      private def finish_refresh : Nil
        @refresh_active = false
        return unless @refresh_pending

        @refresh_pending = false
        refresh
      end

      private def apply_refreshed_preview(
        path : String,
        text : String,
        chat_id : String,
        token : Int64,
        spans : Array(HighlightSpan),
      ) : Nil
        return unless active?(token, chat_id) &&
                      @file_path == path &&
                      !@preview.modified
        change = self.class.text_change(@preview.text, text)
        return unless change

        cursor = @preview.cursor_position
        line_start = @preview.iter_at_offset(change.start)
        line_start.line_offset = 0
        line_start_offset = line_start.offset
        @loading_preview = true
        @preview.delete(
          @preview.iter_at_offset(change.start),
          @preview.iter_at_offset(change.finish)
        )
        @preview.insert(
          @preview.iter_at_offset(change.start),
          change.replacement,
          change.replacement.bytesize
        )
        @preview.remove_all_tags(
          @preview.iter_at_offset(line_start_offset),
          @preview.end_iter
        )
        replacement_size = change.replacement.size
        cursor = if cursor <= change.start
                   cursor
                 elsif cursor >= change.finish
                   cursor - (change.finish - change.start) + replacement_size
                 else
                   change.start + replacement_size
                 end
        @preview.place_cursor(
          @preview.iter_at_offset(Math.min(cursor, @preview.char_count))
        )
        @preview.modified = false
        @loading_preview = false
        update_actions

        span_start = spans.index { |span| span.finish > line_start_offset } ||
                     spans.size
        apply_highlight_batch(path, token, spans, span_start)
      end

      private def start_preview_text(
        path : String,
        text : String,
        chat_id : String,
        token : Int64,
      ) : Nil
        clear_preview_batch(path, text, chat_id, token)
      end

      private def clear_preview_batch(
        path : String,
        text : String,
        chat_id : String,
        token : Int64,
      ) : Nil
        return unless active?(token, chat_id) && @file_path == path

        count = @preview.char_count
        if count > 0
          finish = Math.min(PREVIEW_CLEAR, count)
          @preview.delete(
            @preview.start_iter,
            @preview.iter_at_offset(finish)
          )
          GLib.idle_add do
            clear_preview_batch(path, text, chat_id, token)
            false
          end
          return
        end
        insert_preview_batch(path, text, chat_id, token, 0)
      end

      private def insert_preview_batch(
        path : String,
        text : String,
        chat_id : String,
        token : Int64,
        start : Int,
      ) : Nil
        return unless active?(token, chat_id) && @file_path == path

        chunk = self.class.preview_chunk(text, start)
        unless chunk.empty?
          @preview.insert(@preview.end_iter, chunk, chunk.bytesize)
          GLib.idle_add do
            insert_preview_batch(
              path,
              text,
              chat_id,
              token,
              start + chunk.bytesize
            )
            false
          end
          return
        end

        @preview.modified = false
        @loading_preview = false
        @file_path = path
        @showing_preview = true
        set_header_path(path)
        @stack.visible_child_name = "preview"
        @editor.grab_focus
        update_actions
        highlight_preview(path, text, token)
      end

      private def save_file : Nil
        chat_id = @chat_id
        path = @file_path
        return unless chat_id && path
        return unless @showing_preview &&
                      !@loading_preview &&
                      !@saving &&
                      @preview.modified

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
        @loading_preview = false
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
        update_actions
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
        @save.sensitive = @showing_preview &&
                          modified &&
                          !@loading_preview &&
                          !@saving
        @editor.sensitive = !@loading_preview && !@saving
        @back.sensitive = !@loading_preview &&
                          !@saving &&
                          (@showing_preview || !@path.empty?)
        @refresh.sensitive = !@loading_preview && !@saving
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

      private def highlight_preview(
        path : String,
        text : String,
        token : Int64,
      ) : Nil
        BackgroundWork.submit do
          spans = SyntaxHighlight.prepare(path, text)
          GLib.idle_add do
            apply_highlight_batch(path, token, spans, 0)
          end
          nil
        end
      end

      private def apply_highlight_batch(
        path : String,
        token : Int64,
        spans : Array(HighlightSpan),
        start : Int,
      ) : Bool
        return false unless active?(token, @chat_id || "") &&
                            @file_path == path &&
                            !@preview.modified

        finish = Math.min(start + HIGHLIGHT_BATCH, spans.size)
        spans[start...finish].each do |span|
          if tag = @tags[span.token]?
            @preview.apply_tag(
              tag,
              @preview.iter_at_offset(span.start),
              @preview.iter_at_offset(span.finish)
            )
          end
        end
        if finish < spans.size
          GLib.idle_add do
            apply_highlight_batch(path, token, spans, finish)
          end
        end
        false
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
