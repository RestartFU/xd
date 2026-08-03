require "json"
require "gtk4"
require "../daemon/client"
require "../daemon/endpoint"
require "./adw"

module Xd
  module UI
    class SearchDialog
      def initialize(
        @parent : Gtk::Window,
        @endpoint : Daemon::Endpoint,
        @on_chat : Proc(String, String, Nil),
        @on_close : Proc(Nil),
      )
        @rows = {} of UInt64 => Tuple(String, String)
        @generation = 0_i64
        @closed = false

        @window = Gtk::Window.new
        @window.title = "Search"
        @window.transient_for = @parent
        @window.application = @parent.application
        @window.destroy_with_parent = true
        @window.modal = true
        @window.decorated = false
        @window.set_default_size(560, 480)
        @window.add_css_class("xd-panel")

        @entry = Gtk::SearchEntry.new
        @entry.hexpand = true
        @entry.search_changed_signal.connect { queue_search }

        header = Adw::HeaderBar.new
        header.title_widget = @entry
        close_button = Gtk::Button.new_from_icon_name("window-close-symbolic")
        close_button.add_css_class("flat")
        close_button.tooltip_text = "Close"
        close_button.clicked_signal.connect { close }
        header.pack_end(close_button)

        @results = Gtk::ListBox.new
        @results.selection_mode = :none
        @results.add_css_class("boxed-list")
        @results.valign = :start
        @results.margin_top = 12
        @results.margin_bottom = 12
        @results.margin_start = 12
        @results.margin_end = 12
        @results.row_activated_signal.connect do |row|
          activate_result(row)
        end

        scroll = Gtk::ScrolledWindow.new
        scroll.set_policy(:never, :automatic)
        scroll.child = @results

        @placeholder = Adw::StatusPage.new(
          icon_name: "system-search-symbolic"
        )

        @stack = Gtk::Stack.new
        @stack.vexpand = true
        @stack.add_named(@placeholder, "placeholder")
        @stack.add_named(scroll, "results")
        show_placeholder(
          "Search Chats",
          "Find a conversation by something said in it."
        )

        toolbar = Adw::ToolbarView.new
        toolbar.add_top_bar(header)
        toolbar.content = @stack
        @window.child = toolbar
        @window.destroy_signal.connect { closed }
        @window.close_request_signal.connect do
          close
          true
        end

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, _state|
          if keyval == Gdk::KEY_Escape
            close
            true
          else
            false
          end
        end
        @window.add_controller(keys)
      end

      def present : Nil
        @window.present
        @entry.grab_focus
      end

      private def queue_search : Nil
        return if @closed

        @generation += 1
        generation = @generation
        GLib.timeout(150.milliseconds) do
          run_search if !@closed && generation == @generation
          false
        end
      end

      private def run_search : Nil
        return if @closed

        clear_results
        @generation += 1
        generation = @generation
        query = @entry.text.strip
        if query.empty?
          show_placeholder(
            "Search Chats",
            "Find a conversation by something said in it."
          )
          return
        end

        show_placeholder("Searching…", "Looking through conversations.")
        spawn do
          response : Hash(String, JSON::Any)? = nil
          begin
            response = @endpoint.call({
              "op"    => JSON::Any.new("search"),
              "query" => JSON::Any.new(query),
            })
          rescue Daemon::Client::Error
          end
          GLib.idle_add do
            apply_search(response) if !@closed &&
                                      generation == @generation
            false
          end
        end
      end

      private def apply_search(
        response : Hash(String, JSON::Any)?,
      ) : Nil
        unless response
          show_placeholder(
            "Search Failed",
            "The daemon could not search conversations."
          )
          return
        end

        hits = response["results"].as_a
        if hits.empty?
          show_placeholder("No Results", "Nothing matches that.")
          return
        end

        hits.each { |hit| add_result(hit) }
        @stack.visible_child_name = "results"
      end

      private def add_result(hit : JSON::Any) : Nil
        title = hit["title"].as_s
        row = Adw::ActionRow.new(
          title: title,
          subtitle: hit["snippet"].as_s,
          subtitle_lines: 2
        )
        row.activatable = true
        @rows[row.to_unsafe.address] = {hit["chat"].as_s, title}
        @results.append(row)
      end

      private def activate_result(row : Gtk::ListBoxRow) : Nil
        result = @rows[row.to_unsafe.address]?
        return unless result

        @on_chat.call(result[0], result[1])
        close
      end

      private def close : Nil
        @window.destroy unless @closed
      end

      private def closed : Nil
        return if @closed

        @closed = true
        @generation += 1
        @on_close.call
      end

      private def show_placeholder(title : String, detail : String) : Nil
        @placeholder.title = title
        @placeholder.description = detail
        @stack.visible_child_name = "placeholder"
      end

      private def clear_results : Nil
        @rows.clear
        while child = @results.first_child
          @results.remove(child)
        end
      end
    end
  end
end
