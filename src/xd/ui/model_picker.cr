require "gtk4"
require "set"
require "../agent/catalog"
require "./adw"

module Xd
  module UI
    # Combined assistant/model chooser matching the C provider rail and rows.
    class ModelPicker
      private record Entry,
        backend : Agent::Backend,
        model : Agent::Model

      getter widget : Gtk::MenuButton

      @backend_id : String?
      @model_id : String?
      @filter : Agent::Backend?
      @showing_favorites = true
      @syncing_rail = false
      @visible = [] of Entry
      @rail_buttons = [] of Tuple(Gtk::ToggleButton, Agent::Backend?)

      def initialize(
        @on_selected : Proc(String, String, Nil),
      )
        @backend_id = nil
        @model_id = nil
        @filter = nil
        @settings = Gio::Settings.new(APP_ID)
        @favorites = Set(String).new(@settings.strv("favorite-models"))
        @rail = Gtk::Box.new(:vertical, 4)
        @search = Gtk::SearchEntry.new
        @list = Gtk::ListBox.new
        @stack = Gtk::Stack.new

        content = Gtk::Box.new(:horizontal, 6)
        @button_icon = Gtk::Image.new
        @button_label = Gtk::Label.new("")
        content.append(@button_icon)
        content.append(@button_label)
        content.append(Gtk::Image.new_from_icon_name("pan-down-symbolic"))

        @widget = Gtk::MenuButton.new
        @widget.child = content
        @widget.add_css_class("flat")

        columns = build_popover_content
        columns.add_css_class("xd-menu")
        @popover = Gtk::Popover.new
        @popover.child = columns
        @popover.has_arrow = false
        @popover.show_signal.connect do
          @search.text = ""
          rebuild_list
          @search.grab_focus
        end
        @widget.popover = @popover
        update_button
      end

      def select(backend_id : String?, model_id : String?) : Nil
        return if @backend_id == backend_id && @model_id == model_id

        @backend_id = backend_id
        @model_id = model_id
        @filter = Agent::Catalog.lookup(backend_id)
        @showing_favorites = false
        update_button
        sync_rail
        rebuild_list
      end

      private def build_popover_content : Gtk::Box
        @rail.margin_top = 6
        @rail.margin_bottom = 6
        @rail.margin_start = 6
        @rail.margin_end = 6

        group = add_rail_button(nil, nil)
        Agent::Catalog.all.each do |backend|
          add_rail_button(backend, group)
        end

        @search.placeholder_text = "Search models…"
        @search.margin_top = 6
        @search.margin_start = 6
        @search.margin_end = 6
        @search.search_changed_signal.connect { rebuild_list }

        @list.selection_mode = :none
        @list.row_activated_signal.connect do |row|
          choose(@visible[row.index]?) if row.index >= 0
        end

        scroller = Gtk::ScrolledWindow.new
        scroller.set_policy(:never, :automatic)
        scroller.child = @list
        scroller.vexpand = true

        empty = Adw::StatusPage.new(
          icon_name: "system-search-symbolic",
          title: "No Models"
        )
        @stack.vexpand = true
        @stack.add_named(scroller, "list")
        @stack.add_named(empty, "empty")

        right = Gtk::Box.new(:vertical, 6)
        right.hexpand = true
        right.append(@search)
        right.append(@stack)

        columns = Gtk::Box.new(:horizontal, 0)
        columns.append(@rail)
        columns.append(Gtk::Separator.new(:vertical))
        columns.append(right)
        columns.set_size_request(380, 360)

        keys = Gtk::EventControllerKey.new
        keys.propagation_phase = :capture
        keys.key_pressed_signal.connect do |keyval, _keycode, state|
          if state.includes?(Gdk::ModifierType::ControlMask) &&
             keyval >= Gdk::KEY_1 && keyval <= Gdk::KEY_9
            choose(@visible[(keyval - Gdk::KEY_1).to_i]?)
            true
          else
            false
          end
        end
        columns.add_controller(keys)
        columns
      end

      private def add_rail_button(
        backend : Agent::Backend?,
        group : Gtk::ToggleButton?,
      ) : Gtk::ToggleButton
        button = Gtk::ToggleButton.new
        button.icon_name = backend.try(&.icon_name) || "starred-symbolic"
        button.tooltip_text = backend.try(&.display_name) || "Starred"
        button.add_css_class("flat")
        button.group = group if group
        selected_backend = backend
        button.toggled_signal.connect do
          unless @syncing_rail || !button.active?
            @showing_favorites = selected_backend.nil?
            @filter = selected_backend
            rebuild_list
          end
        end
        @rail.append(button)
        @rail_buttons << {button, backend}
        button
      end

      private def rebuild_list : Nil
        while child = @list.first_child
          @list.remove(child)
        end
        @visible.clear
        needle = @search.text.downcase

        Agent::Catalog.all.each do |backend|
          next if !@showing_favorites && @filter != backend

          backend.models.each do |model|
            entry = Entry.new(backend, model)
            next if @showing_favorites && !favorite?(entry)
            next unless needle.empty? ||
                        model.display_name.downcase.includes?(needle) ||
                        backend.display_name.downcase.includes?(needle)

            @visible << entry
          end
        end

        @visible.each_with_index do |entry, index|
          @list.append(build_row(entry, index))
        end
        @stack.visible_child_name = @visible.empty? ? "empty" : "list"
      end

      private def build_row(entry : Entry, index : Int) : Gtk::ListBoxRow
        icon = Gtk::Image.new_from_icon_name(entry.backend.icon_name)
        name = Gtk::Label.new(entry.model.display_name)
        name.xalign = 0_f32
        provider = Gtk::Label.new(entry.backend.display_name)
        provider.xalign = 0_f32
        provider.add_css_class("dim-label")
        provider.add_css_class("caption")

        names = Gtk::Box.new(:vertical, 0)
        names.hexpand = true
        names.append(name)
        names.append(provider)

        content = Gtk::Box.new(:horizontal, 10)
        content.margin_top = 6
        content.margin_bottom = 6
        content.margin_start = 10
        content.margin_end = 6
        content.append(icon)
        content.append(names)
        if index < 9
          hint = Gtk::Label.new("Ctrl+#{index + 1}")
          hint.add_css_class("dim-label")
          hint.add_css_class("caption")
          hint.valign = :center
          content.append(hint)
        end

        selected = entry
        star = Gtk::Button.new_from_icon_name(
          favorite?(entry) ? "starred-symbolic" : "non-starred-symbolic"
        )
        star.add_css_class("flat")
        star.valign = :center
        star.tooltip_text = "Star this model"
        star.clicked_signal.connect { toggle_favorite(selected) }
        content.append(star)

        row = Gtk::ListBoxRow.new
        row.child = content
        row
      end

      private def choose(entry : Entry?) : Nil
        return unless entry

        @backend_id = entry.backend.id
        @model_id = entry.model.id
        update_button
        @widget.popdown
        @on_selected.call(entry.backend.id, entry.model.id)
      end

      private def favorite_key(entry : Entry) : String
        "#{entry.backend.id}/#{entry.model.id}"
      end

      private def favorite?(entry : Entry) : Bool
        @favorites.includes?(favorite_key(entry))
      end

      private def toggle_favorite(entry : Entry) : Nil
        key = favorite_key(entry)
        if @favorites.includes?(key)
          @favorites.delete(key)
        else
          @favorites << key
        end
        @settings.set_strv("favorite-models", @favorites.to_a)
        rebuild_list
      end

      private def update_button : Nil
        backend = Agent::Catalog.lookup(@backend_id)
        unless backend
          @button_label.text = "No assistant"
          @button_icon.icon_name = "dialog-warning-symbolic"
          return
        end

        @button_icon.icon_name = backend.icon_name
        @button_label.text = backend.model_label(@model_id)
      end

      private def sync_rail : Nil
        @syncing_rail = true
        @rail_buttons.each do |button, backend|
          button.active = if @showing_favorites
                            backend.nil?
                          else
                            backend == @filter
                          end
        end
      ensure
        @syncing_rail = false
      end
    end
  end
end
