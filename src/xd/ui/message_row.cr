require "gtk4"
require "../agent/assistant_sections"
require "../markdown"
require "./adw"
require "./background_work"
require "./diff_view"
require "./host_launch"
require "./message_content"

module Xd
  module UI
    enum MessageKind
      User
      Assistant
      Tool
      Error

      def self.from_role(role : String) : self
        case role
        when "assistant" then Assistant
        when "tool", "event"
          Tool
        when "error" then Error
        else              User
        end
      end

      def role : String
        case self
        when Assistant then "assistant"
        when Tool      then "tool"
        when Error     then "error"
        else                "user"
        end
      end

      def bubble? : Bool
        user?
      end

      def label_css_class : String?
        case self
        when Tool  then "dim-label"
        when Error then "error"
        else            nil
        end
      end
    end

    class MessageRow
      BUBBLE_MAX_WIDTH_CHARS = 60
      alias LiteralPart = String | Gtk::Widget

      getter widget : Adw::Bin
      getter kind : MessageKind
      getter text : String

      @stream_label : Gtk::Label?
      @render_generation = 0_i64

      def initialize(
        @kind : MessageKind,
        @text : String = "",
        @literal_parts : Array(LiteralPart)? = nil,
      )
        @stream_label = nil
        @card = Gtk::Box.new(:vertical, 6)
        @body = Gtk::Box.new(:vertical, 8)
        render_body
        @card.append(@body)

        @card.margin_top = 6
        @card.margin_bottom = 6
        @card.margin_start = 12
        @card.margin_end = 12

        if @kind.bubble?
          @card.add_css_class("card")
          @card.halign = :end
          @body.hexpand = false
          @body.margin_top = 10
          @body.margin_bottom = 10
          @body.margin_start = 14
          @body.margin_end = 14
        else
          @card.halign = :fill
          @card.hexpand = true
          @card.margin_top = 12
        end

        @widget = Adw::Bin.new(child: @card)
      end

      def source=(source : String?) : String?
        @widget.tooltip_text = source unless source.nil? || source.empty?
        source
      end

      def set_text(text : String) : Nil
        return if @stream_label.nil? && @text == text

        @text = text
        @literal_parts = nil
        @stream_label = nil
        render_body
      end

      def set_stream_text(text : String) : Nil
        @text = text
        @literal_parts = nil
        unless label = @stream_label
          @render_generation += 1
          clear_body
          label = make_text_label
          @body.append(label)
          @stream_label = label
        end
        label.text = Agent::AssistantSections.stream(text)
      end

      private def render_body : Nil
        @render_generation += 1
        generation = @render_generation
        clear_body
        return if @text.empty? && @literal_parts.nil?

        unless @kind.assistant?
          if parts = @literal_parts
            parts.each do |part|
              if part.is_a?(String)
                append_prose(Markdown.urls_to_pango(part))
              else
                @body.append(part)
              end
            end
          else
            append_prose(Markdown.urls_to_pango(@text))
          end
          return
        end

        placeholder = make_text_label
        placeholder.text = "Rendering response…"
        placeholder.add_css_class("dim-label")
        @body.append(placeholder)
        queue_assistant_render(@text, generation)
      end

      private def queue_assistant_render(
        text : String,
        generation : Int64,
      ) : Nil
        queued = BackgroundWork.submit do
          parts = MessageContent.prepare(text)
          index = 0
          GLib.idle_add do
            unless @render_generation == generation &&
                   @stream_label.nil?
              next false
            end

            clear_body if index == 0
            if part = parts[index]?
              if part.section.analysis?
                section_id = part.section_id
                analysis_parts = [] of PreparedMessagePart
                while candidate = parts[index]?
                  break unless candidate.section.analysis? &&
                                candidate.section_id == section_id
                  analysis_parts << candidate
                  index += 1
                end
                append_analysis_block(analysis_parts, generation)
              else
                append_prepared_part(part, @body)
                index += 1
              end
            end
            index < parts.size
          end
          nil
        end
        return if queued

        GLib.timeout(25.milliseconds) do
          if @render_generation == generation && @stream_label.nil?
            queue_assistant_render(text, generation)
          end
          false
        end
      end

      private def append_prepared_part(
        part : PreparedMessagePart,
        target : Gtk::Box,
      ) : Nil
        case part.kind
        when MessagePartKind::Prose
          append_prose(target, part.markup || part.text)
        when MessagePartKind::Code
          target.append(make_code_card(part.text, false, true))
        when MessagePartKind::Diff
          target.append(make_code_card(part.text, true, false))
        when MessagePartKind::Table
          target.append(make_code_card(part.text, false, false))
        end
      end

      private def append_analysis_block(
        parts : Array(PreparedMessagePart),
        generation : Int64,
      ) : Nil
        expander = Gtk::Expander.new("Analysis")
        expander.expanded = false
        expander.add_css_class("dim-label")
        loaded = false
        expander.notify_signal["expanded"].connect do |_property|
          unless @render_generation == generation
            next
          end

          if expander.expanded?
            next if loaded
            box = Gtk::Box.new(:vertical, 8)
            box.margin_start = 12
            parts.each do |part|
              append_prepared_part(part, box)
            end
            expander.child = box
            loaded = true
          else
            expander.child = nil
            loaded = false
          end
        end
        @body.append(expander)
      end

      private def append_prose(markup : String) : Nil
        append_prose(@body, markup)
      end

      private def append_prose(target : Gtk::Box, markup : String) : Nil
        return if markup.empty?

        label = make_text_label
        label.markup = markup
        target.append(label)
      end

      private def make_code_card(
        code : String,
        diff : Bool,
        wrap : Bool,
      ) : Gtk::Box
        content : Gtk::Widget
        if diff
          scroller = Gtk::ScrolledWindow.new
          scroller.set_policy(:automatic, :never)
          scroller.propagate_natural_height = true
          scroller.child = DiffView.build_async(code, true)
          content = scroller
        else
          label = Gtk::Label.new(code)
          label.wrap = wrap
          label.wrap_mode = :word_char
          label.xalign = 0_f32
          label.selectable = true
          label.add_css_class("xd-body")

          if wrap
            content = label
          else
            scroller = Gtk::ScrolledWindow.new
            scroller.set_policy(:automatic, :never)
            scroller.propagate_natural_height = true
            scroller.child = label
            content = scroller
          end
        end
        content.hexpand = true

        card = Gtk::Box.new(:vertical, 0)
        card.add_css_class("xd-code")
        card.add_css_class("xd-inline-diff") if diff
        card.append(content)
        card
      end

      private def make_text_label : Gtk::Label
        label = Gtk::Label.new("")
        label.wrap = true
        label.wrap_mode = :word_char
        label.xalign = 0_f32
        label.selectable = true
        label.add_css_class("xd-body")
        label.max_width_chars = BUBBLE_MAX_WIDTH_CHARS if @kind.bubble?
        if css_class = @kind.label_css_class
          label.add_css_class(css_class)
        end
        label.activate_link_signal.connect do |uri|
          HostLaunch.open_uri(uri)
          true
        end
        label
      end

      private def clear_body : Nil
        @stream_label = nil
        while child = @body.first_child
          @body.remove(child)
        end
      end
    end
  end
end
