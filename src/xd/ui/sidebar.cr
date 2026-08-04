require "json"
require "gtk4"
require "set"
require "../agent/catalog"
require "../daemon/endpoint"
require "../remote/connection"
require "../version"
require "./adw"
require "./auth_dialog"
require "./daemon_update_dialog"
require "./devices_dialog"
require "./background_work"
require "./dialogs"
require "./directory_browser"
require "./dots"
require "./folder_dialogs"
require "./workspace_create_dialog"
require "./idle_queue"
require "./git_writing_settings"
require "./share_dialog"
require "./shortcut_dialog"
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
      @reload_generation : Int64
      @root_model : Gio::ListStore
      @tree_model : Gtk::TreeListModel
      @tree_build : TreeBuild?
      @tree_rebuild_pending : Bool
      @retired_node_maps : Deque(Hash(String, Node))
      @node_retirement_scheduled : Bool
      @closed : Bool

      private class Source
        record TreeData,
          folder_ids : Array(String),
          folder_names : Hash(String, String),
          folder_parents : Hash(String, String?),
          children : Hash(String, Array(String)),
          chats : Hash(String, Array(JSON::Any)),
          chat_ids : Set(String)

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
          @chat_ids = Set(String).new
        end

        def self.prepare(response : Hash(String, JSON::Any)) : TreeData
          folder_ids = [] of String
          folder_names = {} of String => String
          folder_parents = {} of String => String?
          children = Hash(String, Array(String)).new do |hash, key|
            hash[key] = [] of String
          end
          chats = Hash(String, Array(JSON::Any)).new do |hash, key|
            hash[key] = [] of JSON::Any
          end
          chat_ids = Set(String).new

          response["folders"].as_a.each do |folder|
            id = folder["id"].as_s
            parent = folder["parent"]?.try(&.as_s?)
            folder_ids << id
            folder_names[id] = folder["name"].as_s
            folder_parents[id] = parent
            children[parent || ROOT] << id
          end
          response["chats"].as_a.each do |chat|
            chats[chat["folder"].as_s] << chat
            chat_ids << chat["id"].as_s
          end

          TreeData.new(
            folder_ids,
            folder_names,
            folder_parents,
            children,
            chats,
            chat_ids
          )
        end

        def update(data : TreeData) : Array(String)
          removed = [] of String
          @chat_ids.each do |id|
            removed << id unless data.chat_ids.includes?(id)
          end
          @folder_ids = data.folder_ids
          @folder_names = data.folder_names
          @folder_parents = data.folder_parents
          @children = data.children
          @chats = data.chats
          @chat_ids = data.chat_ids

          selected = @selected_folder
          unless selected && @folder_names.has_key?(selected)
            @selected_folder = @children[ROOT].first?
          end
          @loaded = true
          removed
        end

        def tree_data : TreeData
          TreeData.new(
            @folder_ids,
            @folder_names,
            @folder_parents,
            @children,
            @chats,
            @chat_ids
          )
        end

        def clear : Nil
          @folder_ids.clear
          @folder_names.clear
          @folder_parents.clear
          @children.clear
          @chats.clear
          @chat_ids.clear
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

      private class TreeBuildJob
        getter source : Source
        getter data : Source::TreeData
        getter model : Gio::ListStore

        def initialize(
          @source : Source,
          @data : Source::TreeData,
          @model : Gio::ListStore,
          @folder_ids : Array(String),
          @chats : Array(JSON::Any),
        )
          @folder_index = 0
          @chat_index = 0
        end

        def next_folder : String?
          id = @folder_ids[@folder_index]?
          @folder_index += 1 if id
          id
        end

        def next_chat : JSON::Any?
          chat = @chats[@chat_index]?
          @chat_index += 1 if chat
          chat
        end

        def more? : Bool
          @folder_index < @folder_ids.size || @chat_index < @chats.size
        end
      end

      private class TreeBuild
        getter root_model = Gio::ListStore.new(Gtk::StringObject.g_type)
        getter nodes = {} of String => Node
        getter jobs = IdleQueue(TreeBuildJob).new
        getter chat_states : Hash(String, SidebarState)

        def initialize(@chat_states : Hash(String, SidebarState))
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
        actions : Gtk::Box,
        drag_source : Gtk::DragSource,
        drop_target : Gtk::DropTarget

      getter widget : Adw::ToolbarView
      getter header : Adw::HeaderBar

      ROOT                  = ""
      TREE_BUILD_BATCH      = 64
      NODE_RETIREMENT_BATCH = 64

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
        @git_writing_settings = GitWritingSettings.new(
          @parent,
          -> { shortcuts_dialog(@local_source, nil, "Global Shortcuts") }
        )
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
        @reload_generation = 0_i64
        @tree_build = nil
        @tree_rebuild_pending = false
        @retired_node_maps = Deque(Hash(String, Node)).new
        @node_retirement_scheduled = false
        @closed = false

        @root_model = Gio::ListStore.new(Gtk::StringObject.g_type)
        @tree_model = create_tree_model(@root_model)
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
        # Keep the gesture target covering the unused part of the sidebar so
        # context menus can use the actual pointer location there as well.
        @list_view.hexpand = true
        @list_view.vexpand = true
        @list_view.add_css_class("navigation-sidebar")
        @list_view.add_css_class("xd-sidebar")
        @list_view.activate_signal.connect do |position|
          activate_row(position)
        end
        free_space_menu = Gtk::GestureClick.new
        free_space_menu.button = Gdk::BUTTON_SECONDARY.to_u32
        free_space_menu.pressed_signal.connect do |_presses, x, y|
          target = @list_view.pick(x, y, Gtk::PickFlags::Default)
          if target.nil? || target.to_unsafe == @list_view.to_unsafe
            open_workspace_menu(x, y)
          end
        end
        @list_view.add_controller(free_space_menu)
        @list_view.add_controller(build_drop_target(nil))

        scroll = Gtk::ScrolledWindow.new
        scroll.vexpand = true
        scroll.set_policy(:never, :external)
        scroll.add_css_class("xd-sidebar")
        scroll.child = @list_view

        add = Gtk::MenuButton.new
        add.icon_name = "list-add-symbolic"
        add.tooltip_text = "Add a workspace or a machine"
        menu_model = Gio::Menu.new
        menu = Gtk::PopoverMenu.new_from_model(menu_model)
        menu.add_css_class("xd-menu-popover")
        menu_actions = Gio::SimpleActionGroup.new
        menu.insert_action_group("add", menu_actions)
        add_header_action(
          menu_model,
          menu_actions,
          menu,
          "New Workspace",
          "new-workspace"
        ) do
          begin_creating(@local_source, nil, NodeKind::Folder)
        end
        add_header_action(
          menu_model,
          menu_actions,
          menu,
          "Connect to a Machine…",
          "pair"
        ) do
          @on_pair.call
        end
        add_header_action(
          menu_model,
          menu_actions,
          menu,
          "Manage Devices…",
          "devices"
        ) do
          DevicesDialog.new(@parent, @local_source.endpoint).present
        end
        add_header_action(
          menu_model,
          menu_actions,
          menu,
          "Add a Device…",
          "share"
        ) do
          ShareDialog.new(@parent, @local_source.endpoint).present
        end
        add_header_action(
          menu_model,
          menu_actions,
          menu,
          "Agent Secrets…",
          "secrets"
        ) do
          dialogs(@local_source).secrets
        end
        added = Gio::Menu.new
        menu_model.append_section(nil, added)
        add_header_action(
          added,
          menu_actions,
          menu,
          "Assistant Accounts…",
          "accounts"
        ) do
          auth(@local_source)
        end
        menu.closed_signal.connect do
          # GtkModelButton closes the popover before activating its action.
          # Wait one idle so the action can enter the pending queue first.
          GLib.idle_add do
            finish_menu_action(menu)
            false
          end
        end
        add.popover = menu

        title = Adw::WindowTitle.new(title: "Workspaces")
        @header = Adw::HeaderBar.new
        @header.title_widget = title
        @header.show_end_title_buttons = false
        @header.pack_start(add)

        accounts = Gtk::Button.new_from_icon_name("system-users-symbolic")
        accounts.tooltip_text = "Assistant Accounts"
        accounts.clicked_signal.connect { auth(@local_source) }
        @header.pack_end(accounts)

        @widget = Adw::ToolbarView.new
        @widget.add_css_class("xd-sidebar")
        @widget.add_top_bar(@header)
        @widget.content = scroll

        settings_button = Gtk::Button.new_from_icon_name(
          "preferences-system-symbolic"
        )
        settings_button.add_css_class("flat")
        settings_button.tooltip_text = "Settings"
        settings_button.clicked_signal.connect do
          @git_writing_settings.present
        end
        settings_bar = Gtk::Box.new(:horizontal, 6)
        settings_bar.add_css_class("xd-sidebar-tools")
        settings_bar.margin_start = 6
        settings_bar.margin_end = 6
        settings_bar.margin_top = 3
        settings_bar.margin_bottom = 6
        settings_bar.append(settings_button)
        @updater = Updater.new(@parent)
        settings_bar.append(@updater.widget)
        @widget.add_bottom_bar(settings_bar)

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

        @reload_generation += 1
        generation = @reload_generation
        load_remote = @remote.connected?
        spawn do
          local = request(
            @local_source,
            {"op" => JSON::Any.new("tree")}
          )
          remote = if load_remote
                     request(
                       @remote_source,
                       {"op" => JSON::Any.new("tree")}
                     )
                   end

          queued = BackgroundWork.submit do
            prepare_tree_reload(local, remote, generation)
          end
          unless queued
            GLib.idle_add do
              unless @closed || generation != @reload_generation
                @on_error.call(
                  "Workspace refresh is busy. Try again in a moment."
                )
              end
              false
            end
          end
        end
      end

      private def prepare_tree_reload(
        local : PanelCallResult,
        remote : PanelCallResult?,
        generation : Int64,
      ) : Nil
        local_data : Source::TreeData? = nil
        local_error = local.error
        if response = local.body
          begin
            local_data = Source.prepare(response)
          rescue error : KeyError | TypeCastError
            local_error =
              error.message || "Daemon returned an invalid workspace tree."
          end
        end

        remote_data : Source::TreeData? = nil
        remote_error : String? = nil
        if response = remote.try(&.body)
          begin
            remote_data = Source.prepare(response)
          rescue error : KeyError | TypeCastError
            remote_error =
              error.message || "Remote returned an invalid workspace tree."
          end
        end

        GLib.idle_add do
          unless @closed || generation != @reload_generation
            if data = local_data
              removed = @local_source.update(data)
              removed.each do |id|
                @on_chat_deleted.call(@local_source.endpoint, id)
              end
            elsif message = local_error
              @on_error.call(message)
            end
            if data = remote_data
              removed = @remote_source.update(data)
              removed.each do |id|
                @on_chat_deleted.call(@remote_source.endpoint, id)
              end
            elsif message = remote_error
              @on_error.call(message)
            end
            rebuild_tree
          end
          false
        end
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
        @closed = true
        @reload_generation += 1
        @remote.unsubscribe(@remote_state_subscription)
        @updater.close
      end

      private def rebuild_tree : Nil
        if @tree_build
          @tree_rebuild_pending = true
          return
        end

        build = TreeBuild.new(@chat_states.dup)
        add_source_build_job(build, @local_source)
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
          append_build_node(build, build.root_model, root)
          add_source_build_job(build, @remote_source, root.children)
        end
        @tree_build = build
        process_tree_build(build)
      end

      private def add_source_build_job(
        build : TreeBuild,
        source : Source,
        model : Gio::ListStore = build.root_model,
      ) : Nil
        data = source.tree_data
        job = TreeBuildJob.new(
          source,
          data,
          model,
          data.children[ROOT],
          [] of JSON::Any
        )
        build.jobs << job if job.more?
      end

      private def process_tree_build(build : TreeBuild) : Nil
        return unless @tree_build.same?(build)

        build.jobs.drain(TREE_BUILD_BATCH) do |job|
          process_tree_build_job(build, job)
        end

        if build.jobs.empty?
          finish_tree_build(build)
        else
          GLib.idle_add do
            process_tree_build(build) unless @closed
            false
          end
        end
      end

      private def process_tree_build_job(
        build : TreeBuild,
        job : TreeBuildJob,
      ) : Nil
        if folder_id = job.next_folder
          prefix = job.source.remote ? "remote" : "local"
          node = Node.new(
            "#{prefix}/folder/#{folder_id}",
            folder_id,
            job.data.folder_names[folder_id],
            NodeKind::Folder,
            job.source,
            folder_id: folder_id
          )
          append_build_node(build, job.model, node)
          build.jobs << job if job.more?

          children = TreeBuildJob.new(
            job.source,
            job.data,
            node.children,
            job.data.children[folder_id],
            job.data.chats[folder_id]
          )
          build.jobs << children if children.more?
          return
        end

        chat = job.next_chat || return
        add_build_chat_node(build, job.source, chat, job.model)
        build.jobs << job if job.more?
      end

      private def add_build_chat_node(
        build : TreeBuild,
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
        current = build.chat_states[key]? || SidebarState::Idle
        state = current.reconcile_tree(
          chat["working"]?.try(&.as_bool?) == true,
          @active_chat_key == key,
          source.remote
        )
        build.chat_states[key] = state
        append_build_node(
          build,
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

      private def append_build_node(
        build : TreeBuild,
        model : Gio::ListStore,
        node : Node,
      ) : Nil
        build.nodes[node.key] = node
        model.append(Gtk::StringObject.new(node.key))
      end

      private def finish_tree_build(build : TreeBuild) : Nil
        return unless @tree_build.same?(build)

        @tree_build = nil
        if @closed
          retire_node_map(build.nodes)
          return
        end
        if @tree_rebuild_pending
          @tree_rebuild_pending = false
          retire_node_map(build.nodes)
          rebuild_tree
          return
        end

        old_nodes = @nodes
        @nodes = build.nodes
        @chat_states = build.chat_states
        @chat_states.select! { |key, _state| @nodes.has_key?(key) }
        @root_model = build.root_model
        @tree_model = create_tree_model(@root_model)
        @selection.model = @tree_model
        retire_node_map(old_nodes)
        queue_restore
      end

      private def retire_node_map(nodes : Hash(String, Node)) : Nil
        return if nodes.empty?

        @retired_node_maps << nodes
        return if @node_retirement_scheduled

        @node_retirement_scheduled = true
        GLib.idle_add do
          NODE_RETIREMENT_BATCH.times do
            retired = @retired_node_maps.first?
            break unless retired

            retired.shift?
            @retired_node_maps.shift if retired.empty?
          end
          more = !@retired_node_maps.empty?
          @node_retirement_scheduled = more
          more
        end
      end

      private def create_tree_model(
        root_model : Gio::ListStore,
      ) : Gtk::TreeListModel
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
        GICrystal.ref(root_model)
        tree_model = LibGtk.gtk_tree_list_model_new(
          root_model.to_unsafe,
          0,
          0,
          create_children.pointer,
          child_model_data,
          destroy_children.pointer
        )
        Gtk::TreeListModel.new(tree_model, GICrystal::Transfer::Full)
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
        actions = Gtk::Box.new(:horizontal, 0)
        rename = Gtk::Button.new_from_icon_name("document-edit-symbolic")
        delete = Gtk::Button.new_from_icon_name("user-trash-symbolic")
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

        actions.visible = false
        actions.opacity = 0.0
        actions.can_target = false
        actions.valign = :center
        rename.add_css_class("flat")
        rename.add_css_class("xd-sidebar-row-action")
        rename.tooltip_text = "Rename Chat"
        delete.add_css_class("flat")
        delete.add_css_class("xd-sidebar-row-action")
        delete.tooltip_text = "Delete Chat"
        actions.append(rename)
        actions.append(delete)

        rename.clicked_signal.connect do
          if node = @bound_nodes[pointer_key(box)]?
            begin_renaming(node) if node.chat? && !node.placeholder?
          end
        end
        delete.clicked_signal.connect do
          if node = @bound_nodes[pointer_key(box)]?
            if node.chat? && !node.placeholder?
              confirm_delete_chat(node.source, node.id, node.name)
            end
          end
        end

        hover = Gtk::EventControllerMotion.new
        hover.enter_signal.connect do |_x, _y|
          reveal_row_actions(box, actions, true)
        end
        hover.leave_signal.connect do
          reveal_row_actions(box, actions, false)
        end
        box.add_controller(hover)

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
        box.append(actions)

        gesture = Gtk::GestureClick.new
        gesture.button = Gdk::BUTTON_SECONDARY.to_u32
        gesture.pressed_signal.connect do |_presses, x, y|
          if node = @bound_nodes[pointer_key(box)]?
            open_row_menu(box, node, x, y)
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
          actions,
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
        if node.backend == "codex"
          widgets.icon.add_css_class("xd-backend-codex")
        else
          widgets.icon.remove_css_class("xd-backend-codex")
        end
        widgets.label.text = node.name
        show_state(widgets, node)

        @bound_nodes[pointer_key(widgets.box)] = node
        reveal_row_actions(widgets.box, widgets.actions, false)
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
          reveal_row_actions(widgets.box, widgets.actions, false)
          @bound_nodes.delete(pointer_key(widgets.box))
          widgets.actions.visible = false
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
        if build = @tree_build
          build.chat_states[key] = state
          build.nodes[key]?.try { |pending| pending.state = state }
        end
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
        reveal_row_actions(widgets.box, widgets.actions, false)
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

      private def reveal_row_actions(
        box : Gtk::Box,
        actions : Gtk::Box,
        reveal : Bool,
      ) : Nil
        node = @bound_nodes[pointer_key(box)]?
        eligible = !!node && node.chat? && !node.placeholder?
        actions.visible = eligible
        active = eligible && reveal && @editing_key != node.try(&.key)
        actions.opacity = active ? 1.0 : 0.0
        actions.can_target = active
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
        if kind.folder? && parent.nil?
          WorkspaceCreateDialog.new(
            @parent,
            panel_call(source),
            ->(name : String, repo : String?, repo_url : String?) {
              create_folder(source, nil, name, repo, repo_url)
              nil
            }
          ).present
          return
        end

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

      private def open_row_menu(
        box : Gtk::Box,
        node : Node,
        x : Float64,
        y : Float64,
      ) : Nil
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
        present_menu(
          popover,
          box,
          Gdk::Rectangle.new(x.to_i32, y.to_i32, 1, 1)
        )
      end

      private def open_workspace_menu(x : Float64, y : Float64) : Nil
        if previous = @row_popover
          previous.popdown
        end

        popover, menu, actions = row_menu_shell
        add_menu_action(
          menu,
          actions,
          popover,
          "New Workspace",
          "new-workspace"
        ) do
          begin_creating(@local_source, nil, NodeKind::Folder)
        end
        present_menu(
          popover,
          @list_view,
          Gdk::Rectangle.new(x.to_i32, y.to_i32, 1, 1)
        )
      end

      private def present_menu(
        popover : Gtk::Popover,
        anchor : Gtk::Widget,
        pointing_to : Gdk::Rectangle? = nil,
      ) : Nil
        popover.has_arrow = false
        popover.halign = :start
        popover.parent = anchor
        popover.pointing_to = pointing_to if pointing_to
        @row_popover = popover
        popover.closed_signal.connect do
          # GtkModelButton closes a popover before its action runs. Keep the
          # anchor attached through that activation, exactly as the C sidebar
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
        # These context menus are attached after the already-visible list or
        # row has been allocated. Recompute the popup layout now, otherwise
        # GTK can retain the parent's origin and open the menu at the top.
        popover.present
        popover.popup
      end

      private def pointer_key(object : GObject::Object) : UInt64
        object.to_unsafe.address
      end

      private def remote_menu(node : Node) : Gtk::Popover
        popover, menu, actions = row_menu_shell
        add_menu_action(
          menu, actions, popover, "New Workspace", "new-workspace"
        ) do
          begin_creating(node.source, node, NodeKind::Folder)
        end
        add_menu_action(
          menu, actions, popover, "Agent Secrets…", "agent-secrets"
        ) do
          dialogs(node.source).secrets
        end
        add_menu_action(
          menu, actions, popover, "Global Shortcuts…", "shortcuts"
        ) do
          shortcuts_dialog(node.source, nil, "Global Shortcuts")
        end
        add_menu_action(
          menu, actions, popover, "Assistant Accounts…", "accounts"
        ) do
          auth(node.source, node.name)
        end
        add_menu_action(
          menu, actions, popover, "Update Daemon…", "update-daemon"
        ) do
          DaemonUpdateDialog.new(@parent, node.source.endpoint, node.name).present
        end
        add_menu_action(menu, actions, popover, "Refresh", "refresh") do
          reload
        end
        section = Gio::Menu.new
        menu.append_section(nil, section)
        add_menu_action(
          section, actions, popover, "Remove Connection…", "remove"
        ) do
          confirm_remove_remote(node.name)
        end
        popover
      end

      private def folder_menu(node : Node) : Gtk::Popover
        source = node.source
        folder_id = node.id
        popover, menu, actions = row_menu_shell
        add_menu_action(menu, actions, popover, "New Chat", "new-chat") do
          begin_creating(source, node, NodeKind::Chat)
        end
        add_menu_action(
          menu, actions, popover, "New Folder", "new-folder"
        ) do
          begin_creating(source, node, NodeKind::Folder)
        end
        add_menu_action(menu, actions, popover, "Rename…", "rename") do
          begin_renaming(node)
        end
        add_menu_action(
          menu, actions, popover, "Agent Context…", "context"
        ) do
          dialogs(source).context(
            folder_id,
            source.folder_names[folder_id]
          )
        end
        add_menu_action(
          menu, actions, popover, "Agent Secrets…", "secrets"
        ) do
          dialogs(source).secrets(
            folder_id,
            source.folder_names[folder_id]
          )
        end
        add_menu_action(
          menu, actions, popover, "Prompt Shortcuts…", "shortcuts"
        ) do
          shortcuts_dialog(
            source,
            folder_id,
            "#{source.folder_names[folder_id]} Shortcuts"
          )
        end
        unless source.remote
          add_menu_action(
            menu, actions, popover, "Folder Settings…", "settings"
          ) do
            dialogs(source).settings(
              folder_id,
              source.folder_names[folder_id]
            )
          end
        end

        section = Gio::Menu.new
        menu.append_section(nil, section)
        add_menu_action(
          section, actions, popover, "Move to Trash", "trash"
        ) do
          confirm_trash_folder(source, folder_id)
        end
        popover
      end

      private def chat_menu(node : Node) : Gtk::Popover
        source = node.source
        chat_id = node.id
        popover, menu, actions = row_menu_shell
        add_menu_action(menu, actions, popover, "Rename…", "rename") do
          begin_renaming(node)
        end
        section = Gio::Menu.new
        menu.append_section(nil, section)
        add_menu_action(
          section, actions, popover, "Delete Chat", "delete"
        ) do
          confirm_delete_chat(source, chat_id, node.name)
        end
        popover
      end

      private def row_menu_shell : {Gtk::PopoverMenu, Gio::Menu, Gio::SimpleActionGroup}
        menu = Gio::Menu.new
        popover = Gtk::PopoverMenu.new_from_model(menu)
        popover.add_css_class("xd-menu-popover")
        actions = Gio::SimpleActionGroup.new
        popover.insert_action_group("row", actions)
        {popover, menu, actions}
      end

      private def add_menu_action(
        menu : Gio::Menu,
        actions : Gio::SimpleActionGroup,
        popover : Gtk::Popover,
        label : String,
        name : String,
        &callback : -> Nil
      ) : Nil
        action = Gio::SimpleAction.new(name, nil)
        action.activate_signal.connect do
          queue_menu_action(popover, callback)
        end
        actions.add_action(action)
        menu.append(label, "row.#{name}")
      end

      private def add_header_action(
        menu : Gio::Menu,
        actions : Gio::SimpleActionGroup,
        popover : Gtk::Popover,
        label : String,
        name : String,
        &callback : -> Nil
      ) : Nil
        action = Gio::SimpleAction.new(name, nil)
        action.activate_signal.connect do
          queue_menu_action(popover, callback)
        end
        actions.add_action(action)
        menu.append(label, "add.#{name}")
      end

      private def queue_menu_action(
        popover : Gtk::Popover,
        action : Proc(Nil),
      ) : Nil
        @pending_menu = popover
        @pending_menu_action = action
        popover.popdown

        # GtkModelButton normally emits ::closed before activating the action,
        # but the activation may arrive after the close handler's idle has
        # already run. In that ordering there is nobody left to start an
        # inline create/rename operation. Schedule the same idempotent finish
        # from activation so either event ordering completes the action.
        GLib.idle_add do
          finish_menu_action(popover) unless popover.visible?
          false
        end
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
        repo : String? = nil,
        repo_url : String? = nil,
      ) : Nil
        request = {
          "op"   => JSON::Any.new("new-folder"),
          "name" => JSON::Any.new(name),
        }
        request["parent"] = JSON::Any.new(parent_id) if parent_id
        request["repo"] = JSON::Any.new(repo) if repo
        request["repo_url"] = JSON::Any.new(repo_url) if repo_url
        call_async(
          source,
          request,
          error_heading: "Could not create the folder"
        ) do |created|
          next unless created

          source.selected_folder = created["id"].as_s
          reload
        end
      end

      private def create_chat(
        source : Source,
        folder_id : String,
        title : String,
      ) : Nil
        call_async(
          source,
          {
            "op"     => JSON::Any.new("folder-settings"),
            "folder" => JSON::Any.new(folder_id),
          },
          error_heading: "Could not start the chat"
        ) do |settings|
          next unless settings

          DirectoryBrowser.present(
            @parent,
            panel_call(source),
            settings["effective_workdir"]?.try(&.as_s?)
          ) do |workdir|
            create_chat_in(source, folder_id, title, workdir)
          end
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
        call_async(
          source,
          request,
          error_heading: "Could not start the chat"
        ) do |created|
          next unless created

          source.selected_folder = folder_id
          @restore_chat_id = created["id"].as_s
          @restore_chat_remote = source.remote
          reload
        end
      end

      private def rename_folder(node : Node, name : String) : Nil
        call_async(
          node.source,
          {
            "op"     => JSON::Any.new("rename-folder"),
            "folder" => JSON::Any.new(node.id),
            "name"   => JSON::Any.new(name),
          },
          error_heading: "Could not rename the folder"
        ) do |renamed|
          reload if renamed
        end
      end

      private def rename_chat(node : Node, title : String) : Nil
        call_async(
          node.source,
          {
            "op"    => JSON::Any.new("rename-chat"),
            "chat"  => JSON::Any.new(node.id),
            "title" => JSON::Any.new(title),
          },
          error_heading: "Could not rename the chat"
        ) do |renamed|
          next unless renamed

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
        call_async(
          source,
          request,
          error_heading: "Cannot Move the Folder"
        ) do |moved|
          queue_tree_reload if moved
        end
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
          call_async(
            source,
            {
              "op"     => JSON::Any.new("trash-folder"),
              "folder" => JSON::Any.new(folder_id),
            },
            error_heading: "Could not move the folder to the trash"
          ) do |trashed|
            reload if trashed
          end
        end
      end

      private def delete_chat(
        source : Source,
        chat_id : String,
      ) : Nil
        call_async(
          source,
          {
            "op"   => JSON::Any.new("delete-chat"),
            "chat" => JSON::Any.new(chat_id),
          },
          error_heading: "Could not delete the chat"
        ) do |deleted|
          next unless deleted

          @on_chat_deleted.call(source.endpoint, chat_id)
          reload
        end
      end

      private def confirm_delete_chat(
        source : Source,
        chat_id : String,
        name : String,
      ) : Nil
        Dialogs.confirm(
          @parent,
          "Delete Chat?",
          "“#{name}” and all its messages will be permanently deleted. " \
          "This cannot be undone.",
          "Delete"
        ) do
          delete_chat(source, chat_id)
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

      private def shortcuts_dialog(
        source : Source,
        folder_id : String?,
        title : String,
      ) : Nil
        ShortcutDialog.new(
          @parent,
          panel_call(source),
          @on_error,
          folder_id,
          title
        ).present
      end

      private def auth(source : Source, machine : String? = nil) : Nil
        AuthDialog.new(@parent, source.endpoint, machine).present
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

      private def request(
        source : Source,
        request : Hash(String, JSON::Any),
      ) : PanelCallResult
        PanelCallResult.new(source.endpoint.call(request), nil)
      rescue error : Daemon::Client::Error
        PanelCallResult.new(
          nil,
          error.message || "Daemon request failed."
        )
      end

      private def call_async(
        source : Source,
        request_fields : Hash(String, JSON::Any),
        quiet : Bool = false,
        error_heading : String? = nil,
        &complete : Hash(String, JSON::Any)? -> Nil
      ) : Nil
        spawn do
          result = request(source, request_fields)
          GLib.idle_add do
            unless @closed
              if message = result.error
                unless quiet
                  if heading = error_heading
                    show_error(heading, message)
                  else
                    @on_error.call(message)
                  end
                end
                complete.call(nil)
              else
                complete.call(result.body)
              end
            end
            false
          end
        end
      end

      private def show_error(heading : String, message : String) : Nil
        Dialogs.alert(@parent, heading, message)
      end

      private def clear(box : Gtk::Box) : Nil
        while child = box.first_child
          box.remove(child)
        end
      end
    end
  end
end
