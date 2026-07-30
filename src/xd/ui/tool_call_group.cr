require "gtk4"

module Xd
  module UI
    # Text-only while collapsed. Hidden tool calls create no label/layout work.
    class ToolCallGroup
      getter widget : Gtk::Expander

      @summaries = [] of String

      def initialize
        @widget = Gtk::Expander.new(nil)
        @widget.expanded = false
        @widget.add_css_class("dim-label")
        @widget.margin_top = 4
        @widget.margin_bottom = 4
        @widget.margin_start = 24
        @widget.margin_end = 24
        @widget.notify_signal["expanded"].connect do |_property|
          if @widget.expanded?
            render
          else
            @widget.child = nil
          end
        end
      end

      def append(summary : String) : Nil
        @summaries << summary
        count = @summaries.size
        @widget.label = count == 1 ? "1 tool call" : "#{count} tool calls"
        render if @widget.expanded?
      end

      private def render : Nil
        label = @widget.child.as?(Gtk::Label)
        unless label
          label = Gtk::Label.new("")
          label.xalign = 0_f32
          label.ellipsize = :middle
          label.max_width_chars = 100
          label.selectable = true
          label.add_css_class("caption")
          label.margin_top = 4
          label.margin_start = 12
          @widget.child = label
        end
        label.text = @summaries.join('\n')
      end
    end
  end
end
