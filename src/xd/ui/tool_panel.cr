require "base64"
require "json"
require "gtk4"
require "./vte"

module Xd
  module UI
    class ToolPanel
      getter widget : Gtk::Box

      @chat_id : String?
      @file_directory = ""
      @open_file : String?
      @diff_mode = "working"
      @terminal_id : String?
      @last_columns = 0_i64
      @last_rows = 0_i64

      @stack : Gtk::Stack
      @file_path = Gtk::Label.new("")
      @file_list = Gtk::Box.new(:vertical, 2)
      @file_editor = Gtk::TextView.new
      @file_save = Gtk::Button.new_with_label("Save")
      @file_status = Gtk::Label.new("")
      @diff_view = Gtk::TextView.new
      @diff_status = Gtk::Label.new("")
      @terminal = Vte::Terminal.new
      @terminal_status = Gtk::Label.new("")

      def initialize(
        @call : Proc(
          Hash(String, JSON::Any),
          Hash(String, JSON::Any)?
        ),
      )
        @chat_id = nil
        @open_file = nil
        @terminal_id = nil

        @widget = Gtk::Box.new(:vertical, 0)
        @widget.width_request = 460
        @widget.add_css_class("xd-tool-panel")
        @widget.visible = false

        @stack = Gtk::Stack.new
        @stack.hexpand = true
        @stack.vexpand = true
        @stack.add_titled(build_files, "files", "Files")
        @stack.add_titled(build_diff, "diff", "Diff")
        @stack.add_titled(build_terminal, "terminal", "Terminal")

        switcher = Gtk::StackSwitcher.new
        switcher.stack = @stack
        switcher.halign = :center

        close = Gtk::Button.new_from_icon_name("window-close-symbolic")
        close.add_css_class("flat")
        close.tooltip_text = "Close developer panel"
        close.clicked_signal.connect { @widget.visible = false }

        header = Gtk::Box.new(:horizontal, 8)
        header.margin_top = 8
        header.margin_bottom = 8
        header.margin_start = 8
        header.margin_end = 8
        header.append(switcher)
        header.append(close)

        @widget.append(header)
        @widget.append(Gtk::Separator.new(:horizontal))
        @widget.append(@stack)

        GLib.timeout(250.milliseconds) do
          sync_terminal_size
          true
        end
      end

      def chat=(chat_id : String?) : String?
        return chat_id if @chat_id == chat_id

        @chat_id = chat_id
        @file_directory = ""
        @open_file = nil
        @terminal_id = nil
        @terminal.reset(true, true)
        @terminal_status.text = ""
        refresh_visible if @widget.visible?
        chat_id
      end

      def toggle(page : String) : Nil
        if @widget.visible? && @stack.visible_child_name == page
          @widget.visible = false
          return
        end

        @stack.visible_child_name = page
        @widget.visible = true
        refresh_visible
      end

      def handle_event(event : Hash(String, JSON::Any)) : Nil
        return unless event["chat"]?.try(&.as_s?) == @chat_id

        case event["event"]?.try(&.as_s?)
        when "terminal-opened"
          @terminal_id = event["terminal"].as_s
          @last_columns = event["columns"]?.try(&.as_i64?) || 0_i64
          @last_rows = event["rows"]?.try(&.as_i64?) || 0_i64
          @terminal_status.text = event["title"]?.try(&.as_s?) || ""
        when "terminal-output"
          return unless event["terminal"]?.try(&.as_s?) == @terminal_id
          if encoded = event["data"]?.try(&.as_s?)
            @terminal.feed(Base64.decode(encoded))
          end
        when "terminal-resized"
          return unless event["terminal"]?.try(&.as_s?) == @terminal_id
          @last_columns = event["columns"]?.try(&.as_i64?) || @last_columns
          @last_rows = event["rows"]?.try(&.as_i64?) || @last_rows
        when "terminal-closed"
          return unless event["terminal"]?.try(&.as_s?) == @terminal_id
          @terminal_id = nil
          @terminal_status.text = "Terminal closed"
        when "turn-finished"
          refresh_diff if @widget.visible? &&
                          @stack.visible_child_name == "diff"
        end
      rescue Base64::Error
        @terminal_status.text = "Terminal sent invalid data"
      end

      private def build_files : Gtk::Widget
        @file_path.text = "/"
        @file_path.xalign = 0_f32
        @file_path.hexpand = true
        @file_path.ellipsize = :middle

        up = Gtk::Button.new_from_icon_name("go-up-symbolic")
        up.add_css_class("flat")
        up.tooltip_text = "Parent directory"
        up.clicked_signal.connect { file_up }

        refresh = Gtk::Button.new_from_icon_name("view-refresh-symbolic")
        refresh.add_css_class("flat")
        refresh.tooltip_text = "Refresh files"
        refresh.clicked_signal.connect { refresh_files }

        file_header = Gtk::Box.new(:horizontal, 6)
        file_header.margin_top = 8
        file_header.margin_bottom = 8
        file_header.margin_start = 8
        file_header.margin_end = 8
        file_header.append(up)
        file_header.append(@file_path)
        file_header.append(refresh)

        list_scroll = Gtk::ScrolledWindow.new
        list_scroll.min_content_height = 180
        list_scroll.vexpand = false
        list_scroll.child = @file_list

        @file_editor.monospace = true
        @file_editor.wrap_mode = :none
        @file_editor.editable = false
        editor_scroll = Gtk::ScrolledWindow.new
        editor_scroll.vexpand = true
        editor_scroll.child = @file_editor

        @file_save.sensitive = false
        @file_save.add_css_class("suggested-action")
        @file_save.clicked_signal.connect { save_file }

        @file_status.xalign = 0_f32
        @file_status.hexpand = true
        @file_status.ellipsize = :middle
        @file_status.add_css_class("dim-label")

        editor_actions = Gtk::Box.new(:horizontal, 8)
        editor_actions.margin_top = 6
        editor_actions.margin_bottom = 8
        editor_actions.margin_start = 8
        editor_actions.margin_end = 8
        editor_actions.append(@file_status)
        editor_actions.append(@file_save)

        box = Gtk::Box.new(:vertical, 0)
        box.append(file_header)
        box.append(list_scroll)
        box.append(Gtk::Separator.new(:horizontal))
        box.append(editor_scroll)
        box.append(editor_actions)
        box
      end

      private def build_diff : Gtk::Widget
        working = Gtk::Button.new_with_label("Working")
        working.add_css_class("flat")
        working.clicked_signal.connect do
          @diff_mode = "working"
          refresh_diff
        end
        branch = Gtk::Button.new_with_label("Branch")
        branch.add_css_class("flat")
        branch.clicked_signal.connect do
          @diff_mode = "branch"
          refresh_diff
        end
        refresh = Gtk::Button.new_from_icon_name("view-refresh-symbolic")
        refresh.add_css_class("flat")
        refresh.tooltip_text = "Refresh diff"
        refresh.clicked_signal.connect { refresh_diff }

        @diff_status.xalign = 0_f32
        @diff_status.hexpand = true
        @diff_status.add_css_class("dim-label")

        header = Gtk::Box.new(:horizontal, 6)
        header.margin_top = 8
        header.margin_bottom = 8
        header.margin_start = 8
        header.margin_end = 8
        header.append(working)
        header.append(branch)
        header.append(@diff_status)
        header.append(refresh)

        @diff_view.monospace = true
        @diff_view.editable = false
        @diff_view.wrap_mode = :none
        scroll = Gtk::ScrolledWindow.new
        scroll.vexpand = true
        scroll.child = @diff_view

        box = Gtk::Box.new(:vertical, 0)
        box.append(header)
        box.append(scroll)
        box
      end

      private def build_terminal : Gtk::Widget
        new_terminal = Gtk::Button.new_with_label("New")
        new_terminal.add_css_class("flat")
        new_terminal.clicked_signal.connect { open_terminal(false) }
        close_terminal = Gtk::Button.new_with_label("Close")
        close_terminal.add_css_class("flat")
        close_terminal.clicked_signal.connect { kill_terminal }

        @terminal_status.xalign = 0_f32
        @terminal_status.hexpand = true
        @terminal_status.ellipsize = :middle
        @terminal_status.add_css_class("dim-label")

        header = Gtk::Box.new(:horizontal, 6)
        header.margin_top = 8
        header.margin_bottom = 8
        header.margin_start = 8
        header.margin_end = 8
        header.append(new_terminal)
        header.append(close_terminal)
        header.append(@terminal_status)

        @terminal.hexpand = true
        @terminal.vexpand = true
        @terminal.input_enabled = true
        @terminal.scroll_on_keystroke = true
        @terminal.scroll_on_output = true
        @terminal.scrollback_lines = 10_000_u32
        @terminal.add_css_class("xd-terminal")
        @terminal.commit_signal.connect do |text, size|
          bytes = text.to_slice
          length = Math.min(size.to_i, bytes.size)
          send_terminal_input(bytes[0, length]) if length > 0
        end

        box = Gtk::Box.new(:vertical, 0)
        box.append(header)
        box.append(@terminal)
        box
      end

      private def refresh_visible : Nil
        return unless @chat_id

        case @stack.visible_child_name
        when "files"    then refresh_files
        when "diff"     then refresh_diff
        when "terminal" then load_terminal
        end
      end

      private def refresh_files : Nil
        chat_id = @chat_id
        return unless chat_id

        response = @call.call({
          "op"     => JSON::Any.new("file-browse"),
          "chat"   => JSON::Any.new(chat_id),
          "action" => JSON::Any.new("list"),
          "path"   => JSON::Any.new(@file_directory),
        })
        return unless response

        clear(@file_list)
        @file_path.text = @file_directory.empty? ?
                          "/" : "/#{@file_directory}"
        response["entries"].as_a.each do |entry|
          name = entry["name"].as_s
          directory = entry["directory"].as_bool
          relative = join_path(@file_directory, name)
          button = Gtk::Button.new_with_label(
            directory ? "▸ #{name}" : name
          )
          button.add_css_class("flat")
          button.halign = :fill
          button.clicked_signal.connect do
            if directory
              @file_directory = relative
              refresh_files
            else
              read_file(relative)
            end
          end
          @file_list.append(button)
        end
      end

      private def file_up : Nil
        return if @file_directory.empty?

        parts = @file_directory.split('/')
        parts.pop?
        @file_directory = parts.join("/")
        refresh_files
      end

      private def read_file(path : String) : Nil
        chat_id = @chat_id
        return unless chat_id

        response = @call.call({
          "op"     => JSON::Any.new("file-browse"),
          "chat"   => JSON::Any.new(chat_id),
          "action" => JSON::Any.new("read"),
          "path"   => JSON::Any.new(path),
        })
        return unless response

        @open_file = path
        @file_editor.buffer.text = response["content"].as_s
        @file_editor.editable = true
        @file_save.sensitive = true
        @file_status.text = path
      end

      private def save_file : Nil
        chat_id = @chat_id
        path = @open_file
        return unless chat_id && path

        if @call.call({
             "op"      => JSON::Any.new("file-browse"),
             "chat"    => JSON::Any.new(chat_id),
             "action"  => JSON::Any.new("write"),
             "path"    => JSON::Any.new(path),
             "content" => JSON::Any.new(@file_editor.buffer.text),
           })
          @file_status.text = "#{path} · saved"
        end
      end

      private def refresh_diff : Nil
        chat_id = @chat_id
        return unless chat_id

        request = {
          "op"   => JSON::Any.new("diff-read"),
          "chat" => JSON::Any.new(chat_id),
        }
        if @diff_mode == "branch"
          base_response = @call.call(
            request.merge({
              "read" => JSON::Any.new("base"),
            })
          )
          return unless base_response
          base = base_response["output"].as_s.strip
          if base.empty?
            @diff_status.text = "No base branch"
            @diff_view.buffer.text = ""
            return
          end
          request["read"] = JSON::Any.new("branch-all")
          request["base"] = JSON::Any.new(base)
          @diff_status.text = "#{base}…HEAD"
        else
          request["read"] = JSON::Any.new("working-all")
          @diff_status.text = "HEAD + untracked"
        end

        response = @call.call(request)
        return unless response
        output = response["output"].as_s
        @diff_view.buffer.text = output
        @diff_status.text += output.empty? ? " · clean" : ""
      end

      private def load_terminal : Nil
        chat_id = @chat_id
        return unless chat_id

        response = @call.call({
          "op"   => JSON::Any.new("terminal-list"),
          "chat" => JSON::Any.new(chat_id),
        })
        return unless response

        terminals = response["terminals"].as_a
        if terminal = terminals.first?
          @terminal_id = terminal["id"].as_s
          @terminal_status.text = terminal["title"].as_s
          @terminal.reset(true, true)
          terminal["replay"].as_a.each do |item|
            if encoded = item["data"]?.try(&.as_s?)
              @terminal.feed(Base64.decode(encoded))
            else
              columns = item["columns"].as_i64
              rows = item["rows"].as_i64
              @terminal.set_size(columns, rows)
              @last_columns = columns
              @last_rows = rows
            end
          end
        else
          open_terminal(true)
        end
      rescue Base64::Error
        @terminal_status.text = "Terminal replay is invalid"
      end

      private def open_terminal(reuse : Bool) : Nil
        chat_id = @chat_id
        return unless chat_id

        @terminal.reset(true, true) unless reuse
        response = @call.call({
          "op"      => JSON::Any.new("terminal-open"),
          "chat"    => JSON::Any.new(chat_id),
          "columns" => JSON::Any.new(100_i64),
          "rows"    => JSON::Any.new(30_i64),
          "reuse"   => JSON::Any.new(reuse),
        })
        return unless response

        @terminal_id = response["id"].as_s
        @last_columns = 100_i64
        @last_rows = 30_i64
        @terminal_status.text = "Terminal"
        @terminal.grab_focus
      end

      private def send_terminal_input(data : Bytes) : Nil
        terminal_id = @terminal_id
        return unless terminal_id

        @call.call({
          "op"       => JSON::Any.new("terminal-input"),
          "terminal" => JSON::Any.new(terminal_id),
          "data"     => JSON::Any.new(Base64.strict_encode(data)),
        })
      end

      private def sync_terminal_size : Nil
        terminal_id = @terminal_id
        return unless terminal_id && @widget.visible? &&
                      @stack.visible_child_name == "terminal"

        columns = @terminal.column_count
        rows = @terminal.row_count
        return if columns <= 0 || rows <= 0
        return if columns == @last_columns && rows == @last_rows

        @last_columns = columns
        @last_rows = rows
        @call.call({
          "op"       => JSON::Any.new("terminal-resize"),
          "terminal" => JSON::Any.new(terminal_id),
          "columns"  => JSON::Any.new(columns),
          "rows"     => JSON::Any.new(rows),
        })
      end

      private def kill_terminal : Nil
        terminal_id = @terminal_id
        return unless terminal_id

        @call.call({
          "op"       => JSON::Any.new("terminal-kill"),
          "terminal" => JSON::Any.new(terminal_id),
        })
      end

      private def join_path(parent : String, name : String) : String
        parent.empty? ? name : "#{parent}/#{name}"
      end

      private def clear(box : Gtk::Box) : Nil
        while child = box.first_child
          box.remove(child)
        end
      end
    end
  end
end
