require "json"
require "gtk4"
require "../daemon/endpoint"
require "../remote/connection"
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
        @rows = Gtk::Box.new(:vertical, 2)
        @rows.margin_top = 6
        @rows.margin_bottom = 6
        @rows.margin_start = 6
        @rows.margin_end = 6

        scroll = Gtk::ScrolledWindow.new
        scroll.vexpand = true
        scroll.child = @rows

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

        clear(@rows)
        render_source(@rows, @local_source)
        render_remote if @remote.configured?

        if @local_source.folder_ids.empty? && !@remote.configured?
          empty = Gtk::Label.new("Create a workspace to start")
          empty.wrap = true
          empty.margin_top = 24
          empty.add_css_class("dim-label")
          @rows.append(empty)
        end
      end

      def reload(endpoint : Daemon::Endpoint) : Nil
        return reload if endpoint.same?(@local_source.endpoint)
        return reload if endpoint.same?(@remote_source.endpoint)
      end

      def close : Nil
        @remote.unsubscribe(@remote_state_subscription)
      end

      private def render_remote : Nil
        snapshot = @remote.snapshot
        host = snapshot.host || "Remote"

        icon = Gtk::Image.new_from_icon_name("network-server-symbolic")
        icon.add_css_class("xd-offline") if snapshot.state.offline?

        label = Gtk::Label.new(host)
        label.xalign = 0_f32
        label.hexpand = true
        label.add_css_class("xd-offline") if snapshot.state.offline?

        menu = Gtk::MenuButton.new
        menu.icon_name = "view-more-symbolic"
        menu.tooltip_text = "#{host} actions"
        menu.add_css_class("flat")
        menu.popover = remote_menu(host)

        heading = Gtk::Box.new(:horizontal, 6)
        heading.append(icon)
        heading.append(label)
        heading.append(menu)

        contents = Gtk::Box.new(:vertical, 2)
        contents.margin_start = 14
        if @remote_source.loaded
          render_source(contents, @remote_source)
        else
          status = case snapshot.state
                   when Remote::ConnectionState::Connecting
                     "Connecting…"
                   when Remote::ConnectionState::Offline
                     snapshot.error || "Remote unavailable"
                   else
                     "No workspaces"
                   end
          message = Gtk::Label.new(status)
          message.xalign = 0_f32
          message.wrap = true
          message.add_css_class("dim-label")
          message.add_css_class("xd-offline") if snapshot.state.offline?
          contents.append(message)
        end

        expander = Gtk::Expander.new
        expander.label_widget = heading
        expander.child = contents
        expander.expanded = true
        @rows.append(expander)
      end

      private def render_source(
        container : Gtk::Box,
        source : Source,
      ) : Nil
        source.children[ROOT].each do |folder_id|
          add_folder(container, source, folder_id)
        end
      end

      private def add_folder(
        container : Gtk::Box,
        source : Source,
        folder_id : String,
      ) : Nil
        name = source.folder_names[folder_id]
        label = Gtk::Label.new(name)
        label.xalign = 0_f32
        label.hexpand = true

        menu = Gtk::MenuButton.new
        menu.icon_name = "view-more-symbolic"
        menu.tooltip_text = "#{name} actions"
        menu.add_css_class("flat")
        menu.popover = folder_menu(source, folder_id)

        heading = Gtk::Box.new(:horizontal, 4)
        heading.add_css_class("xd-folder-row")
        heading.append(label)
        heading.append(menu)

        contents = Gtk::Box.new(:vertical, 2)
        contents.margin_start = 14
        source.children[folder_id].each do |child_id|
          add_folder(contents, source, child_id)
        end
        source.chats[folder_id].each do |chat|
          add_chat(contents, source, chat)
        end

        expander = Gtk::Expander.new
        expander.label_widget = heading
        expander.child = contents
        expander.expanded = true
        container.append(expander)
      end

      private def add_chat(
        container : Gtk::Box,
        source : Source,
        chat : JSON::Any,
      ) : Nil
        id = chat["id"].as_s
        folder_id = chat["folder"].as_s
        title = chat["title"].as_s? || "New Chat"
        title = "New Chat" if title.empty?
        display = chat["working"]?.try(&.as_bool?) == true ? "#{title}  •" : title

        open = Gtk::Button.new_with_label(display)
        open.hexpand = true
        open.halign = :fill
        open.add_css_class("flat")
        open.add_css_class("xd-chat-row")
        open.clicked_signal.connect do
          source.selected_folder = folder_id
          @on_chat.call(source.endpoint, id, title)
        end

        menu = Gtk::MenuButton.new
        menu.icon_name = "view-more-symbolic"
        menu.tooltip_text = "#{title} actions"
        menu.add_css_class("flat")
        menu.popover = chat_menu(source, id, title)

        row = Gtk::Box.new(:horizontal, 2)
        row.append(open)
        row.append(menu)
        container.append(row)
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
