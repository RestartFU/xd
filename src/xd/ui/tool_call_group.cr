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
      getter widget : Gtk::Box

      # The subagent card binds its own toggle to this, so it needs the
      # expander rather than the container.
      getter expander : Gtk::Expander

      @summaries = [] of String
      @single : Gtk::Label

      # Collapsing one call hides its command and shows a count instead, which
      # is less in the same space. Two or more is where it starts paying.
      def self.collapse?(count : Int) : Bool
        count > 1
      end

      def self.collapsed_label(count : Int) : String
        count == 1 ? "1 tool call" : "#{count} tool calls"
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
            render
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
        @expander.label = self.class.collapsed_label(@summaries.size)
        render if @expander.expanded?
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
        @expander.label = self.class.collapsed_label(@summaries.size)
        render if @expander.expanded?
      end

      private def render : Nil
        label = @expander.child.as?(Gtk::Label)
        unless label
          label = summary_label
          label.margin_top = 4
          label.margin_start = 12
          @expander.child = label
        end
        label.text = @summaries.join('\n')
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
