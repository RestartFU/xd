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

      ACCESS = [
        Agent::Access::ReadOnly,
        Agent::Access::Edit,
        Agent::Access::Full,
      ]

      @backend = "claude"
      @model : String?
      @effort = Agent::Effort::High
      @efforts : Array(Agent::Effort)
      @access = Agent::Access::ReadOnly
      @updating = false
      @model_picker : ModelPicker
      @effort_picker : OptionPicker
      @fast_button : Gtk::ToggleButton
      @claude_mode_button : Gtk::ToggleButton
      @access_picker : OptionPicker
      @build_button : Gtk::ToggleButton
      @plan_button : Gtk::ToggleButton
      @workspace_picker : OptionPicker
      @remove_worktree_button : Gtk::Button
      @workspace_values = [] of Tuple(String, String?)
      @selected_worktree : String?
      @context_meter : Gtk::ProgressBar

      def initialize(
        @on_option : Proc(String, String?, Nil),
        @on_model : Proc(String, String, Nil),
        @on_remove_worktree : Proc(String, Nil),
      )
        @model = nil
        @selected_worktree = nil
        @efforts = Agent::Catalog::CLAUDE.efforts
        @widget = Gtk::Box.new(:vertical, 2)
        @widget.add_css_class("xd-controls")
        @identity = Gtk::Box.new(:horizontal, 8)
        @run = Gtk::Box.new(:horizontal, 8)

        @model_picker = ModelPicker.new(@on_model)
        @model_picker.widget.tooltip_text =
          "Which assistant and model answer in this chat"

        @effort_picker = OptionPicker.new(
          effort_options(@efforts),
          ->(index : Int32) {
            @on_option.call("effort", @efforts[index].wire_name)
          }
        )
        @effort_picker.widget.tooltip_text =
          "How hard the model is asked to think"

        @fast_button = Gtk::ToggleButton.new_with_label("Fast")
        @fast_button.tooltip_text =
          "Use Codex priority service. May consume usage credits faster."
        @fast_button.visible = false
        @fast_button.toggled_signal.connect do
          unless @updating
            @on_option.call(
              "fast",
              @fast_button.active? ? "true" : "false"
            )
          end
        end

        @claude_mode_button = Gtk::ToggleButton.new_with_label("Claude mode")
        @claude_mode_button.tooltip_text =
          "Run the selected Codex model through Claude Code's agent harness."
        @claude_mode_button.visible = false
        @claude_mode_button.toggled_signal.connect do
          unless @updating
            @on_option.call(
              "claude-mode",
              @claude_mode_button.active? ? "true" : "false"
            )
          end
        end

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

        @remove_worktree_button = Gtk::Button.new_from_icon_name(
          "user-trash-symbolic"
        )
        @remove_worktree_button.add_css_class("flat")
        @remove_worktree_button.add_css_class("destructive-action")
        @remove_worktree_button.tooltip_text = "Remove selected worktree"
        @remove_worktree_button.visible = false
        @remove_worktree_button.clicked_signal.connect do
          if worktree = @selected_worktree
            @on_remove_worktree.call(worktree)
          end
        end

        @context_meter = Gtk::ProgressBar.new
        @context_meter.show_text = true
        @context_meter.set_size_request(108, -1)
        @context_meter.valign = :center
        @context_meter.visible = false
        @context_meter.add_css_class("xd-context-meter")

        @identity.append(@workspace_picker.widget)
        @identity.append(@remove_worktree_button)
        @identity.append(@model_picker.widget)
        @identity.append(@context_meter)
        @run.append(@effort_picker.widget)
        @run.append(@fast_button)
        @run.append(@claude_mode_button)
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
        unless enabled
          @selected_worktree = nil
          @remove_worktree_button.visible = false
        end
        @model_picker.widget.sensitive = enabled
        @effort_picker.widget.sensitive = enabled
        @fast_button.sensitive = enabled && @backend == "codex"
        @claude_mode_button.sensitive = enabled && @backend == "codex"
        @access_picker.widget.sensitive =
          enabled && !@plan_button.active?
        @build_button.sensitive = enabled
        @plan_button.sensitive = enabled
        @workspace_picker.widget.sensitive = enabled
        @remove_worktree_button.sensitive = enabled && !!@selected_worktree
        enabled
      end

      def update(state : Hash(String, JSON::Any)) : Nil
        @updating = true
        @backend = state["backend"]?.try(&.as_s?) || "claude"
        backend = Agent::Catalog.lookup(@backend) || Agent::Catalog::CLAUDE
        claude_mode = backend.id == "codex" &&
                      (state["claude_mode"]?.try(&.as_bool?) || false)
        efforts = backend.efforts.reject do |effort|
          claude_mode && effort.ultra?
        end
        if @efforts != efforts
          @efforts = efforts
          @effort_picker.options = effort_options(@efforts)
        end
        @model = state["model"]?.try(&.as_s?) || backend.default_model
        selected_effort = Agent::Effort.from_wire(
          state["effort"]?.try(&.as_s?)
        )
        @effort = backend.supports_effort?(selected_effort) ? selected_effort : Agent::Effort::High
        @access = Agent::Access.from_wire(
          state["access"]?.try(&.as_s?)
        )

        @model_picker.select(backend.id, @model)
        @effort_picker.selected = @efforts.index(@effort) || 0
        @fast_button.visible = backend.id == "codex"
        @fast_button.active =
          backend.id == "codex" &&
            (state["fast"]?.try(&.as_bool?) || false)
        @claude_mode_button.visible = backend.id == "codex"
        @claude_mode_button.active = claude_mode
        @access_picker.selected = ACCESS.index(@access) || 0
        plan = state["plan"]?.try(&.as_bool?) || false
        (plan ? @plan_button : @build_button).active = true
        @access_picker.widget.sensitive = !plan
        update_context_meter(state)

        new_worktree = state["new_worktree"]?.try(&.as_bool?) || false
        @selected_worktree = state["selected_worktree"]?.try(&.as_s?)
        @remove_worktree_button.visible = !!@selected_worktree
        build_workspace_menu(state)
        @workspace_picker.selected = new_worktree ? 1 : 0
        has_messages = state["has_messages"]?.try(&.as_bool?) || false
        self.sensitive = true
        @workspace_picker.widget.sensitive = !has_messages
        @remove_worktree_button.sensitive = !has_messages && !!@selected_worktree
      ensure
        @updating = false
      end

      private def effort_options(
        efforts : Array(Agent::Effort),
      ) : Array(OptionPicker::Option)
        efforts.map do |effort|
          OptionPicker::Option.new(
            effort.label,
            case effort
            when Agent::Effort::Low
              "Quick answers, little deliberation."
            when Agent::Effort::Medium
              "Balanced speed and depth."
            when Agent::Effort::High
              "Thinks longer before answering."
            when Agent::Effort::XHigh
              "Extended reasoning for hard problems."
            when Agent::Effort::Max
              "Very deep reasoning for difficult problems."
            when Agent::Effort::Ultra
              "Longest available reasoning for the hardest problems."
            when Agent::Effort::UltraCode
              "Maximum Claude Code effort for the hardest problems."
            else
              "How hard the model is asked to think."
            end
          )
        end
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
