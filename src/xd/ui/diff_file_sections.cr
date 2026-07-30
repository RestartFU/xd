require "html"
require "set"
require "gtk4"
require "../unified_diff"
require "./diff_text"

module Xd
  module UI
    class DiffFileSections
      CHUNK_ROWS        =    80
      MAX_RENDERED_ROWS = 4_000

      record Prepared,
        parsed : ParsedDiff,
        sections : Array(DiffFileSection)

      record RenderPlan,
        finish : Int32,
        omitted : Int32

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
      @render_jobs = {} of UInt64 => Int64
      @render_sequence = 0_i64

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
        fill(self.class.prepare(patch))
      end

      def self.prepare(patch : String) : Prepared
        parsed = UnifiedDiff.parse(patch)
        Prepared.new(
          parsed,
          UnifiedDiff.file_sections(parsed.lines)
        )
      end

      def fill(prepared : Prepared) : ParsedDiff
        @render_jobs.clear
        @widget.model = nil
        @parsed = prepared.parsed
        @sections = prepared.sections

        descriptors = Gtk::StringList.new(
          @sections.each_index.map(&.to_s).to_a
        )
        @widget.model = Gtk::NoSelection.new(descriptors)
        @parsed
      end

      def self.render_plan(start : Int32, finish : Int32) : RenderPlan
        visible_finish = Math.min(finish, start + MAX_RENDERED_ROWS)
        RenderPlan.new(
          visible_finish,
          Math.max(finish - visible_finish, 0)
        )
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
          fill_body(key, widgets.body, section)
        else
          cancel_body(key)
          clear(widgets.body)
        end
      end

      private def unbind_item(object : GObject::Object) : Nil
        item = list_item(object)
        key = pointer_key(item)
        cancel_body(key)
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
        cancel_body(key)
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
          fill_body(key, widgets.body, section)
        else
          keep_scroll_position(widgets.expander)
          @collapsed << section.path
          cancel_body(key)
          clear(widgets.body)
        end
      end

      private def fill_body(
        key : UInt64,
        body : Gtk::Box,
        section : DiffFileSection,
      ) : Nil
        cancel_body(key)
        clear(body)
        start = section.start
        if @parsed.lines[start]?.try(&.kind.file?)
          start += 1
        end

        plan = self.class.render_plan(start, section.finish)
        @render_sequence += 1
        token = @render_sequence
        @render_jobs[key] = token
        append_body_chunk(
          key,
          body,
          section,
          start,
          plan,
          token
        )
      end

      private def append_body_chunk(
        key : UInt64,
        body : Gtk::Box,
        section : DiffFileSection,
        start : Int32,
        plan : RenderPlan,
        token : Int64,
      ) : Nil
        return unless body_job_active?(key, body, section, token)

        if start < plan.finish
          finish = Math.min(start + CHUNK_ROWS, plan.finish)
          rendered = UnifiedDiff.markup_slice(
            @parsed.lines,
            false,
            start,
            finish
          )
          text = DiffText.new(
            rendered.markup,
            rendered.row_kinds,
            chunk: true
          )
          text.widget.hexpand = true
          body.append(text.widget)

          if finish < plan.finish
            GLib.idle_add do
              append_body_chunk(
                key,
                body,
                section,
                finish,
                plan,
                token
              )
              false
            end
            return
          end
        end

        append_omitted_notice(body, plan.omitted) if plan.omitted > 0
        @render_jobs.delete(key) if @render_jobs[key]? == token
      end

      private def body_job_active?(
        key : UInt64,
        body : Gtk::Box,
        section : DiffFileSection,
        token : Int64,
      ) : Bool
        return false unless @render_jobs[key]? == token
        return false unless @bound_sections[key]? == section

        widgets = @row_widgets[key]?
        !!widgets && widgets.body.to_unsafe == body.to_unsafe
      end

      private def append_omitted_notice(
        body : Gtk::Box,
        omitted : Int32,
      ) : Nil
        noun = omitted == 1 ? "row" : "rows"
        notice = Gtk::Label.new(
          "#{omitted} more diff #{noun} not shown"
        )
        notice.xalign = 0_f32
        notice.margin_top = 8
        notice.margin_bottom = 8
        notice.margin_start = 12
        notice.add_css_class("dim-label")
        body.append(notice)
      end

      private def cancel_body(key : UInt64) : Nil
        @render_jobs.delete(key)
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
