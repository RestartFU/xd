require "gtk4"
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
        label.text = @text
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

        MessageContent.parse(@text).each do |part|
          case part.kind
          when MessagePartKind::Prose
            append_assistant_prose(part.text, generation)
          when MessagePartKind::Code
            @body.append(make_code_card(part.text, false, true))
          when MessagePartKind::Diff
            @body.append(make_code_card(part.text, true, false))
          when MessagePartKind::Table
            @body.append(make_code_card(part.text, false, false))
          end
        end
      end

      private def append_assistant_prose(
        text : String,
        generation : Int64,
      ) : Nil
        return if text.empty?

        label = make_text_label
        label.text = text
        @body.append(label)
        BackgroundWork.submit do
          markup = Markdown.to_pango(text)
          GLib.idle_add do
            if @render_generation == generation &&
               label.parent &&
               @stream_label.nil?
              label.markup = markup
            end
            false
          end
          nil
        end
      end

      private def append_prose(markup : String) : Nil
        return if markup.empty?

        label = make_text_label
        label.markup = markup
        @body.append(label)
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
          scroller.child = DiffView.build(code, true)
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
