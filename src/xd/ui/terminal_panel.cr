require "base64"
require "json"
require "gtk4"
require "set"
require "./adw"
require "./vte"

module Xd
  module UI
    # Daemon-backed terminal sessions grouped by chat.
    #
    # Local and remote terminals use the same RPC contract. AdwTabView state
    # remains device-local, while daemon replay makes reconnects authoritative.
    class TerminalPanel
      private class Session
        getter id : String
        getter terminal : Vte::Terminal
        getter page : Adw::TabPage
        property columns : Int64
        property rows : Int64
        property removing = false

        def initialize(
          @id : String,
          @terminal : Vte::Terminal,
          @page : Adw::TabPage,
          @columns : Int64,
          @rows : Int64,
        )
        end
      end

      private class View
        getter tabs : Adw::TabView
        getter sessions = {} of String => Session
        property loaded = false

        def initialize(@tabs : Adw::TabView)
        end
      end

      getter widget : Gtk::Box

      @chat_id : String?
      @view_key : String?
      @current : View?
      @views = {} of String => View
      @stack : Gtk::Stack
      @bar : Adw::TabBar
      @title : Gtk::Label
      @title_sizes : Gtk::SizeGroup
      @focus_next = false

      def initialize(
        @call : Proc(
          Hash(String, JSON::Any),
          Hash(String, JSON::Any)?,
        ),
        @on_empty : Proc(Nil),
      )
        @chat_id = nil
        @view_key = nil
        @current = nil
        @stack = Gtk::Stack.new
        @stack.hexpand = true
        @stack.vexpand = true

        @title = Gtk::Label.new("")
        @title.xalign = 0.5_f32
        @title.hexpand = true
        @title.halign = :fill
        @title.valign = :center
        @title.ellipsize = :end
        @title.add_css_class("heading")
        @title.can_target = false
        @title.visible = false

        @bar = Adw::TabBar.new
        @bar.autohide = false
        @bar.hexpand = true
        @bar.add_css_class("inline")
        @bar.visible = false

        add = Gtk::Button.new_from_icon_name("list-add-symbolic")
        add.add_css_class("flat")
        add.tooltip_text = "New session"
        add.clicked_signal.connect { open_terminal(false) }

        kill = Gtk::Button.new_from_icon_name("user-trash-symbolic")
        kill.add_css_class("flat")
        kill.tooltip_text = "Kill this session"
        kill.clicked_signal.connect { kill_selected }

        controls = Gtk::Box.new(:horizontal, 2)
        controls.margin_top = 4
        controls.margin_bottom = 4
        controls.margin_start = 4
        controls.margin_end = 8
        controls.append(add)
        controls.append(kill)

        title_start = Gtk::Box.new(:horizontal, 0)
        title_end = Gtk::Box.new(:horizontal, 0)
        @title_sizes = Gtk::SizeGroup.new(:horizontal)
        @title_sizes.add_widget(title_start)
        @title_sizes.add_widget(title_end)
        @title_sizes.add_widget(controls)

        title_row = Gtk::Box.new(:horizontal, 0)
        title_row.append(title_start)
        title_row.append(@title)
        title_row.append(title_end)
        title_row.can_target = false

        tabs = Gtk::Box.new(:horizontal, 0)
        tabs.append(@bar)
        tabs.append(controls)

        header = Gtk::Overlay.new
        header.child = tabs
        header.add_overlay(title_row)
        header.add_css_class("xd-divider-bottom")

        @widget = Gtk::Box.new(:vertical, 0)
        @widget.append(header)
        @widget.append(@stack)

        GLib.timeout(250.milliseconds) do
          sync_size
          true
        end
      end

      def select_chat(chat_id : String?, view_key : String?) : Nil
        return if @chat_id == chat_id && @view_key == view_key

        @chat_id = chat_id
        @view_key = view_key
        @focus_next = false
        unless chat_id && view_key
          @current = nil
          @bar.view = nil
          update_title
          return
        end

        view = ensure_view(view_key)
        @current = view
        @stack.visible_child_name = view_key
        @bar.view = view.tabs
        update_title
      end

      def activate(focus : Bool = true) : Nil
        chat_id = @chat_id
        view = @current
        return unless chat_id && view

        load_sessions(view) unless view.loaded
        if view.sessions.empty?
          @focus_next = focus
          open_terminal(true)
        elsif focus
          selected_terminal(view).try(&.grab_focus)
        end
      end

      def handle_event(event : Hash(String, JSON::Any)) : Nil
        return unless event["chat"]?.try(&.as_s?) == @chat_id
        id = event["terminal"]?.try(&.as_s?) || return
        view = @current || return

        case event["event"]?.try(&.as_s?)
        when "terminal-opened"
          session = add_session(
            view,
            id,
            event["title"]?.try(&.as_s?) || "shell",
            event["columns"]?.try(&.as_i64?) || 80_i64,
            event["rows"]?.try(&.as_i64?) || 24_i64,
            nil
          )
          view.tabs.selected_page = session.page
          if @focus_next
            @focus_next = false
            session.terminal.grab_focus
          end
        when "terminal-output"
          session = view.sessions[id]? || return
          encoded = event["data"]?.try(&.as_s?) || return
          session.terminal.feed(Base64.decode(encoded))
        when "terminal-resized"
          session = view.sessions[id]? || return
          columns = event["columns"]?.try(&.as_i64?) || session.columns
          rows = event["rows"]?.try(&.as_i64?) || session.rows
          session.columns = columns
          session.rows = rows
          session.terminal.set_size(columns, rows)
        when "terminal-closed"
          remove_session(view, id)
        end
      rescue Base64::Error
      end

      private def ensure_view(key : String) : View
        @views[key]? || begin
          tabs = Adw::TabView.new
          view = View.new(tabs)
          tabs.close_page_signal.connect do |page|
            close_page(view, page)
          end
          tabs.notify_signal["selected-page"].connect do |_property|
            update_title if @current.same?(view)
          end
          tabs.page_attached_signal.connect do |_page, _position|
            update_title if @current.same?(view)
          end
          tabs.page_detached_signal.connect do |page, _position|
            detached(view, page)
          end
          @stack.add_named(tabs, key)
          @views[key] = view
          view
        end
      end

      private def load_sessions(view : View) : Nil
        chat_id = @chat_id
        return unless chat_id

        response = @call.call({
          "op"   => JSON::Any.new("terminal-list"),
          "chat" => JSON::Any.new(chat_id),
        })
        return unless response

        seen = Set(String).new
        response["terminals"].as_a.each do |terminal|
          id = terminal["id"].as_s
          seen << id
          add_session(
            view,
            id,
            terminal["title"]?.try(&.as_s?) || "shell",
            terminal["columns"]?.try(&.as_i64?) || 80_i64,
            terminal["rows"]?.try(&.as_i64?) || 24_i64,
            terminal["replay"]?.try(&.as_a?)
          )
        end

        view.sessions.keys.each do |id|
          remove_session(view, id) unless seen.includes?(id)
        end
        view.loaded = true
        update_title
      end

      private def add_session(
        view : View,
        id : String,
        title : String,
        columns : Int64,
        rows : Int64,
        replay : Array(JSON::Any)?,
      ) : Session
        if session = view.sessions[id]?
          session.page.title = title
          session.columns = columns
          session.rows = rows
          if replay
            session.terminal.reset(true, true)
            feed_replay(session, replay)
          end
          session.terminal.set_size(columns, rows)
          return session
        end

        terminal = Vte::Terminal.new
        configure(terminal, id)
        terminal.set_size(columns, rows)
        page = view.tabs.append(terminal)
        page.title = title
        session = Session.new(id, terminal, page, columns, rows)
        view.sessions[id] = session
        feed_replay(session, replay) if replay
        terminal.set_size(columns, rows)
        update_title
        session
      end

      private def configure(terminal : Vte::Terminal, id : String) : Nil
        terminal.hexpand = true
        terminal.vexpand = true
        terminal.input_enabled = true
        terminal.scroll_on_keystroke = true
        terminal.scroll_on_output = false
        terminal.scrollback_lines = 10_000_u32
        terminal.add_css_class("xd-terminal")
        terminal.commit_signal.connect do |text, size|
          bytes = text.to_slice
          length = Math.min(size.to_i, bytes.size)
          send_input(id, bytes[0, length]) if length > 0
        end
      end

      private def feed_replay(
        session : Session,
        replay : Array(JSON::Any),
      ) : Nil
        replay.each do |item|
          if encoded = item["data"]?.try(&.as_s?)
            session.terminal.feed(Base64.decode(encoded))
          else
            columns = item["columns"]?.try(&.as_i64?) || session.columns
            rows = item["rows"]?.try(&.as_i64?) || session.rows
            session.terminal.set_size(columns, rows)
          end
        end
      end

      private def open_terminal(reuse : Bool) : Nil
        chat_id = @chat_id
        view = @current
        return unless chat_id && view

        response = @call.call({
          "op"      => JSON::Any.new("terminal-open"),
          "chat"    => JSON::Any.new(chat_id),
          "columns" => JSON::Any.new(80_i64),
          "rows"    => JSON::Any.new(24_i64),
          "reuse"   => JSON::Any.new(reuse),
        })
        return unless response

        view.loaded = false
        load_sessions(view)
        if session = view.sessions[response["id"].as_s]?
          view.tabs.selected_page = session.page
          if @focus_next
            @focus_next = false
            session.terminal.grab_focus
          end
        end
      end

      private def kill_selected : Nil
        view = @current || return
        page = view.tabs.selected_page || return
        view.tabs.close_page(page)
      end

      private def close_page(
        view : View,
        page : Adw::TabPage,
      ) : Bool
        session = session_for_page(view, page)
        if session && !session.removing
          @call.call({
            "op"       => JSON::Any.new("terminal-kill"),
            "terminal" => JSON::Any.new(session.id),
          })
        end
        @on_empty.call if @current.same?(view) && view.tabs.n_pages == 1
        false
      end

      private def detached(view : View, page : Adw::TabPage) : Nil
        if session = session_for_page(view, page)
          view.sessions.delete(session.id)
        end
        update_title if @current.same?(view)
      end

      private def remove_session(view : View, id : String) : Nil
        session = view.sessions[id]? || return
        session.removing = true
        view.tabs.close_page(session.page)
      end

      private def session_for_page(
        view : View,
        page : Adw::TabPage,
      ) : Session?
        pointer = page.to_unsafe
        view.sessions.values.find do |session|
          session.page.to_unsafe == pointer
        end
      end

      private def selected_session(view : View) : Session?
        page = view.tabs.selected_page || return
        session_for_page(view, page)
      end

      private def selected_terminal(view : View) : Vte::Terminal?
        selected_session(view).try(&.terminal)
      end

      private def send_input(id : String, data : Bytes) : Nil
        @call.call({
          "op"       => JSON::Any.new("terminal-input"),
          "terminal" => JSON::Any.new(id),
          "data"     => JSON::Any.new(Base64.strict_encode(data)),
        })
      end

      private def sync_size : Nil
        return unless @widget.visible?
        view = @current || return
        session = selected_session(view) || return
        columns = session.terminal.column_count
        rows = session.terminal.row_count
        return if columns <= 0 || rows <= 0
        return if columns == session.columns && rows == session.rows

        session.columns = columns
        session.rows = rows
        @call.call({
          "op"       => JSON::Any.new("terminal-resize"),
          "terminal" => JSON::Any.new(session.id),
          "columns"  => JSON::Any.new(columns),
          "rows"     => JSON::Any.new(rows),
        })
      end

      private def update_title : Nil
        view = @current
        page = view.try(&.tabs.selected_page)
        show_tabs = view && view.tabs.n_pages > 1
        title = page.try(&.title?)

        @title.text = title || ""
        @bar.visible = !!show_tabs
        @title.visible = !title.nil? && !show_tabs
      end
    end
  end
end
