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
require "../remote/connection"
require "../version"
require "./adw"
require "./chat_controls"
require "./message_row"
require "./pair_dialog"
require "./pane_state"
require "./search_dialog"
require "./sidebar"
require "./tool_panel"
require "./transcript_paging"

module Xd
  module UI
    class Window
      class TranscriptPage
        getter key : String
        getter chat_id : String
        getter endpoint : Daemon::Endpoint
        getter transcript : Gtk::Box
        getter paging = TranscriptPaging.new
        getter workflow_ids = Set(String).new
        property revision = -1_i64
        property choices_visible = false

        def initialize(
          @key : String,
          @chat_id : String,
          @endpoint : Daemon::Endpoint,
          @transcript : Gtk::Box,
        )
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

      MAX_IMAGES      = 4
      MAX_IMAGE_BYTES = 10 * 1024 * 1024
      MAX_TOTAL_BYTES = 20 * 1024 * 1024

      getter widget : Adw::ApplicationWindow

      @active_chat : String?
      @stream_row : MessageRow?
      @working = false
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

      def initialize(
        application : Gtk::Application,
        local_client : Daemon::Endpoint,
        @remote : Remote::Connection,
      )
        @local_client = local_client
        @client = local_client
        @active_chat = nil
        @stream_row = nil
        @search_dialog = nil
        @settings = Gio::Settings.new(APP_ID)
        @widget = Adw::ApplicationWindow.new(application: application)
        @widget.title = "xd"
        @widget.set_default_size(
          @settings.int("window-width"),
          @settings.int("window-height")
        )
        @widget.maximize if @settings.boolean("window-maximized")

        @status = Gtk::Label.new("")
        @status.xalign = 0_f32
        @status.hexpand = true
        @status.add_css_class("dim-label")

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
          ->(request : Hash(String, JSON::Any)) { call(request) },
          -> { close_terminal_panel }
        )

        @chat_title = Adw::WindowTitle.new(title: "xd")
        chat_header = Adw::HeaderBar.new
        chat_header.title_widget = @chat_title
        chat_header.show_start_title_buttons = false
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
        chat_header.pack_end(@terminal_button)
        chat_header.pack_end(@file_button)
        chat_header.pack_end(@diff_button)

        @controls = ChatControls.new(
          ->(option : String, value : String?) {
            set_option(option, value)
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

        @attach = Gtk::Button.new_from_icon_name(
          "mail-attachment-symbolic"
        )
        @attach.sensitive = false
        @attach.tooltip_text = "Attach images"
        @attach.add_css_class("flat")
        @attach.clicked_signal.connect { choose_images }

        @send = Gtk::Button.new_from_icon_name("go-up-symbolic")
        @send.sensitive = false
        @send.add_css_class("suggested-action")
        @send.add_css_class("circular")
        @send.tooltip_text = "Send (Enter)"
        @send.clicked_signal.connect do
          @working ? cancel_turn : send_message
        end

        @queue_box = Gtk::Box.new(:vertical, 4)
        @queue_box.margin_start = 18
        @queue_box.margin_end = 18
        @queue_box.add_css_class("xd-queue")
        @queue_box.visible = false

        @attachments_bar = Gtk::Box.new(:horizontal, 6)
        @attachments_bar.margin_start = 18
        @attachments_bar.margin_end = 18
        @attachments_bar.margin_top = 8
        @attachments_bar.add_css_class("xd-attachments")
        @attachments_bar.visible = false

        filler = Gtk::Box.new(:horizontal, 0)
        filler.hexpand = true
        @controls.run.append(filler)
        @controls.run.append(@attach)
        @controls.run.append(@send)

        entry_scroll = Gtk::ScrolledWindow.new
        entry_scroll.set_policy(:never, :automatic)
        entry_scroll.max_content_height = 180
        entry_scroll.propagate_natural_height = true
        entry_scroll.child = @entry

        composer_column = Gtk::Box.new(:vertical, 0)
        composer_column.append(@queue_box)
        composer_column.append(@attachments_bar)
        composer_column.append(entry_scroll)
        composer_column.append(@controls.widget)
        composer_frame = Gtk::Frame.new
        composer_frame.child = composer_column
        composer_frame.margin_top = 6
        composer_frame.margin_start = 12
        composer_frame.margin_end = 12

        context = Gtk::Box.new(:horizontal, 8)
        context.append(@status)
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
          persist_window_layout
          false
        end
        install_window_actions(application)

        subscribe(@local_client)
        subscribe(@remote)
        @remote.on_state do |snapshot|
          GLib.idle_add do
            @tool_panel.remote_connection_changed(
              snapshot.state.connected?,
              snapshot.error
            )
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
          ->(request : Hash(String, JSON::Any)) {
            call_on(endpoint, request)
          },
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
          @widget.is_maximized
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
        endpoint.subscribe do |event|
          GLib.idle_add do
            handle_event(endpoint, event)
            false
          end
        end
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

      private def call(fields : Hash(String, JSON::Any))
        call_on(@client, fields)
      end

      private def call_on(
        endpoint : Daemon::Endpoint,
        fields : Hash(String, JSON::Any),
      ) : Hash(String, JSON::Any)?
        @status.text = ""
        endpoint.call(fields)
      rescue error : Daemon::Client::Error
        @status.text = error.message || "Daemon request failed."
        nil
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
        @transcript_stack.remove(page.transcript)
      end

      private def current_transcript_cacheable? : Bool
        page = @transcript_page
        return false unless page
        return true unless @client.same?(@local_client)

        !@working && !page.choices_visible
      end

      private def leave_current_transcript(keep : Bool) : Nil
        page = @transcript_page
        return unless page

        page.revision = -1_i64 if @working
        @transcript_lru.touch(page.key)
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
          leave_current_transcript(current_transcript_cacheable?)
          @follow_bottom = true
          @history_bottom_distance = -1.0
        end
        remember_panes
        hide_panes_for_switch
        clear_attachments
        @client = endpoint
        @active_chat = id
        @sidebar.activate_chat(endpoint, id)
        prefix = endpoint.same?(@local_client) ? "local:" : "remote:"
        @settings.set_string("active-chat", "#{prefix}#{id}")
        @stream_row = nil
        @chat_title.title = title
        @chat_stack.visible_child_name = "chat"
        @composer.visible = true
        @entry.sensitive = true
        @attach.sensitive = true
        @send.sensitive = true
        @controls.sensitive = true
        @terminal_button.sensitive = true
        @file_button.sensitive = true
        @diff_button.sensitive = true
        @tool_panel.select_chat(id, pane_key)
        apply_panes(saved_panes)
        if changed
          activate_transcript_page(endpoint, id)
          begin_bottom_jump
        end
        load_chat_state
        load_messages
        @entry.grab_focus
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
        leave_current_transcript(false)
        @active_chat = nil
        @sidebar.clear_active_chat
        @settings.set_string("active-chat", "")
        @stream_row = nil
        @working = false
        @chat_title.title = "xd"
        @chat_stack.visible_child_name = "empty"
        @composer.visible = false
        @entry.buffer.text = ""
        @entry.sensitive = false
        @attach.sensitive = false
        @send.label = "Send"
        @send.sensitive = false
        @send.remove_css_class("destructive-action")
        @send.add_css_class("suggested-action")
        @controls.sensitive = false
        @terminal_button.sensitive = false
        @file_button.sensitive = false
        @diff_button.sensitive = false
        @tool_panel.select_chat(nil, nil)
        clear(@queue_box)
        @queue_box.visible = false
        clear_attachments
        @status.text = ""
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

        response = call({
          "op"    => JSON::Any.new("messages"),
          "chat"  => JSON::Any.new(chat_id),
          "limit" => JSON::Any.new(page.paging.query_limit.to_i64),
        })
        return unless response
        return unless @transcript_page.same?(page)

        revision = response["last_message_id"]?.try(&.as_i64?) || 0_i64
        return if !force && page.revision == revision

        if @follow_bottom
          begin_bottom_jump
        end
        page.choices_visible = false
        clear(@transcript)
        @workflow_ids.clear
        messages = response["messages"]?.try(&.as_a?) || [] of JSON::Any
        total = response["total_messages"]?.try(&.as_i64?) ||
                messages.size.to_i64
        append_history_button(page, total, messages.size)
        start = page.paging.start(messages.size)
        (start...messages.size).each do |index|
          message = messages[index]
          add_message(
            message["role"].as_s,
            message["content"].as_s,
            message["label"]?.try(&.as_s?),
            reply_answerable?(messages, index)
          )
        end
        page.revision = revision
        @stream_row = nil
        scroll_to_bottom
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

      private def load_earlier_messages(page : TranscriptPage) : Nil
        return unless @transcript_page.same?(page)

        adjustment = @transcript_scroll.vadjustment
        @follow_bottom = false
        @history_bottom_distance =
          adjustment.upper - adjustment.value
        page.paging.load_earlier
        load_messages(force: true)
        queue_history_restore
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
            add_diff_message(patch)
            return
          end
          if workflow = Agent::WorkflowRun.parse(content)
            add_workflow_message(workflow)
            return
          end
          if subagent = Agent::SubagentTool.parse(content)
            add_subagent_message(subagent[0], subagent[1])
            return
          end
          content = "Files changed" if Agent::GitDiffTracker.file_change?(content)
        end

        images = role == "assistant" ? nil : Agent::ImageReference.parse(content)
        content = images.remainder if images
        workspace = role == "assistant" ? Agent::WorkspaceBlock.parse(content) : nil
        assistant_text = workspace.try(&.remainder) || content
        parsed = role == "assistant" ? Agent::Ask.parse(assistant_text) : nil
        shown = if parsed
                  [parsed.remainder, parsed.ask.question]
                    .reject(&.empty?).join("\n\n")
                else
                  assistant_text
                end
        row = MessageRow.new(MessageKind.from_role(role), shown)
        row.source = label
        @transcript.append(row.widget)
        append_message_images(images.paths) if images
        append_ask(parsed.ask) if parsed && answerable
        row
      end

      private def add_diff_message(patch : String) : Gtk::Label
        row = Gtk::Label.new("Files changed\n#{patch}")
        row.xalign = 0_f32
        row.wrap = false
        row.selectable = true
        row.add_css_class("xd-body")
        row.add_css_class("xd-message")
        row.add_css_class("xd-message-diff")
        @transcript.append(row)
        row
      end

      private def add_workflow_message(
        workflow : Agent::WorkflowRun::Run,
      ) : Gtk::Label?
        return if @workflow_ids.includes?(workflow.id)
        @workflow_ids << workflow.id

        title = Gtk::Label.new(
          "GitHub Actions · Run ##{workflow.id}"
        )
        title.xalign = 0_f32
        title.add_css_class("title")

        status = Gtk::Label.new(workflow.repository)
        status.xalign = 0_f32
        status.add_css_class("dim-label")

        link = Gtk::LinkButton.new_with_label(
          workflow.url,
          "Open live status and logs"
        )
        link.halign = :start

        card = Gtk::Box.new(:vertical, 5)
        card.add_css_class("xd-workflow")
        card.append(title)
        card.append(status)
        card.append(link)
        @transcript.append(card)
        title
      end

      private def add_subagent_message(
        identity : String,
        task : String,
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
        card.append(title)
        card.append(detail)
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
        if page = @transcript_page
          page.choices_visible = true
        end
        choices = Gtk::Box.new(:vertical, 5)
        choices.add_css_class("xd-ask")

        ask.options.each do |option|
          answer = option
          button = Gtk::Button.new_with_label(answer)
          button.hexpand = true
          button.halign = :fill
          button.add_css_class("xd-choice")
          button.clicked_signal.connect { answer_ask(answer) }
          choices.append(button)
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
          choices.append(row)
        end

        @transcript.append(choices)
      end

      private def answer_ask(answer : String) : Nil
        text = answer.strip
        return if text.empty?

        @entry.buffer.text = text
        send_message
      end

      private def send_message : Nil
        chat_id = @active_chat
        return unless chat_id
        text = @entry.buffer.text.strip
        return if text.empty? && @attachments.empty?
        @sidebar.answer_chat(@client, chat_id)

        request = {
          "op"   => JSON::Any.new("send"),
          "chat" => JSON::Any.new(chat_id),
          "text" => JSON::Any.new(text),
        }
        unless @attachments.empty?
          attachments = @attachments.map do |attachment|
            JSON::Any.new({
              "name" => JSON::Any.new(attachment.name),
              "mime" => JSON::Any.new("image/png"),
              "data" => JSON::Any.new(attachment.data),
            })
          end
          request["attachments"] = JSON::Any.new(attachments)
        end

        begin_bottom_jump
        response = call(request)
        if response
          @entry.buffer.text = ""
          clear_attachments
          if response["queued"]?.try(&.as_bool?) == true
            @status.text = "Message queued"
          end
          load_messages
          load_chat_state
        end
      end

      private def choose_images : Nil
        return unless @active_chat

        filter = Gtk::FileFilter.new
        filter.name = "Images"
        filter.add_mime_type("image/*")
        dialog = Gtk::FileDialog.new(
          title: "Attach images",
          modal: true,
          default_filter: filter
        )
        dialog.open_multiple(@widget, nil) do |source, result|
          begin
            files = source.as(Gtk::FileDialog)
              .open_multiple_finish(result)
            if files
              files.n_items.times do |index|
                object = files.item(index)
                add_file_attachment(object.as(Gio::File)) if object
              end
            end
          rescue Gio::IOErrorEnum::Cancelled
          rescue error
            @status.text = error.message || "Cannot attach that image."
          end
        end
      end

      private def add_file_attachment(file : Gio::File) : Nil
        path = file.path
        unless path
          @status.text = "Only local image files can be attached."
          return
        end
        info = File.info(path)
        if info.size > MAX_IMAGE_BYTES
          @status.text = "Each source image must be 10 MiB or smaller."
          return
        end

        texture = Gdk::Texture.new_from_file(file)
        add_attachment(
          file.basename.try(&.to_s) || "image.png",
          texture
        )
      rescue error
        @status.text = error.message || "Cannot attach that image."
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
              add_attachment(
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

      private def add_attachment(
        name : String,
        texture : Gdk::Texture,
      ) : Nil
        if @attachments.size >= MAX_IMAGES
          @status.text = "A message can contain at most 4 images."
          return
        end

        data = texture.save_to_png_bytes.data
        unless data
          @status.text = "Cannot encode that image as PNG."
          return
        end
        total = @attachments.sum(&.bytesize)
        if data.size > MAX_IMAGE_BYTES ||
           total > MAX_TOTAL_BYTES - data.size
          @status.text = "Attached images must stay under 20 MiB total."
          return
        end

        attachment = Attachment.new(
          File.basename(name),
          Base64.strict_encode(data),
          data.size,
          texture
        )
        @attachments << attachment
        append_attachment_chip(attachment)
        @status.text = ""
      end

      private def append_attachment_chip(
        attachment : Attachment,
      ) : Nil
        picture = Gtk::Picture.new_for_paintable(attachment.texture)
        picture.content_fit = :contain
        picture.can_shrink = true
        picture.set_size_request(168, 96)

        label = Gtk::Label.new(attachment.name)
        label.ellipsize = :middle
        label.max_width_chars = 18
        label.add_css_class("dim-label")

        card = Gtk::Box.new(:vertical, 4)
        card.append(picture)
        card.append(label)
        card.add_css_class("xd-attachment")

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

      private def append_message_images(paths : Array(String)) : Nil
        images = Gtk::Box.new(:horizontal, 8)
        images.add_css_class("xd-message-images")

        paths.each_with_index do |path, index|
          images.append(image_preview(path, index + 1))
        end
        @transcript.append(images)
      end

      private def image_preview(
        path : String,
        number : Int32,
      ) : Gtk::Widget
        response = fetch_image(path, true)
        texture = response.try { |body| texture_from(body) }

        content = if texture
                    picture = Gtk::Picture.new_for_paintable(texture)
                    picture.content_fit = :scale_down
                    picture.set_size_request(168, 96)
                    button = Gtk::Button.new
                    button.child = picture
                    button.add_css_class("flat")
                    button.tooltip_text = "Open image"
                    button.clicked_signal.connect do
                      open_image(path, number)
                    end
                    button
                  else
                    unavailable = Gtk::Label.new("Preview unavailable")
                    unavailable.add_css_class("dim-label")
                    unavailable
                  end

        label = Gtk::Label.new("Image ##{number}")
        label.xalign = 0_f32
        label.add_css_class("dim-label")

        card = Gtk::Box.new(:vertical, 4)
        card.add_css_class("xd-image-preview")
        card.append(content)
        card.append(label)
        card
      end

      private def open_image(path : String, number : Int32) : Nil
        response = fetch_image(path, false)
        unless response
          @status.text = "Cannot load that image."
          return
        end
        texture = texture_from(response)
        unless texture
          @status.text = "Cannot decode that image."
          return
        end

        picture = Gtk::Picture.new_for_paintable(texture)
        picture.content_fit = :scale_down
        picture.can_shrink = true

        scroll = Gtk::ScrolledWindow.new
        scroll.child = picture

        viewer = Gtk::Window.new
        viewer.title = "Image ##{number}"
        viewer.transient_for = @widget
        viewer.destroy_with_parent = true
        viewer.modal = true
        viewer.set_default_size(960, 720)
        viewer.child = scroll
        viewer.present
      end

      private def fetch_image(
        path : String,
        preview : Bool,
      ) : Hash(String, JSON::Any)?
        @client.call({
          "op"      => JSON::Any.new("image-read"),
          "path"    => JSON::Any.new(path),
          "preview" => JSON::Any.new(preview),
        })
      rescue Daemon::Client::Error
        nil
      end

      private def texture_from(
        body : Hash(String, JSON::Any),
      ) : Gdk::Texture?
        return unless body["mime"]?.try(&.as_s?) == "image/png"
        encoded = body["data"]?.try(&.as_s?) || return
        encoded_limit = ((MAX_IMAGE_BYTES + 2) // 3) * 4
        return if encoded.bytesize > encoded_limit

        data = Base64.decode(encoded)
        return if data.empty? || data.size > MAX_IMAGE_BYTES

        bytes = GLib::Bytes.new(data.to_unsafe, data.size)
        Gdk::Texture.new_from_bytes(bytes)
      rescue Base64::Error | GLib::Error
        nil
      end

      private def cancel_turn : Nil
        chat_id = @active_chat
        return unless chat_id

        call({
          "op"   => JSON::Any.new("cancel"),
          "chat" => JSON::Any.new(chat_id),
        })
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
        load_chat_state if call(request)
      end

      private def load_chat_state : Nil
        chat_id = @active_chat
        return unless chat_id

        state = call({
          "op"   => JSON::Any.new("chat"),
          "chat" => JSON::Any.new(chat_id),
        })
        return unless state

        @controls.update(state)
        @working = state["working"]?.try(&.as_bool?) || false
        @send.label = @working ? "Stop" : "Send"
        if @working
          @send.remove_css_class("suggested-action")
          @send.add_css_class("destructive-action")
        else
          @send.remove_css_class("destructive-action")
          @send.add_css_class("suggested-action")
        end
        queue = state["queue"]?.try(&.as_a?) || [] of JSON::Any
        render_queue(queue)
      end

      private def render_queue(queue : Array(JSON::Any)) : Nil
        clear(@queue_box)
        @queue_box.visible = !queue.empty?

        queue.each_with_index do |node, index|
          text = node.as_s
          label = Gtk::Label.new(text)
          label.xalign = 0_f32
          label.hexpand = true
          label.ellipsize = :end

          steer = Gtk::Button.new_with_label("Run next")
          steer.add_css_class("flat")
          steer.clicked_signal.connect do
            steer_queue(index, text)
          end

          remove = Gtk::Button.new_with_label("×")
          remove.add_css_class("flat")
          remove.tooltip_text = "Remove queued message"
          remove.clicked_signal.connect { drop_queue(index) }

          row = Gtk::Box.new(:horizontal, 6)
          row.add_css_class("xd-queue-row")
          row.append(label)
          row.append(steer)
          row.append(remove)
          @queue_box.append(row)
        end
      end

      private def steer_queue(index : Int, text : String) : Nil
        chat_id = @active_chat
        return unless chat_id
        call({
          "op"    => JSON::Any.new("steer-queue"),
          "chat"  => JSON::Any.new(chat_id),
          "index" => JSON::Any.new(index.to_i64),
          "text"  => JSON::Any.new(text),
        })
      end

      private def drop_queue(index : Int) : Nil
        chat_id = @active_chat
        return unless chat_id
        call({
          "op"    => JSON::Any.new("drop-queue"),
          "chat"  => JSON::Any.new(chat_id),
          "index" => JSON::Any.new(index.to_i64),
        })
      end

      private def handle_event(
        endpoint : Daemon::Endpoint,
        event : Hash(String, JSON::Any),
      ) : Nil
        if @client.same?(endpoint)
          @tool_panel.handle_event(event)
        end
        name = event["event"]?.try(&.as_s?) || return
        @sidebar.handle_event(endpoint, event)
        case name
        when "tree"
          @sidebar.reload(endpoint)
        when "text"
          return unless active_event?(endpoint, event)
          text = event["text"]?.try(&.as_s?) || return
          row = @stream_row
          unless row
            row = add_message("assistant", "")
            @stream_row = row
          end
          if row
            row.set_stream_text(row.text + text)
          end
          scroll_to_bottom
        when "tool"
          return unless active_event?(endpoint, event)
          add_message("tool", event["text"]?.try(&.as_s?) || "Used a tool")
          @stream_row = nil
          scroll_to_bottom
        when "turn-started"
          return unless active_event?(endpoint, event)
          @status.text = "Working…"
          @stream_row = nil
          load_chat_state
        when "turn-finished"
          if active_event?(endpoint, event)
            load_messages
            load_chat_state
            if event["waiting"]?.try(&.as_bool?) == true
              @status.text = "Waiting for your answer"
            end
          end
        when "changed"
          if active_event?(endpoint, event)
            load_messages
            load_chat_state
          end
        when "queued"
          return unless active_event?(endpoint, event)
          queue = event["queue"]?.try(&.as_a?)
          @status.text = queue && !queue.empty? ? "Message queued" : ""
          load_chat_state
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
        end
      end

      private def on_transcript_scrolled(dy : Float64) : Nil
        adjustment = @transcript_scroll.vadjustment
        cancel_history_restore if dy != 0
        if dy < 0 && adjustment.value > adjustment.lower
          @follow_bottom = false
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
            adjustment.value = value if adjustment.value != value

            if upper == @history_restore_upper &&
               page_size == @history_restore_page_size
              @history_restore_stable_frames += 1
            else
              @history_restore_stable_frames = 0
            end
            @history_restore_upper = upper
            @history_restore_page_size = page_size

            if @history_restore_stable_frames >= 2
              @history_restore_tick = 0_u32
              @history_bottom_distance = -1.0
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
