require "json"
require "gtk4"

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
        @generation = 0
        @window = Gtk::Window.new
        @window.title = "Search Chats"
        @window.transient_for = @parent
        @window.modal = true
        @window.destroy_with_parent = true
        @window.set_default_size(560, 480)

        @entry = Gtk::SearchEntry.new
        @entry.hexpand = true
        @entry.placeholder_text = "Find a conversation…"
        @entry.search_changed_signal.connect { queue_search }

        close = Gtk::Button.new_from_icon_name("window-close-symbolic")
        close.add_css_class("flat")
        close.tooltip_text = "Close"
        close.clicked_signal.connect { @window.destroy }

        header = Gtk::Box.new(:horizontal, 8)
        header.margin_top = 12
        header.margin_bottom = 12
        header.margin_start = 12
        header.margin_end = 12
        header.append(@entry)
        header.append(close)

        @results = Gtk::Box.new(:vertical, 4)
        @results.valign = :start
        @results.margin_top = 12
        @results.margin_bottom = 12
        @results.margin_start = 12
        @results.margin_end = 12

        scroll = Gtk::ScrolledWindow.new
        scroll.vexpand = true
        scroll.child = @results

        root = Gtk::Box.new(:vertical, 0)
        root.append(header)
        root.append(Gtk::Separator.new(:horizontal))
        root.append(scroll)
        @window.child = root
        placeholder(
          "Search Chats",
          "Find a conversation by something said in it."
        )
      end

      def present : Nil
        @window.present
        @entry.grab_focus
      end

      private def queue_search : Nil
        @generation += 1
        generation = @generation
        GLib.timeout(150.milliseconds) do
          run_search if generation == @generation
          false
        end
      end

      private def run_search : Nil
        clear_results
        query = @entry.text.strip
        if query.empty?
          placeholder(
            "Search Chats",
            "Find a conversation by something said in it."
          )
          return
        end

        response = @call.call({
          "op"    => JSON::Any.new("search"),
          "query" => JSON::Any.new(query),
        })
        return unless response

        hits = response["results"].as_a
        if hits.empty?
          placeholder("No Results", "Nothing matches that.")
          return
        end

        hits.each do |hit|
          add_result(hit)
        end
      end

      private def add_result(hit : JSON::Any) : Nil
        title_text = hit["title"].as_s
        snippet_text = hit["snippet"].as_s
        role = hit["role"].as_s.capitalize
        chat_id = hit["chat"].as_s

        title = Gtk::Label.new(title_text)
        title.xalign = 0_f32
        title.add_css_class("title")

        snippet = Gtk::Label.new("#{role} · #{snippet_text}")
        snippet.xalign = 0_f32
        snippet.wrap = true
        snippet.wrap_mode = :word_char
        snippet.lines = 2
        snippet.ellipsize = :end
        snippet.add_css_class("dim-label")

        content = Gtk::Box.new(:vertical, 3)
        content.append(title)
        content.append(snippet)

        button = Gtk::Button.new
        button.child = content
        button.halign = :fill
        button.add_css_class("flat")
        button.add_css_class("xd-search-result")
        button.clicked_signal.connect do
          @on_chat.call(chat_id, title_text)
          @window.destroy
        end
        @results.append(button)
      end

      private def placeholder(title : String, detail : String) : Nil
        clear_results

        heading = Gtk::Label.new(title)
        heading.add_css_class("title-2")
        heading.margin_top = 48

        description = Gtk::Label.new(detail)
        description.wrap = true
        description.add_css_class("dim-label")

        @results.append(heading)
        @results.append(description)
      end

      private def clear_results : Nil
        while child = @results.first_child
          @results.remove(child)
        end
      end
    end
  end
end
