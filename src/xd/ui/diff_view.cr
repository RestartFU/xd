require "gtk4"
require "../unified_diff"
require "./diff_text"

module Xd
  module UI
    module DiffView
      extend self

      INLINE_EAGER_ROWS   =  60
      INLINE_PREVIEW_ROWS = 120

      def build(
        patch : String,
        show_file_headers : Bool = true,
      ) : Gtk::Box
        parsed = UnifiedDiff.parse(patch)
        rows = UnifiedDiff.display_rows(
          parsed.lines,
          show_file_headers
        )
        box = Gtk::Box.new(:vertical, 0)
        box.valign = :start
        box.hexpand = true
        box.add_css_class("xd-diff-view")

        if !show_file_headers || rows <= INLINE_EAGER_ROWS
          fill_parsed(
            box,
            parsed.lines,
            show_file_headers,
            INLINE_PREVIEW_ROWS
          )
          return box
        end

        summary =
          "Large diff · #{rows} rows · " \
          "+#{parsed.additions}  −#{parsed.deletions}"
        expander = Gtk::Expander.new(summary)
        preview = Gtk::Box.new(:vertical, 0)
        loaded = false
        expander.child = preview
        expander.add_css_class("xd-diff-expander")
        expander.notify_signal["expanded"].connect do |_property|
          if expander.expanded? && !loaded
            fill_parsed(
              preview,
              parsed.lines,
              true,
              INLINE_PREVIEW_ROWS
            )
            loaded = true
          end
        end
        box.append(expander)
        box
      end

      private def fill_parsed(
        box : Gtk::Box,
        lines : Array(DiffLine),
        show_file_headers : Bool,
        limit : Int32,
      ) : Nil
        while child = box.first_child
          box.remove(child)
        end
        result = UnifiedDiff.markup(lines, show_file_headers, limit)
        text = DiffText.new(result.markup, result.row_kinds)
        text.widget.hexpand = true
        box.append(text.widget)
      end
    end
  end
end
