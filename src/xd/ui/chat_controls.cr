require "gtk4"
require "../agent/catalog"
require "./adw"
require "./context_usage"
require "./model_picker"
require "./option_picker"

module Xd
  module UI
    class ChatControls
      getter widget : Gtk::Box
      getter identity : Gtk::Box
      getter run : Gtk::Box

      EFFORTS = [
        Agent::Effort::Low,
        Agent::Effort::Medium,
        Agent::Effort::High,
        Agent::Effort::XHigh,
        Agent::Effort::Max,
      ]
      ACCESS = [
        Agent::Access::ReadOnly,
        Agent::Access::Edit,
        Agent::Access::Full,
      ]

      @backend = "claude"
      @model : String?
      @effort = Agent::Effort::High
      @access = Agent::Access::ReadOnly
      @updating = false
      @model_picker : ModelPicker
      @effort_picker : OptionPicker
      @access_picker : OptionPicker
      @build_button : Gtk::ToggleButton
      @plan_button : Gtk::ToggleButton
      @workspace_picker : OptionPicker
      @workspace_values = [] of Tuple(String, String?)
      @context_meter : Gtk::ProgressBar

      def initialize(
        @on_option : Proc(String, String?, Nil),
        @on_model : Proc(String, String, Nil),
      )
        @model = nil
        @widget = Gtk::Box.new(:vertical, 2)
        @widget.add_css_class("xd-controls")
        @identity = Gtk::Box.new(:horizontal, 8)
        @run = Gtk::Box.new(:horizontal, 8)

        @model_picker = ModelPicker.new(@on_model)
        @model_picker.widget.tooltip_text =
          "Which assistant and model answer in this chat"

        @effort_picker = OptionPicker.new(
          EFFORTS.map_with_index do |effort, index|
            OptionPicker::Option.new(
              effort.label,
              [
                "Quick answers, little deliberation.",
                "Balanced speed and depth.",
                "Thinks longer before answering.",
                "Extended reasoning for hard problems.",
                "Everything the model has.",
              ][index]
            )
          end,
          ->(index : Int32) {
            @on_option.call("effort", EFFORTS[index].wire_name)
          }
        )
        @effort_picker.widget.tooltip_text =
          "How hard the model is asked to think"

        @access_picker = OptionPicker.new(
          ACCESS.map_with_index do |access, index|
            OptionPicker::Option.new(
              access.label,
              [
                "Look at anything, change nothing.",
                "Edit the working tree; ask before commands.",
                "Run commands and edit without asking.",
              ][index]
            )
          end,
          ->(index : Int32) {
            @on_option.call("access", ACCESS[index].wire_name)
          }
        )
        @access_picker.widget.tooltip_text =
          "What the assistant may do in the working directory"

        @build_button = Gtk::ToggleButton.new
        @build_button.child = Adw::ButtonContent.new(
          icon_name: "package-x-generic-symbolic",
          label: "Build"
        )
        @build_button.tooltip_text = "Carry the work out"

        @plan_button = Gtk::ToggleButton.new
        @plan_button.child = Adw::ButtonContent.new(
          icon_name: "view-list-bullet-symbolic",
          label: "Plan"
        )
        @plan_button.tooltip_text =
          "Work out an approach without changing anything"
        @plan_button.group = @build_button
        @build_button.active = true
        @plan_button.toggled_signal.connect do
          unless @updating
            @access_picker.widget.sensitive = !@plan_button.active?
            @on_option.call(
              "plan",
              @plan_button.active? ? "true" : "false"
            )
          end
        end

        @workspace_picker = OptionPicker.new(
          [
            OptionPicker::Option.new(
              "Current checkout",
              "Use the checkout this chat currently points at."
            ),
            OptionPicker::Option.new(
              "New worktree",
              "Create an isolated branch and checkout for this chat."
            ),
          ],
          ->(index : Int32) { select_workspace(index) }
        )
        @workspace_picker.widget.tooltip_text =
          "Where this chat works; locked after the first message"

        @context_meter = Gtk::ProgressBar.new
        @context_meter.show_text = true
        @context_meter.set_size_request(108, -1)
        @context_meter.valign = :center
        @context_meter.visible = false
        @context_meter.add_css_class("xd-context-meter")

        @identity.append(@workspace_picker.widget)
        @identity.append(@model_picker.widget)
        @identity.append(@context_meter)
        @run.append(@effort_picker.widget)
        @run.append(@access_picker.widget)
        modes = Gtk::Box.new(:horizontal, 0)
        modes.add_css_class("linked")
        modes.append(@build_button)
        modes.append(@plan_button)
        @run.append(modes)
        @widget.append(@identity)
        @widget.append(@run)
        self.sensitive = false
      end

      def sensitive=(enabled : Bool) : Bool
        @model_picker.widget.sensitive = enabled
        @effort_picker.widget.sensitive = enabled
        @access_picker.widget.sensitive =
          enabled && !@plan_button.active?
        @build_button.sensitive = enabled
        @plan_button.sensitive = enabled
        @workspace_picker.widget.sensitive = enabled
        enabled
      end

      def update(state : Hash(String, JSON::Any)) : Nil
        @updating = true
        @backend = state["backend"]?.try(&.as_s?) || "claude"
        backend = Agent::Catalog.lookup(@backend) || Agent::Catalog::CLAUDE
        @model = state["model"]?.try(&.as_s?) || backend.default_model
        @effort = Agent::Effort.from_wire(
          state["effort"]?.try(&.as_s?)
        )
        @access = Agent::Access.from_wire(
          state["access"]?.try(&.as_s?)
        )

        @model_picker.select(backend.id, @model)
        @effort_picker.selected = EFFORTS.index(@effort) || 0
        @access_picker.selected = ACCESS.index(@access) || 0
        plan = state["plan"]?.try(&.as_bool?) || false
        (plan ? @plan_button : @build_button).active = true
        @access_picker.widget.sensitive = !plan
        update_context_meter(state)

        new_worktree = state["new_worktree"]?.try(&.as_bool?) || false
        build_workspace_menu(state)
        @workspace_picker.selected = new_worktree ? 1 : 0
        has_messages = state["has_messages"]?.try(&.as_bool?) || false
        self.sensitive = true
        @workspace_picker.widget.sensitive = !has_messages
      ensure
        @updating = false
      end

      private def update_context_meter(
        state : Hash(String, JSON::Any),
      ) : Nil
        used = state["context_used"]?.try(&.as_i64?) || 0_i64
        window = state["context_window"]?.try(&.as_i64?) || 0_i64
        meter = if used > 0 && window > 0
                  ContextUsage.meter(used.to_u64, window.to_u64)
                end

        @context_meter.remove_css_class("warning")
        @context_meter.remove_css_class("error")
        unless meter
          @context_meter.visible = false
          return
        end

        @context_meter.fraction = meter.fraction
        @context_meter.text = meter.label
        @context_meter.tooltip_text = meter.tooltip
        case meter.severity
        when ContextUsage::Severity::Warning
          @context_meter.add_css_class("warning")
        when ContextUsage::Severity::Error
          @context_meter.add_css_class("error")
        else
        end
        @context_meter.visible = true
      end

      private def build_workspace_menu(
        state : Hash(String, JSON::Any),
      ) : Nil
        workdir = state["workdir"]?.try(&.as_s?)
        linked = state["linked_worktree"]?.try(&.as_bool?) || false
        options = [
          OptionPicker::Option.new(
            linked ? "Current worktree" : "Current checkout",
            "Keep using this chat's current checkout."
          ),
          OptionPicker::Option.new(
            "New worktree",
            "Create an isolated branch and checkout for this chat."
          ),
        ]
        @workspace_values = [
          {"new-worktree", "false"},
          {"new-worktree", "true"},
        ] of Tuple(String, String?)
        state["worktrees"]?.try(&.as_a?).try do |worktrees|
          worktrees.each do |node|
            path = node["path"].as_s
            next if node["current"]?.try(&.as_bool?) == true
            next if workdir == path

            branch = node["branch"]?.try(&.as_s?)
            detached = node["detached"]?.try(&.as_bool?) || false
            label = if branch
                      detached ? "Detached at #{branch}" : branch
                    else
                      File.basename(path)
                    end
            options << OptionPicker::Option.new(label, path)
            @workspace_values << {"workspace", path}
          end
        end
        @workspace_picker.options = options
      end

      private def select_workspace(index : Int32) : Nil
        return if @updating
        return unless selected = @workspace_values[index]?

        @on_option.call(
          selected[0],
          selected[1]
        )
      end
    end
  end
end
