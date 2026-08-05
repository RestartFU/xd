require "gtk4"
require "../agent/subagent_tool"
require "./run_card"
require "./tool_call_group"

module Xd
  module UI
    # A delegated run uses the same status card as a GitHub Actions run. The
    # task is the summary, agent state uses the shared spinner/status treatment,
    # and bounded tool activity is mounted lazily in the shared card body.
    class SubagentCard
      record Presentation,
        detail : String,
        status : String,
        spinning : Bool,
        css_class : String

      getter widget : Gtk::Box

      @toggle : Gtk::ToggleButton
      @activity = ToolCallGroup::ActivityBuffer.new
      @activity_label : Gtk::Label?
      @activity_render_source = 0_u32

      def self.presentation(task : String) : Presentation
        parts = task.split(" · ", 2)
        state = parts.first
        recognized = {
          "Starting", "Running", "Started", "Completed", "Failed",
          "Spawn failed", "Interrupted", "Stopped", "Not found", "Delegated",
        }.includes?(state)
        detail = if recognized
                   parts[1]? || "Delegated task"
                 else
                   task
                 end
        status = recognized ? state : "Delegated"
        spinning = {"Starting", "Running", "Started"}.includes?(status)
        css_class = case status
                    when "Completed"
                      "xd-workflow-success"
                    when "Failed", "Spawn failed"
                      "xd-workflow-failure"
                    when "Starting", "Running", "Started"
                      "xd-workflow-running"
                    else
                      "xd-workflow-finished"
                    end
        Presentation.new(detail, status, spinning, css_class)
      end

      def initialize(
        identity : String,
        task : String,
        activity : ToolCallGroup?,
      )
        @activity_label = nil

        @identity = Gtk::Label.new(identity)
        @identity.xalign = 0_f32
        @identity.hexpand = true
        @identity.ellipsize = :end
        @identity.tooltip_text = identity
        @identity.add_css_class("title")

        @card = RunCard.new(
          "Subagent ·",
          heading_suffix: @identity
        )
        @widget = @card.widget
        @detail = @card.name
        @detail.ellipsize = :end
        @detail.max_width_chars = 100
        @card.items.visible = false

        indicator = Gtk::Image.new_from_icon_name("pan-end-symbolic")
        toggle = Gtk::ToggleButton.new
        toggle.child = indicator
        toggle.tooltip_text = "Show subagent activity"
        toggle.add_css_class("flat")
        toggle.add_css_class("xd-run-card-toggle")
        toggle.toggled_signal.connect do
          expanded = toggle.active?
          indicator.icon_name =
            expanded ? "pan-down-symbolic" : "pan-end-symbolic"
          toggle.tooltip_text =
            expanded ? "Hide subagent activity" : "Show subagent activity"
          refresh_activity
        end
        @toggle = toggle
        @card.summary.prepend(toggle)

        update(identity, task)
        absorb(activity) if activity
        refresh_activity
      end

      # The same agent, further along. The shared card stays mounted while its
      # heading, task, spinner, and status are updated in place.
      def update(identity : String, task : String) : Nil
        if @identity.text != identity
          @identity.text = identity
          @identity.tooltip_text = identity
        end

        presentation = self.class.presentation(task)
        if @detail.text != presentation.detail
          @detail.text = presentation.detail
          @detail.tooltip_text = presentation.detail
        end
        @card.spinner.visible = presentation.spinning
        @card.spinner.spinning = presentation.spinning
        @card.status.text = presentation.status
        @card.status.visible = true
        @card.apply_status_class(presentation.css_class)
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
        @toggle.sensitive = count > 0
        unless count > 0 && @toggle.active?
          @card.items.visible = false
          return
        end

        label = @activity_label
        unless label
          label = ToolCallGroup.summary_label
          label.add_css_class("xd-workflow-log")
          @activity_label = label
          @card.items.append(label)
        end

        @card.items.visible = true
        schedule_activity_render
      end

      private def schedule_activity_render : Nil
        return unless @activity_render_source == 0

        @activity_render_source = GLib.idle_add do
          @activity_render_source = 0_u32
          render_activity if @toggle.active?
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
    end
  end
end
