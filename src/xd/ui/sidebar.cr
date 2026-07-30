require "json"
require "gtk4"
require "set"
require "../agent/catalog"
require "../daemon/endpoint"
require "../remote/connection"
require "../version"
require "./adw"
require "./dialogs"
require "./directory_browser"
require "./dots"
require "./folder_dialogs"
require "./sidebar_state"
require "./updater"

module Xd
  module UI
    class Sidebar
      @remote_state_subscription : Int64
      @row_popover : Gtk::Popover?
      @selected_key : String?
      @editing_key : String?
      @editing_model : Gio::ListStore?
      @editing_source : Source?
      @editing_parent_id : String?
      @pending_menu : Gtk::Popover?
      @pending_menu_action : Proc(Nil)?
      @restore_chat_id : String?
      @active_chat_key : String?

      private class Source
        getter endpoint : Daemon::Endpoint
        getter remote : Bool
        getter folder_ids = [] of String
        getter folder_names = {} of String => String
        getter folder_parents = {} of String => String?
        getter children : Hash(String, Array(String))
        getter chats : Hash(String, Array(JSON::Any))
        property selected_folder : String?
        property loaded = false

        def initialize(
          @endpoint : Daemon::Endpoint,
          @remote : Bool,
        )
          @selected_folder = nil
          @children = Hash(String, Array(String)).new do |hash, key|
            hash[key] = [] of String
          end
          @chats = Hash(String, Array(JSON::Any)).new do |hash, key|
            hash[key] = [] of JSON::Any
          end
        end

        def update(response : Hash(String, JSON::Any)) : Nil
          @folder_ids.clear
          @folder_names.clear
          @folder_parents.clear
          @children.clear
          @chats.clear

          response["folders"].as_a.each do |folder|
            id = folder["id"].as_s
            parent = folder["parent"]?.try(&.as_s?)
            @folder_ids << id
            @folder_names[id] = folder["name"].as_s
            @folder_parents[id] = parent
            @children[parent || ROOT] << id
          end
          response["chats"].as_a.each do |chat|
            @chats[chat["folder"].as_s] << chat
          end

          selected = @selected_folder
          unless selected && @folder_names.has_key?(selected)
            @selected_folder = @children[ROOT].first?
          end
          @loaded = true
        end

        def clear : Nil
          @folder_ids.clear
          @folder_names.clear
          @folder_parents.clear
          @children.clear
          @chats.clear
          @selected_folder = nil
          @loaded = false
        end
      end

      private enum NodeKind
        Folder
        Chat
        RemoteRoot
      end

      private class Node
        getter key : String
        getter id : String
        getter name : String
        getter kind : NodeKind
        getter source : Source
        getter children : Gio::ListStore
        getter folder_id : String?
        getter backend : String?
        property state : SidebarState
        getter placeholder : Bool

        def initialize(
          @key : String,
          @id : String,
          @name : String,
          @kind : NodeKind,
          @source : Source,
          @folder_id : String? = nil,
          @backend : String? = nil,
          @state : SidebarState = SidebarState::Idle,
          @placeholder : Bool = false,
        )
          @children = Gio::ListStore.new(Gtk::StringObject.g_type)
        end

        def folder? : Bool
          @kind.folder? || @kind.remote_root?
        end

        def chat? : Bool
          @kind.chat?
        end

        def placeholder? : Bool
          @placeholder
        end

        def icon_name : String
          if @kind.remote_root?
            return @state.offline? ? "network-offline-symbolic" : "network-server-symbolic"
          end
          return "folder-symbolic" if @kind.folder?
          Agent::Catalog.lookup(@backend).try(&.icon_name) ||
            "chat-message-symbolic"
        end
      end

      private record RowWidgets,
        expander : Gtk::TreeExpander,
        box : Gtk::Box,
        icon_overlay : Gtk::Overlay,
        icon : Gtk::Image,
        status : Gtk::Box,
        working : Dots,
        label : Gtk::Label,
        entry : Gtk::Entry,
        drag_source : Gtk::DragSource,
        drop_target : Gtk::DropTarget

      getter widget : Adw::ToolbarView
      getter header : Adw::HeaderBar

      ROOT = ""

      def initialize(
        @parent : Gtk::Window,
        local : Daemon::Endpoint,
        @remote : Remote::Connection,
        @on_chat : Proc(Daemon::Endpoint, String, String, Nil),
        @on_chat_deleted : Proc(Daemon::Endpoint, String, Nil),
        @on_pair : Proc(Nil),
        @on_remote_forgot : Proc(Nil),
        @on_error : Proc(String, Nil),
      )
        @local_source = Source.new(local, false)
        @remote_source = Source.new(@remote, true)
        @settings = Gio::Settings.new(APP_ID)
        @expanded = Set(String).new(@settings.strv("expanded-folders"))
        @nodes = {} of String => Node
        @row_widgets = {} of UInt64 => RowWidgets
        @bound_nodes = {} of UInt64 => Node
        @expand_connections =
          {} of UInt64 => GObject::SignalConnection
        @row_popover = nil
        @selected_key = nil
        @editing_key = nil
        @editing_model = nil
        @editing_source = nil
        @editing_parent_id = nil
        @editing_kind = NodeKind::Folder
        @creating = false
        @placeholder_serial = 0_u64
        @reload_after_edit = false
        @tree_reload_queued = false
        @pending_menu = nil
        @pending_menu_action = nil
        @restore_chat_id = nil
        @restore_chat_remote = false
        @restoring_chat = false
        @active_chat_key = nil
        @chat_states = {} of String => SidebarState
        @restore_queued = false
        @restoring_expanded = false
        @closing = {} of UInt64 => {Gtk::TreeListRow, String}
        @save_expanded_queued = false

        @root_model = Gio::ListStore.new(Gtk::StringObject.g_type)
        child_model_for_item = ->(item : Pointer(Void)) : Pointer(Void) {
          string = Gtk::StringObject.new(item, GICrystal::Transfer::None)
          node = @nodes[string.string]?

          if node && node.folder?
            model = node.children
            LibGObject.g_object_ref(model.to_unsafe)
            model.to_unsafe
          else
            Pointer(Void).null
          end
        }
        child_model_data = GICrystal::ClosureDataManager.register(
          ::Box.box(child_model_for_item)
        )
        create_children = ->(item : Pointer(Void), user_data : Pointer(Void)) : Pointer(Void) {
          ::Box(Proc(Pointer(Void), Pointer(Void)))
            .unbox(user_data)
            .call(item)
        }
        destroy_children = ->GICrystal::ClosureDataManager.deregister(Pointer(Void))

        # gi-crystal's generated wrapper returns the Crystal ListModel wrapper
        # itself from this callback instead of its GObject pointer. Use the raw
        # constructor until that callback ABI is fixed upstream.
        GICrystal.ref(@root_model)
        tree_model = LibGtk.gtk_tree_list_model_new(
          @root_model.to_unsafe,
          0,
          0,
          create_children.pointer,
          child_model_data,
          destroy_children.pointer
        )
        @tree_model = Gtk::TreeListModel.new(
          tree_model,
          GICrystal::Transfer::Full
        )
        @selection = Gtk::SingleSelection.new(@tree_model)
        @selection.autoselect = false
        @selection.can_unselect = true
        @selection.notify_signal["selected"].connect do |_property|
          selection_changed
        end

        factory = Gtk::SignalListItemFactory.new
        factory.setup_signal.connect { |object| setup_item(object) }
        factory.bind_signal.connect { |object| bind_item(object) }
        factory.unbind_signal.connect { |object| unbind_item(object) }
        factory.teardown_signal.connect { |object| teardown_item(object) }

        @list_view = Gtk::ListView.new(@selection, factory)
        @list_view.single_click_activate = false
        @list_view.add_css_class("navigation-sidebar")
        @list_view.add_css_class("xd-sidebar")
        @list_view.activate_signal.connect do |position|
          activate_row(position)
        end
        @list_view.add_controller(build_drop_target(nil))

        scroll = Gtk::ScrolledWindow.new
        scroll.vexpand = true
        scroll.set_policy(:never, :external)
        scroll.add_css_class("xd-sidebar")
        scroll.child = @list_view

        add = Gtk::MenuButton.new
        add.icon_name = "list-add-symbolic"
        add.tooltip_text = "Add a workspace or a machine"
        menu = Gtk::Popover.new
        choices = Gtk::Box.new(:vertical, 2)
        choices.margin_top = 6
        choices.margin_bottom = 6
        choices.margin_start = 6
        choices.margin_end = 6
        add_choice(choices, menu, "New Workspace") do
          begin_creating(@local_source, nil, NodeKind::Folder)
        end
        add_choice(choices, menu, "Connect to a Machine…") do
          @on_pair.call
        end
        add_choice(choices, menu, "Agent Secrets…") do
          dialogs(@local_source).secrets
        end
        menu.child = choices
        menu.add_css_class("xd-menu-popover")
        menu.closed_signal.connect { finish_menu_action(menu) }
        add.popover = menu

        title = Adw::WindowTitle.new(title: "Workspaces")
        @header = Adw::HeaderBar.new
        @header.title_widget = title
        @header.show_end_title_buttons = false
        @header.pack_start(add)

        @widget = Adw::ToolbarView.new
        @widget.add_css_class("xd-sidebar")
        @widget.add_top_bar(@header)
        @widget.content = scroll
        @updater = Updater.new(@parent)
        @widget.add_bottom_bar(@updater.widget)

        @remote_state_subscription = @remote.on_state do |_snapshot|
          GLib.idle_add do
            reload
            false
          end
        end
      end

      def reload : Nil
        if @editing_key
          @reload_after_edit = true
          return
        end

        if response = call(
             @local_source,
             {"op" => JSON::Any.new("tree")}
           )
          @local_source.update(response)
        end

        if @remote.connected?
          if response = call(
               @remote_source,
               {"op" => JSON::Any.new("tree")},
               quiet: true
             )
            @remote_source.update(response)
          end
        end

        rebuild_tree
      end

      def reload(endpoint : Daemon::Endpoint) : Nil
        return reload if endpoint.same?(@local_source.endpoint)
        return reload if endpoint.same?(@remote_source.endpoint)
      end

      def restore_chat(id : String, remote : Bool) : Nil
        return if id.empty?

        @restore_chat_id = id
        @restore_chat_remote = remote
        queue_restore
      end

      def activate_chat(endpoint : Daemon::Endpoint, id : String) : Nil
        source = source_for(endpoint) || return
        key = chat_key(source, id)
        @active_chat_key = key
        set_chat_state(source, id, chat_state(key).opened)
      end

      def clear_active_chat : Nil
        @active_chat_key = nil
      end

      def answer_chat(endpoint : Daemon::Endpoint, id : String) : Nil
        source = source_for(endpoint) || return
        key = chat_key(source, id)
        set_chat_state(source, id, chat_state(key).answered)
      end

      def handle_event(
        endpoint : Daemon::Endpoint,
        event : Hash(String, JSON::Any),
      ) : Nil
        source = source_for(endpoint) || return
        id = event["chat"]?.try(&.as_s?) || return
        name = event["event"]?.try(&.as_s?) || return
        key = chat_key(source, id)
        state = case name
                when "turn-started"
                  SidebarState::Working
                when "turn-finished"
                  chat_state(key).finish(
                    event["waiting"]?.try(&.as_bool?) == true,
                    @active_chat_key == key
                  )
                else
                  return
                end
        set_chat_state(source, id, state)
      end

      def close : Nil
        @remote.unsubscribe(@remote_state_subscription)
        @updater.close
      end

      private def rebuild_tree : Nil
        @root_model.remove_all
        @nodes.clear

        add_source_roots(@local_source, @root_model)
        if @remote.configured?
          snapshot = @remote.snapshot
          host = snapshot.host || "Remote"
          port = snapshot.port || 0
          root = Node.new(
            "remote/root/#{host}:#{port}",
            "remote://#{host}:#{port}",
            host,
            NodeKind::RemoteRoot,
            @remote_source,
            state: snapshot.state.connected? ? SidebarState::Idle : SidebarState::Offline
          )
          append_node(@root_model, root)
          add_source_roots(@remote_source, root.children)
        end
        @chat_states.select! { |key, _state| @nodes.has_key?(key) }
        queue_restore
      end

      private def add_source_roots(
        source : Source,
        model : Gio::ListStore,
      ) : Nil
        source.children[ROOT].each do |folder_id|
          add_folder_node(source, folder_id, model)
        end
      end

      private def add_folder_node(
        source : Source,
        folder_id : String,
        model : Gio::ListStore,
      ) : Nil
        prefix = source.remote ? "remote" : "local"
        node = Node.new(
          "#{prefix}/folder/#{folder_id}",
          folder_id,
          source.folder_names[folder_id],
          NodeKind::Folder,
          source,
          folder_id: folder_id
        )
        append_node(model, node)

        source.children[folder_id].each do |child_id|
          add_folder_node(source, child_id, node.children)
        end
        source.chats[folder_id].each do |chat|
          add_chat_node(source, chat, node.children)
        end
      end

      private def add_chat_node(
        source : Source,
        chat : JSON::Any,
        model : Gio::ListStore,
      ) : Nil
        id = chat["id"].as_s
        folder_id = chat["folder"].as_s
        title = chat["title"].as_s? || "New Chat"
        title = "New Chat" if title.empty?
        prefix = source.remote ? "remote" : "local"
        key = "#{prefix}/chat/#{id}"
        current = chat_state(key)
        state = current.reconcile_tree(
          chat["working"]?.try(&.as_bool?) == true,
          @active_chat_key == key,
          source.remote
        )
        @chat_states[key] = state
        append_node(
          model,
          Node.new(
            key,
            id,
            title,
            NodeKind::Chat,
            source,
            folder_id: folder_id,
            backend: chat["backend"]?.try(&.as_s?),
            state: state
          )
        )
      end

      private def append_node(model : Gio::ListStore, node : Node) : Nil
        @nodes[node.key] = node
        model.append(Gtk::StringObject.new(node.key))
      end

      private def setup_item(object : GObject::Object) : Nil
        item = Gtk::ListItem.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        expander = Gtk::TreeExpander.new
        box = Gtk::Box.new(:horizontal, 8)
        icon_overlay = Gtk::Overlay.new
        icon = Gtk::Image.new
        status = Gtk::Box.new(:horizontal, 0)
        working = Dots.new
        label = Gtk::Label.new("")
        entry = Gtk::Entry.new
        drag_source = Gtk::DragSource.new
        drop_target = build_drop_target(box)

        label.xalign = 0_f32
        label.ellipsize = :end
        label.hexpand = true

        entry.visible = false
        entry.hexpand = true
        entry.valign = :center
        entry.add_css_class("xd-inline-entry")
        entry.activate_signal.connect { end_editing(true) }

        editor_keys = Gtk::EventControllerKey.new
        editor_keys.key_pressed_signal.connect do |keyval, _keycode, _state|
          if keyval == Gdk::KEY_Escape
            end_editing(false)
            true
          else
            false
          end
        end
        entry.add_controller(editor_keys)

        editor_focus = Gtk::EventControllerFocus.new
        editor_focus.leave_signal.connect do
          node = @bound_nodes[pointer_key(box)]?
          if node && @editing_key == node.key
            # Rebuilding the list inside GTK's focus traversal invalidates the
            # widget chain that the current click is still walking. The C tree
            # mutates individual rows; this snapshot-backed client waits one
            # idle before its equivalent rebuild.
            key = node.key
            GLib.idle_add do
              end_editing(true) if @editing_key == key
              false
            end
          end
        end
        entry.add_controller(editor_focus)

        working.visible = false
        working.widget.add_css_class("dim-label")

        status.visible = false
        status.halign = :end
        status.valign = :end
        status.can_target = false
        status.add_css_class("xd-status-dot")

        icon_overlay.child = icon
        icon_overlay.add_overlay(status)
        box.append(icon_overlay)
        box.append(working.widget)
        box.append(label)
        box.append(entry)

        gesture = Gtk::GestureClick.new
        gesture.button = Gdk::BUTTON_SECONDARY.to_u32
        gesture.pressed_signal.connect do |_presses, _x, _y|
          if node = @bound_nodes[pointer_key(box)]?
            open_row_menu(box, node)
          end
        end
        box.add_controller(gesture)

        drag_source.drag_begin_signal.connect do |_drag|
          drag_source.set_icon(Gtk::WidgetPaintable.new(box), 0, 0)
        end
        box.add_controller(drag_source)
        box.add_controller(drop_target)

        expander.child = box
        item.child = expander
        @row_widgets[pointer_key(item)] = RowWidgets.new(
          expander,
          box,
          icon_overlay,
          icon,
          status,
          working,
          label,
          entry,
          drag_source,
          drop_target
        )
      end

      private def bind_item(object : GObject::Object) : Nil
        item = Gtk::ListItem.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        widgets = @row_widgets[pointer_key(item)]? || return
        row_object = item.item || return
        row = Gtk::TreeListRow.new(
          row_object.to_unsafe,
          GICrystal::Transfer::None
        )
        node = node_for_row(row) || return

        widgets.expander.list_row = row
        widgets.icon.icon_name = node.icon_name
        widgets.label.text = node.name
        show_state(widgets, node)

        @bound_nodes[pointer_key(widgets.box)] = node
        if node.kind.folder? && !node.placeholder?
          widgets.drag_source.actions = Gdk::DragAction::Move
          widgets.drag_source.content = Gdk::ContentProvider.new_for_value(
            Gtk::StringObject.new(node.key)
          )
        else
          widgets.drag_source.actions = Gdk::DragAction.new(0_u32)
          widgets.drag_source.content = nil
        end
        show_editor(widgets, node, @editing_key == node.key)
        if node.folder?
          connection = row.notify_signal["expanded"].connect do |_property|
            expanded_changed(row, node)
          end
          @expand_connections[pointer_key(item)] = connection
        end
        queue_restore
      end

      private def unbind_item(object : GObject::Object) : Nil
        item = Gtk::ListItem.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        key = pointer_key(item)
        @expand_connections.delete(key).try(&.disconnect)
        if widgets = @row_widgets[key]?
          @bound_nodes.delete(pointer_key(widgets.box))
          widgets.drag_source.actions = Gdk::DragAction.new(0_u32)
          widgets.drag_source.content = nil
          widgets.expander.list_row = nil
        end
      end

      private def teardown_item(object : GObject::Object) : Nil
        item = Gtk::ListItem.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        key = pointer_key(item)
        @expand_connections.delete(key).try(&.disconnect)
        if widgets = @row_widgets.delete(key)
          @bound_nodes.delete(pointer_key(widgets.box))
          widgets.drag_source.content = nil
        end
      end

      private def widgets_for_node(key : String) : RowWidgets?
        @row_widgets.each_value do |widgets|
          node = @bound_nodes[pointer_key(widgets.box)]?
          return widgets if node && node.key == key
        end
        nil
      end

      private def show_state(widgets : RowWidgets, node : Node) : Nil
        state = node.state
        waiting = state.waiting?
        done = state.done?

        widgets.working.visible = state.working?
        widgets.icon_overlay.visible = !state.working?
        widgets.status.visible = waiting || done

        widgets.status.remove_css_class("xd-status-waiting")
        widgets.status.remove_css_class("xd-status-done")
        widgets.status.add_css_class("xd-status-waiting") if waiting
        widgets.status.add_css_class("xd-status-done") if done

        if state.offline?
          widgets.icon.add_css_class("xd-offline")
        else
          widgets.icon.remove_css_class("xd-offline")
        end
        widgets.icon_overlay.tooltip_text =
          if state.offline?
            "Not connected. Trying again every few seconds."
          elsif waiting
            "Waiting for your answer"
          elsif done
            "New reply"
          end
      end

      private def source_for(endpoint : Daemon::Endpoint) : Source?
        return @local_source if endpoint.same?(@local_source.endpoint)
        return @remote_source if endpoint.same?(@remote_source.endpoint)
      end

      private def chat_key(source : Source, id : String) : String
        "#{source.remote ? "remote" : "local"}/chat/#{id}"
      end

      private def chat_state(key : String) : SidebarState
        @chat_states[key]? || SidebarState::Idle
      end

      private def set_chat_state(
        source : Source,
        id : String,
        state : SidebarState,
      ) : Nil
        key = chat_key(source, id)
        @chat_states[key] = state
        node = @nodes[key]?
        return unless node

        node.state = state
        if widgets = widgets_for_node(key)
          show_state(widgets, node)
        end
      end

      private def show_editor(
        widgets : RowWidgets,
        node : Node,
        editing : Bool,
      ) : Nil
        widgets.label.visible = !editing
        widgets.entry.visible = editing
        return unless editing

        widgets.entry.text = node.name
        widgets.entry.select_region(0, -1)
        widgets.entry.grab_focus

        key = node.key
        GLib.idle_add do
          current = @bound_nodes[pointer_key(widgets.box)]?
          if current &&
             current.key == key &&
             @editing_key == key &&
             widgets.entry.mapped &&
             widgets.entry.visible?
            widgets.entry.grab_focus
            widgets.entry.select_region(0, -1)
          end
          false
        end
      end

      private def begin_renaming(node : Node) : Nil
        key = node.key
        end_editing(true)
        current = @nodes[key]? || return
        return if current.placeholder? || current.kind.remote_root?

        @editing_key = current.key
        @editing_model = nil
        @editing_source = current.source
        @editing_parent_id = current.folder_id
        @editing_kind = current.kind
        @creating = false

        if widgets = widgets_for_node(current.key)
          show_editor(widgets, current, true)
        end
      end

      private def begin_creating(
        source : Source,
        parent : Node?,
        kind : NodeKind,
      ) : Nil
        parent_key = parent.try(&.key)
        end_editing(true)

        current_parent = parent_key ? @nodes[parent_key]? : nil
        return if parent_key && !current_parent
        return if kind.chat? && !current_parent.try(&.kind.folder?)

        model = current_parent.try(&.children) || @root_model
        parent_id = if current_parent.try(&.kind.folder?)
                      current_parent.not_nil!.id
                    end
        name = kind.chat? ? "New Chat" : ""
        @placeholder_serial += 1
        key = "placeholder/#{@placeholder_serial}"
        placeholder = Node.new(
          key,
          "",
          name,
          kind,
          source,
          folder_id: kind.chat? ? parent_id : nil,
          placeholder: true
        )

        if current_parent
          if widgets = widgets_for_node(current_parent.key)
            widgets.expander.list_row.try { |row| row.expanded = true }
          end
        end

        position = placeholder_position(model, kind)
        @editing_key = key
        @editing_model = model
        @editing_source = source
        @editing_parent_id = parent_id
        @editing_kind = kind
        @creating = true
        @nodes[key] = placeholder
        model.insert(position, Gtk::StringObject.new(key))
      end

      private def placeholder_position(
        model : Gio::ListStore,
        kind : NodeKind,
      ) : UInt32
        return 0_u32 unless kind.chat?

        position = 0_u32
        while position < model.n_items
          child = node_for_model_item(model.item(position))
          break unless child.try(&.kind.folder?)
          position += 1
        end
        position
      end

      private def end_editing(keep : Bool) : Nil
        key = @editing_key
        return unless key

        node = @nodes[key]?
        widgets = widgets_for_node(key)
        name = widgets.try(&.entry.text)
        creating = @creating
        model = @editing_model
        source = @editing_source
        parent_id = @editing_parent_id
        kind = @editing_kind
        reload_after = @reload_after_edit

        @editing_key = nil
        @editing_model = nil
        @editing_source = nil
        @editing_parent_id = nil
        @creating = false
        @reload_after_edit = false

        show_editor(widgets, node, false) if widgets && node
        if creating && model
          remove_model_key(model, key)
          @nodes.delete(key)
        end

        if keep && source && node && name && !name.empty?
          if creating
            if kind.chat?
              create_chat(source, parent_id.not_nil!, name) if parent_id
            else
              create_folder(source, parent_id, name)
            end
          elsif kind.chat?
            rename_chat(node, name)
          elsif kind.folder? && name != node.name
            rename_folder(node, name)
          end
        end

        reload if reload_after
      end

      private def remove_model_key(
        model : Gio::ListStore,
        key : String,
      ) : Nil
        position = 0_u32
        while position < model.n_items
          node = node_for_model_item(model.item(position))
          if node.try(&.key) == key
            model.remove(position)
            return
          end
          position += 1
        end
      end

      private def node_for_model_item(object : GObject::Object?) : Node?
        return unless object

        string = Gtk::StringObject.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        @nodes[string.string]?
      end

      private def build_drop_target(box : Gtk::Box?) : Gtk::DropTarget
        target = Gtk::DropTarget.new(
          GObject::TYPE_OBJECT,
          Gdk::DragAction::Move
        )
        target.drop_signal.connect do |value, _x, _y|
          drop_folder(value, box)
        end
        target
      end

      private def drop_folder(
        value : GObject::Value,
        target_box : Gtk::Box?,
      ) : Bool
        object = value.as_gobject?
        return false unless object

        dragged_object = Gtk::StringObject.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        dragged = @nodes[dragged_object.string]?
        return false unless dragged &&
                            dragged.kind.folder? &&
                            !dragged.placeholder?

        onto = if target_box
                 @bound_nodes[pointer_key(target_box)]?
               end
        target_source = onto.try(&.source) || @local_source

        if onto.try(&.kind.chat?)
          folder_id = onto.not_nil!.folder_id
          onto = folder_id ? folder_node(target_source, folder_id) : nil
          return false unless onto
        end
        return false unless dragged.source.same?(target_source)

        parent_id = case onto.try(&.kind)
                    when NodeKind::Folder
                      onto.not_nil!.id
                    when NodeKind::RemoteRoot, nil
                      nil
                    else
                      return false
                    end
        current_parent = dragged.source.folder_parents[dragged.id]?
        return true if current_parent == parent_id

        move_folder_request(dragged.source, dragged.id, parent_id)
      end

      private def folder_node(source : Source, id : String) : Node?
        @nodes.each_value.find do |node|
          node.source.same?(source) &&
            node.kind.folder? &&
            node.id == id
        end
      end

      private def selection_changed : Nil
        object = @selection.selected_item || return
        row = Gtk::TreeListRow.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        node = node_for_row(row) || return
        return if node.placeholder?
        return if @selected_key == node.key

        @restore_chat_id = nil unless @restoring_chat
        @selected_key = node.key
        if folder_id = node.folder_id
          node.source.selected_folder = folder_id
        end
        open_node(node) if node.chat?
      end

      private def activate_row(position : UInt32) : Nil
        row = @tree_model.row(position) || return
        node = node_for_row(row) || return
        return if node.placeholder?

        if node.folder?
          row.expanded = !row.expanded?
        else
          open_node(node)
        end
      end

      private def open_node(node : Node) : Nil
        return unless node.chat?

        if folder_id = node.folder_id
          node.source.selected_folder = folder_id
        end
        @on_chat.call(node.source.endpoint, node.id, node.name)
      end

      private def node_for_row(row : Gtk::TreeListRow) : Node?
        object = row.item || return
        string = Gtk::StringObject.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        @nodes[string.string]?
      end

      private def expanded_changed(
        row : Gtk::TreeListRow,
        node : Node,
      ) : Nil
        return if @restoring_expanded || node.placeholder?

        key = pointer_key(row)
        if row.expanded?
          @expanded << node.id
          @closing.delete(key)
        else
          @closing[key] = {row, node.id}
        end
        queue_save_expanded
      end

      private def queue_save_expanded : Nil
        return if @save_expanded_queued

        @save_expanded_queued = true
        GLib.idle_add do
          @save_expanded_queued = false
          @closing.each_value do |row, id|
            @expanded.delete(id) if row_in_tree?(row)
          end
          @closing.clear
          @settings.set_strv("expanded-folders", @expanded.to_a)
          false
        end
      end

      private def row_in_tree?(row : Gtk::TreeListRow) : Bool
        current = @tree_model.row(row.position)
        !!current && current.to_unsafe == row.to_unsafe
      end

      private def queue_restore : Nil
        return if @restore_queued

        @restore_queued = true
        GLib.idle_add do
          @restore_queued = false
          restore_tree_state
          false
        end
      end

      private def restore_tree_state : Nil
        restore = restore_node
        if restore
          folder_id = restore.folder_id
          while folder_id
            @expanded << folder_id
            folder_id = restore.source.folder_parents[folder_id]?
          end
          if restore.source.remote
            remote_root = @nodes.each_value.find(&.kind.remote_root?)
            @expanded << remote_root.id if remote_root
          end
        end

        @restoring_expanded = true
        index = 0_u32
        while index < @tree_model.n_items
          row = @tree_model.row(index)
          if row
            node = node_for_row(row)
            if node && node.folder? && @expanded.includes?(node.id)
              row.expanded = true unless row.expanded?
            end
          end
          index += 1
        end
        @restoring_expanded = false

        selected = restore.try(&.key) || @selected_key
        return unless selected

        index = 0_u32
        while index < @tree_model.n_items
          row = @tree_model.row(index)
          if row && node_for_row(row).try(&.key) == selected
            @restoring_chat = !!restore
            @selection.selected = index
            @restoring_chat = false
            @restore_chat_id = nil if restore
            break
          end
          index += 1
        end
      ensure
        @restoring_chat = false
        @restoring_expanded = false
      end

      private def restore_node : Node?
        id = @restore_chat_id
        return unless id

        @nodes.each_value.find do |node|
          node.chat? &&
            node.id == id &&
            node.source.remote == @restore_chat_remote
        end
      end

      private def open_row_menu(box : Gtk::Box, node : Node) : Nil
        return if node.placeholder?

        if previous = @row_popover
          previous.popdown
        end

        popover = case node.kind
                  when NodeKind::RemoteRoot
                    remote_menu(node)
                  when NodeKind::Folder
                    folder_menu(node)
                  when NodeKind::Chat
                    chat_menu(node)
                  else
                    raise "Unknown sidebar node kind"
                  end
        popover.has_arrow = false
        popover.halign = :start
        popover.parent = box
        @row_popover = popover
        popover.closed_signal.connect do
          # GtkModelButton closes a popover before its action runs. Keep the
          # row attached through that activation, exactly as the C sidebar
          # does, then detach from an idle so recycled list rows stay safe.
          GLib.idle_add do
            unless popover.visible?
              popover.unparent if popover.parent
              if @row_popover.try(&.to_unsafe) == popover.to_unsafe
                @row_popover = nil
              end
              finish_menu_action(popover)
            end
            false
          end
        end
        popover.popup
      end

      private def pointer_key(object : GObject::Object) : UInt64
        object.to_unsafe.address
      end

      private def remote_menu(node : Node) : Gtk::Popover
        popover, choices = menu_shell
        add_choice(choices, popover, "New Workspace") do
          begin_creating(node.source, node, NodeKind::Folder)
        end
        add_choice(choices, popover, "Agent Secrets…") do
          dialogs(node.source).secrets
        end
        add_choice(choices, popover, "Refresh") { reload }
        choices.append(Gtk::Separator.new(:horizontal))
        add_choice(choices, popover, "Remove Connection…") do
          confirm_remove_remote(node.name)
        end
        popover
      end

      private def folder_menu(node : Node) : Gtk::Popover
        source = node.source
        folder_id = node.id
        popover, choices = menu_shell
        add_choice(choices, popover, "New Chat") do
          begin_creating(source, node, NodeKind::Chat)
        end
        add_choice(choices, popover, "New Folder") do
          begin_creating(source, node, NodeKind::Folder)
        end
        add_choice(choices, popover, "Rename…") do
          begin_renaming(node)
        end
        add_choice(choices, popover, "Agent Context…") do
          dialogs(source).context(
            folder_id,
            source.folder_names[folder_id]
          )
        end
        add_choice(choices, popover, "Agent Secrets…") do
          dialogs(source).secrets(
            folder_id,
            source.folder_names[folder_id]
          )
        end
        unless source.remote
          add_choice(choices, popover, "Folder Settings…") do
            dialogs(source).settings(
              folder_id,
              source.folder_names[folder_id]
            )
          end
        end

        choices.append(Gtk::Separator.new(:horizontal))
        add_choice(choices, popover, "Move to Trash") do
          confirm_trash_folder(source, folder_id)
        end
        popover
      end

      private def chat_menu(node : Node) : Gtk::Popover
        source = node.source
        chat_id = node.id
        popover, choices = menu_shell
        add_choice(choices, popover, "Rename…") do
          begin_renaming(node)
        end
        choices.append(Gtk::Separator.new(:horizontal))
        add_choice(choices, popover, "Delete Chat") do
          delete_chat(source, chat_id)
        end
        popover
      end

      private def menu_shell : {Gtk::Popover, Gtk::Box}
        popover = Gtk::Popover.new
        choices = Gtk::Box.new(:vertical, 2)
        choices.margin_top = 6
        choices.margin_bottom = 6
        choices.margin_start = 6
        choices.margin_end = 6
        popover.child = choices
        {popover, choices}
      end

      private def add_choice(
        choices : Gtk::Box,
        popover : Gtk::Popover,
        label : String,
        &action : -> Nil
      ) : Nil
        button = Gtk::Button.new_with_label(label)
        button.add_css_class("flat")
        button.halign = :fill
        button.clicked_signal.connect do
          queue_menu_action(popover, action)
        end
        choices.append(button)
      end

      private def queue_menu_action(
        popover : Gtk::Popover,
        action : Proc(Nil),
      ) : Nil
        @pending_menu = popover
        @pending_menu_action = action
        popover.popdown
      end

      private def finish_menu_action(popover : Gtk::Popover) : Nil
        pending = @pending_menu
        return unless pending &&
                      pending.to_unsafe == popover.to_unsafe

        action = @pending_menu_action
        @pending_menu = nil
        @pending_menu_action = nil
        return unless action

        GLib.idle_add do
          action.call
          false
        end
      end

      private def create_folder(
        source : Source,
        parent_id : String?,
        name : String,
      ) : Nil
        request = {
          "op"   => JSON::Any.new("new-folder"),
          "name" => JSON::Any.new(name),
        }
        request["parent"] = JSON::Any.new(parent_id) if parent_id
        if created = call(source, request)
          source.selected_folder = created["id"].as_s
          reload
        end
      end

      private def create_chat(
        source : Source,
        folder_id : String,
        title : String,
      ) : Nil
        settings = call(source, {
          "op"     => JSON::Any.new("folder-settings"),
          "folder" => JSON::Any.new(folder_id),
        })
        return unless settings

        DirectoryBrowser.present(
          @parent,
          panel_call(source),
          settings["effective_workdir"]?.try(&.as_s?)
        ) do |workdir|
          create_chat_in(source, folder_id, title, workdir)
        end
      end

      private def create_chat_in(
        source : Source,
        folder_id : String,
        title : String,
        workdir : String?,
      ) : Nil
        request = {
          "op"     => JSON::Any.new("new-chat"),
          "folder" => JSON::Any.new(folder_id),
          "title"  => JSON::Any.new(title),
        }
        request["workdir"] = JSON::Any.new(workdir) if workdir
        created = call(source, request)
        return unless created

        source.selected_folder = folder_id
        reload
        @on_chat.call(source.endpoint, created["id"].as_s, title)
      end

      private def rename_folder(node : Node, name : String) : Nil
        if call(node.source, {
             "op"     => JSON::Any.new("rename-folder"),
             "folder" => JSON::Any.new(node.id),
             "name"   => JSON::Any.new(name),
           })
          reload
        end
      end

      private def rename_chat(node : Node, title : String) : Nil
        if call(node.source, {
             "op"    => JSON::Any.new("rename-chat"),
             "chat"  => JSON::Any.new(node.id),
             "title" => JSON::Any.new(title),
           })
          reload
          @on_chat.call(node.source.endpoint, node.id, title)
        end
      end

      private def move_folder(
        source : Source,
        folder_id : String,
        parent_id : String?,
      ) : Nil
        move_folder_request(source, folder_id, parent_id)
      end

      private def move_folder_request(
        source : Source,
        folder_id : String,
        parent_id : String?,
      ) : Bool
        request = {
          "op"     => JSON::Any.new("move-folder"),
          "folder" => JSON::Any.new(folder_id),
        }
        request["parent"] = JSON::Any.new(parent_id) if parent_id
        return false unless call(source, request)

        queue_tree_reload
        true
      end

      private def queue_tree_reload : Nil
        return if @tree_reload_queued

        @tree_reload_queued = true
        GLib.idle_add do
          @tree_reload_queued = false
          reload
          false
        end
      end

      private def confirm_trash_folder(
        source : Source,
        folder_id : String,
      ) : Nil
        name = source.folder_names[folder_id]
        Dialogs.confirm(
          @parent,
          "Move Folder to Trash?",
          "“#{name}” and everything inside it will be moved to the trash.",
          "Move to Trash"
        ) do
          if call(source, {
               "op"     => JSON::Any.new("trash-folder"),
               "folder" => JSON::Any.new(folder_id),
             })
            reload
          end
        end
      end

      private def delete_chat(
        source : Source,
        chat_id : String,
      ) : Nil
        if call(source, {
             "op"   => JSON::Any.new("delete-chat"),
             "chat" => JSON::Any.new(chat_id),
           })
          @on_chat_deleted.call(source.endpoint, chat_id)
          reload
        end
      end

      private def confirm_remove_remote(host : String) : Nil
        Dialogs.confirm(
          @parent,
          "Remove Remote Connection?",
          "“#{host}” will be removed from this device. Its workspaces and " \
          "chats will stay on the remote machine. Pair again to reconnect.",
          "Remove"
        ) do
          begin
            @remote.forget
            @remote_source.clear
            @on_remote_forgot.call
            reload
          rescue error
            @on_error.call(
              error.message || "Cannot remove remote connection."
            )
          end
        end
      end

      private def dialogs(source : Source) : FolderDialogs
        FolderDialogs.new(
          @parent,
          panel_call(source),
          @on_error,
          source.remote
        )
      end

      private def panel_call(source : Source) : PanelCall
        ->(request : Hash(String, JSON::Any)) {
          begin
            PanelCallResult.new(source.endpoint.call(request), nil)
          rescue error : Daemon::Client::Error
            PanelCallResult.new(
              nil,
              error.message || "Daemon request failed."
            )
          end
        }
      end

      private def call(
        source : Source,
        request : Hash(String, JSON::Any),
        quiet : Bool = false,
      ) : Hash(String, JSON::Any)?
        source.endpoint.call(request)
      rescue error : Daemon::Client::Error
        unless quiet
          @on_error.call(error.message || "Daemon request failed.")
        end
        nil
      end

      private def clear(box : Gtk::Box) : Nil
        while child = box.first_child
          box.remove(child)
        end
      end
    end
  end
end
