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
      RETIRE_BATCH      =     4

      record RenderPlan,
        finish : Int32,
        omitted : Int32

      record PreparedBody,
        chunks : Array(DiffMarkup),
        omitted : Int32

      record Prepared,
        parsed : ParsedDiff,
        sections : Array(DiffFileSection),
        bodies : Hash(Int32, PreparedBody)

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
      @prepared_bodies = {} of Int32 => PreparedBody
      @section_paths = [] of String
      @sections_by_path = {} of String => DiffFileSection
      @model : Gtk::StringList
      @retired_bodies = Deque(Gtk::Box).new
      @retire_scheduled = false

      def initialize
        factory = Gtk::SignalListItemFactory.new
        factory.setup_signal.connect { |object| setup_item(object) }
        factory.bind_signal.connect { |object| bind_item(object) }
        factory.unbind_signal.connect { |object| unbind_item(object) }
        factory.teardown_signal.connect { |object| teardown_item(object) }

        @model = Gtk::StringList.new([] of String)
        @widget = Gtk::ListView.new(Gtk::NoSelection.new(@model), factory)
        @widget.add_css_class("xd-diff-list")
        @widget.hexpand = true
      end

      def fill(patch : String) : ParsedDiff
        fill(self.class.prepare(patch))
      end

      def self.prepare(patch : String) : Prepared
        parsed = UnifiedDiff.parse(patch)
        sections = UnifiedDiff.file_sections(parsed.lines)
        bodies = {} of Int32 => PreparedBody
        remaining = MAX_RENDERED_ROWS
        sections.each do |section|
          body, rendered = prepare_body(parsed, section, remaining)
          bodies[section.start] = body
          remaining = Math.max(remaining - rendered, 0)
        end
        Prepared.new(
          parsed,
          sections,
          bodies
        )
      end

      def fill(prepared : Prepared) : ParsedDiff
        paths = prepared.sections.map(&.path)
        parsed = prepared.parsed
        # Bodies already contain rendered markup. Retaining every parsed line
        # until next refresh doubles large-pane memory for no UI benefit.
        @parsed = ParsedDiff.new(
          [] of DiffLine,
          parsed.additions,
          parsed.deletions
        )
        @sections = prepared.sections
        @prepared_bodies = prepared.bodies
        @sections_by_path = @sections.to_h { |section| {section.path, section} }
        reconcile_model(paths)
        refresh_bound_sections
        parsed
      end

      def self.render_plan(start : Int32, finish : Int32) : RenderPlan
        visible_finish = Math.min(finish, start + MAX_RENDERED_ROWS)
        RenderPlan.new(
          visible_finish,
          Math.max(finish - visible_finish, 0)
        )
      end

      private def self.prepare_body(
        parsed : ParsedDiff,
        section : DiffFileSection,
        available : Int32,
      ) : {PreparedBody, Int32}
        start = section.start
        if parsed.lines[start]?.try(&.kind.file?)
          start += 1
        end
        visible_finish = Math.min(
          section.finish,
          start + Math.min(MAX_RENDERED_ROWS, available)
        )
        plan = RenderPlan.new(
          visible_finish,
          Math.max(section.finish - visible_finish, 0)
        )
        rendered = plan.finish - start
        chunks = [] of DiffMarkup
        while start < plan.finish
          finish = Math.min(start + CHUNK_ROWS, plan.finish)
          chunks << UnifiedDiff.markup_slice(
            parsed.lines,
            false,
            start,
            finish
          )
          start = finish
        end
        {PreparedBody.new(chunks, plan.omitted), rendered}
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
        section = section_for_item(item)
        return unless section

        bind_section(key, widgets, section)
      end

      private def bind_section(
        key : UInt64,
        widgets : RowWidgets,
        section : DiffFileSection,
      ) : Nil
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
          fill_body(key, widgets, section)
        else
          cancel_body(key)
          replace_body(key, widgets)
        end
      end

      private def section_for_item(item : Gtk::ListItem) : DiffFileSection?
        object = item.item
        return unless object

        path = Gtk::StringObject.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        ).string
        @sections_by_path[path]?
      end

      private def reconcile_model(paths : Array(String)) : Nil
        wanted = paths.to_set
        index = @section_paths.size - 1
        while index >= 0
          unless wanted.includes?(@section_paths[index])
            @model.splice(index.to_u32, 1_u32, nil)
            @section_paths.delete_at(index)
          end
          index -= 1
        end

        paths.each_with_index do |path, target|
          next if @section_paths[target]? == path

          if current = @section_paths.index(path, target + 1)
            @model.splice(current.to_u32, 1_u32, nil)
            @section_paths.delete_at(current)
          end
          @model.splice(target.to_u32, 0_u32, [path])
          @section_paths.insert(target, path)
        end
      end

      private def refresh_bound_sections : Nil
        @bound_sections.keys.each do |key|
          old = @bound_sections[key]? || next
          current = @sections_by_path[old.path]?
          next unless current
          next if current == old
          widgets = @row_widgets[key]? || next

          bind_section(key, widgets, current)
        end
      end

      private def unbind_item(object : GObject::Object) : Nil
        item = list_item(object)
        key = pointer_key(item)
        cancel_body(key)
        @bound_sections.delete(key)
        if widgets = @row_widgets[key]?
          replace_body(key, widgets)
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
        if widgets = @row_widgets.delete(key)
          retire_body(widgets.body)
        end
      end

      private def expanded_changed(key : UInt64) : Nil
        return if @binding.includes?(key)
        widgets = @row_widgets[key]? || return
        section = @bound_sections[key]? || return

        if widgets.expander.expanded?
          @collapsed.delete(section.path)
          fill_body(key, widgets, section)
        else
          keep_scroll_position(widgets.expander)
          @collapsed << section.path
          cancel_body(key)
          replace_body(key, widgets)
        end
      end

      private def fill_body(
        key : UInt64,
        widgets : RowWidgets,
        section : DiffFileSection,
      ) : Nil
        cancel_body(key)
        body = replace_body(key, widgets)
        prepared = @prepared_bodies[section.start]? || return
        @render_sequence += 1
        token = @render_sequence
        @render_jobs[key] = token
        append_body_chunk(
          key,
          body,
          section,
          prepared,
          0,
          token
        )
      end

      private def append_body_chunk(
        key : UInt64,
        body : Gtk::Box,
        section : DiffFileSection,
        prepared : PreparedBody,
        chunk : Int32,
        token : Int64,
      ) : Nil
        return unless body_job_active?(key, body, section, token)

        if rendered = prepared.chunks[chunk]?
          text = DiffText.new(
            rendered.markup,
            rendered.row_kinds,
            chunk: true
          )
          text.widget.hexpand = true
          body.append(text.widget)

          if chunk + 1 < prepared.chunks.size
            GLib.idle_add do
              append_body_chunk(
                key,
                body,
                section,
                prepared,
                chunk + 1,
                token
              )
              false
            end
            return
          end
        end

        if prepared.omitted > 0
          append_omitted_notice(body, prepared.omitted)
        end
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

      private def replace_body(
        key : UInt64,
        widgets : RowWidgets,
      ) : Gtk::Box
        replacement = Gtk::Box.new(:vertical, 0)
        widgets.expander.child = replacement
        @row_widgets[key] = RowWidgets.new(
          widgets.expander,
          widgets.path,
          widgets.counts,
          replacement
        )
        retire_body(widgets.body)
        replacement
      end

      private def retire_body(body : Gtk::Box) : Nil
        return unless body.first_child

        @retired_bodies << body
        return if @retire_scheduled

        @retire_scheduled = true
        GLib.idle_add do
          drain_retired_bodies
        end
      end

      private def drain_retired_bodies : Bool
        RETIRE_BATCH.times do
          body = @retired_bodies.first?
          break unless body

          if child = body.first_child
            body.remove(child)
          end
          @retired_bodies.shift unless body.first_child
        end
        more = !@retired_bodies.empty?
        @retire_scheduled = more
        more
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
