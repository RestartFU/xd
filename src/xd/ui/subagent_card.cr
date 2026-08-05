require "gtk4"
require "../agent/subagent_tool"
require "./tool_call_group"

module Xd
  module UI
    # A delegated run, kept the way a workflow run is kept: one card, made
    # once, updated in place afterwards.
    #
    # The card deliberately uses one toggle and one lazily-created label for
    # activity. A Gtk::Expander plus property bindings for every report made
    # repeated Codex status updates expensive even after the card itself was
    # keyed. Activity remains bounded data until the user opens the card.
    class SubagentCard
      getter widget : Gtk::Box

      @toggle : Gtk::ToggleButton
      @activity = ToolCallGroup::ActivityBuffer.new
      @activity_label : Gtk::Label?
      @activity_render_source = 0_u32

      def initialize(
        identity : String,
        task : String,
        activity : ToolCallGroup?,
      )
        @activity_label = nil
        @title = Gtk::Label.new(title_for(identity))
        @title.xalign = 0_f32
        @title.add_css_class("title")

        @detail = Gtk::Label.new(task)
        @detail.xalign = 0_f32
        # Single-line headers: a wrapped one is measured height-for-width, and
        # a run of cards would pay for that measurement on every layout pass.
        # The whole task stays reachable as the tooltip.
        @detail.ellipsize = :end
        @detail.max_width_chars = 100
        @detail.tooltip_text = task
        @detail.add_css_class("xd-body")

        @widget = Gtk::Box.new(:vertical, 0)
        @widget.add_css_class("xd-subagent")

        indicator = Gtk::Image.new_from_icon_name("pan-end-symbolic")
        indicator.valign = :start
        indicator.margin_top = 3

        body = Gtk::Box.new(:vertical, 6)
        body.append(@title)
        body.append(@detail)

        header = Gtk::Box.new(:horizontal, 8)
        header.hexpand = true
        header.margin_top = 12
        header.margin_bottom = 12
        header.margin_start = 14
        header.margin_end = 14
        header.append(indicator)
        header.append(body)

        toggle = Gtk::ToggleButton.new
        toggle.child = header
        toggle.hexpand = true
        toggle.tooltip_text = "Show subagent activity"
        toggle.add_css_class("xd-subagent-toggle")
        toggle.toggled_signal.connect do
          expanded = toggle.active?
          indicator.icon_name =
            expanded ? "pan-down-symbolic" : "pan-end-symbolic"
          toggle.tooltip_text =
            expanded ? "Hide subagent activity" : "Show subagent activity"
          refresh_activity
        end
        @toggle = toggle
        @widget.append(toggle)

        absorb(activity) if activity
      end

      # The same agent, further along. Only the two labels move: the card keeps
      # its place, its size and its activity.
      def update(identity : String, task : String) : Nil
        heading = title_for(identity)
        @title.text = heading unless @title.text == heading
        return if @detail.text == task

        @detail.text = task
        @detail.tooltip_text = task
      end

      # Calls made between two reports of the same agent belong under it, not
      # loose in the transcript behind it. The group transfers only its data;
      # no transient GTK activity widget is created for the hand-off.
      def absorb(activity : ToolCallGroup) : Bool
        if activity.mounted?
          if parent = activity.widget.parent.as?(Gtk::Box)
            parent.remove(activity.widget)
          end
        end
        @activity.merge(activity.take_activity)
        refresh_activity
        true
      end

      def close : Nil
        unless @activity_render_source == 0
          GLib.source_remove(@activity_render_source)
          @activity_render_source = 0_u32
        end
      end

      private def refresh_activity : Nil
        count = @activity.count
        label = @activity_label
        if count == 0
          label.try { |current| current.visible = false }
          return
        end
        unless @toggle.active?
          label.try { |current| current.visible = false }
          return
        end

        unless label
          label = ToolCallGroup.summary_label
          label.margin_start = 26
          label.margin_end = 12
          label.margin_bottom = 8
          @activity_label = label
          @widget.append(label)
        end

        label.visible = true
        schedule_activity_render
      end

      private def schedule_activity_render : Nil
        return unless @activity_render_source == 0

        @activity_render_source = GLib.idle_add do
          @activity_render_source = 0_u32
          if @toggle.active?
            render_activity
          end
          false
        end
      end

      private def render_activity : Nil
        label = @activity_label || return
        label.text = ToolCallGroup.rendered_label(
          @activity.summaries,
          @activity.count
        )
      end

      private def title_for(identity : String) : String
        "Subagent · #{identity}"
      end
    end
  end
end
