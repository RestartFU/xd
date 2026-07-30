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
require "./adw"
require "./chat_controls"
require "./sidebar"
require "./tool_panel"

module Xd
  module UI
    class Window
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
      @stream_label : Gtk::Label?
      @working = false
      @workflow_ids = Set(String).new
      @attachments = [] of Attachment

      def initialize(
        application : Gtk::Application,
        @client : Daemon::Endpoint,
      )
        @active_chat = nil
        @stream_label = nil
        @widget = Adw::ApplicationWindow.new(application: application)
        @widget.title = "xd"
        @widget.set_default_size(1100, 720)

        @sidebar = Sidebar.new(
          @widget,
          ->(request : Hash(String, JSON::Any)) { call(request) },
          ->(id : String, title : String) { open_chat(id, title) },
          ->(id : String) { chat_deleted(id) }
        )
        @tool_panel = ToolPanel.new(
          ->(request : Hash(String, JSON::Any)) { call(request) }
        )

        @chat_title = Adw::WindowTitle.new(title: "xd")
        chat_header = Adw::HeaderBar.new
        chat_header.title_widget = @chat_title
        chat_header.show_start_title_buttons = false
        {
          "folder-symbolic"             => {"files", "Browse files"},
          "view-list-ordered-symbolic"  => {"diff", "Changed files"},
          "utilities-terminal-symbolic" => {
            "terminal",
            "Terminal",
          },
        }.each do |label, page|
          name, tooltip = page
          button = Gtk::Button.new_from_icon_name(label)
          button.add_css_class("flat")
          button.tooltip_text = tooltip
          button.clicked_signal.connect { @tool_panel.toggle(name) }
          chat_header.pack_end(button)
        end

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

        @transcript = Gtk::Box.new(:vertical, 8)
        @transcript.valign = :start

        @transcript_scroll = Gtk::ScrolledWindow.new
        @transcript_scroll.vexpand = true
        @transcript_scroll.set_policy(:never, :external)
        transcript_clamp = Adw::Clamp.new(
          child: @transcript,
          maximum_size: 1040,
          tightening_threshold: 1040
        )
        transcript_clamp.margin_top = 12
        transcript_clamp.margin_bottom = 12
        @transcript_scroll.child = transcript_clamp

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

        @status = Gtk::Label.new("")
        @status.xalign = 0_f32
        @status.hexpand = true
        @status.add_css_class("dim-label")

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

        side_split = Gtk::Paned.new(:horizontal)
        side_split.start_child = content
        side_split.end_child = @tool_panel.widget
        side_split.resize_start_child = true
        side_split.shrink_start_child = false
        side_split.resize_end_child = false
        side_split.shrink_end_child = false

        chat = Adw::ToolbarView.new
        chat.add_css_class("xd-surface")
        chat.add_top_bar(chat_header)
        chat.content = side_split

        root = Gtk::Paned.new(:horizontal)
        root.start_child = @sidebar.widget
        root.end_child = chat
        root.position = 280
        root.resize_start_child = false
        root.shrink_start_child = false
        root.resize_end_child = true
        root.shrink_end_child = false
        @widget.content = root

        headers = Gtk::SizeGroup.new(:vertical)
        headers.add_widget(@sidebar.header)
        headers.add_widget(chat_header)

        @client.subscribe do |event|
          GLib.idle_add do
            handle_event(event)
            false
          end
        end

        @sidebar.reload
      end

      def present : Nil
        @widget.present
      end

      private def call(fields : Hash(String, JSON::Any))
        @status.text = ""
        @client.call(fields)
      rescue error : Daemon::Client::Error
        @status.text = error.message || "Daemon request failed."
        nil
      end

      private def open_chat(id : String, title : String) : Nil
        clear_attachments
        @active_chat = id
        @stream_label = nil
        @chat_title.title = title
        @chat_stack.visible_child_name = "chat"
        @composer.visible = true
        @entry.sensitive = true
        @attach.sensitive = true
        @send.sensitive = true
        @controls.sensitive = true
        @tool_panel.chat = id
        load_chat_state
        load_messages
        @entry.grab_focus
      end

      private def chat_deleted(id : String) : Nil
        return unless @active_chat == id

        @active_chat = nil
        @stream_label = nil
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
        @tool_panel.chat = nil
        @tool_panel.widget.visible = false
        clear(@transcript)
        @workflow_ids.clear
        clear(@queue_box)
        @queue_box.visible = false
        clear_attachments
        @status.text = ""
      end

      private def load_messages : Nil
        chat_id = @active_chat
        return unless chat_id

        response = call({
          "op"   => JSON::Any.new("messages"),
          "chat" => JSON::Any.new(chat_id),
        })
        return unless response

        clear(@transcript)
        @workflow_ids.clear
        messages = response["messages"].as_a
        messages.each_with_index do |message, index|
          add_message(
            message["role"].as_s,
            message["content"].as_s,
            message["label"]?.try(&.as_s?),
            reply_answerable?(messages, index)
          )
        end
        @stream_label = nil
        scroll_to_bottom
      end

      private def add_message(
        role : String,
        content : String,
        label : String? = nil,
        answerable : Bool = false,
      ) : Gtk::Label?
        if role == "duration"
          @status.text = "Finished in #{content}s"
          return
        end

        if role == "tool"
          if patch = Agent::GitDiffTracker.patch(content)
            return add_diff_message(patch)
          end
          if workflow = Agent::WorkflowRun.parse(content)
            return add_workflow_message(workflow)
          end
          if subagent = Agent::SubagentTool.parse(content)
            return add_subagent_message(subagent[0], subagent[1])
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
        heading = case role
                  when "user"      then "You"
                  when "assistant" then label || "Assistant"
                  when "tool"      then "Tool"
                  when "error"     then "Error"
                  else                  role.capitalize
                  end
        text = shown.empty? ? heading : "#{heading}\n#{shown}"
        row = Gtk::Label.new(text)
        row.xalign = 0_f32
        row.wrap = true
        row.wrap_mode = :word_char
        row.selectable = true
        row.add_css_class("xd-message")
        row.add_css_class("xd-message-#{role}")
        @transcript.append(row)
        append_message_images(images.paths) if images
        append_ask(parsed.ask) if parsed && answerable
        row
      end

      private def add_diff_message(patch : String) : Gtk::Label
        row = Gtk::Label.new("Files changed\n#{patch}")
        row.xalign = 0_f32
        row.wrap = false
        row.selectable = true
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

      private def handle_event(event : Hash(String, JSON::Any)) : Nil
        @tool_panel.handle_event(event)
        name = event["event"]?.try(&.as_s?) || return
        case name
        when "tree"
          @sidebar.reload
        when "text"
          return unless active_event?(event)
          text = event["text"]?.try(&.as_s?) || return
          label = @stream_label
          unless label
            label = add_message("assistant", "")
            @stream_label = label
          end
          if label
            current = label.text
            current = "Assistant\n" if current == "Assistant"
            label.text = current + text
          end
          scroll_to_bottom
        when "tool"
          return unless active_event?(event)
          add_message("tool", event["text"]?.try(&.as_s?) || "Used a tool")
          @stream_label = nil
          scroll_to_bottom
        when "turn-started"
          return unless active_event?(event)
          @status.text = "Working…"
          @stream_label = nil
          load_chat_state
        when "turn-finished"
          if active_event?(event)
            load_messages
            load_chat_state
            if event["waiting"]?.try(&.as_bool?) == true
              @status.text = "Waiting for your answer"
            end
          end
        when "changed"
          if active_event?(event)
            load_messages
            load_chat_state
          end
        when "queued"
          return unless active_event?(event)
          queue = event["queue"]?.try(&.as_a?)
          @status.text = queue && !queue.empty? ? "Message queued" : ""
          load_chat_state
        end
      end

      private def active_event?(event : Hash(String, JSON::Any)) : Bool
        event["chat"]?.try(&.as_s?) == @active_chat
      end

      private def clear(box : Gtk::Box) : Nil
        while child = box.first_child
          box.remove(child)
        end
      end

      private def scroll_to_bottom : Nil
        GLib.idle_add do
          adjustment = @transcript_scroll.vadjustment
          adjustment.value = adjustment.upper
          false
        end
      end
    end
  end
end
