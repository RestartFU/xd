require "html"
require "set"
require "gtk4"
require "../unified_diff"
require "./diff_text"

module Xd
  module UI
    class DiffFileSections
      CHUNK_ROWS = 80

      record RowWidgets,
        expander : Gtk::Expander,
        path : Gtk::Label,
        counts : Gtk::Label,
        body : Gtk::Box

      getter widget : Gtk::ListView
      getter parsed = ParsedDiff.new(
        [] of DiffLine,
        0_u32,
        0_u32
      )
      getter sections = [] of DiffFileSection

      @collapsed = Set(String).new
      @row_widgets = {} of UInt64 => RowWidgets
      @bound_sections = {} of UInt64 => DiffFileSection
      @binding = Set(UInt64).new

      def initialize
        factory = Gtk::SignalListItemFactory.new
        factory.setup_signal.connect { |object| setup_item(object) }
        factory.bind_signal.connect { |object| bind_item(object) }
        factory.unbind_signal.connect { |object| unbind_item(object) }
        factory.teardown_signal.connect { |object| teardown_item(object) }

        @widget = Gtk::ListView.new(nil, factory)
        @widget.add_css_class("xd-diff-list")
        @widget.hexpand = true
      end

      def fill(patch : String) : ParsedDiff
        @widget.model = nil
        @parsed = UnifiedDiff.parse(patch)
        @sections = UnifiedDiff.file_sections(@parsed.lines)

        descriptors = Gtk::StringList.new(
          @sections.each_index.map(&.to_s).to_a
        )
        @widget.model = Gtk::NoSelection.new(descriptors)
        @parsed
      end

      private def setup_item(object : GObject::Object) : Nil
        item = list_item(object)
        path = Gtk::Label.new("")
        path.xalign = 0_f32
        path.ellipsize = :middle
        path.hexpand = true

        counts = Gtk::Label.new("")
        counts.xalign = 1_f32

        header = Gtk::Box.new(:horizontal, 8)
        header.hexpand = true
        header.append(path)
        header.append(counts)

        body = Gtk::Box.new(:vertical, 0)
        expander = Gtk::Expander.new(nil)
        expander.label_widget = header
        expander.child = body
        expander.hexpand = true
        expander.add_css_class("xd-diff-expander")

        key = pointer_key(item)
        expander.notify_signal["expanded"].connect do |_property|
          expanded_changed(key)
        end
        item.child = expander
        @row_widgets[key] = RowWidgets.new(
          expander,
          path,
          counts,
          body
        )
      end

      private def bind_item(object : GObject::Object) : Nil
        item = list_item(object)
        key = pointer_key(item)
        widgets = @row_widgets[key]? || return
        section = @sections[item.position.to_i]?
        return unless section

        widgets.path.markup =
          %(<span foreground="#ffbe6f" weight="bold">) +
            HTML.escape(UnifiedDiff.display_text(section.path)) +
            "</span>"
        widgets.counts.markup =
          %(<span foreground="#57e389">+#{section.additions}</span>) +
            %(  <span foreground="#f66151">−#{section.deletions}</span>)
        @bound_sections[key] = section

        expanded = !@collapsed.includes?(section.path)
        @binding << key
        widgets.expander.expanded = expanded
        @binding.delete(key)
        if expanded
          fill_body(widgets.body, section)
        else
          clear(widgets.body)
        end
      end

      private def unbind_item(object : GObject::Object) : Nil
        item = list_item(object)
        key = pointer_key(item)
        @bound_sections.delete(key)
        if widgets = @row_widgets[key]?
          clear(widgets.body)
          widgets.path.text = ""
          widgets.counts.text = ""
        end
      end

      private def teardown_item(object : GObject::Object) : Nil
        item = list_item(object)
        key = pointer_key(item)
        @binding.delete(key)
        @bound_sections.delete(key)
        @row_widgets.delete(key)
      end

      private def expanded_changed(key : UInt64) : Nil
        return if @binding.includes?(key)
        widgets = @row_widgets[key]? || return
        section = @bound_sections[key]? || return

        if widgets.expander.expanded?
          @collapsed.delete(section.path)
          fill_body(widgets.body, section)
        else
          keep_scroll_position(widgets.expander)
          @collapsed << section.path
          clear(widgets.body)
        end
      end

      private def fill_body(
        body : Gtk::Box,
        section : DiffFileSection,
      ) : Nil
        clear(body)
        start = section.start
        if @parsed.lines[start]?.try(&.kind.file?)
          start += 1
        end

        at = start
        while at < section.finish
          finish = Math.min(at + CHUNK_ROWS, section.finish)
          rendered = UnifiedDiff.markup_slice(
            @parsed.lines,
            false,
            at,
            finish
          )
          text = DiffText.new(
            rendered.markup,
            rendered.row_kinds,
            chunk: true
          )
          text.widget.hexpand = true
          body.append(text.widget)
          at = finish
        end
      end

      private def keep_scroll_position(
        expander : Gtk::Expander,
      ) : Nil
        ancestor = expander.ancestor(Gtk::ScrolledWindow.g_type)
        return unless ancestor

        scroller = Gtk::ScrolledWindow.new(
          ancestor.to_unsafe,
          GICrystal::Transfer::None
        )
        adjustment = scroller.vadjustment
        value = adjustment.value
        callback = ->(_widget : Gtk::Widget, _clock : Gdk::FrameClock) {
          adjustment.value = value
          false
        }
        scroller.add_tick_callback(callback)
      end

      private def clear(box : Gtk::Box) : Nil
        while child = box.first_child
          box.remove(child)
        end
      end

      private def list_item(object : GObject::Object) : Gtk::ListItem
        Gtk::ListItem.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
      end

      private def pointer_key(object : GObject::Object) : UInt64
        object.to_unsafe.address
      end
    end
  end
end
