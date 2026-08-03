require "gtk4"
require "../unified_diff"
require "./background_work"
require "./diff_text"

module Xd
  module UI
    module DiffView
      extend self

      INLINE_EAGER_ROWS   =  60
      INLINE_PREVIEW_ROWS = 120

      record Prepared,
        rows : Int32,
        additions : UInt32,
        deletions : UInt32,
        markup : DiffMarkup

      def build(
        patch : String,
        show_file_headers : Bool = true,
      ) : Gtk::Box
        build_prepared(prepare(patch, show_file_headers), show_file_headers)
      end

      def build_async(
        patch : String,
        show_file_headers : Bool = true,
      ) : Gtk::Box
        box = container
        loading = Gtk::Label.new("Preparing diff…")
        loading.xalign = 0_f32
        loading.margin_top = 8
        loading.margin_bottom = 8
        loading.margin_start = 12
        loading.add_css_class("dim-label")
        box.append(loading)

        queued = BackgroundWork.submit do
          prepared : Prepared? = nil
          message : String? = nil
          begin
            prepared = prepare(patch, show_file_headers)
          rescue error
            message = error.message || "Diff preview could not be prepared."
          end
          GLib.idle_add do
            clear(box)
            if result = prepared
              fill_prepared(box, result, show_file_headers)
            else
              append_error(
                box,
                message || "Diff preview could not be prepared."
              )
            end
            false
          end
          nil
        end
        unless queued
          clear(box)
          append_error(box, "Diff preview queue is busy.")
        end
        box
      end

      def prepare(
        patch : String,
        show_file_headers : Bool = true,
      ) : Prepared
        parsed = UnifiedDiff.parse(patch)
        # Inline in a transcript there is one narrow column, and the path a
        # tool reported is usually absolute: the name is the part that would
        # otherwise be pushed off the end.
        lines = UnifiedDiff.name_only(parsed.lines)
        rows = UnifiedDiff.display_rows(
          lines,
          show_file_headers
        )
        Prepared.new(
          rows,
          parsed.additions,
          parsed.deletions,
          UnifiedDiff.markup(
            lines,
            show_file_headers,
            INLINE_PREVIEW_ROWS
          )
        )
      end

      def build_prepared(
        prepared : Prepared,
        show_file_headers : Bool = true,
      ) : Gtk::Box
        box = container
        fill_prepared(box, prepared, show_file_headers)
        box
      end

      private def fill_prepared(
        box : Gtk::Box,
        prepared : Prepared,
        show_file_headers : Bool,
      ) : Nil
        if !show_file_headers || prepared.rows <= INLINE_EAGER_ROWS
          fill_markup(box, prepared.markup)
          return
        end

        summary =
          "Large diff · #{prepared.rows} rows · " \
          "+#{prepared.additions}  −#{prepared.deletions}"
        expander = Gtk::Expander.new(summary)
        preview = Gtk::Box.new(:vertical, 0)
        loaded = false
        expander.child = preview
        expander.add_css_class("xd-diff-expander")
        expander.notify_signal["expanded"].connect do |_property|
          if expander.expanded? && !loaded
            fill_markup(preview, prepared.markup)
            loaded = true
          end
        end
        box.append(expander)
      end

      private def fill_markup(
        box : Gtk::Box,
        result : DiffMarkup,
      ) : Nil
        clear(box)
        text = DiffText.new(result.markup, result.row_kinds)
        text.widget.hexpand = true
        box.append(text.widget)
      end

      private def container : Gtk::Box
        box = Gtk::Box.new(:vertical, 0)
        box.valign = :start
        box.hexpand = true
        box.add_css_class("xd-diff-view")
        box
      end

      private def append_error(box : Gtk::Box, message : String) : Nil
        label = Gtk::Label.new(message)
        label.xalign = 0_f32
        label.wrap = true
        label.margin_top = 8
        label.margin_bottom = 8
        label.margin_start = 12
        label.add_css_class("error")
        box.append(label)
      end

      private def clear(box : Gtk::Box) : Nil
        while child = box.first_child
          box.remove(child)
        end
      end
    end
  end
end
