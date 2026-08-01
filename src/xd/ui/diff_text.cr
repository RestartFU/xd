require "gtk4"
require "../unified_diff"

@[Link("cairo", pkg_config: "cairo")]
lib LibXdCairoDraw
  fun cairo_rectangle(
    context : Pointer(Void),
    x : Float64,
    y : Float64,
    width : Float64,
    height : Float64,
  ) : Void
  fun cairo_fill(context : Pointer(Void)) : Void
end

module Xd
  module UI
    class DiffText
      PANGO_SCALE = 1024_f32

      getter widget : Gtk::Overlay
      getter label : Gtk::Label
      getter row_kinds : Array(DiffLineKind)

      @drawing : Gtk::DrawingArea
      @chunk : Bool

      def initialize(
        markup : String = "",
        @row_kinds : Array(DiffLineKind) = [] of DiffLineKind,
        @chunk : Bool = false,
      )
        @drawing = Gtk::DrawingArea.new
        @drawing.can_target = false
        @drawing.hexpand = true
        @drawing.vexpand = true

        @label = Gtk::Label.new("")
        @label.selectable = true
        @label.xalign = 0_f32
        @label.yalign = 0_f32
        @label.hexpand = true
        @label.add_css_class("xd-diff-text")
        @label.add_css_class("xd-diff-chunk") if @chunk

        @widget = Gtk::Overlay.new
        @widget.child = @drawing
        @widget.add_overlay(@label)
        @widget.set_measure_overlay(@label, true)
        @widget.hexpand = true

        @drawing.draw_func = ->(_area : Gtk::DrawingArea, context : Cairo::Context, width : Int32, height : Int32) {
          draw_backgrounds(context, width, height)
        }
        set_rows(markup, @row_kinds)
      end

      def set_rows(
        markup : String,
        row_kinds : Array(DiffLineKind),
      ) : Nil
        @row_kinds = row_kinds.dup
        @label.markup = markup
        @drawing.queue_draw
      end

      def self.line_y(
        label_origin_y : Float32,
        layout_origin_y : Int32,
        layout_units : Int32,
      ) : Float32
        label_origin_y + layout_origin_y.to_f32 +
          layout_units.to_f32 / PANGO_SCALE
      end

      private def draw_backgrounds(
        context : Cairo::Context,
        width : Int32,
        height : Int32,
      ) : Nil
        return if @row_kinds.empty? || width <= 0

        layout = @label.layout
        iter = layout.iter
        label_origin = @label.compute_point(
          @drawing,
          Graphene::Point.zero
        )
        layout_origin_y = 0
        LibGtk.gtk_label_get_layout_offsets(
          @label.to_unsafe,
          Pointer(Int32).null,
          pointerof(layout_origin_y)
        )

        row = 0
        block_colour : String? = nil
        block_top = 0_f32
        block_bottom = 0_f32

        loop do
          top_units = 0
          bottom_units = 0
          LibPango.pango_layout_iter_get_line_yrange(
            iter.to_unsafe,
            pointerof(top_units),
            pointerof(bottom_units)
          )
          top = self.class.line_y(
            label_origin.y,
            layout_origin_y,
            top_units
          )
          bottom = self.class.line_y(
            label_origin.y,
            layout_origin_y,
            bottom_units
          )
          colour = @row_kinds[row]?.try(&.background)
          top = 0_f32 if @chunk && row == 0 && colour

          if colour != block_colour
            append_block(
              context,
              block_colour,
              block_top,
              block_bottom,
              width
            )
            block_colour = colour
            block_top = top
          end
          block_bottom = bottom
          row += 1
          break unless iter.next_line
        end

        if @chunk && block_colour
          block_bottom = Math.max(block_bottom, height.to_f32)
        end
        append_block(
          context,
          block_colour,
          block_top,
          block_bottom,
          width
        )
      end

      private def append_block(
        context : Cairo::Context,
        colour : String?,
        top : Float32,
        bottom : Float32,
        width : Int32,
      ) : Nil
        return unless colour
        return if bottom <= top || width <= 0

        rgba = Gdk::RGBA.new
        return unless rgba.parse(colour)

        Gdk.cairo_set_source_rgba(context, rgba)
        LibXdCairoDraw.cairo_rectangle(
          context.to_unsafe,
          0_f64,
          top.to_f64,
          width.to_f64,
          (bottom - top).to_f64
        )
        LibXdCairoDraw.cairo_fill(context.to_unsafe)
      end
    end
  end
end
