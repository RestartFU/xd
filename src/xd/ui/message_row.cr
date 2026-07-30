require "gtk4"
require "../markdown"
require "./adw"
require "./host_launch"

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

      getter widget : Adw::Bin
      getter kind : MessageKind
      getter text : String

      @stream_label : Gtk::Label?

      def initialize(
        @kind : MessageKind,
        @text : String = "",
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
        @stream_label = nil
        render_body
      end

      def set_stream_text(text : String) : Nil
        @text = text
        unless label = @stream_label
          clear_body
          label = make_text_label
          @body.append(label)
          @stream_label = label
        end
        label.text = @text
      end

      private def render_body : Nil
        clear_body
        return if @text.empty?

        label = make_text_label
        label.markup = if @kind.assistant?
                         Markdown.to_pango(@text)
                       else
                         Markdown.urls_to_pango(@text)
                       end
        @body.append(label)
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
