require "base64"
require "gtk4"
require "set"
require "../agent/ask"
require "../agent/git_diff_tracker"
require "../agent/image_reference"
require "../agent/subagent_tool"
require "../agent/workflow_run"
require "../agent/workspace_block"
require "../daemon/endpoint"
require "../integrations/discord_presence"
require "../remote/connection"
require "../version"
require "./adw"
require "./auth_dialog"
require "./background_work"
require "./chat_controls"
require "./command_suggestions"
require "./dots"
require "./event_inbox"
require "./git_actions"
require "./image_attachment"
require "./image_presenter"
require "./message_row"
require "./pair_dialog"
require "./pane_state"
require "./queue_presentation"
require "./search_dialog"
require "./sidebar"
require "./text_reveal"
require "./tool_call_group"
require "./tool_panel"
require "./transcript_paging"
require "./turn_recovery"
require "./turn_timing"
require "./voice_input"
require "./workflow_card"

module Xd
  module UI
    class Window
      class TranscriptPage
        getter key : String
        getter chat_id : String
        getter endpoint : Daemon::Endpoint
        property transcript : Gtk::Box
        getter paging = TranscriptPaging.new
        getter workflow_ids = Set(String).new
        getter workflow_cards = [] of WorkflowCard
        property revision = -1_i64
        property choices_visible = false
        property tool_group : ToolCallGroup?

        def initialize(
          @key : String,
          @chat_id : String,
          @endpoint : Daemon::Endpoint,
          @transcript : Gtk::Box,
        )
          @tool_group = nil
        end

        def clear_workflows : Nil
          @workflow_cards.each(&.close)
          @workflow_cards.clear
          @workflow_ids.clear
        end
      end

      class Attachment
        getter name : String
        getter data : String
        getter bytesize : Int32
        getter texture : Gdk::Texture

        def initialize(
          @name : String,
          @data : String,
          @bytesize : Int32,
          @texture : Gdk::Texture,
        )
        end
      end

      record PreparedAttachment,
        name : String,
        data : String,
        bytesize : Int32,
        preview : ImageAttachment::Pixels

      MAX_IMAGES                = 4
      MAX_IMAGE_BYTES           = 10 * 1024 * 1024
      MAX_TOTAL_BYTES           = 20 * 1024 * 1024
      QUEUE_RENDER_BATCH        =   8
      QUEUE_RETIRE_BATCH        =   8
      TURN_RECOVERY_EVENT_BATCH =  32
      TURN_RECOVERY_EVENT_LIMIT = 512
      RENDER_SAFE_MODE          = ENV["XD_RENDER_SAFE_MODE"]? == "1"

      getter widget : Adw::ApplicationWindow

      @active_chat : String?
      @chat_backend = "claude"
      @auth_state = "unknown"
      @stream_row : MessageRow?
      @working = false
      @waiting_for_input = false
      @workflow_ids = Set(String).new
      @attachments = [] of Attachment
      @client : Daemon::Endpoint
      @local_client : Daemon::Endpoint
      @settings : Gio::Settings
      @root_split : Gtk::Paned
      @terminal_split : Gtk::Paned
      @side_split : Gtk::Paned
      @terminal_button : Gtk::ToggleButton
      @file_button : Gtk::ToggleButton
      @diff_button : Gtk::ToggleButton
      @git_actions : GitActions
      @header_sizes : Gtk::SizeGroup
      @syncing_panes = false
      @search_dialog : SearchDialog?
      @empty_transcript : Gtk::Box
      @transcript : Gtk::Box
      @transcript_stack : Gtk::Stack
      @transcript_pages : Hash(String, TranscriptPage)
      @transcript_lru : TranscriptLru
      @transcript_page : TranscriptPage?
      @follow_bottom = true
      @history_bottom_distance = -1.0
      @bottom_pin_tick = 0_u32
      @bottom_jump_tick = 0_u32
      @bottom_jump_upper = -1.0
      @bottom_jump_page_size = -1.0
      @bottom_jump_stable_frames = 0
      @history_restore_tick = 0_u32
      @history_restore_upper = -1.0
      @history_restore_page_size = -1.0
      @history_restore_stable_frames = 0
      @working_row : Gtk::Box?
      @working_label : Gtk::Label?
      @working_dots : Dots?
      @working_timer = 0_u32
      @working_started_at : Time::Instant?
      @subscriptions = [] of {Daemon::Endpoint, Int64}
      @remote_state_subscription : Int64?
      @stream_buffer = ""
      @stream_source : String?
      @stream_reveal = TextReveal.new
      @stream_render_timer = 0_u32
      @live_turn_key : String?
      @clock_origin : Time::Instant
      @commands = [] of String
      @queue = [] of String
      @queue_generation = 0_i64
      @retired_queue_boxes = Deque(Gtk::Box).new
      @queue_retirement_scheduled = false
      @commands_bar : Gtk::ScrolledWindow
      @commands_flow : Gtk::FlowBox
      @queue_host : Gtk::Box
      @queue_box : Gtk::Box
      @choices_bar : Gtk::Box
      @messages_request = 0_i64
      @history_request : Int64?
      @state_request = 0_i64
      @turn_recovery_request : Int64?
      @pending_turn_recovery : Hash(String, JSON::Any)?
      @pending_turn_recovery_finish : {Int64?, Int64?}?
      @turn_recovery_watermark : {Int64?, Int64?}?
      @state_reload_after_recovery = false
      @transcript_reload_after_recovery = false
      @turn_recovery_events =
        Deque({Daemon::Endpoint, Hash(String, JSON::Any)}).new
      @send_pending = false
      @cancel_pending = false
      @closed = false
      @retired_transcripts = Deque(Gtk::Box).new
      @transcript_retirement_scheduled = false

      def initialize(
        application : Gtk::Application,
        local_client : Daemon::Endpoint,
        @remote : Remote::Connection,
      )
        @local_client = local_client
        @client = local_client
        @active_chat = nil
        @stream_row = nil
        @working_row = nil
        @working_label = nil
        @working_dots = nil
        @working_started_at = nil
        @presence = DiscordPresence.new
        @stream_source = nil
        @event_inbox = EventInbox(Daemon::Endpoint).new
        @live_turn_key = nil
        @history_request = nil
        @turn_recovery_request = nil
        @pending_turn_recovery = nil
        @pending_turn_recovery_finish = nil
        @turn_recovery_watermark = nil
        @state_reload_after_recovery = false
        @transcript_reload_after_recovery = false
        @clock_origin = Time.instant
        @search_dialog = nil
        @settings = Gio::Settings.new(APP_ID)
        @widget = Adw::ApplicationWindow.new(application: application)
        @widget.title = "xd"
        @widget.set_default_size(
          @settings.int("window-width"),
          @settings.int("window-height")
        )
        @widget.maximize if @settings.boolean("window-maximized")
        @image_presenter = ImagePresenter.new

        @status = Gtk::Label.new("")
        @status.xalign = 0_f32
        @status.add_css_class("dim-label")

        @auth_status = Gtk::Label.new("")
        @auth_status.xalign = 0_f32
        @auth_status.ellipsize = :end
        @auth_status.add_css_class("dim-label")
        @auth_button = Gtk::Button.new_with_label("Sign In")
        @auth_button.add_css_class("flat")
        @auth_button.visible = false
        @auth_button.clicked_signal.connect { show_auth_dialog }

        @sidebar = Sidebar.new(
          @widget,
          @local_client,
          @remote,
          ->(endpoint : Daemon::Endpoint, id : String, title : String) {
            open_chat(endpoint, id, title)
          },
          ->(endpoint : Daemon::Endpoint, id : String) {
            chat_deleted(endpoint, id)
          },
          -> { show_pair_dialog },
          -> { remote_forgot },
          ->(message : String) { @status.text = message }
        )
        @tool_panel = ToolPanel.new(
          ->(request : Hash(String, JSON::Any)) {
            panel_call(request)
          },
          -> { close_terminal_panel }
        )

        @chat_title = Adw::WindowTitle.new(title: "xd")
        chat_header = Adw::HeaderBar.new
        chat_header.title_widget = @chat_title
        chat_header.show_start_title_buttons = false
        @git_actions = GitActions.new(
          @widget,
          ->(request : Hash(String, JSON::Any)) {
            panel_call(request)
          }
        )
        chat_header.pack_end(@git_actions.widget)
        @terminal_button = pane_button(
          "utilities-terminal-symbolic",
          "Terminal"
        )
        @file_button = pane_button("folder-symbolic", "Browse files")
        @diff_button = pane_button(
          "view-list-ordered-symbolic",
          "Changed files"
        )
        @terminal_button.toggled_signal.connect { terminal_toggled }
        @file_button.toggled_signal.connect { file_toggled }
        @diff_button.toggled_signal.connect { diff_toggled }
        {% if flag?(:win32) %}
          @terminal_button.visible = false
        {% end %}
        chat_header.pack_end(@terminal_button)
        chat_header.pack_end(@file_button)
        chat_header.pack_end(@diff_button)

        @controls = ChatControls.new(
          ->(option : String, value : String?) {
            set_option(option, value)
          },
          ->(backend : String, model : String) {
            set_model(backend, model)
          }
        )
        @controls.widget.add_css_class("xd-composer")
        @controls.widget.margin_top = 10
        @controls.widget.margin_bottom = 6
        @controls.widget.margin_start = 6
        @controls.widget.margin_end = 6

        @transcript_pages = {} of String => TranscriptPage
        @transcript_lru = TranscriptLru.new
        @transcript_page = nil
        @empty_transcript = new_transcript
        @transcript = @empty_transcript
        @transcript_stack = Gtk::Stack.new
        @transcript_stack.hhomogeneous = true
        @transcript_stack.vhomogeneous = false
        @transcript_stack.transition_type = :none
        @transcript_stack.add_named(@empty_transcript, "empty")

        @transcript_scroll = Gtk::ScrolledWindow.new
        @transcript_scroll.vexpand = true
        @transcript_scroll.set_policy(:never, :external)
        transcript_clamp = Adw::Clamp.new(
          child: @transcript_stack,
          maximum_size: 1040,
          tightening_threshold: 1040
        )
        transcript_clamp.margin_top = 12
        transcript_clamp.margin_bottom = 12
        @transcript_scroll.child = transcript_clamp
        install_transcript_scrolling

        @entry = Gtk::TextView.new
        @entry.hexpand = true
        @entry.wrap_mode = :word_char
        @entry.top_margin = 10
        @entry.bottom_margin = 10
        @entry.left_margin = 10
        @entry.right_margin = 10
        @entry.sensitive = false
        paste_keys = Gtk::EventControllerKey.new
        paste_keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if keyval == Gdk::KEY_v &&
             state.includes?(Gdk::ModifierType::ControlMask)
            paste_image
          elsif (keyval == Gdk::KEY_Return ||
                keyval == Gdk::KEY_KP_Enter) &&
                !state.includes?(Gdk::ModifierType::ShiftMask)
            send_message
            true
          else
            false
          end
        end
        @entry.add_controller(paste_keys)
        @entry.buffer.changed_signal.connect do
          refresh_command_suggestions
        end

        @commands_flow = Gtk::FlowBox.new
        @commands_flow.selection_mode = :none
        @commands_flow.min_children_per_line = 1_u32
        @commands_flow.max_children_per_line = 4_u32
        @commands_flow.column_spacing = 4_u32
        @commands_flow.row_spacing = 4_u32
        @commands_flow.halign = :fill

        @commands_bar = Gtk::ScrolledWindow.new
        @commands_bar.set_policy(:never, :automatic)
        @commands_bar.max_content_height = 144
        @commands_bar.propagate_natural_height = true
        @commands_bar.child = @commands_flow
        @commands_bar.visible = false
        @commands_bar.margin_top = 6
        @commands_bar.margin_start = 10
        @commands_bar.margin_end = 10

        @attach = Gtk::Button.new_from_icon_name(
          "mail-attachment-symbolic"
        )
        @attach.sensitive = false
        @attach.tooltip_text = "Attach image"
        @attach.add_css_class("flat")
        @attach.clicked_signal.connect { choose_image }

        @send = Gtk::Button.new_from_icon_name("go-up-symbolic")
        @send.sensitive = false
        @send.add_css_class("suggested-action")
        @send.add_css_class("circular")
        @send.tooltip_text = "Send (Enter)"
        @send.clicked_signal.connect do
          @working ? cancel_turn : send_message
        end
        @voice = VoiceInput.new(@widget, @entry)

        @queue_box = new_queue_box
        @queue_host = Gtk::Box.new(:vertical, 0)
        @queue_host.append(@queue_box)

        @choices_bar = Gtk::Box.new(:vertical, 6)
        @choices_bar.margin_top = 6
        @choices_bar.margin_start = 10
        @choices_bar.margin_end = 10
        @choices_bar.visible = false

        @attachments_bar = Gtk::Box.new(:horizontal, 6)
        @attachments_bar.margin_start = 10
        @attachments_bar.margin_end = 10
        @attachments_bar.margin_top = 8
        @attachments_bar.add_css_class("xd-attachments")
        @attachments_bar.visible = false

        filler = Gtk::Box.new(:horizontal, 0)
        filler.hexpand = true
        @controls.run.append(filler)
        @controls.run.append(@attach)
        @controls.run.append(@voice.widget)
        @controls.run.append(@send)

        entry_scroll = Gtk::ScrolledWindow.new
        entry_scroll.set_policy(:never, :automatic)
        entry_scroll.max_content_height = 180
        entry_scroll.propagate_natural_height = true
        entry_scroll.child = @entry

        composer_column = Gtk::Box.new(:vertical, 0)
        composer_column.append(@queue_host)
        composer_column.append(@choices_bar)
        composer_column.append(@attachments_bar)
        composer_column.append(@commands_bar)
        composer_column.append(entry_scroll)
        composer_column.append(@controls.widget)
        composer_frame = Gtk::Frame.new
        composer_frame.child = composer_column
        composer_frame.margin_top = 6
        composer_frame.margin_start = 12
        composer_frame.margin_end = 12

        @context_label = Gtk::Label.new("")
        @context_label.xalign = 0_f32
        @context_label.hexpand = true
        @context_label.ellipsize = :middle
        @context_label.add_css_class("dim-label")
        @context_label.add_css_class("caption")

        context_line = Gtk::Box.new(:horizontal, 8)
        context_line.append(@context_label)
        context_line.append(@status)

        auth_controls = Gtk::Box.new(:horizontal, 8)
        auth_controls.append(@auth_status)
        auth_controls.append(@auth_button)
        auth_controls.halign = :end
        auth_controls.add_css_class("xd-auth-controls")

        context = Gtk::Overlay.new
        context.child = context_line
        context.add_overlay(auth_controls)
        context.add_css_class("xd-context")
        context.add_css_class("dim-label")
        context.margin_start = 26
        context.margin_end = 26
        context.margin_bottom = 12

        @composer = Gtk::Box.new(:vertical, 0)
        @composer.append(composer_frame)
        @composer.append(context)
        @composer.visible = false
        composer_clamp = Adw::Clamp.new(
          child: @composer,
          maximum_size: 1040,
          tightening_threshold: 1040
        )

        empty = Adw::StatusPage.new(
          icon_name: "chat-message-new-symbolic",
          title: "No Chat Selected",
          description: "Pick a chat in the sidebar, or start a new one " \
                       "in a folder."
        )
        @chat_stack = Gtk::Stack.new
        @chat_stack.vexpand = true
        @chat_stack.add_named(empty, "empty")
        @chat_stack.add_named(@transcript_scroll, "chat")
        @chat_stack.visible_child_name = "empty"

        content = Gtk::Box.new(:vertical, 0)
        content.append(@chat_stack)
        content.append(composer_clamp)
        content.add_css_class("xd-surface")

        @terminal_split = Gtk::Paned.new(:vertical)
        @terminal_split.start_child = content
        @terminal_split.end_child = @tool_panel.terminal_widget
        @terminal_split.resize_start_child = true
        @terminal_split.shrink_start_child = false
        @terminal_split.resize_end_child = false
        @terminal_split.shrink_end_child = false
        @terminal_split.notify_signal["position"].connect do |_property|
          remember_terminal_height
        end

        @side_split = Gtk::Paned.new(:horizontal)
        @side_split.start_child = @terminal_split
        @side_split.end_child = @tool_panel.repository_widget
        @side_split.resize_start_child = true
        @side_split.shrink_start_child = false
        @side_split.resize_end_child = false
        @side_split.shrink_end_child = false
        @side_split.notify_signal["position"].connect do |_property|
          remember_repository_width
        end

        chat = Adw::ToolbarView.new
        chat.add_css_class("xd-surface")
        chat.add_top_bar(chat_header)
        chat.content = @side_split
        chat.add_css_class("xd-divider-left")

        @root_split = Gtk::Paned.new(:horizontal)
        @root_split.start_child = @sidebar.widget
        @root_split.end_child = chat
        @root_split.position = @settings.int("sidebar-width")
        @root_split.resize_start_child = false
        @root_split.shrink_start_child = false
        @root_split.resize_end_child = true
        @root_split.shrink_end_child = false

        header_spacer = Gtk::Box.new(:vertical, 0)
        divider = Gtk::Separator.new(:horizontal)
        divider.add_css_class("xd-header-divider")
        divider_layer = Gtk::Box.new(:vertical, 0)
        divider_layer.append(header_spacer)
        divider_layer.append(divider)
        divider_layer.halign = :fill
        divider_layer.valign = :start
        divider_layer.can_target = false
        header_spacer.can_target = false
        divider.can_target = false

        overlay = Gtk::Overlay.new
        overlay.child = @root_split
        overlay.add_overlay(divider_layer)
        @widget.content = overlay
        install_selection_clearer

        @header_sizes = Gtk::SizeGroup.new(:vertical)
        @header_sizes.add_widget(@sidebar.header)
        @header_sizes.add_widget(chat_header)
        @header_sizes.add_widget(header_spacer)

        @widget.close_request_signal.connect do
          @closed = true
          @subscriptions.each do |endpoint, id|
            endpoint.unsubscribe(id)
          end
          @subscriptions.clear
          if id = @remote_state_subscription
            @remote.unsubscribe(id)
            @remote_state_subscription = nil
          end
          unless @working_timer == 0
            GLib.source_remove(@working_timer)
            @working_timer = 0_u32
          end
          unless @stream_render_timer == 0
            GLib.source_remove(@stream_render_timer)
            @stream_render_timer = 0_u32
          end
          @transcript_pages.each_value(&.clear_workflows)
          @event_inbox.clear
          @tool_panel.close
          @voice.close
          @presence.close
          persist_window_layout
          false
        end
        install_window_actions(application)

        subscribe(@local_client)
        subscribe(@remote)
        @remote_state_subscription = @remote.on_state do |snapshot|
          GLib.idle_add do
            next false if @closed
            @tool_panel.remote_connection_changed(
              snapshot.state.connected?,
              snapshot.error
            )
            if @client.same?(@remote)
              @git_actions.connection_changed(snapshot.state.connected?)
              update_presence
            end
            false
          end
        end

        restore_active_chat
        @sidebar.reload
      end

      def present : Nil
        @widget.present
      end

      private def install_window_actions(
        application : Gtk::Application,
      ) : Nil
        search = Gio::SimpleAction.new("search", nil)
        search.activate_signal.connect { show_search_dialog }
        @widget.add_action(search)
        application.set_accels_for_action(
          "win.search",
          ["<Control>k", "<Control>f"]
        )
      end

      private def install_selection_clearer : Nil
        press = Gtk::GestureClick.new
        press.propagation_phase = Gtk::PropagationPhase::Capture
        press.pressed_signal.connect do |_count, x, y|
          focus = @widget.focus_widget
          if label = focus.as?(Gtk::Label)
            if label.has_css_class("xd-body")
              target = @widget.pick(x, y, Gtk::PickFlags::Default)
              label.select_region(0, 0) unless target &&
                                               target.to_unsafe == label.to_unsafe
            end
          end
        end
        @widget.add_controller(press)
      end

      private def show_search_dialog : Nil
        if dialog = @search_dialog
          dialog.present
          return
        end

        endpoint = @client
        dialog = SearchDialog.new(
          @widget,
          endpoint,
          ->(id : String, title : String) {
            open_chat(endpoint, id, title)
          },
          -> {
            @search_dialog = nil
          }
        )
        @search_dialog = dialog
        dialog.present
      end

      private def pane_button(
        icon_name : String,
        tooltip : String,
      ) : Gtk::ToggleButton
        button = Gtk::ToggleButton.new
        button.icon_name = icon_name
        button.add_css_class("flat")
        button.tooltip_text = tooltip
        button.sensitive = false
        button
      end

      private def terminal_toggled : Nil
        {% if flag?(:win32) %}
          @terminal_button.active = false if @terminal_button.active?
          return
        {% end %}

        shown = @terminal_button.active?
        unless shown
          remember_terminal_height
          @tool_panel.show_terminal(false)
          remember_panes
          return
        end

        @tool_panel.show_terminal(true, focus: !@syncing_panes)
        set_end_child_size(
          @terminal_split,
          @settings.int("terminal-height"),
          vertical: true
        )
        remember_panes
      end

      private def close_terminal_panel : Nil
        @terminal_button.active = false
      end

      private def file_toggled : Nil
        if @file_button.active?
          @diff_button.active = false if @diff_button.active?
          @tool_panel.show_repository("files")
          set_end_child_size(
            @side_split,
            @settings.int("diff-width"),
            vertical: false
          )
        elsif !@diff_button.active?
          remember_repository_width
          @tool_panel.show_repository(nil)
        end
        remember_panes
      end

      private def diff_toggled : Nil
        if @diff_button.active?
          @file_button.active = false if @file_button.active?
          @tool_panel.show_repository("diff")
          set_end_child_size(
            @side_split,
            @settings.int("diff-width"),
            vertical: false
          )
        elsif !@file_button.active?
          remember_repository_width
          @tool_panel.show_repository(nil)
        end
        remember_panes
      end

      private def set_end_child_size(
        paned : Gtk::Paned,
        size : Int32,
        vertical : Bool,
      ) : Nil
        return if size <= 0

        attempts = 0
        GLib.timeout(16.milliseconds) do
          attempts += 1
          available = vertical ? paned.height : paned.width
          if available > 0
            paned.position = Math.max(available - size, 0)
            false
          else
            attempts < 30
          end
        end
      end

      private def remember_terminal_height : Nil
        return unless @tool_panel.terminal_widget.visible?

        height = @tool_panel.terminal_widget.height
        @settings.set_int("terminal-height", height.to_i32) if height > 0
      end

      private def remember_repository_width : Nil
        return unless @tool_panel.repository_widget.visible?

        width = @tool_panel.repository_widget.width
        @settings.set_int("diff-width", width.to_i32) if width > 0
      end

      private def persist_window_layout : Nil
        remember_panes
        remember_terminal_height
        remember_repository_width
        @settings.set_int("window-width", @widget.default_width)
        @settings.set_int("window-height", @widget.default_height)
        @settings.set_int("sidebar-width", @root_split.position)
        @settings.set_boolean(
          "window-maximized",
          {% if flag?(:win32) || flag?(:darwin) %}
            @widget.is_maximized?
          {% else %}
            @widget.is_maximized
          {% end %}
        )
      end

      private def pane_key : String?
        chat_id = @active_chat
        return unless chat_id

        if @client.same?(@local_client)
          "local/#{chat_id}"
        else
          snapshot = @remote.snapshot
          host = snapshot.host || "remote"
          port = snapshot.port || 0
          "remote/#{host}:#{port}/#{chat_id}"
        end
      end

      private def current_panes : UInt32
        state = PaneState::None
        state |= PaneState::Terminal if @terminal_button.active?
        state |= PaneState::Files if @file_button.active?
        state |= PaneState::Diff if @diff_button.active?
        state
      end

      private def remember_panes : Nil
        return if @syncing_panes
        key = pane_key
        return unless key

        states = PaneState.update(
          @settings.value("pane-state"),
          key,
          current_panes
        )
        @settings.set_value("pane-state", states)
      end

      private def saved_panes : UInt32
        key = pane_key
        return PaneState::None unless key

        PaneState.fetch(@settings.value("pane-state"), key)
      end

      private def apply_panes(state : UInt32) : Nil
        {% if flag?(:win32) %}
          state &= ~PaneState::Terminal
        {% end %}
        if (state & PaneState::Files) != 0
          state &= ~PaneState::Diff
        end
        return if current_panes == state

        @syncing_panes = true
        @terminal_button.active = (state & PaneState::Terminal) != 0
        @file_button.active = false
        @diff_button.active = false
        if (state & PaneState::Files) != 0
          @file_button.active = true
        elsif (state & PaneState::Diff) != 0
          @diff_button.active = true
        end
      ensure
        @syncing_panes = false
      end

      private def hide_panes_for_switch : Nil
        @syncing_panes = true
        @terminal_button.active = false
        @file_button.active = false
        @diff_button.active = false
      ensure
        @syncing_panes = false
      end

      private def subscribe(endpoint : Daemon::Endpoint) : Nil
        id = endpoint.subscribe do |event|
          next if @closed
          if @event_inbox.push(endpoint, event)
            GLib.idle_add do
              drain_events
            end
          end
        end
        @subscriptions << {endpoint, id}
      end

      private def drain_events : Bool
        events, more = @event_inbox.drain
        events.each do |endpoint, event|
          handle_event(endpoint, event) unless @closed
        end
        more && !@closed
      end

      private def show_pair_dialog : Nil
        PairDialog.new(
          @widget,
          @remote,
          -> {
            @status.text = "Connected to #{@remote.snapshot.host}"
            @sidebar.reload
          }
        ).present
      end

      private def remote_forgot : Nil
        if @client.same?(@remote)
          @client = @local_client
          clear_active_chat
        end
        keys = @transcript_pages.values
          .select { |page| page.endpoint.same?(@remote) }
          .map(&.key)
        keys.each { |key| remove_transcript_page(key) }
      end

      private def panel_call(
        fields : Hash(String, JSON::Any),
      ) : PanelCallResult
        PanelCallResult.new(@client.call(fields), nil)
      rescue error : Daemon::Client::Error
        message = error.message || "Daemon request failed."
        PanelCallResult.new(nil, message)
      end

      private def call_async(
        endpoint : Daemon::Endpoint,
        fields : Hash(String, JSON::Any),
        &complete : Hash(String, JSON::Any)?, String? -> Nil
      ) : Nil
        spawn do
          body : Hash(String, JSON::Any)? = nil
          message : String? = nil
          begin
            body = endpoint.call(fields)
          rescue error : Daemon::Client::Error
            message = error.message || "Daemon request failed."
          end
          GLib.idle_add do
            complete.call(body, message) unless @closed
            false
          end
        end
      end

      private def new_transcript : Gtk::Box
        transcript = Gtk::Box.new(:vertical, 8)
        transcript.valign = :start
        transcript
      end

      private def transcript_page_key(
        endpoint : Daemon::Endpoint,
        chat_id : String,
      ) : String
        if endpoint.same?(@local_client)
          "local:#{chat_id}"
        else
          "remote:#{endpoint.object_id}:#{chat_id}"
        end
      end

      private def activate_transcript_page(
        endpoint : Daemon::Endpoint,
        chat_id : String,
      ) : Bool
        key = transcript_page_key(endpoint, chat_id)
        reused = !!@transcript_pages[key]?
        page = @transcript_pages[key]? || begin
          transcript = new_transcript
          created = TranscriptPage.new(
            key,
            chat_id,
            endpoint,
            transcript
          )
          @transcript_stack.add_named(transcript, key)
          @transcript_pages[key] = created
          created
        end

        @transcript_stack.visible_child = page.transcript
        @transcript = page.transcript
        @transcript_page = page
        @workflow_ids = page.workflow_ids
        if evicted = @transcript_lru.touch(key)
          remove_transcript_page(evicted)
        end
        reused
      end

      private def activate_empty_transcript : Nil
        @transcript_stack.visible_child = @empty_transcript
        @transcript = @empty_transcript
        @transcript_page = nil
        @workflow_ids = Set(String).new
      end

      private def remove_transcript_page(key : String) : Nil
        page = @transcript_pages.delete(key)
        return unless page

        @transcript_lru.delete(key)
        page.clear_workflows
        @transcript_stack.remove(page.transcript)
        retire_transcript(page.transcript)
      end

      private def current_transcript_cacheable? : Bool
        page = @transcript_page
        return false unless page
        return false if @working
        return true unless @client.same?(@local_client)

        !page.choices_visible
      end

      private def leave_current_transcript(keep : Bool) : Nil
        page = @transcript_page
        return unless page

        page.revision = -1_i64 if @working
        @transcript_lru.touch(page.key)
        reset_live_turn_ui
        return if keep

        activate_empty_transcript
        remove_transcript_page(page.key)
      end

      private def open_chat(
        endpoint : Daemon::Endpoint,
        id : String,
        title : String,
      ) : Nil
        changed = @active_chat != id || !@client.same?(endpoint)
        if changed
          keep_previous = current_transcript_cacheable?
          retire_open_questions
          leave_current_transcript(keep_previous)
          clear_queue
          @follow_bottom = true
          @history_bottom_distance = -1.0
        end
        remember_panes
        hide_panes_for_switch
        clear_attachments
        @client = endpoint
        @active_chat = id
        @waiting_for_input = false
        @auth_state = "unknown"
        if changed
          @voice.select(
            endpoint,
            id,
            remote: !endpoint.same?(@local_client)
          )
        end
        @sidebar.activate_chat(endpoint, id)
        prefix = endpoint.same?(@local_client) ? "local:" : "remote:"
        @settings.set_string("active-chat", "#{prefix}#{id}")
        @stream_row = nil
        @chat_title.title = title
        @chat_stack.visible_child_name = "chat"
        @composer.visible = true
        update_auth_controls
        @controls.sensitive = true
        @terminal_button.sensitive = {{ !flag?(:win32) }}
        @file_button.sensitive = true
        @diff_button.sensitive = true
        @tool_panel.select_chat(id, pane_key)
        @git_actions.select_chat(id)
        apply_panes(saved_panes)
        if changed
          activate_transcript_page(endpoint, id)
          begin_bottom_jump
        end
        load_messages
        load_chat_state
        @entry.grab_focus
        update_presence
      end

      private def chat_deleted(
        endpoint : Daemon::Endpoint,
        id : String,
      ) : Nil
        active = @client.same?(endpoint) && @active_chat == id
        unless active
          remove_transcript_page(transcript_page_key(endpoint, id))
          return
        end

        clear_active_chat
      end

      private def clear_active_chat : Nil
        remember_panes
        hide_panes_for_switch
        retire_open_questions
        leave_current_transcript(false)
        @active_chat = nil
        @waiting_for_input = false
        @sidebar.clear_active_chat
        @settings.set_string("active-chat", "")
        @stream_row = nil
        @working = false
        @auth_state = "unknown"
        @chat_title.title = "xd"
        @chat_stack.visible_child_name = "empty"
        @composer.visible = false
        @entry.buffer.text = ""
        @entry.sensitive = false
        @attach.sensitive = false
        @voice.select(nil, nil)
        update_send_button
        @send.sensitive = false
        @send.remove_css_class("destructive-action")
        @send.add_css_class("suggested-action")
        @controls.sensitive = false
        @terminal_button.sensitive = false
        @file_button.sensitive = false
        @diff_button.sensitive = false
        update_presence
        @tool_panel.select_chat(nil, nil)
        @git_actions.select_chat(nil)
        clear_queue
        @commands.clear
        refresh_command_suggestions
        clear_attachments
        @context_label.text = ""
        @context_label.tooltip_text = nil
        @status.text = ""
        @auth_status.text = ""
        @auth_button.visible = false
      end

      private def update_presence : Nil
        state = if @active_chat.nil?
                  "Browsing workspaces"
                elsif @client.same?(@remote) && !@remote.connected?
                  "Remote unavailable"
                elsif @waiting_for_input
                  "Waiting for input"
                elsif @working
                  "Agent working"
                else
                  "Reviewing a conversation"
                end
        @presence.state = state
      end

      private def restore_active_chat : Nil
        saved = @settings.string("active-chat")
        if saved.starts_with?("local:")
          id = saved.lchop("local:")
          @sidebar.restore_chat(id, false) unless id.empty?
        elsif saved.starts_with?("remote:")
          id = saved.lchop("remote:")
          @sidebar.restore_chat(id, true) unless id.empty?
        end
      end

      private def load_messages(force = false) : Nil
        chat_id = @active_chat
        return unless chat_id
        page = @transcript_page
        return unless page
        endpoint = @client
        @messages_request += 1
        request = @messages_request
        @history_request = request

        call_async(endpoint, {
          "op"    => JSON::Any.new("messages"),
          "chat"  => JSON::Any.new(chat_id),
          "limit" => JSON::Any.new(page.paging.query_limit.to_i64),
        }) do |response, error|
          next unless request == @messages_request &&
                      @client.same?(endpoint) &&
                      @active_chat == chat_id &&
                      @transcript_page.same?(page)
          if error
            @status.text = error
            finish_history_request(request)
            next
          end
          apply_messages(response.not_nil!, page, force, request)
        end
      end

      private def apply_messages(
        response : Hash(String, JSON::Any),
        page : TranscriptPage,
        force : Bool,
        request : Int64,
      ) : Nil
        revision = response["last_message_id"]?.try(&.as_i64?) || 0_i64
        if !force && page.revision == revision
          finish_history_request(request)
          return
        end

        if @follow_bottom
          begin_bottom_jump
        end
        retire_open_questions
        @live_turn_key = nil
        reset_stream_segment
        remove_working_row(reset_started_at: false)
        page.clear_workflows
        replace_transcript(page, request)
        messages = response["messages"]?.try(&.as_a?) || [] of JSON::Any
        total = response["total_messages"]?.try(&.as_i64?) ||
                messages.size.to_i64
        append_history_button(page, total, messages.size)
        start = page.paging.start(messages.size)
        batch = TranscriptBatch(JSON::Any).new(messages, start)
        GLib.idle_add do
          active = !@closed &&
                   request == @messages_request &&
                   @transcript_page.same?(page) &&
                   @client.same?(page.endpoint) &&
                   @active_chat == page.chat_id
          if active
            batch.next_batch.each do |entry|
              index, message = entry
              if seconds = turn_duration(messages, index)
                append_worked_for(seconds)
              end
              next if message["role"].as_s == "duration"

              add_message(
                message["role"].as_s,
                message["content"].as_s,
                message["label"]?.try(&.as_s?),
                reply_answerable?(messages, index)
              )
            end

            if batch.done?
              page.revision = revision
              @stream_row = nil
              set_working(@working)
              scroll_to_bottom
              if force && !@follow_bottom &&
                 @history_bottom_distance >= 0
                queue_history_restore
              end
              finish_history_request(request)
              false
            else
              true
            end
          else
            false
          end
        end
      end

      private def finish_history_request(request : Int64) : Nil
        return unless @history_request == request

        @history_request = nil
        resume_turn_recovery
      end

      private def turn_duration(
        messages : Array(JSON::Any),
        position : Int,
      ) : Int64?
        return if position <= 0
        before = messages[position - 1]
        return unless before["role"].as_s == "user"

        seconds : Int64? = nil
        (position...messages.size).each do |index|
          at = messages[index]
          role = at["role"].as_s
          break if role == "user"
          next unless role == "duration"

          stored = at["content"].as_s.to_i64?
          seconds = stored if stored && stored >= 0
          break
        end

        message = messages[position]
        if seconds.nil? && message["role"].as_s == "assistant"
          last = message
          (position...messages.size).each do |index|
            at = messages[index]
            break unless at["role"].as_s == "assistant"
            last = at
          end
          started_at = before["at"]?.try(&.as_i64?)
          finished_at = last["at"]?.try(&.as_i64?)
          if started_at && finished_at
            seconds = finished_at - started_at
          end
        end

        seconds if seconds && seconds >= 1
      end

      private def append_worked_for(seconds : Int64) : Gtk::Label
        row = Gtk::Label.new(TurnTiming.format("Worked", seconds))
        row.xalign = 0_f32
        row.add_css_class("caption")
        row.add_css_class("dim-label")
        row.margin_start = 24
        row.margin_top = 6
        @transcript.append(row)
        row
      end

      private def append_history_button(
        page : TranscriptPage,
        total : Int64,
        fetched : Int,
      ) : Nil
        label = page.paging.earlier_label(total, fetched)
        return unless label

        button = Gtk::Button.new_with_label(label)
        button.halign = :center
        button.margin_bottom = 8
        button.add_css_class("flat")
        button.add_css_class("pill")
        button.clicked_signal.connect { load_earlier_messages(page) }
        @transcript.append(button)
      end

      # Swapping one stack child is bounded. Keep the old tree parented and
      # hidden while its rows are retired in small idle batches; dropping a
      # large transcript subtree in one callback can stall or crash GTK.
      private def replace_transcript(
        page : TranscriptPage,
        request : Int64,
      ) : Nil
        retired = page.transcript
        replacement = new_transcript
        page.transcript = replacement
        page.tool_group = nil
        @transcript = replacement
        @transcript_stack.add_named(
          replacement,
          "#{page.key}:reload:#{request}"
        )
        @transcript_stack.visible_child = replacement

        retire_transcript(retired)
      end

      private def retire_transcript(transcript : Gtk::Box) : Nil
        return unless transcript.first_child

        @retired_transcripts << transcript
        return if @transcript_retirement_scheduled

        @transcript_retirement_scheduled = true
        GLib.idle_add do
          4.times do
            retired = @retired_transcripts.first?
            break unless retired

            if child = retired.first_child
              retired.remove(child)
            end
            unless retired.first_child
              @transcript_stack.remove(retired) if retired.parent
              @retired_transcripts.shift
            end
          end
          more = !@retired_transcripts.empty?
          @transcript_retirement_scheduled = more
          more
        end
      end

      private def load_earlier_messages(page : TranscriptPage) : Nil
        return unless @transcript_page.same?(page)

        adjustment = @transcript_scroll.vadjustment
        @follow_bottom = false
        @history_bottom_distance =
          adjustment.upper - adjustment.value
        @transcript_scroll.opacity = 0.0
        page.paging.load_earlier
        load_messages(force: true)
      end

      private def add_message(
        role : String,
        content : String,
        label : String? = nil,
        answerable : Bool = false,
      ) : MessageRow?
        if role == "duration"
          @status.text = "Finished in #{content}s"
          return
        end

        if role == "tool"
          if patch = Agent::GitDiffTracker.patch(content)
            end_tool_group
            add_diff_message(patch)
            return
          end
          if workflow = Agent::WorkflowRun.parse(content)
            end_tool_group
            add_workflow_message(workflow)
            return
          end
          if subagent = Agent::SubagentTool.parse(content)
            activity = @transcript_page.try(&.tool_group)
            end_tool_group
            add_subagent_message(subagent[0], subagent[1], activity)
            return
          end
          content = "Files changed" if Agent::GitDiffTracker.file_change?(content)
          append_tool_line(content)
          return
        end
        end_tool_group

        images = role == "assistant" ? nil : Agent::ImageReference.parse(content)
        content = images.remainder if images
        workspace = role == "assistant" ? Agent::WorkspaceBlock.parse(content) : nil
        assistant_text = workspace.try(&.remainder) || content
        parsed = role == "assistant" ? Agent::Ask.parse(assistant_text) : nil
        shown = if parsed
                  [parsed.remainder, "**#{parsed.ask.question}**"]
                    .reject(&.empty?).join("\n\n")
                else
                  assistant_text
                end
        literal_parts = if images
                          number = 0
                          images.parts.map do |part|
                            if path = part.path
                              number += 1
                              @image_presenter.preview(
                                @client,
                                path,
                                number
                              ).as(MessageRow::LiteralPart)
                            else
                              part.text.not_nil!
                                .as(MessageRow::LiteralPart)
                            end
                          end
                        end
        row = MessageRow.new(
          MessageKind.from_role(role),
          shown,
          literal_parts
        )
        row.source = label
        @transcript.append(row.widget)
        append_ask(parsed.ask) if parsed && answerable
        row
      end

      private def append_tool_line(summary : String) : Nil
        page = @transcript_page
        return unless page

        group = page.tool_group
        unless group && group.widget.parent
          group = ToolCallGroup.new
          page.tool_group = group
          @transcript.append(group.widget)
        end
        group.append(summary)
      end

      private def end_tool_group : Nil
        @transcript_page.try(&.tool_group=(nil))
      end

      private def add_diff_message(patch : String) : MessageRow
        row = MessageRow.new(
          MessageKind::Assistant,
          "```diff\n#{patch}\n```"
        )
        @transcript.append(row.widget)
        row
      end

      private def add_workflow_message(
        workflow : Agent::WorkflowRun::Run,
      ) : Nil
        return if @workflow_ids.includes?(workflow.id)
        @workflow_ids << workflow.id

        card = WorkflowCard.new(workflow)
        @transcript_page.try { |page| page.workflow_cards << card }
        @transcript.append(card.widget)
      end

      private def add_subagent_message(
        identity : String,
        task : String,
        activity : ToolCallGroup?,
      ) : Gtk::Label
        title = Gtk::Label.new("Subagent · #{identity}")
        title.xalign = 0_f32
        title.add_css_class("title")

        detail = Gtk::Label.new(task)
        detail.xalign = 0_f32
        detail.wrap = true
        detail.wrap_mode = :word_char
        detail.selectable = true
        detail.add_css_class("xd-body")

        card = Gtk::Box.new(:vertical, 6)
        card.add_css_class("xd-subagent")
        if activity && (parent = activity.widget.parent.as?(Gtk::Box))
          # The subagent's toggle becomes the disclosure for this run.
          activity.absorb
          parent.remove(activity.widget)

          indicator = Gtk::Image.new_from_icon_name("pan-end-symbolic")
          indicator.valign = :start
          indicator.margin_top = 3

          body = Gtk::Box.new(:vertical, 6)
          body.append(title)
          body.append(detail)

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
          toggle.toggled_signal.connect do
            expanded = toggle.active?
            indicator.icon_name =
              expanded ? "pan-down-symbolic" : "pan-end-symbolic"
            toggle.tooltip_text =
              expanded ? "Hide subagent activity" : "Show subagent activity"
          end

          activity.widget.margin_start = 12
          activity.widget.margin_end = 0
          card.append(toggle)
          card.append(activity.widget)
        else
          card.append(title)
          card.append(detail)
        end
        @transcript.append(card)
        title
      end

      private def reply_answerable?(
        messages : Array(JSON::Any),
        position : Int,
      ) : Bool
        return false unless messages[position]["role"].as_s == "assistant"

        ((position + 1)...messages.size).all? do |index|
          messages[index]["role"].as_s == "duration"
        end
      end

      private def append_ask(ask : Agent::Ask) : Nil
        retire_open_questions
        @waiting_for_input = true
        update_presence
        if page = @transcript_page
          page.choices_visible = true
        end

        unless ask.options.empty?
          choices = Gtk::FlowBox.new
          choices.selection_mode = :none
          choices.row_spacing = 4_u32
          choices.max_children_per_line = 1_u32
          choices.homogeneous = true

          ask.options.each do |option|
            answer = option
            button = Gtk::Button.new_with_label(answer)
            button.add_css_class("xd-choice")
            if label = button.child.as?(Gtk::Label)
              label.wrap = true
            end
            button.clicked_signal.connect { answer_ask(answer) }
            choices.append(button)
          end

          @choices_bar.append(choices)
        end

        if ask.accepts_input
          input = Gtk::Entry.new
          input.hexpand = true
          input.placeholder_text = "Type your answer"
          input.activate_signal.connect { answer_ask(input.text) }

          send = Gtk::Button.new_with_label("Send")
          send.add_css_class("suggested-action")
          send.clicked_signal.connect { answer_ask(input.text) }

          row = Gtk::Box.new(:horizontal, 6)
          row.append(input)
          row.append(send)
          @choices_bar.append(row)
        end

        @choices_bar.visible = true
      end

      private def answer_ask(answer : String) : Nil
        text = answer.strip
        return if text.empty?

        @waiting_for_input = false
        update_presence
        @entry.grab_focus
        send_message(text)
      end

      private def retire_open_questions : Nil
        clear(@choices_bar)
        @choices_bar.visible = false
        @transcript_page.try(&.choices_visible=(false))
      end

      private def send_message(explicit_text : String? = nil) : Nil
        chat_id = @active_chat
        return unless chat_id
        return unless @auth_state == "signed-in"
        return if @send_pending
        endpoint = @client
        text = (explicit_text || @entry.buffer.text).strip
        attachments = explicit_text ? [] of Attachment : @attachments
        if !explicit_text && text.empty? && attachments.empty? && !@queue.empty?
          steer_queue(0, @queue.first)
          return
        end
        return if text.empty? && attachments.empty?
        @sidebar.answer_chat(@client, chat_id)

        request = {
          "op"   => JSON::Any.new("send"),
          "chat" => JSON::Any.new(chat_id),
          "text" => JSON::Any.new(text),
        }
        unless attachments.empty?
          encoded = attachments.map do |attachment|
            JSON::Any.new({
              "name" => JSON::Any.new(attachment.name),
              "mime" => JSON::Any.new("image/png"),
              "data" => JSON::Any.new(attachment.data),
            })
          end
          request["attachments"] = JSON::Any.new(encoded)
        end

        begin_bottom_jump
        @send_pending = true
        update_send_button
        call_async(endpoint, request) do |response, error|
          @send_pending = false
          update_send_button
          unless @client.same?(endpoint) && @active_chat == chat_id
            next
          end
          if error
            @status.text = error
            next
          end
          if response
            @status.text = ""
            retire_open_questions
            unless explicit_text
              @entry.buffer.text = ""
              clear_attachments
            end
            if response["queued"]?.try(&.as_bool?) == true
              @status.text = "Message queued"
            end
            load_messages
            load_chat_state
          end
        end
      end

      private def paste_image : Bool
        clipboard = @entry.clipboard
        formats = clipboard.formats
        return false unless formats.contain_gtype(Gdk::Texture.g_type)

        clipboard.read_texture_async(nil) do |source, result|
          begin
            texture = source.as(Gdk::Clipboard)
              .read_texture_finish(result)
            if texture
              prepare_texture_attachment(
                "paste-#{Time.utc.to_unix_ms}.png",
                texture
              )
            end
          rescue error
            @status.text = error.message || "Cannot paste that image."
          end
        end
        true
      end

      private def choose_image : Nil
        return unless @active_chat

        filter = Gtk::FileFilter.new
        filter.name = "Images"
        filter.add_mime_type("image/*")
        dialog = Gtk::FileDialog.new(
          title: "Attach image",
          modal: true,
          default_filter: filter
        )
        dialog.open(@widget, nil) do |_source, result|
          begin
            if file = dialog.open_finish(result)
              if path = file.path
                prepare_file_attachment(path.to_s)
              else
                @status.text = "Only local image files can be attached."
              end
            end
          rescue Gio::IOErrorEnum::Cancelled
          rescue error
            @status.text = error.message || "Cannot attach that image."
          end
        end
      end

      private def prepare_file_attachment(path : String) : Nil
        chat_id = @active_chat || return
        endpoint = @client
        @status.text = "Preparing image…"
        queued = BackgroundWork.submit do
          prepared : PreparedAttachment? = nil
          message : String? = nil
          begin
            image = ImageAttachment.prepare_file(path)
            prepared = PreparedAttachment.new(
              File.basename(path),
              Base64.strict_encode(image.png),
              image.png.size,
              image.preview
            )
          rescue error
            message = error.message || "Cannot attach that image."
          end
          GLib.idle_add do
            if @client.same?(endpoint) && @active_chat == chat_id
              if attachment = prepared
                finish_file_attachment(attachment)
              else
                @status.text = message || "Cannot attach that image."
              end
            end
            false
          end
          nil
        end
        unless queued
          @status.text =
            "Image workers are busy. Try again shortly."
        end
      end

      private def finish_file_attachment(
        prepared : PreparedAttachment,
      ) : Nil
        if @attachments.size >= MAX_IMAGES
          @status.text = "A message can contain at most 4 images."
          return
        end
        total = @attachments.sum(&.bytesize)
        if total > MAX_TOTAL_BYTES - prepared.bytesize
          @status.text = "Attached images must stay under 20 MiB total."
          return
        end

        attachment = Attachment.new(
          prepared.name,
          prepared.data,
          prepared.bytesize,
          ImageAttachment.texture(prepared.preview)
        )
        @attachments << attachment
        append_attachment_chip(attachment)
        @status.text = ""
      end

      private def prepare_texture_attachment(
        name : String,
        texture : Gdk::Texture,
      ) : Nil
        if @attachments.size >= MAX_IMAGES
          @status.text = "A message can contain at most 4 images."
          return
        end

        chat_id = @active_chat || return
        endpoint = @client
        @status.text = "Preparing image…"
        queued = BackgroundWork.submit do
          prepared : PreparedAttachment? = nil
          message : String? = nil
          begin
            encoded = texture.save_to_png_bytes
            data = encoded.data.try(&.dup) ||
                   raise IO::Error.new(
                     "Cannot encode that image as PNG."
                   )
            image = ImageAttachment.prepare(data)
            prepared = PreparedAttachment.new(
              File.basename(name),
              Base64.strict_encode(image.png),
              image.png.size,
              image.preview
            )
          rescue error
            message = error.message || "Cannot attach that image."
          end
          GLib.idle_add do
            if @client.same?(endpoint) && @active_chat == chat_id
              if attachment = prepared
                finish_file_attachment(attachment)
              else
                @status.text = message || "Cannot attach that image."
              end
            end
            false
          end
          nil
        end
        unless queued
          @status.text =
            "Image workers are busy. Try again shortly."
        end
      end

      private def append_attachment_chip(
        attachment : Attachment,
      ) : Nil
        picture = Gtk::Picture.new_for_paintable(attachment.texture)
        picture.halign = :center
        picture.valign = :center

        label = Gtk::Label.new(attachment.name)
        label.ellipsize = :middle
        label.max_width_chars = 18
        label.add_css_class("caption")
        label.add_css_class("dim-label")

        card = Gtk::Box.new(:vertical, 4)
        card.append(picture)
        card.append(label)
        card.add_css_class("card")
        card.margin_top = 6
        card.margin_bottom = 6
        card.margin_start = 6
        card.margin_end = 6
        card.halign = :start

        remove = Gtk::Button.new_from_icon_name(
          "window-close-symbolic"
        )
        remove.tooltip_text = "Remove"
        remove.halign = :end
        remove.valign = :start
        remove.add_css_class("circular")

        chip = Gtk::Overlay.new
        chip.child = card
        chip.add_overlay(remove)
        chip.halign = :start
        remove.clicked_signal.connect do
          @attachments.delete(attachment)
          @attachments_bar.remove(chip)
          @attachments_bar.visible = !@attachments.empty?
        end

        @attachments_bar.append(chip)
        @attachments_bar.visible = true
      end

      private def clear_attachments : Nil
        @attachments.clear
        clear(@attachments_bar)
        @attachments_bar.visible = false
      end

      private def cancel_turn : Nil
        chat_id = @active_chat
        return unless chat_id
        return if @cancel_pending

        endpoint = @client
        @cancel_pending = true
        @status.text = "Stopping…"
        update_send_button

        call_async(endpoint, {
          "op"   => JSON::Any.new("cancel"),
          "chat" => JSON::Any.new(chat_id),
        }) do |_response, error|
          if error && @client.same?(endpoint) &&
             @active_chat == chat_id
            @cancel_pending = false
            @status.text = error
            update_send_button
          end
        end
      end

      private def set_option(option : String, value : String?) : Nil
        chat_id = @active_chat
        return unless chat_id

        request = {
          "op"     => JSON::Any.new("set-option"),
          "chat"   => JSON::Any.new(chat_id),
          "option" => JSON::Any.new(option),
        }
        request["value"] = JSON::Any.new(value) if value
        endpoint = @client
        call_async(endpoint, request) do |response, error|
          if error
            @status.text = error if @client.same?(endpoint)
          elsif response && @client.same?(endpoint) &&
                @active_chat == chat_id
            load_chat_state
          end
        end
      end

      private def set_model(backend : String, model : String) : Nil
        chat_id = @active_chat
        return unless chat_id

        endpoint = @client
        call_async(endpoint, {
          "op"      => JSON::Any.new("set-option"),
          "chat"    => JSON::Any.new(chat_id),
          "option"  => JSON::Any.new("model"),
          "backend" => JSON::Any.new(backend),
          "value"   => JSON::Any.new(model),
        }) do |response, error|
          if error
            @status.text = error if @client.same?(endpoint)
          elsif response && @client.same?(endpoint) &&
                @active_chat == chat_id
            load_chat_state
          end
        end
      end

      private def monotonic_microseconds : Int64
        ((Time.instant - @clock_origin).total_seconds * 1_000_000).to_i64
      end

      private def reset_stream_segment : Nil
        unless @stream_render_timer == 0
          GLib.source_remove(@stream_render_timer)
          @stream_render_timer = 0_u32
        end
        @stream_row = nil
        @stream_buffer = ""
        @stream_reveal.reset
      end

      private def finish_stream_segment : Nil
        unless @stream_render_timer == 0
          GLib.source_remove(@stream_render_timer)
          @stream_render_timer = 0_u32
        end

        unless @stream_buffer.empty?
          row = @stream_row || add_message("assistant", "")
          if row
            row.source = @stream_source
            row.set_text(@stream_buffer)
          end
          keep_working_last
          scroll_to_bottom
        end
        @stream_row = nil
        @stream_buffer = ""
        @stream_reveal.reset
      end

      private def schedule_stream_text(text : String) : Nil
        @stream_buffer += text
        @stream_reveal.note_append(monotonic_microseconds)
        return unless @stream_render_timer == 0

        @stream_render_timer = GLib.timeout(
          TextReveal::FRAME_MILLISECONDS.milliseconds
        ) do
          render_stream_text
        end
      end

      private def render_stream_text : Bool
        if @stream_buffer.empty?
          @stream_render_timer = 0_u32
          return false
        end

        frame = @stream_reveal.advance(
          @stream_buffer,
          monotonic_microseconds
        )
        if frame.shown > 0
          row = @stream_row
          unless row
            row = add_message("assistant", "")
            @stream_row = row
            row.try { |created| created.source = @stream_source }
          end
          if row
            row.set_stream_text(
              TextReveal.prefix(@stream_buffer, frame.shown)
            )
          end
          keep_working_last
          scroll_to_bottom
        end

        if frame.caught_up
          # A token pause is not the end of the response. Keep one plain label
          # alive so the next delta never tears down a Markdown widget tree.
          @stream_row.try(&.set_stream_text(@stream_buffer))
          @stream_render_timer = 0_u32
          false
        else
          true
        end
      end

      private def working_seconds : Int64
        started_at = @working_started_at
        return 0_i64 unless started_at

        Math.max(
          (Time.instant - started_at).total_seconds.to_i64,
          0_i64
        )
      end

      private def update_working_label : Nil
        @working_label.try do |label|
          label.text = TurnTiming.format("Working", working_seconds)
        end
      end

      private def set_working_animation(animated : Bool) : Nil
        return unless @working_label

        if animated
          update_working_label
          # Safe mode suppresses continuous decoration redraws, but the
          # elapsed label still needs its low-frequency clock tick.
          if @working_timer == 0
            @working_timer = GLib.timeout(1.second) do
              if @closed
                @working_timer = 0_u32
                false
              else
                update_working_label
                true
              end
            end
          end
        elsif @working_timer != 0
          GLib.source_remove(@working_timer)
          @working_timer = 0_u32
        end

        @working_dots.try do |dots|
          dots.animated = animated && !RENDER_SAFE_MODE
        end
      end

      private def remove_working_row(reset_started_at = true) : Nil
        unless @working_timer == 0
          GLib.source_remove(@working_timer)
          @working_timer = 0_u32
        end
        @working_label = nil
        @working_dots = nil
        if row = @working_row
          if parent = row.parent.as?(Gtk::Box)
            parent.remove(row)
          end
        end
        @working_row = nil
        @working_started_at = nil if reset_started_at
      end

      private def set_working(working : Bool) : Nil
        @working = working
        @waiting_for_input = false if working
        @cancel_pending = false unless working
        update_send_button
        update_presence
        unless working
          remove_working_row
          return
        end

        @working_started_at ||= Time.instant
        if @working_row
          update_working_label if @follow_bottom
          keep_working_last
          return
        end

        dots = Dots.new
        label = Gtk::Label.new("Working for 0s")
        label.xalign = 0_f32
        label.add_css_class("caption")
        label.add_css_class("dim-label")
        dots.widget.add_css_class("caption")
        dots.widget.add_css_class("dim-label")

        row = Gtk::Box.new(:horizontal, 4)
        row.halign = :start
        row.margin_start = 24
        row.margin_top = 6
        row.append(label)
        row.append(dots.widget)
        @transcript.append(row)

        @working_row = row
        @working_label = label
        @working_dots = dots
        set_working_animation(@follow_bottom)
      end

      private def keep_working_last : Nil
        row = @working_row
        return unless row
        parent = row.parent
        return unless parent &&
                      parent.to_unsafe == @transcript.to_unsafe
        last = @transcript.last_child
        return if last && last.to_unsafe == row.to_unsafe

        @transcript.reorder_child_after(row, last)
      end

      private def reset_live_turn_ui : Nil
        cancel_turn_recovery
        finish_stream_segment
        remove_working_row
        @working = false
        @waiting_for_input = false
        update_send_button
        update_presence
        @stream_source = nil
        @live_turn_key = nil
      end

      private def recover_active_turn(
        state : Hash(String, JSON::Any),
        request : Int64,
      ) : Nil
        page = @transcript_page
        return unless page
        return if @live_turn_key == page.key

        @stream_source = state["label"]?.try(&.as_s?)
        elapsed = state["working_for"]?.try(&.as_i64?) || 0_i64
        @working_started_at = Time.instant - Math.max(elapsed, 0_i64).seconds

        items = state["items"]?.try(&.as_a?) || [] of JSON::Any
        @live_turn_key = page.key
        turn_id = state["turn_id"]?.try(&.as_i64?)
        sequence = state["turn_sequence"]?.try(&.as_i64?)
        batch = TranscriptBatch(JSON::Any).new(items)
        GLib.idle_add do
          active = !@closed &&
                   @turn_recovery_request == request &&
                   @transcript_page.same?(page) &&
                   @client.same?(page.endpoint) &&
                   @active_chat == page.chat_id
          unless active
            next false
          end

          batch.next_batch.each do |_index, node|
            item = node.as_h
            text = item["text"]?.try(&.as_s?) || ""
            if item["tool"]?.try(&.as_bool?) == true
              add_message("tool", text)
            elsif !text.empty?
              row = add_message("assistant", text)
              row.try { |message| message.source = @stream_source }
            end
          end
          keep_working_last

          unless batch.done?
            next true
          end

          segment = state["segment"]?.try(&.as_s?) || ""
          unless segment.empty?
            @stream_buffer = segment
            @stream_reveal.sync(segment)
            @stream_row = add_message("assistant", "")
            @stream_row.try do |message|
              message.source = @stream_source
              message.set_stream_text(segment)
            end
          end

          keep_working_last
          scroll_to_bottom
          finish_turn_recovery(request, turn_id, sequence)
          false
        end
      end

      private def queue_turn_recovery(
        state : Hash(String, JSON::Any),
        request : Int64,
      ) : Nil
        return unless @turn_recovery_request == request

        @pending_turn_recovery = state
        @pending_turn_recovery_finish = nil
        resume_turn_recovery
      end

      private def resume_turn_recovery : Nil
        return if @history_request
        request = @turn_recovery_request || return
        if finished = @pending_turn_recovery_finish
          @pending_turn_recovery_finish = nil
          drain_turn_recovery_events(request, finished[0], finished[1])
          return
        end
        if watermark = @turn_recovery_watermark
          drain_turn_recovery_events(request, watermark[0], watermark[1])
          return
        end
        state = @pending_turn_recovery || return

        @pending_turn_recovery = nil
        recover_active_turn(state, request)
      end

      private def finish_turn_recovery(
        request : Int64,
        turn_id : Int64? = nil,
        sequence : Int64? = nil,
      ) : Nil
        return unless @turn_recovery_request == request

        @pending_turn_recovery = nil
        if @history_request
          @pending_turn_recovery_finish = {turn_id, sequence}
          return
        end
        @turn_recovery_watermark = {turn_id, sequence}
        drain_turn_recovery_events(request, turn_id, sequence)
      end

      private def drain_turn_recovery_events(
        request : Int64,
        turn_id : Int64?,
        sequence : Int64?,
      ) : Nil
        GLib.idle_add do
          next false unless @turn_recovery_request == request

          TURN_RECOVERY_EVENT_BATCH.times do
            pending = @turn_recovery_events.shift?
            break unless pending

            endpoint, event = pending
            next if event["event"]?.try(&.as_s?) == "changed"
            if replay = TurnRecovery.replay(event, turn_id, sequence)
              handle_event(endpoint, replay, bypass_recovery: true)
            end
            break if @history_request
          end

          if @history_request
            next false
          end

          if @turn_recovery_events.empty?
            reload_transcript = @transcript_reload_after_recovery
            reload_state = @state_reload_after_recovery
            @turn_recovery_request = nil
            @turn_recovery_watermark = nil
            @transcript_reload_after_recovery = false
            @state_reload_after_recovery = false
            if reload_transcript
              load_messages
              load_chat_state
            elsif reload_state
              load_chat_state(recover_turn: false)
            end
            false
          else
            true
          end
        end
      end

      private def cancel_turn_recovery : Nil
        @turn_recovery_request = nil
        @pending_turn_recovery = nil
        @pending_turn_recovery_finish = nil
        @turn_recovery_watermark = nil
        @state_reload_after_recovery = false
        @transcript_reload_after_recovery = false
        @turn_recovery_events.clear
      end

      private def load_chat_state(recover_turn : Bool = true) : Nil
        if @turn_recovery_request
          @state_reload_after_recovery = true
          return
        end
        chat_id = @active_chat
        return unless chat_id
        endpoint = @client
        @state_request += 1
        request = @state_request
        if recover_turn && !@turn_recovery_request &&
           @transcript_page &&
           @live_turn_key != @transcript_page.try(&.key)
          @turn_recovery_request = request
          @pending_turn_recovery = nil
          @pending_turn_recovery_finish = nil
          @turn_recovery_watermark = nil
        end
        call_async(endpoint, {
          "op"   => JSON::Any.new("chat"),
          "chat" => JSON::Any.new(chat_id),
        }) do |state, error|
          next unless request == @state_request &&
                      @client.same?(endpoint) &&
                      @active_chat == chat_id
          if error
            @status.text = error
            finish_turn_recovery(request) if recover_turn
            next
          end
          apply_chat_state(state.not_nil!, recover_turn, request)
        end
      end

      private def apply_chat_state(
        state : Hash(String, JSON::Any),
        recover_turn : Bool,
        request : Int64,
      ) : Nil
        @controls.update(state)
        @chat_backend = state["backend"]?.try(&.as_s?) || "claude"
        @auth_state = state["auth_state"]?.try(&.as_s?) || "unknown"
        update_auth_controls(state["auth_detail"]?.try(&.as_s?))
        if context = state["context"]?.try(&.as_s?)
          @context_label.text = context
          @context_label.tooltip_text = context
        end
        @commands = CommandSuggestions.normalize(
          state["commands"]?.try(&.as_a?) || [] of JSON::Any
        )
        refresh_command_suggestions
        working = state["working"]?.try(&.as_bool?) || false
        if working
          queue_turn_recovery(state, request) if recover_turn
          set_working(true)
          keep_working_last
        else
          @live_turn_key = nil
          set_working(false)
          finish_turn_recovery(request) if recover_turn
        end
        update_send_button
        queue = state["queue"]?.try(&.as_a?) || [] of JSON::Any
        render_queue(queue)
      end

      private def update_send_button : Nil
        if @working
          @send.icon_name = "media-playback-stop-symbolic"
          @send.tooltip_text = @cancel_pending ? "Stopping…" : "Stop"
          @send.remove_css_class("suggested-action")
          @send.add_css_class("destructive-action")
        else
          @send.icon_name = "go-up-symbolic"
          @send.tooltip_text = "Send (Enter)"
          @send.remove_css_class("destructive-action")
          @send.add_css_class("suggested-action")
        end
        @send.sensitive = (@working && !@cancel_pending) ||
                          (@auth_state == "signed-in" && !@send_pending)
      end

      private def update_auth_controls(detail : String? = nil) : Nil
        signed_in = @auth_state == "signed-in"
        @entry.sensitive = signed_in
        @attach.sensitive = signed_in
        @voice.button.sensitive = signed_in
        @auth_button.visible = !signed_in && !!@active_chat
        @auth_status.text = case @auth_state
                            when "signed-in"
                              ""
                            when "checking", "unknown"
                              "Checking sign-in…"
                            when "signing-in"
                              "Finish signing in"
                            when "signing-out"
                              "Signing out…"
                            when "signed-out"
                              "Sign in to use this assistant"
                            else
                              detail || "Cannot verify sign-in"
                            end
        update_send_button
      end

      private def show_auth_dialog : Nil
        machine = unless @client.same?(@local_client)
          @remote.snapshot.host
        end
        AuthDialog.new(@widget, @client, machine).present
      end

      private def refresh_command_suggestions : Nil
        while child = @commands_flow.first_child
          @commands_flow.remove(child)
        end

        matches = CommandSuggestions.matches(
          @commands,
          @entry.buffer.text
        )
        matches.each do |command|
          selected = command
          button = Gtk::Button.new_with_label("/#{selected}")
          button.add_css_class("flat")
          button.halign = :fill
          button.clicked_signal.connect do
            @entry.buffer.text = "/#{selected} "
            @entry.buffer.place_cursor(@entry.buffer.end_iter)
            @entry.cursor_visible = true
            @entry.grab_focus
          end
          @commands_flow.append(button)
        end
        @commands_bar.visible = !matches.empty?
      end

      private def render_queue(queue : Array(JSON::Any)) : Nil
        plan = QueuePresentation.prepare(queue)
        @queue = plan.rows
        @queue_generation &+= 1
        generation = @queue_generation
        box = replace_queue_box(!plan.rows.empty? || plan.hidden > 0)
        index = 0

        GLib.idle_add do
          unless !@closed &&
                 generation == @queue_generation &&
                 @queue_box.same?(box)
            next false
          end

          finish = Math.min(index + QUEUE_RENDER_BATCH, plan.rows.size)
          while index < finish
            box.append(queue_row(index, plan.rows[index]))
            index += 1
          end
          next true unless index >= plan.rows.size

          if plan.hidden > 0
            label = Gtk::Label.new(
              "#{plan.hidden} more queued messages"
            )
            label.xalign = 0_f32
            label.margin_start = 30
            label.add_css_class("caption")
            label.add_css_class("dim-label")
            box.append(label)
          end
          false
        end
      end

      private def clear_queue : Nil
        @queue_generation &+= 1
        @queue.clear
        replace_queue_box(false)
      end

      private def new_queue_box : Gtk::Box
        box = Gtk::Box.new(:vertical, 2)
        box.margin_top = 6
        box.margin_start = 10
        box.margin_end = 6
        box.visible = false
        box
      end

      private def replace_queue_box(visible : Bool) : Gtk::Box
        previous = @queue_box
        box = new_queue_box
        box.visible = visible
        @queue_box = box
        @queue_host.remove(previous)
        @queue_host.append(box)
        retire_queue_box(previous)
        box
      end

      private def retire_queue_box(box : Gtk::Box) : Nil
        return unless box.first_child

        @retired_queue_boxes << box
        return if @queue_retirement_scheduled

        @queue_retirement_scheduled = true
        GLib.idle_add do
          QUEUE_RETIRE_BATCH.times do
            retired = @retired_queue_boxes.first?
            break unless retired

            if child = retired.first_child
              retired.remove(child)
            else
              @retired_queue_boxes.shift
            end
          end
          while retired = @retired_queue_boxes.first?
            break if retired.first_child
            @retired_queue_boxes.shift
          end
          more = !@retired_queue_boxes.empty? && !@closed
          @queue_retirement_scheduled = more
          more
        end
      end

      private def queue_row(index : Int, text : String) : Gtk::Box
        icon = Gtk::Image.new_from_icon_name("document-send-symbolic")
        label = Gtk::Label.new(text)
        label.xalign = 0_f32
        label.hexpand = true
        label.ellipsize = :end
        label.add_css_class("dim-label")

        edit = Gtk::Button.new_from_icon_name(
          "document-edit-symbolic"
        )
        edit.add_css_class("flat")
        edit.tooltip_text = "Edit queued message"

        steer = Gtk::Button.new_from_icon_name(
          "media-skip-forward-symbolic"
        )
        steer.add_css_class("flat")
        steer.tooltip_text = "Send this now, interrupting the agent"
        steer.clicked_signal.connect do
          steer_queue(index, text)
        end

        remove = Gtk::Button.new_from_icon_name(
          "window-close-symbolic"
        )
        remove.add_css_class("flat")
        remove.tooltip_text = "Discard"
        remove.clicked_signal.connect { drop_queue(index) }

        row = Gtk::Box.new(:horizontal, 6)
        edit.clicked_signal.connect do
          show_queue_editor(row, index, text)
        end
        row.append(icon)
        row.append(label)
        row.append(edit)
        row.append(steer)
        row.append(remove)
        row
      end

      private def show_queue_editor(
        row : Gtk::Box,
        index : Int,
        old_text : String,
      ) : Nil
        clear(row)
        icon = Gtk::Image.new_from_icon_name("document-send-symbolic")
        editor = Gtk::TextView.new
        editor.wrap_mode = :word_char
        editor.top_margin = 6
        editor.bottom_margin = 6
        editor.left_margin = 8
        editor.right_margin = 8
        editor.buffer.text = old_text
        editor.buffer.place_cursor(editor.buffer.end_iter)

        scroller = Gtk::ScrolledWindow.new
        scroller.set_policy(:never, :automatic)
        scroller.max_content_height = 120
        scroller.propagate_natural_height = true
        scroller.hexpand = true
        scroller.add_css_class("card")
        scroller.child = editor

        save = Gtk::Button.new_from_icon_name("document-save-symbolic")
        save.add_css_class("flat")
        save.tooltip_text = "Save queued message"
        save.clicked_signal.connect do
          save_queue_editor(index, old_text, editor)
        end

        cancel = Gtk::Button.new_from_icon_name(
          "window-close-symbolic"
        )
        cancel.add_css_class("flat")
        cancel.tooltip_text = "Cancel editing"
        cancel.clicked_signal.connect { load_chat_state }

        keys = Gtk::EventControllerKey.new
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if keyval == Gdk::KEY_Escape
            load_chat_state
            true
          elsif (keyval == Gdk::KEY_Return ||
                keyval == Gdk::KEY_KP_Enter) &&
                !state.includes?(Gdk::ModifierType::ShiftMask)
            save_queue_editor(index, old_text, editor)
            true
          else
            false
          end
        end
        editor.add_controller(keys)

        row.append(icon)
        row.append(scroller)
        row.append(save)
        row.append(cancel)
        editor.grab_focus
      end

      private def save_queue_editor(
        index : Int,
        old_text : String,
        editor : Gtk::TextView,
      ) : Nil
        text = editor.buffer.text.strip
        return if text.empty?
        return load_chat_state if text == old_text

        edit_queue(index, old_text, text)
      end

      private def steer_queue(index : Int, text : String) : Nil
        chat_id = @active_chat
        return unless chat_id
        endpoint = @client
        call_async(endpoint, {
          "op"    => JSON::Any.new("steer-queue"),
          "chat"  => JSON::Any.new(chat_id),
          "index" => JSON::Any.new(index.to_i64),
          "text"  => JSON::Any.new(text),
        }) do |_response, error|
          @status.text = error if error && @client.same?(endpoint)
        end
      end

      private def drop_queue(index : Int) : Nil
        chat_id = @active_chat
        return unless chat_id
        endpoint = @client
        call_async(endpoint, {
          "op"    => JSON::Any.new("drop-queue"),
          "chat"  => JSON::Any.new(chat_id),
          "index" => JSON::Any.new(index.to_i64),
        }) do |response, error|
          if error
            @status.text = error if @client.same?(endpoint)
          elsif response && @client.same?(endpoint) &&
                @active_chat == chat_id
            load_chat_state(recover_turn: false)
          end
        end
      end

      private def edit_queue(
        index : Int,
        old_text : String,
        text : String,
      ) : Nil
        chat_id = @active_chat
        return unless chat_id
        endpoint = @client
        call_async(endpoint, {
          "op"       => JSON::Any.new("edit-queue"),
          "chat"     => JSON::Any.new(chat_id),
          "index"    => JSON::Any.new(index.to_i64),
          "old-text" => JSON::Any.new(old_text),
          "text"     => JSON::Any.new(text),
        }) do |response, error|
          if error
            @status.text = error if @client.same?(endpoint)
          elsif response && @client.same?(endpoint) &&
                @active_chat == chat_id
            load_chat_state(recover_turn: false)
          end
        end
      end

      private def handle_event(
        endpoint : Daemon::Endpoint,
        event : Hash(String, JSON::Any),
        bypass_recovery : Bool = false,
      ) : Nil
        if @client.same?(endpoint)
          @tool_panel.handle_event(event)
          @git_actions.handle_event(event)
        end
        name = event["event"]?.try(&.as_s?) || return
        @sidebar.handle_event(endpoint, event)
        if !bypass_recovery &&
           defer_turn_recovery_event?(endpoint, event, name)
          @transcript_reload_after_recovery = true if name == "changed"
          @turn_recovery_events.shift if @turn_recovery_events.size >= TURN_RECOVERY_EVENT_LIMIT
          @turn_recovery_events << {endpoint, event}
          return
        end
        case name
        when "tree"
          @sidebar.reload(endpoint)
        when "commands"
          return unless active_event?(endpoint, event)
          @commands = CommandSuggestions.normalize(
            event["commands"]?.try(&.as_a?) || [] of JSON::Any
          )
          refresh_command_suggestions
        when "text"
          return unless active_event?(endpoint, event)
          text = event["text"]?.try(&.as_s?) || return
          schedule_stream_text(text)
        when "tool"
          return unless active_event?(endpoint, event)
          finish_stream_segment
          if context = event["context"]?.try(&.as_s?)
            @context_label.text = context
            @context_label.tooltip_text = context
          end
          add_message("tool", event["text"]?.try(&.as_s?) || "Used a tool")
          keep_working_last
          scroll_to_bottom
        when "turn-started"
          return unless active_event?(endpoint, event)
          @cancel_pending = false
          end_tool_group
          reset_stream_segment
          @live_turn_key = nil
          @stream_source = event["label"]?.try(&.as_s?)
          @working_started_at = Time.instant
          @working = true
          @waiting_for_input = false
          update_presence
          load_messages
          # This event attaches us to a new turn before its ordered deltas.
          # The state call below may already see those deltas in the daemon;
          # recovering that snapshot and then consuming the queued events
          # would draw the same live text twice.
          @live_turn_key = @transcript_page.try(&.key)
          load_chat_state(recover_turn: false)
        when "turn-finished"
          if active_event?(endpoint, event)
            end_tool_group
            @messages_request += 1
            finish_stream_segment
            set_working(false)
            if last_id = event["last_message_id"]?.try(&.as_i64?)
              if event["silent"]?.try(&.as_bool?) == true
                add_message("assistant", "(no reply)")
              end
              if seconds = event["duration"]?.try(&.as_i64?)
                append_worked_for(seconds)
              end
              if error = event["error"]?.try(&.as_s?)
                add_message("error", error)
              end
              if event["waiting"]?.try(&.as_bool?) == true
                options = (
                  event["options"]?.try(&.as_a?) || [] of JSON::Any
                ).compact_map(&.as_s?)
                question = event["question"]?.try(&.as_s?) ||
                           "Which one?"
                accepts_input =
                  event["accepts_input"]?.try(&.as_bool?) || false
                append_ask(Agent::Ask.new(
                  question,
                  options,
                  accepts_input
                ))
              end
              @transcript_page.try(&.revision=(last_id))
              keep_working_last
              scroll_to_bottom
            else
              # Compatibility with daemons predating finish metadata.
              load_messages
            end
            # A queued turn may already be running in the daemon while its
            # ordered start/text events are still waiting in this GTK idle
            # queue. Refresh controls and queue, but let those events attach
            # and draw it.
            load_chat_state(recover_turn: false)
            if event["waiting"]?.try(&.as_bool?) == true
              @status.text = "Waiting for your answer"
            end
          end
        when "changed"
          if active_event?(endpoint, event)
            load_messages
            load_chat_state
          end
        when "repository-changed"
          load_chat_state(recover_turn: false) if active_event?(endpoint, event)
        when "queued"
          return unless active_event?(endpoint, event)
          queue = event["queue"]?.try(&.as_a?)
          @status.text = queue && !queue.empty? ? "Message queued" : ""
          load_chat_state(recover_turn: false)
        when "agent-auth-changed"
          return unless @client.same?(endpoint)
          return unless event["provider"]?.try(&.as_s?) == @chat_backend
          load_chat_state(recover_turn: false)
        end
      end

      private def defer_turn_recovery_event?(
        endpoint : Daemon::Endpoint,
        event : Hash(String, JSON::Any),
        name : String,
      ) : Bool
        return false unless @turn_recovery_request
        return false unless active_event?(endpoint, event)

        case name
        when "turn-started", "text", "tool", "turn-finished", "changed"
          true
        else
          false
        end
      end

      private def active_event?(
        endpoint : Daemon::Endpoint,
        event : Hash(String, JSON::Any),
      ) : Bool
        @client.same?(endpoint) &&
          event["chat"]?.try(&.as_s?) == @active_chat
      end

      private def clear(box : Gtk::Box) : Nil
        while child = box.first_child
          box.remove(child)
        end
      end

      private def install_transcript_scrolling : Nil
        adjustment = @transcript_scroll.vadjustment
        adjustment.changed_signal.connect do
          on_scroll_adjustment_changed(adjustment)
        end
        adjustment.value_changed_signal.connect do
          on_scroll_adjustment_changed(adjustment)
        end

        scroll = Gtk::EventControllerScroll.new(
          Gtk::EventControllerScrollFlags::Vertical
        )
        scroll.propagation_phase = Gtk::PropagationPhase::Capture
        scroll.scroll_signal.connect do |_dx, dy|
          on_transcript_scrolled(dy)
          false
        end
        @transcript_scroll.add_controller(scroll)
      end

      private def set_scroll_at_bottom(
        adjustment = @transcript_scroll.vadjustment,
      ) : Nil
        bottom = Math.max(
          adjustment.lower,
          adjustment.upper - adjustment.page_size
        )
        adjustment.value = bottom if adjustment.value != bottom
      end

      private def queue_bottom_pin : Nil
        return unless @bottom_pin_tick == 0

        callback = ->(_widget : Gtk::Widget, _clock : Gdk::FrameClock) {
          @bottom_pin_tick = 0_u32
          set_scroll_at_bottom if @follow_bottom
          false
        }
        @bottom_pin_tick =
          @transcript_scroll.add_tick_callback(callback)
      end

      private def begin_bottom_jump : Nil
        @follow_bottom = true
        @history_bottom_distance = -1.0
        set_working_animation(true)
        @bottom_jump_upper = -1.0
        @bottom_jump_page_size = -1.0
        @bottom_jump_stable_frames = 0
        @transcript_scroll.opacity = 0.0
        return unless @bottom_jump_tick == 0

        callback = ->(_widget : Gtk::Widget, _clock : Gdk::FrameClock) {
          adjustment = @transcript_scroll.vadjustment
          upper = adjustment.upper
          page_size = adjustment.page_size
          set_scroll_at_bottom(adjustment)

          if upper == @bottom_jump_upper &&
             page_size == @bottom_jump_page_size
            @bottom_jump_stable_frames += 1
          else
            @bottom_jump_stable_frames = 0
          end
          @bottom_jump_upper = upper
          @bottom_jump_page_size = page_size

          if @bottom_jump_stable_frames >= 2
            @bottom_jump_tick = 0_u32
            @transcript_scroll.queue_draw
            @transcript_scroll.opacity = 1.0
            false
          else
            true
          end
        }
        @bottom_jump_tick =
          @transcript_scroll.add_tick_callback(callback)
      end

      private def on_scroll_adjustment_changed(
        adjustment : Gtk::Adjustment,
      ) : Nil
        bottom = Math.max(
          adjustment.lower,
          adjustment.upper - adjustment.page_size
        )

        if @follow_bottom
          queue_bottom_pin
        elsif @history_bottom_distance >= 0
          value = Math.max(
            adjustment.lower,
            adjustment.upper - @history_bottom_distance
          )
          adjustment.value = value if adjustment.value != value
        elsif adjustment.value >= bottom - 1.0
          @follow_bottom = true
          @history_bottom_distance = -1.0
          set_working_animation(true)
        end
      end

      private def on_transcript_scrolled(dy : Float64) : Nil
        adjustment = @transcript_scroll.vadjustment
        cancel_history_restore if dy != 0
        if dy < 0 && adjustment.value > adjustment.lower
          @follow_bottom = false
          @history_bottom_distance = -1.0
          set_working_animation(false)
        end
      end

      private def queue_history_restore : Nil
        @history_restore_upper = -1.0
        @history_restore_page_size = -1.0
        @history_restore_stable_frames = 0
        return unless @history_restore_tick == 0

        callback = ->(_widget : Gtk::Widget, _clock : Gdk::FrameClock) {
          adjustment = @transcript_scroll.vadjustment
          distance = @history_bottom_distance
          if @follow_bottom || distance < 0
            @history_restore_tick = 0_u32
            false
          else
            upper = adjustment.upper
            page_size = adjustment.page_size
            value = Math.max(adjustment.lower, upper - distance)
            value_held = adjustment.value == value
            adjustment.value = value unless value_held

            if upper == @history_restore_upper &&
               page_size == @history_restore_page_size &&
               value_held
              @history_restore_stable_frames += 1
            else
              @history_restore_stable_frames = 0
            end
            @history_restore_upper = upper
            @history_restore_page_size = page_size

            if @history_restore_stable_frames >= 2
              @history_restore_tick = 0_u32
              @history_bottom_distance = -1.0
              bottom = Math.max(
                adjustment.lower,
                upper - page_size
              )
              nudge = if value < bottom
                        Math.min(value + 1.0, bottom)
                      else
                        Math.max(value - 1.0, adjustment.lower)
                      end
              adjustment.value = nudge if nudge != value
              adjustment.value = value
              @transcript_scroll.queue_draw
              @transcript_scroll.opacity = 1.0
              false
            else
              true
            end
          end
        }
        @history_restore_tick =
          @transcript_scroll.add_tick_callback(callback)
      end

      private def cancel_history_restore : Nil
        @history_bottom_distance = -1.0
        @transcript_scroll.opacity = 1.0
        return if @history_restore_tick == 0

        @transcript_scroll.remove_tick_callback(@history_restore_tick)
        @history_restore_tick = 0_u32
      end

      private def scroll_to_bottom : Nil
        return unless @follow_bottom

        @history_bottom_distance = -1.0
        set_scroll_at_bottom
        queue_bottom_pin
      end
    end
  end
end
