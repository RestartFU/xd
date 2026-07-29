require "gtk4"
require "../agent/catalog"

module Xd
  module UI
    class ChatControls
      getter widget : Gtk::Box

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
      @backend_button : Gtk::Button
      @model_button : Gtk::Button
      @effort_button : Gtk::Button
      @access_button : Gtk::Button
      @plan_button : Gtk::ToggleButton
      @workspace_button : Gtk::MenuButton

      def initialize(
        @on_option : Proc(String, String?, Nil),
      )
        @model = nil
        @widget = Gtk::Box.new(:horizontal, 6)
        @widget.add_css_class("xd-controls")

        @backend_button = control_button("Claude Code")
        @backend_button.tooltip_text = "Agent backend"
        @backend_button.clicked_signal.connect { cycle_backend }

        @model_button = control_button("Model")
        @model_button.tooltip_text = "Model"
        @model_button.clicked_signal.connect { cycle_model }

        @effort_button = control_button("High")
        @effort_button.tooltip_text = "Reasoning effort"
        @effort_button.clicked_signal.connect { cycle_effort }

        @access_button = control_button("Read only")
        @access_button.tooltip_text = "Filesystem access"
        @access_button.clicked_signal.connect { cycle_access }

        @plan_button = Gtk::ToggleButton.new_with_label("Plan")
        @plan_button.add_css_class("flat")
        @plan_button.tooltip_text = "Plan without editing"
        @plan_button.toggled_signal.connect do
          unless @updating
            @on_option.call(
              "plan",
              @plan_button.active? ? "true" : "false"
            )
          end
        end

        @workspace_button = Gtk::MenuButton.new
        @workspace_button.label = "Workspace"
        @workspace_button.add_css_class("flat")
        @workspace_button.tooltip_text = "Working directory"

        @widget.append(@backend_button)
        @widget.append(@model_button)
        @widget.append(@effort_button)
        @widget.append(@access_button)
        @widget.append(@plan_button)
        @widget.append(@workspace_button)
        self.sensitive = false
      end

      def sensitive=(enabled : Bool) : Bool
        @backend_button.sensitive = enabled
        @model_button.sensitive = enabled
        @effort_button.sensitive = enabled
        @access_button.sensitive = enabled
        @plan_button.sensitive = enabled
        @workspace_button.sensitive = enabled
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

        @backend_button.label = backend.display_name
        @model_button.label = backend.model_label(@model)
        @effort_button.label = @effort.label
        @access_button.label = @access.label
        @plan_button.active = state["plan"]?.try(&.as_bool?) || false

        workdir = state["workdir"]?.try(&.as_s?)
        new_worktree = state["new_worktree"]?.try(&.as_bool?) || false
        linked = state["linked_worktree"]?.try(&.as_bool?) || false
        @workspace_button.label = if new_worktree
                                    "New worktree"
                                  elsif workdir
                                    linked ?
                                      "#{File.basename(workdir)} · worktree" :
                                      File.basename(workdir)
                                  else
                                    "Workspace"
                                  end
        build_workspace_menu(state)
        has_messages = state["has_messages"]?.try(&.as_bool?) || false
        self.sensitive = true
        @workspace_button.sensitive = !has_messages
      ensure
        @updating = false
      end

      private def control_button(label : String) : Gtk::Button
        button = Gtk::Button.new_with_label(label)
        button.add_css_class("flat")
        button
      end

      private def cycle_backend : Nil
        backends = Agent::Catalog.all
        index = backends.index { |item| item.id == @backend } || 0
        selected = backends[(index + 1) % backends.size]
        @on_option.call("backend", selected.id)
      end

      private def cycle_model : Nil
        backend = Agent::Catalog.lookup(@backend) || Agent::Catalog::CLAUDE
        index = backend.models.index { |item| item.id == @model } || -1
        selected = backend.models[(index + 1) % backend.models.size]
        @on_option.call("model", selected.id)
      end

      private def cycle_effort : Nil
        index = EFFORTS.index(@effort) || 0
        selected = EFFORTS[(index + 1) % EFFORTS.size]
        @on_option.call("effort", selected.wire_name)
      end

      private def cycle_access : Nil
        index = ACCESS.index(@access) || 0
        selected = ACCESS[(index + 1) % ACCESS.size]
        @on_option.call("access", selected.wire_name)
      end

      private def build_workspace_menu(
        state : Hash(String, JSON::Any),
      ) : Nil
        popover = Gtk::Popover.new
        choices = Gtk::Box.new(:vertical, 2)
        choices.margin_top = 6
        choices.margin_bottom = 6
        choices.margin_start = 6
        choices.margin_end = 6

        add_workspace_choice(
          choices,
          popover,
          "New worktree",
          "new-worktree",
          "true"
        )
        state["worktrees"]?.try(&.as_a?).try do |worktrees|
          worktrees.each do |node|
            path = node["path"].as_s
            branch = node["branch"]?.try(&.as_s?)
            label = branch || File.basename(path)
            label += " · main" if node["main"]?.try(&.as_bool?) == true
            label += " · current" if node["current"]?.try(&.as_bool?) == true
            add_workspace_choice(
              choices,
              popover,
              label,
              "workspace",
              path
            )
          end
        end
        popover.child = choices
        @workspace_button.popover = popover
      end

      private def add_workspace_choice(
        choices : Gtk::Box,
        popover : Gtk::Popover,
        label : String,
        option : String,
        value : String,
      ) : Nil
        button = Gtk::Button.new_with_label(label)
        button.add_css_class("flat")
        button.halign = :fill
        button.clicked_signal.connect do
          popover.popdown
          @on_option.call(option, value)
        end
        choices.append(button)
      end
    end
  end
end
