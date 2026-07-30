require "json"
require "gtk4"
require "./file_pane"
require "./panel_call"
require "./terminal_panel"

module Xd
  module UI
    class ToolPanel
      getter terminal_widget : Gtk::Box
      getter repository_widget : Gtk::Stack
      getter repository_page : String?

      @chat_id : String?
      @view_key : String?
      @diff_mode = "working"
      @repository_page : String?

      @diff_view = Gtk::TextView.new
      @diff_status = Gtk::Label.new("")
      @call : Proc(
        Hash(String, JSON::Any),
        Hash(String, JSON::Any)?,
      )
      @file_pane : FilePane
      @terminal_panel : TerminalPanel

      def initialize(
        @request : PanelCall,
        on_terminal_empty : Proc(Nil),
      )
        @chat_id = nil
        @view_key = nil
        @repository_page = nil
        @call = ->(request : Hash(String, JSON::Any)) {
          @request.call(request).body
        }
        @file_pane = FilePane.new(@request)

        @repository_widget = Gtk::Stack.new
        @repository_widget.hexpand = true
        @repository_widget.vexpand = true
        @repository_widget.add_named(@file_pane.widget, "files")
        @repository_widget.add_named(build_diff, "diff")
        @repository_widget.add_css_class("xd-tool-panel")
        @repository_widget.add_css_class("xd-divider-left")
        @repository_widget.visible = false

        @terminal_panel = TerminalPanel.new(@call, on_terminal_empty)
        @terminal_widget = @terminal_panel.widget
        @terminal_widget.add_css_class("xd-tool-panel")
        @terminal_widget.add_css_class("xd-divider-top")
        @terminal_widget.visible = false
      end

      def select_chat(chat_id : String?, view_key : String?) : Nil
        return if @chat_id == chat_id && @view_key == view_key

        @chat_id = chat_id
        @view_key = view_key
        @file_pane.select_chat(chat_id)
        @terminal_panel.select_chat(chat_id, view_key)
        refresh_visible
      end

      def show_terminal(shown : Bool, focus : Bool = true) : Nil
        @terminal_widget.visible = shown
        return unless shown

        @terminal_panel.activate(focus)
      end

      def show_repository(page : String?) : Nil
        @repository_page = page
        unless page
          @repository_widget.visible = false
          return
        end

        @repository_widget.visible_child_name = page
        @repository_widget.visible = true
        refresh_repository(page)
      end

      def handle_event(event : Hash(String, JSON::Any)) : Nil
        return unless event["chat"]?.try(&.as_s?) == @chat_id

        @terminal_panel.handle_event(event)
        if event["event"]?.try(&.as_s?) == "turn-finished"
          refresh_diff if @repository_widget.visible? &&
                          @repository_page == "diff"
        end
      end

      def remote_connection_changed(
        connected : Bool,
        error : String?,
      ) : Nil
        @terminal_panel.remote_connection_changed(connected, error)
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

      private def refresh_visible : Nil
        return unless @chat_id

        @terminal_panel.activate(false) if @terminal_widget.visible?
        refresh_repository(@repository_page) if @repository_widget.visible?
      end

      private def refresh_repository(page : String?) : Nil
        case page
        when "files" then @file_pane.refresh
        when "diff"  then refresh_diff
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
    end
  end
end
