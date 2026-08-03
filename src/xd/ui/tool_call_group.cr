require "gtk4"

module Xd
  module UI
    # Contiguous tool activity, collapsed only when collapsing hides something.
    #
    # A single call shows its command: "1 tool call" behind an arrow is strictly
    # less information than the command itself, in the same space. A run of them
    # collapses, because that is where the transcript actually gets buried.
    #
    # Text-only while collapsed. Hidden tool calls create no label/layout work.
    class ToolCallGroup
      MAX_RENDERED_CALLS = 48
      MAX_RENDERED_CHARS = 12 * 1024

      getter widget : Gtk::Box

      # The subagent card binds its own toggle to this, so it needs the
      # expander rather than the container.
      getter expander : Gtk::Expander

      @summaries = [] of String
      @single : Gtk::Label
      @render_source = 0_u32
      @count_source = 0_u32

      # Collapsing one call hides its command and shows a count instead, which
      # is less in the same space. Two or more is where it starts paying.
      def self.collapse?(count : Int) : Bool
        count > 1
      end

      def self.collapsed_label(count : Int) : String
        count == 1 ? "1 tool call" : "#{count} tool calls"
      end

      def self.rendered_label(summaries : Array(String)) : String
        return "" if summaries.empty?

        shown = [] of String
        characters = 0
        index = summaries.size - 1
        while index >= 0 && shown.size < MAX_RENDERED_CALLS
          remaining = MAX_RENDERED_CHARS - characters
          break if remaining <= 0

          summary = summaries[index]
          if summary.size > remaining
            summary = if remaining == 1
                        "…"
                      else
                        "#{summary[0, remaining - 1]}…"
                      end
          end
          shown << summary
          characters += summary.size
          index -= 1
        end
        shown.reverse!

        if index >= 0
          "… #{index + 1} earlier tool calls …\n#{shown.join('\n')}"
        else
          shown.join('\n')
        end
      end

      def initialize
        @widget = Gtk::Box.new(:vertical, 0)
        @widget.margin_top = 4
        @widget.margin_bottom = 4
        @widget.margin_start = 24
        @widget.margin_end = 24

        @single = summary_label
        @single.visible = false
        @widget.append(@single)

        @expander = Gtk::Expander.new(nil)
        @expander.expanded = false
        @expander.add_css_class("dim-label")
        @expander.visible = false
        @expander.notify_signal["expanded"].connect do |_property|
          if @expander.expanded?
            schedule_render
          else
            @expander.child = nil
          end
        end
        @widget.append(@expander)
      end

      def append(summary : String) : Nil
        @summaries << summary

        unless self.class.collapse?(@summaries.size)
          @single.text = summary
          @single.visible = true
          @expander.visible = false
          return
        end

        # The second call is what turns the run into something worth hiding.
        @single.visible = false
        @expander.visible = true
        schedule_count
        schedule_render if @expander.expanded?
      end

      # Hands the run to a subagent card, which supplies the disclosure.
      #
      # The expander becomes the content either way: a lone call is revealed by
      # the subagent's own toggle, so showing it inline as well would put the
      # same command in two places.
      def absorb : Nil
        # A subagent card has its own header, so the single-call label is no
        # longer visible. Remove it instead of keeping a needless Pango child
        # in every collapsed card.
        @widget.remove(@single)
        @single.visible = false
        @expander.visible = true
        schedule_count
        schedule_render if @expander.expanded?
      end

      # A count that changes is a resize, and a resize in the transcript box is
      # measured against everything else in it. Tool calls arrive in bursts, so
      # the count is written once per turn of the main loop rather than once
      # per call: the reader cannot tell the difference and the layout can.
      private def schedule_count : Nil
        return unless @count_source == 0

        @count_source = GLib.idle_add do
          @count_source = 0_u32
          label = self.class.collapsed_label(@summaries.size)
          @expander.label = label unless @expander.label == label
          false
        end
      end

      private def schedule_render : Nil
        return unless @render_source == 0

        @render_source = GLib.idle_add do
          @render_source = 0_u32
          render if @expander.expanded?
          false
        end
      end

      private def render : Nil
        label = @expander.child.as?(Gtk::Label)
        unless label
          label = summary_label
          label.margin_top = 4
          label.margin_start = 12
          @expander.child = label
        end
        label.text = self.class.rendered_label(@summaries)
      end

      private def summary_label : Gtk::Label
        label = Gtk::Label.new("")
        label.xalign = 0_f32
        label.ellipsize = :middle
        label.max_width_chars = 100
        label.selectable = true
        label.add_css_class("caption")
        label.add_css_class("dim-label")
        label
      end
    end
  end
end
