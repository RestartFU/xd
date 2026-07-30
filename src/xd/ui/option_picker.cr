require "gtk4"

module Xd
  module UI
    # Compact composer picker with the same descriptive popover rows as C.
    class OptionPicker
      record Option, label : String, description : String

      private class Choice
        property option : Option
        getter row : Gtk::ListBoxRow
        getter label : Gtk::Label
        getter description : Gtk::Label
        getter check : Gtk::Image

        def initialize(
          @option : Option,
          @row : Gtk::ListBoxRow,
          @label : Gtk::Label,
          @description : Gtk::Label,
          @check : Gtk::Image,
        )
        end
      end

      getter widget : Gtk::MenuButton
      getter selected = 0

      @choices = [] of Choice
      @on_selected : Proc(Int32, Nil)

      def initialize(
        options : Array(Option),
        @on_selected : Proc(Int32, Nil),
      )
        content = Gtk::Box.new(:horizontal, 6)
        @button_label = Gtk::Label.new("")
        content.append(@button_label)
        content.append(Gtk::Image.new_from_icon_name("pan-down-symbolic"))

        @widget = Gtk::MenuButton.new
        @widget.child = content
        @widget.add_css_class("flat")

        @list = Gtk::ListBox.new
        @list.selection_mode = :single
        @list.set_size_request(320, -1)
        @list.row_activated_signal.connect do |row|
          choose(row.index)
        end

        scroller = Gtk::ScrolledWindow.new
        scroller.set_policy(:never, :automatic)
        scroller.max_content_height = 420
        scroller.propagate_natural_height = true
        scroller.child = @list

        panel = Gtk::Box.new(:vertical, 0)
        panel.append(scroller)
        panel.add_css_class("xd-menu")

        @popover = Gtk::Popover.new
        @popover.child = panel
        @popover.has_arrow = false
        @widget.popover = @popover
        self.options = options
      end

      def options=(options : Array(Option)) : Array(Option)
        options.each_with_index do |option, index|
          if index < @choices.size
            update_choice(@choices[index], option)
          else
            append_choice(option)
          end
        end
        while @choices.size > options.size
          choice = @choices.pop
          @list.remove(choice.row)
        end
        @selected = 0
        sync_selection
        options
      end

      def selected=(index : Int) : Int
        return @selected unless index >= 0 && index < @choices.size

        @selected = index.to_i32
        sync_selection
        index
      end

      def label(index : Int, text : String) : Nil
        return unless choice = @choices[index]?

        option = Option.new(text, choice.option.description)
        update_choice(choice, option)
        @button_label.text = text if index == @selected
      end

      private def append_choice(option : Option) : Nil
        title = Gtk::Label.new(option.label)
        title.xalign = 0_f32
        detail = Gtk::Label.new(option.description)
        detail.xalign = 0_f32
        detail.wrap = true
        detail.add_css_class("caption")
        detail.add_css_class("dim-label")

        text = Gtk::Box.new(:vertical, 2)
        text.hexpand = true
        text.append(title)
        text.append(detail)

        check = Gtk::Image.new_from_icon_name("object-select-symbolic")
        check.valign = :center
        content = Gtk::Box.new(:horizontal, 12)
        content.margin_top = 8
        content.margin_bottom = 8
        content.margin_start = 10
        content.margin_end = 10
        content.append(text)
        content.append(check)

        row = Gtk::ListBoxRow.new
        row.child = content
        @list.append(row)
        @choices << Choice.new(option, row, title, detail, check)
      end

      private def update_choice(choice : Choice, option : Option) : Nil
        choice.option = option
        choice.label.text = option.label
        choice.description.text = option.description
      end

      private def choose(index : Int) : Nil
        return unless index >= 0 && index < @choices.size

        self.selected = index
        @widget.popdown
        @on_selected.call(index.to_i32)
      end

      private def sync_selection : Nil
        return if @choices.empty?

        choice = @choices[@selected]
        @button_label.text = choice.option.label
        @list.select_row(choice.row)
        @choices.each_with_index do |item, index|
          item.check.opacity = index == @selected ? 1.0 : 0.0
        end
      end
    end
  end
end
