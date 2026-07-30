require "json"
require "gtk4"
require "./adw"
require "./diff_file_sections"
require "./panel_call"

module Xd
  module UI
    class DiffPane
      getter widget : Adw::Bin

      @chat_id : String?
      @branch_mode = false
      @base : String?
      @summary : Gtk::Label
      @stack : Gtk::Stack
      @sections : DiffFileSections

      def initialize(@request : PanelCall)
        @chat_id = nil
        @base = nil

        @summary = Gtk::Label.new("No changes")
        @summary.xalign = 0_f32
        @summary.hexpand = true
        @summary.add_css_class("heading")

        working = Gtk::ToggleButton.new_with_label("Working")
        working.tooltip_text = "Changes not yet committed"
        working.active = true
        branch = Gtk::ToggleButton.new_with_label("Branch")
        branch.tooltip_text = "Everything this branch changes"
        branch.group = working
        branch.toggled_signal.connect do
          @branch_mode = branch.active?
          refresh
        end

        modes = Gtk::Box.new(:horizontal, 0)
        modes.add_css_class("linked")
        modes.append(working)
        modes.append(branch)

        refresh = Gtk::Button.new_from_icon_name(
          "view-refresh-symbolic"
        )
        refresh.add_css_class("flat")
        refresh.tooltip_text = "Read again"
        refresh.clicked_signal.connect { refresh }

        header = Gtk::Box.new(:horizontal, 8)
        header.margin_start = 12
        header.margin_end = 6
        header.margin_top = 6
        header.margin_bottom = 6
        header.append(@summary)
        header.append(modes)
        header.append(refresh)

        @sections = DiffFileSections.new
        changes = Gtk::ScrolledWindow.new
        changes.set_policy(:external, :external)
        changes.vexpand = true
        changes.child = @sections.widget

        empty = Adw::StatusPage.new(
          icon_name: "object-select-symbolic",
          title: "Nothing Changed"
        )
        @stack = Gtk::Stack.new
        @stack.add_named(empty, "empty")
        @stack.add_named(changes, "changes")
        @stack.vexpand = true

        box = Gtk::Box.new(:vertical, 0)
        box.append(header)
        box.append(Gtk::Separator.new(:horizontal))
        box.append(@stack)
        @widget = Adw::Bin.new(child: box)
      end

      def select_chat(chat_id : String?) : Nil
        return if @chat_id == chat_id

        @chat_id = chat_id
        @base = nil
        clear
      end

      def refresh : Nil
        chat_id = @chat_id
        unless chat_id
          show_empty("No working directory")
          return
        end

        state = call({
          "op"   => JSON::Any.new("chat"),
          "chat" => JSON::Any.new(chat_id),
        })
        return unless state
        unless state["workdir"]?.try(&.as_s?)
          show_empty("No working directory")
          return
        end

        if @branch_mode
          base = call({
            "op"   => JSON::Any.new("diff-read"),
            "chat" => JSON::Any.new(chat_id),
            "read" => JSON::Any.new("base"),
          })
          return unless base

          @base = base["output"]?.try(&.as_s?).try(&.strip)
          unless @base.presence
            show_empty("No branch to compare against")
            return
          end
        end
        read_diff(chat_id)
      end

      private def read_diff(chat_id : String) : Nil
        request = {
          "op"   => JSON::Any.new("diff-read"),
          "chat" => JSON::Any.new(chat_id),
          "read" => JSON::Any.new(
            @branch_mode ? "branch-all" : "working-all"
          ),
        }
        if @branch_mode
          request["base"] = JSON::Any.new(@base.not_nil!)
        end
        response = call(request)
        return unless response

        output = response["output"]?.try(&.as_s?) || ""
        if output.empty?
          clear
          show_empty("No changes")
          return
        end

        parsed = @sections.fill(output)
        changed = @sections.sections.size
        noun = changed == 1 ? "file" : "files"
        @summary.text =
          "#{changed} #{noun} changed  ·  " \
          "+#{parsed.additions}  −#{parsed.deletions}"
        @summary.tooltip_text = nil
        @stack.visible_child_name = "changes"
      end

      private def call(
        request : Hash(String, JSON::Any),
      ) : Hash(String, JSON::Any)?
        result = @request.call(request)
        return result.body if result.body

        show_empty(
          "Could not read changes",
          result.error
        )
        nil
      end

      private def clear : Nil
        @sections.fill("")
      end

      private def show_empty(
        summary : String,
        tooltip : String? = nil,
      ) : Nil
        @summary.text = summary
        @summary.tooltip_text = tooltip
        @stack.visible_child_name = "empty"
      end
    end
  end
end
