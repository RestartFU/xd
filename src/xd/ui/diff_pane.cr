require "json"
require "gtk4"
require "./adw"
require "./background_work"
require "./diff_file_sections"
require "./panel_call"

module Xd
  module UI
    class DiffPane
      private record PendingPrepare,
        output : String,
        token : Int64,
        chat_id : String

      getter widget : Adw::Bin

      @chat_id : String?
      @branch_mode = false
      @base : String?
      @sequence = 0_i64
      @summary : Gtk::Label
      @stack : Gtk::Stack
      @status : Adw::StatusPage
      @sections : DiffFileSections
      @refresh_active = false
      @refresh_pending = false
      @prepare_active = false
      @pending_prepare : PendingPrepare?

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

        @status = Adw::StatusPage.new(
          icon_name: "object-select-symbolic",
          title: "Nothing Changed"
        )
        @stack = Gtk::Stack.new
        @stack.add_named(@status, "empty")
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
        @sequence += 1
        @base = nil
        @pending_prepare = nil
        clear
      end

      def refresh : Nil
        chat_id = @chat_id
        unless chat_id
          show_empty("No working directory")
          return
        end

        if @refresh_active
          @refresh_pending = true
          return
        end
        @refresh_active = true

        token = next_token
        show_empty("Loading changes…") unless @stack.visible_child_name == "changes"
        request_async({
          "op"   => JSON::Any.new("chat"),
          "chat" => JSON::Any.new(chat_id),
        }) do |result|
          unless active?(token, chat_id)
            finish_refresh
            next
          end
          unless state = result.body
            show_call_error(result)
            finish_refresh
            next
          end
          unless state["workdir"]?.try(&.as_s?)
            show_empty("No working directory")
            finish_refresh
            next
          end

          if @branch_mode
            read_base(chat_id, token)
          else
            read_diff(chat_id, token)
          end
        end
      end

      private def read_base(chat_id : String, token : Int64) : Nil
        request_async({
          "op"   => JSON::Any.new("diff-read"),
          "chat" => JSON::Any.new(chat_id),
          "read" => JSON::Any.new("base"),
        }) do |result|
          unless active?(token, chat_id)
            finish_refresh
            next
          end
          unless base = result.body
            show_call_error(result)
            finish_refresh
            next
          end
          @base = base["output"]?.try(&.as_s?).try(&.strip)
          unless @base.presence
            show_empty("No branch to compare against")
            finish_refresh
            next
          end
          read_diff(chat_id, token)
        end
      end

      private def read_diff(chat_id : String, token : Int64) : Nil
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
        request_async(request) do |result|
          unless active?(token, chat_id)
            finish_refresh
            next
          end
          unless response = result.body
            show_call_error(result)
            finish_refresh
            next
          end

          output = response["output"]?.try(&.as_s?) || ""
          if output.empty?
            clear
            show_empty("No changes")
            finish_refresh
            next
          end

          prepare_diff(output, token, chat_id)
          finish_refresh
        end
      end

      private def finish_refresh : Nil
        @refresh_active = false
        return unless @refresh_pending

        @refresh_pending = false
        refresh
      end

      private def prepare_diff(
        output : String,
        token : Int64,
        chat_id : String,
      ) : Nil
        @pending_prepare = PendingPrepare.new(output, token, chat_id)
        start_diff_prepare
      end

      private def start_diff_prepare : Nil
        return if @prepare_active
        work = @pending_prepare || return
        @pending_prepare = nil
        @prepare_active = true
        output = work.output
        token = work.token
        chat_id = work.chat_id
        queued = BackgroundWork.submit do
          prepared : DiffFileSections::Prepared? = nil
          message : String? = nil
          begin
            prepared = DiffFileSections.prepare(output)
          rescue error
            message = error.message || "The diff could not be parsed."
          end
          GLib.idle_add do
            @prepare_active = false
            if active?(token, chat_id)
              if result = prepared
                parsed = @sections.fill(result)
                changed = result.sections.size
                noun = changed == 1 ? "file" : "files"
                @summary.text =
                  "#{changed} #{noun} changed  ·  " \
                  "+#{parsed.additions}  −#{parsed.deletions}"
                @summary.tooltip_text = nil
                @stack.visible_child_name = "changes"
              else
                show_empty(
                  "Could Not Read Changes",
                  message || "The diff could not be parsed."
                )
              end
            end
            start_diff_prepare
            false
          end
          nil
        end
        unless queued
          @prepare_active = false
          @pending_prepare = nil
          show_empty(
            "Still Loading Changes",
            "Too many previews are being prepared. Try again shortly."
          )
        end
      end

      private def show_call_error(result : PanelCallResult) : Nil
        detail = result.error || "The diff could not be read."
        title = self.class.error_title(detail)
        if title == "Not a Git Repository"
          detail += " Inline file edits still appear in the chat."
        end
        show_empty(
          title,
          detail
        )
      end

      def self.error_title(detail : String) : String
        normalized = detail.downcase
        if normalized.includes?("too large")
          "Diff Too Large"
        elsif normalized.includes?("not in a git repository") ||
              normalized.includes?("not a git repository")
          "Not a Git Repository"
        else
          "Could Not Read Changes"
        end
      end

      private def request_async(
        fields : Hash(String, JSON::Any),
        &complete : PanelCallResult -> Nil
      ) : Nil
        spawn do
          result = @request.call(fields)
          GLib.idle_add do
            complete.call(result)
            false
          end
        end
      end

      private def active?(token : Int64, chat_id : String) : Bool
        token == @sequence && @chat_id == chat_id
      end

      private def next_token : Int64
        @sequence += 1
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
        @status.title = summary
        @status.description = tooltip || ""
        @stack.visible_child_name = "empty"
      end
    end
  end
end
