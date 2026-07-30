require "json"
require "gtk4"
require "./adw"

module Xd
  module UI
    class SearchDialog
      def initialize(
        @parent : Gtk::Window,
        @call : Proc(
          Hash(String, JSON::Any),
          Hash(String, JSON::Any)?,
        ),
        @on_chat : Proc(String, String, Nil),
      )
        @rows = {} of UInt64 => Tuple(String, String)

        @dialog = Adw::Dialog.new
        @dialog.title = "Search"
        @dialog.content_width = 560
        @dialog.content_height = 480

        @entry = Gtk::SearchEntry.new
        @entry.hexpand = true
        @entry.search_changed_signal.connect { run_search }

        header = Adw::HeaderBar.new
        header.title_widget = @entry

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
        @dialog.child = toolbar
      end

      def present : Nil
        @dialog.present(@parent)
        @entry.grab_focus
      end

      private def run_search : Nil
        clear_results
        query = @entry.text.strip
        if query.empty?
          show_placeholder(
            "Search Chats",
            "Find a conversation by something said in it."
          )
          return
        end

        response = @call.call({
          "op"    => JSON::Any.new("search"),
          "query" => JSON::Any.new(query),
        })
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
        @dialog.close
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
