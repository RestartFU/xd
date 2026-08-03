require "gtk4"

module Xd
  module UI
    # Three fixed labels whose opacity changes without replacing Pango markup.
    #
    # The old animation rewrote one label's markup every frame. On large GTK
    # transcripts that could trigger expensive layout, so render-safe mode had
    # to disable it entirely. Fixed glyphs keep animation paint-only and allow
    # the sidebar and transcript indicators to animate on every renderer.
    class Dots
      DIM_OPACITY = 0.3
      DOTS        = 3
      FRAMES      = 4

      getter widget : Gtk::Box

      def initialize
        @widget = Gtk::Box.new(:horizontal, 0)
        @widget.valign = :center
        @labels = Array(Gtk::Label).new(DOTS) do
          label = Gtk::Label.new(".")
          label.valign = :center
          @widget.append(label)
          label
        end
        @at = 0
        @tick_id = 0_u32
        @animated = true
        apply_frame
        @widget.map_signal.connect { start }
        @widget.unmap_signal.connect { stop }
      end

      def visible=(visible : Bool) : Nil
        @widget.visible = visible
      end

      def animated=(animated : Bool) : Bool
        return animated if @animated == animated

        @animated = animated
        animated ? start : stop
        animated
      end

      def self.opacity(frame : Int, dot : Int) : Float64
        dot < frame ? 1.0 : DIM_OPACITY
      end

      private def start : Nil
        return unless @animated
        return unless @widget.mapped
        return unless @tick_id == 0

        @tick_id = GLib.timeout(400.milliseconds) do
          @at = (@at + 1) % FRAMES
          apply_frame
          true
        end
      end

      private def stop : Nil
        return if @tick_id == 0

        GLib.source_remove(@tick_id)
        @tick_id = 0_u32
      end

      private def apply_frame : Nil
        @labels.each_with_index do |label, index|
          label.opacity = self.class.opacity(@at, index)
        end
      end
    end
  end
end
