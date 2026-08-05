require "gtk4"

module Xd
  module UI
    # Contiguous tool activity, collapsed only when collapsing hides something.
    #
    # A single call shows its command: "1 tool call" behind an arrow is strictly
    # less information than the command itself, in the same space. A run of them
    # collapses, because that is where the transcript actually gets buried.
    #
    # Activity is collected without touching GTK. The widget is built only when
    # the group is finally committed to the transcript; delegated status rows
    # can therefore hand their activity to a card without creating and removing
    # a widget for every update.
    class ToolCallGroup
      MAX_RENDERED_CALLS = 48
      MAX_RENDERED_CHARS = 12 * 1024

      # Keeps the latest useful activity without allowing a long delegated turn
      # to retain every tool payload for the lifetime of its transcript page.
      class ActivityBuffer
        getter count = 0_i64
        getter summaries = [] of String
        getter characters = 0

        def append(summary : String) : Nil
          @count += 1
          store(summary)
        end

        def merge(other : ActivityBuffer) : Nil
          @count += other.count
          other.summaries.each { |summary| store(summary) }
        end

        private def store(summary : String) : Nil
          retained = if summary.size > MAX_RENDERED_CHARS
                       "#{summary[0, MAX_RENDERED_CHARS - 1]}…"
                     else
                       summary
                     end
          @summaries << retained
          @characters += retained.size

          while @summaries.size > MAX_RENDERED_CALLS ||
                @characters > MAX_RENDERED_CHARS
            removed = @summaries.shift
            @characters -= removed.size
          end
        end
      end

      @activity = ActivityBuffer.new
      @widget : Gtk::Box?
      @expander : Gtk::Expander?
      @single : Gtk::Label?
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

      def self.rendered_label(
        summaries : Array(String),
        total : Int64 = summaries.size.to_i64,
      ) : String
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

        hidden = Math.max(total - shown.size, 0_i64)
        if hidden > 0
          "… #{hidden} earlier tool calls …\n#{shown.join('\n')}"
        else
          shown.join('\n')
        end
      end

      def initialize
      end

      # GTK is deliberately lazy. Most groups are consumed by a subagent card
      # before they need a visible widget, and constructing a box/expander for
      # those short-lived groups was enough to make a busy transcript crawl.
      def widget : Gtk::Box
        @widget ||= build_widget
      end

      def mounted? : Bool
        !!(@widget && @widget.not_nil!.parent)
      end

      def empty? : Bool
        @activity.count == 0
      end

      def append(summary : String) : Nil
        @activity.append(summary)
        refresh
      end

      # Transfers the bounded data without forcing the caller to construct GTK.
      def take_activity : ActivityBuffer
        cancel_pending
        activity = @activity
        @activity = ActivityBuffer.new
        activity
      end

      # A count that changes is a resize, and a resize in the transcript box is
      # measured against everything else in it. Tool calls arrive in bursts, so
      # the count is written once per turn of the main loop rather than once
      # per call: the reader cannot tell the difference and the layout can.
      private def schedule_count : Nil
        return unless @count_source == 0

        @count_source = GLib.idle_add do
          @count_source = 0_u32
          expander = @expander
          if expander
            label = self.class.collapsed_label(@activity.count)
            expander.label = label unless expander.label == label
          end
          false
        end
      end

      private def schedule_render : Nil
        return unless @render_source == 0

        @render_source = GLib.idle_add do
          @render_source = 0_u32
          render if @expander.try(&.expanded?)
          false
        end
      end

      private def refresh : Nil
        single = @single
        expander = @expander
        return unless single && expander

        if !self.class.collapse?(@activity.count)
          single.text = @activity.summaries.last? || ""
          single.visible = true
          expander.visible = false
          return
        end

        single.visible = false
        expander.visible = true
        schedule_count
        schedule_render if expander.expanded?
      end

      private def render : Nil
        expander = @expander || return
        label = expander.child.as?(Gtk::Label)
        unless label
          label = self.class.summary_label
          label.margin_top = 4
          label.margin_start = 12
          expander.child = label
        end
        label.text = self.class.rendered_label(
          @activity.summaries,
          @activity.count
        )
      end

      private def build_widget : Gtk::Box
        box = Gtk::Box.new(:vertical, 0)
        box.margin_top = 4
        box.margin_bottom = 4
        box.margin_start = 24
        box.margin_end = 24

        single = self.class.summary_label
        single.visible = false
        box.append(single)
        @single = single

        expander = Gtk::Expander.new(nil)
        expander.expanded = false
        expander.add_css_class("dim-label")
        expander.visible = false
        expander.notify_signal["expanded"].connect do |_property|
          if expander.expanded?
            schedule_render
          else
            expander.child = nil
          end
        end
        box.append(expander)
        @expander = expander
        @widget = box
        refresh
        box
      end

      private def cancel_pending : Nil
        unless @count_source == 0
          GLib.source_remove(@count_source)
          @count_source = 0_u32
        end
        unless @render_source == 0
          GLib.source_remove(@render_source)
          @render_source = 0_u32
        end
      end

      def self.summary_label : Gtk::Label
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
