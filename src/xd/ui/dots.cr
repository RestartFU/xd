require "gtk4"

module Xd
  module UI
    class Dots
      FRAMES = [
        "<span alpha='30%'>...</span>",
        ".<span alpha='30%'>..</span>",
        "..<span alpha='30%'>.</span>",
        "...",
      ]

      getter widget : Gtk::Label

      def initialize
        @widget = Gtk::Label.new("")
        @widget.markup = FRAMES[0]
        @widget.valign = :center
        @at = 0
        @tick_id = 0_u32
        @animated = true
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

      private def start : Nil
        return unless @animated
        return unless @widget.mapped
        return unless @tick_id == 0

        @tick_id = GLib.timeout(400.milliseconds) do
          @at = (@at + 1) % FRAMES.size
          @widget.markup = FRAMES[@at]
          true
        end
      end

      private def stop : Nil
        return if @tick_id == 0

        GLib.source_remove(@tick_id)
        @tick_id = 0_u32
      end
    end
  end
end
