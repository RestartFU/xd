require "json"
require "gtk4"
require "set"
require "../agent/catalog"
require "../daemon/endpoint"
require "../remote/connection"
require "../version"
require "./adw"
require "./dialogs"
require "./folder_dialogs"

module Xd
  module UI
    class Sidebar
      @remote_state_subscription : Int64

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
        getter working : Bool
        getter offline : Bool

        def initialize(
          @key : String,
          @id : String,
          @name : String,
          @kind : NodeKind,
          @source : Source,
          @folder_id : String? = nil,
          @backend : String? = nil,
          @working : Bool = false,
          @offline : Bool = false,
        )
          @children = Gio::ListStore.new(Gtk::StringObject.g_type)
        end

        def folder? : Bool
          @kind.folder? || @kind.remote_root?
        end

        def chat? : Bool
          @kind.chat?
        end

        def icon_name : String
          return @offline ? "network-offline-symbolic" : "network-server-symbolic" if @kind.remote_root?
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
        working : Gtk::Label,
        label : Gtk::Label,
        entry : Gtk::Entry

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
        @restore_queued = false
        @restoring_expanded = false
        @closing = {} of UInt64 => {Gtk::TreeListRow, String}
        @save_expanded_queued = false

        @root_model = Gio::ListStore.new(Gtk::StringObject.g_type)
        create_children = ->(object : GObject::Object) : Gio::ListModel {
          string = Gtk::StringObject.new(
            object.to_unsafe,
            GICrystal::Transfer::None
          )
          model = @nodes[string.string].children
          LibGObject.g_object_ref(model.to_unsafe)
          model
        }
        @tree_model = Gtk::TreeListModel.new(
          @root_model,
          false,
          false,
          create_children
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
          prompt_new_folder(@local_source, nil)
        end
        add_choice(choices, menu, "Connect to a Machine…") do
          @on_pair.call
        end
        add_choice(choices, menu, "Agent Secrets…") do
          dialogs(@local_source).secrets
        end
        menu.child = choices
        menu.add_css_class("xd-menu-popover")
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

        @remote_state_subscription = @remote.on_state do |_snapshot|
          GLib.idle_add do
            reload
            false
          end
        end
      end

      def reload : Nil
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

      def close : Nil
        @remote.unsubscribe(@remote_state_subscription)
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
            offline: !snapshot.state.connected?
          )
          append_node(@root_model, root)
          add_source_roots(@remote_source, root.children)
        end
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
        append_node(
          model,
          Node.new(
            "#{prefix}/chat/#{id}",
            id,
            title,
            NodeKind::Chat,
            source,
            folder_id: folder_id,
            backend: chat["backend"]?.try(&.as_s?),
            working: chat["working"]?.try(&.as_bool?) == true
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
        working = Gtk::Label.new("...")
        label = Gtk::Label.new("")
        entry = Gtk::Entry.new

        label.xalign = 0_f32
        label.ellipsize = :end
        label.hexpand = true

        entry.visible = false
        entry.hexpand = true
        entry.valign = :center
        entry.add_css_class("xd-inline-entry")

        working.visible = false
        working.valign = :center
        working.add_css_class("dim-label")

        status.visible = false
        status.halign = :end
        status.valign = :end
        status.can_target = false
        status.add_css_class("xd-status-dot")

        icon_overlay.child = icon
        icon_overlay.add_overlay(status)
        box.append(icon_overlay)
        box.append(working)
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
          entry
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
        widgets.label.visible = true
        widgets.entry.visible = false
        widgets.working.visible = node.working
        widgets.icon_overlay.visible = !node.working
        widgets.status.visible = false

        if node.offline
          widgets.icon.add_css_class("xd-offline")
          widgets.icon_overlay.tooltip_text =
            "Not connected. Trying again every few seconds."
        else
          widgets.icon.remove_css_class("xd-offline")
          widgets.icon_overlay.tooltip_text = nil
        end

        @bound_nodes[pointer_key(widgets.box)] = node
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
        end
      end

      private def selection_changed : Nil
        object = @selection.selected_item || return
        row = Gtk::TreeListRow.new(
          object.to_unsafe,
          GICrystal::Transfer::None
        )
        node = node_for_row(row) || return
        return if @selected_key == node.key

        @selected_key = node.key
        if folder_id = node.folder_id
          node.source.selected_folder = folder_id
        end
        open_node(node) if node.chat?
      end

      private def activate_row(position : UInt32) : Nil
        row = @tree_model.row(position) || return
        node = node_for_row(row) || return

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
        return if @restoring_expanded

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

        selected = @selected_key
        return unless selected

        index = 0_u32
        while index < @tree_model.n_items
          row = @tree_model.row(index)
          if row && node_for_row(row).try(&.key) == selected
            @selection.selected = index
            break
          end
          index += 1
        end
      ensure
        @restoring_expanded = false
      end

      private def open_row_menu(box : Gtk::Box, node : Node) : Nil
        if previous = @row_popover
          previous.popdown
          previous.unparent if previous.parent
        end

        popover = case node.kind
                  when NodeKind::RemoteRoot
                    remote_menu(node.name)
                  when NodeKind::Folder
                    folder_menu(node.source, node.id)
                  when NodeKind::Chat
                    chat_menu(node.source, node.id, node.name)
                  end
        popover.has_arrow = false
        popover.halign = :start
        popover.parent = box
        @row_popover = popover
        popover.closed_signal.connect do
          popover.unparent if popover.parent
          if @row_popover.try(&.to_unsafe) == popover.to_unsafe
            @row_popover = nil
          end
        end
        popover.popup
      end

      private def pointer_key(object : GObject::Object) : UInt64
        object.to_unsafe.address
      end

      private def remote_menu(host : String) : Gtk::Popover
        popover, choices = menu_shell
        add_choice(choices, popover, "New Workspace") do
          prompt_new_folder(@remote_source, nil)
        end
        add_choice(choices, popover, "Agent Secrets…") do
          dialogs(@remote_source).secrets
        end
        add_choice(choices, popover, "Refresh") { reload }
        choices.append(Gtk::Separator.new(:horizontal))
        add_choice(choices, popover, "Remove Connection…") do
          confirm_remove_remote(host)
        end
        popover
      end

      private def folder_menu(
        source : Source,
        folder_id : String,
      ) : Gtk::Popover
        popover, choices = menu_shell
        add_choice(choices, popover, "New Chat") do
          prompt_new_chat(source, folder_id)
        end
        add_choice(choices, popover, "New Folder") do
          prompt_new_folder(source, folder_id)
        end
        add_choice(choices, popover, "Rename…") do
          prompt_rename_folder(source, folder_id)
        end
        add_choice(choices, popover, "Settings…") do
          dialogs(source).settings(
            folder_id,
            source.folder_names[folder_id]
          )
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
            "#{source.folder_names[folder_id]} Agent Secrets"
          )
        end

        if source.folder_parents[folder_id]?
          add_choice(choices, popover, "Move to top level") do
            move_folder(source, folder_id, nil)
          end
        end
        source.folder_ids.each do |candidate|
          next if candidate == folder_id
          next if descendant?(source, candidate, folder_id)

          target = candidate
          add_choice(
            choices,
            popover,
            "Move into #{folder_path(source, target)}"
          ) do
            move_folder(source, folder_id, target)
          end
        end

        choices.append(Gtk::Separator.new(:horizontal))
        add_choice(choices, popover, "Move to Trash") do
          confirm_trash_folder(source, folder_id)
        end
        popover
      end

      private def chat_menu(
        source : Source,
        chat_id : String,
        title : String,
      ) : Gtk::Popover
        popover, choices = menu_shell
        add_choice(choices, popover, "Rename…") do
          prompt_rename_chat(source, chat_id, title)
        end
        choices.append(Gtk::Separator.new(:horizontal))
        add_choice(choices, popover, "Delete Chat") do
          confirm_delete_chat(source, chat_id, title)
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
          popover.popdown
          action.call
        end
        choices.append(button)
      end

      private def prompt_new_folder(
        source : Source,
        parent_id : String?,
      ) : Nil
        workspace = parent_id.nil?
        Dialogs.prompt(
          @parent,
          workspace ? "New Workspace" : "New Folder",
          workspace ? "Workspace name" : "Folder name",
          workspace ? "New Workspace" : "New Folder"
        ) do |name|
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
      end

      private def prompt_new_chat(
        source : Source,
        folder_id : String?,
      ) : Nil
        folder = folder_id ||
                 source.selected_folder ||
                 source.folder_ids.first?
        unless folder
          created = call(source, {
            "op"   => JSON::Any.new("new-folder"),
            "name" => JSON::Any.new("Workspace"),
          })
          return unless created
          folder = created["id"].as_s
          source.selected_folder = folder
          reload
        end

        target = folder.not_nil!
        Dialogs.prompt(
          @parent,
          "New Chat",
          "Chat title",
          "New Chat"
        ) do |title|
          create_chat(source, target, title)
        end
      end

      private def create_chat(
        source : Source,
        folder_id : String,
        title : String,
      ) : Nil
        created = call(source, {
          "op"     => JSON::Any.new("new-chat"),
          "folder" => JSON::Any.new(folder_id),
          "title"  => JSON::Any.new(title),
        })
        return unless created

        source.selected_folder = folder_id
        reload
        @on_chat.call(source.endpoint, created["id"].as_s, title)
      end

      private def prompt_rename_folder(
        source : Source,
        folder_id : String,
      ) : Nil
        Dialogs.prompt(
          @parent,
          "Rename Folder",
          "Folder name",
          source.folder_names[folder_id]
        ) do |name|
          if call(source, {
               "op"     => JSON::Any.new("rename-folder"),
               "folder" => JSON::Any.new(folder_id),
               "name"   => JSON::Any.new(name),
             })
            reload
          end
        end
      end

      private def prompt_rename_chat(
        source : Source,
        chat_id : String,
        current : String,
      ) : Nil
        Dialogs.prompt(
          @parent,
          "Rename Chat",
          "Chat title",
          current
        ) do |title|
          if call(source, {
               "op"    => JSON::Any.new("rename-chat"),
               "chat"  => JSON::Any.new(chat_id),
               "title" => JSON::Any.new(title),
             })
            reload
            @on_chat.call(source.endpoint, chat_id, title)
          end
        end
      end

      private def move_folder(
        source : Source,
        folder_id : String,
        parent_id : String?,
      ) : Nil
        request = {
          "op"     => JSON::Any.new("move-folder"),
          "folder" => JSON::Any.new(folder_id),
        }
        request["parent"] = JSON::Any.new(parent_id) if parent_id
        reload if call(source, request)
      end

      private def confirm_trash_folder(
        source : Source,
        folder_id : String,
      ) : Nil
        name = source.folder_names[folder_id]
        Dialogs.confirm(
          @parent,
          "Move #{name} to Trash?",
          "Workspace and everything inside it will leave the sidebar.",
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

      private def confirm_delete_chat(
        source : Source,
        chat_id : String,
        title : String,
      ) : Nil
        Dialogs.confirm(
          @parent,
          "Delete #{title}?",
          "Messages and active terminals for this chat will be deleted.",
          "Delete Chat"
        ) do
          if call(source, {
               "op"   => JSON::Any.new("delete-chat"),
               "chat" => JSON::Any.new(chat_id),
             })
            @on_chat_deleted.call(source.endpoint, chat_id)
            reload
          end
        end
      end

      private def confirm_remove_remote(host : String) : Nil
        Dialogs.confirm(
          @parent,
          "Remove Remote Connection?",
          "“#{host}” will be removed from this device. Its workspaces and " \
          "chats will stay on the remote machine.",
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

      private def descendant?(
        source : Source,
        candidate : String,
        folder_id : String,
      ) : Bool
        current = source.folder_parents[candidate]?
        while current
          return true if current == folder_id
          current = source.folder_parents[current]?
        end
        false
      end

      private def folder_path(
        source : Source,
        folder_id : String,
      ) : String
        names = [] of String
        current : String? = folder_id
        while current
          names.unshift(source.folder_names[current])
          current = source.folder_parents[current]?
        end
        names.join(" / ")
      end

      private def dialogs(source : Source) : FolderDialogs
        FolderDialogs.new(
          @parent,
          ->(request : Hash(String, JSON::Any)) {
            call(source, request)
          }
        )
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
