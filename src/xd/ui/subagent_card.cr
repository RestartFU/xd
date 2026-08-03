require "gtk4"
require "../agent/subagent_tool"
require "./tool_call_group"

module Xd
  module UI
    # A delegated run, kept the way a workflow run is kept: one card, made
    # once, updated in place afterwards.
    #
    # A backend that reports an agent again on every state change used to add
    # a card per report. Each one appended a toggle, a header and an activity
    # group to the transcript box -- which is a plain box, so every addition
    # re-measured everything already in it. A fan-out of agents reporting a few
    # times each was enough to make the client crawl. The card now keeps a
    # fixed shape and a repeat only sets two labels.
    class SubagentCard
      getter widget : Gtk::Box

      @toggle : Gtk::ToggleButton?

      def initialize(
        identity : String,
        task : String,
        activity : ToolCallGroup?,
      )
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

        @widget = Gtk::Box.new(:vertical, 6)
        @widget.add_css_class("xd-subagent")

        if activity && (parent = activity.widget.parent.as?(Gtk::Box))
          # The card's toggle becomes the disclosure for the run of calls.
          activity.absorb
          parent.remove(activity.widget)

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
          end
          @toggle = toggle

          @widget.append(toggle)
          adopt(activity)
        else
          @toggle = nil
          @widget.append(@title)
          @widget.append(@detail)
        end
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
      # loose in the transcript behind it.
      def absorb(activity : ToolCallGroup) : Bool
        return false unless @toggle
        parent = activity.widget.parent.as?(Gtk::Box)
        return false unless parent

        activity.absorb
        parent.remove(activity.widget)
        adopt(activity)
        true
      end

      private def adopt(activity : ToolCallGroup) : Nil
        toggle = @toggle || return

        toggle.bind_property(
          "active",
          activity.expander,
          "expanded",
          GObject::BindingFlags::SyncCreate
        )
        toggle.bind_property(
          "active",
          activity.widget,
          "visible",
          GObject::BindingFlags::SyncCreate
        )
        activity.widget.margin_start = 12
        activity.widget.margin_end = 0
        @widget.append(activity.widget)
      end

      private def title_for(identity : String) : String
        "Subagent · #{identity}"
      end
    end
  end
end
