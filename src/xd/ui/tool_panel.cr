require "json"
require "gtk4"
require "./diff_pane"
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
      @repository_page : String?

      @call : Proc(
        Hash(String, JSON::Any),
        Hash(String, JSON::Any)?,
      )
      @diff_pane : DiffPane
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
        @diff_pane = DiffPane.new(@request)
        @file_pane = FilePane.new(@request)

        @repository_widget = Gtk::Stack.new
        @repository_widget.hexpand = true
        @repository_widget.vexpand = true
        @repository_widget.add_named(@file_pane.widget, "files")
        @repository_widget.add_named(@diff_pane.widget, "diff")
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
        @diff_pane.select_chat(chat_id)
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
          @diff_pane.refresh if @repository_widget.visible? &&
                                @repository_page == "diff"
        end
      end

      def remote_connection_changed(
        connected : Bool,
        error : String?,
      ) : Nil
        @terminal_panel.remote_connection_changed(connected, error)
      end

      private def refresh_visible : Nil
        return unless @chat_id

        @terminal_panel.activate(false) if @terminal_widget.visible?
        refresh_repository(@repository_page) if @repository_widget.visible?
      end

      private def refresh_repository(page : String?) : Nil
        case page
        when "files" then @file_pane.refresh
        when "diff"  then @diff_pane.refresh
        end
      end
    end
  end
end
