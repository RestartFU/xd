require "json"
require "gtk4"
require "./adw"
require "./dialogs"
require "./folder_dialogs"
require "./search_dialog"

module Xd
  module UI
    class Sidebar
      getter widget : Adw::ToolbarView
      getter header : Adw::HeaderBar

      ROOT = ""

      @folder_ids = [] of String
      @folder_names = {} of String => String
      @folder_parents = {} of String => String?
      @selected_folder : String?

      def initialize(
        @parent : Gtk::Window,
        @call : Proc(
          Hash(String, JSON::Any),
          Hash(String, JSON::Any)?,
        ),
        @on_chat : Proc(String, String, Nil),
        @on_chat_deleted : Proc(String, Nil),
        @on_pair : Proc(Nil)? = nil,
      )
        @selected_folder = nil
        @dialogs = FolderDialogs.new(@parent, @call)
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
          prompt_new_folder(nil)
        end
        if @on_pair
          add_choice(choices, menu, "Connect to a Machine…") do
            @on_pair.try(&.call)
          end
        end
        add_choice(choices, menu, "Agent Secrets…") do
          @dialogs.secrets
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
        @widget.width_request = 300
        @widget.add_css_class("xd-sidebar")
        @widget.add_top_bar(@header)
        @widget.content = scroll
      end

      def reload : Nil
        response = @call.call({"op" => JSON::Any.new("tree")})
        return unless response

        folders = response["folders"].as_a
        chats = response["chats"].as_a
        clear(@rows)
        @folder_ids.clear
        @folder_names.clear
        @folder_parents.clear

        children = Hash(String, Array(String)).new do |hash, key|
          hash[key] = [] of String
        end
        chats_by_folder = Hash(String, Array(JSON::Any)).new do |hash, key|
          hash[key] = [] of JSON::Any
        end

        folders.each do |folder|
          id = folder["id"].as_s
          parent = folder["parent"]?.try(&.as_s?)
          @folder_ids << id
          @folder_names[id] = folder["name"].as_s
          @folder_parents[id] = parent
          children[parent || ROOT] << id
        end
        chats.each do |chat|
          chats_by_folder[chat["folder"].as_s] << chat
        end

        selected = @selected_folder
        unless selected && @folder_names.has_key?(selected)
          @selected_folder = children[ROOT].first?
        end

        children[ROOT].each do |folder_id|
          add_folder(@rows, folder_id, children, chats_by_folder)
        end

        if @folder_ids.empty?
          empty = Gtk::Label.new("Create a workspace to start")
          empty.wrap = true
          empty.margin_top = 24
          empty.add_css_class("dim-label")
          @rows.append(empty)
        end
      end

      private def add_folder(
        container : Gtk::Box,
        folder_id : String,
        children : Hash(String, Array(String)),
        chats : Hash(String, Array(JSON::Any)),
      ) : Nil
        name = @folder_names[folder_id]
        label = Gtk::Label.new(name)
        label.xalign = 0_f32
        label.hexpand = true

        menu = Gtk::MenuButton.new
        menu.icon_name = "view-more-symbolic"
        menu.tooltip_text = "#{name} actions"
        menu.add_css_class("flat")
        menu.popover = folder_menu(folder_id)

        heading = Gtk::Box.new(:horizontal, 4)
        heading.add_css_class("xd-folder-row")
        heading.append(label)
        heading.append(menu)

        contents = Gtk::Box.new(:vertical, 2)
        contents.margin_start = 14
        children[folder_id].each do |child_id|
          add_folder(contents, child_id, children, chats)
        end
        chats[folder_id].each do |chat|
          add_chat(contents, chat)
        end

        expander = Gtk::Expander.new
        expander.label_widget = heading
        expander.child = contents
        expander.expanded = true
        container.append(expander)
      end

      private def add_chat(
        container : Gtk::Box,
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
          @selected_folder = folder_id
          @on_chat.call(id, title)
        end

        menu = Gtk::MenuButton.new
        menu.icon_name = "view-more-symbolic"
        menu.tooltip_text = "#{title} actions"
        menu.add_css_class("flat")
        menu.popover = chat_menu(id, title)

        row = Gtk::Box.new(:horizontal, 2)
        row.append(open)
        row.append(menu)
        container.append(row)
      end

      private def folder_menu(folder_id : String) : Gtk::Popover
        popover, choices = menu_shell
        add_choice(choices, popover, "New Chat") do
          prompt_new_chat(folder_id)
        end
        add_choice(choices, popover, "New Folder") do
          prompt_new_folder(folder_id)
        end
        add_choice(choices, popover, "Rename…") do
          prompt_rename_folder(folder_id)
        end
        add_choice(choices, popover, "Settings…") do
          @dialogs.settings(folder_id, @folder_names[folder_id])
        end
        add_choice(choices, popover, "Agent Context…") do
          @dialogs.context(folder_id, @folder_names[folder_id])
        end
        add_choice(choices, popover, "Agent Secrets…") do
          @dialogs.secrets(
            folder_id,
            "#{@folder_names[folder_id]} Agent Secrets"
          )
        end

        if @folder_parents[folder_id]?
          add_choice(choices, popover, "Move to top level") do
            move_folder(folder_id, nil)
          end
        end
        @folder_ids.each do |candidate|
          next if candidate == folder_id
          next if descendant?(candidate, folder_id)

          target = candidate
          add_choice(
            choices,
            popover,
            "Move into #{folder_path(target)}"
          ) do
            move_folder(folder_id, target)
          end
        end

        choices.append(Gtk::Separator.new(:horizontal))
        add_choice(choices, popover, "Move to Trash") do
          confirm_trash_folder(folder_id)
        end
        popover
      end

      private def chat_menu(
        chat_id : String,
        title : String,
      ) : Gtk::Popover
        popover, choices = menu_shell
        add_choice(choices, popover, "Rename…") do
          prompt_rename_chat(chat_id, title)
        end
        choices.append(Gtk::Separator.new(:horizontal))
        add_choice(choices, popover, "Delete Chat") do
          confirm_delete_chat(chat_id, title)
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

      private def prompt_new_folder(parent_id : String?) : Nil
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
          if created = @call.call(request)
            @selected_folder = created["id"].as_s
            reload
          end
        end
      end

      private def prompt_new_chat(folder_id : String?) : Nil
        folder = folder_id || @selected_folder || @folder_ids.first?
        unless folder
          created = @call.call({
            "op"   => JSON::Any.new("new-folder"),
            "name" => JSON::Any.new("Workspace"),
          })
          return unless created
          folder = created["id"].as_s
          @selected_folder = folder
          reload
        end

        target = folder.not_nil!
        Dialogs.prompt(
          @parent,
          "New Chat",
          "Chat title",
          "New Chat"
        ) do |title|
          create_chat(target, title)
        end
      end

      private def create_chat(folder_id : String, title : String) : Nil
        created = @call.call({
          "op"     => JSON::Any.new("new-chat"),
          "folder" => JSON::Any.new(folder_id),
          "title"  => JSON::Any.new(title),
        })
        return unless created

        @selected_folder = folder_id
        reload
        @on_chat.call(created["id"].as_s, title)
      end

      private def prompt_rename_folder(folder_id : String) : Nil
        Dialogs.prompt(
          @parent,
          "Rename Folder",
          "Folder name",
          @folder_names[folder_id]
        ) do |name|
          if @call.call({
               "op"     => JSON::Any.new("rename-folder"),
               "folder" => JSON::Any.new(folder_id),
               "name"   => JSON::Any.new(name),
             })
            reload
          end
        end
      end

      private def prompt_rename_chat(
        chat_id : String,
        current : String,
      ) : Nil
        Dialogs.prompt(
          @parent,
          "Rename Chat",
          "Chat title",
          current
        ) do |title|
          if @call.call({
               "op"    => JSON::Any.new("rename-chat"),
               "chat"  => JSON::Any.new(chat_id),
               "title" => JSON::Any.new(title),
             })
            reload
            @on_chat.call(chat_id, title)
          end
        end
      end

      private def move_folder(
        folder_id : String,
        parent_id : String?,
      ) : Nil
        request = {
          "op"     => JSON::Any.new("move-folder"),
          "folder" => JSON::Any.new(folder_id),
        }
        request["parent"] = JSON::Any.new(parent_id) if parent_id
        reload if @call.call(request)
      end

      private def confirm_trash_folder(folder_id : String) : Nil
        name = @folder_names[folder_id]
        Dialogs.confirm(
          @parent,
          "Move #{name} to Trash?",
          "Workspace and everything inside it will leave the sidebar.",
          "Move to Trash"
        ) do
          if @call.call({
               "op"     => JSON::Any.new("trash-folder"),
               "folder" => JSON::Any.new(folder_id),
             })
            reload
          end
        end
      end

      private def confirm_delete_chat(
        chat_id : String,
        title : String,
      ) : Nil
        Dialogs.confirm(
          @parent,
          "Delete #{title}?",
          "Messages and active terminals for this chat will be deleted.",
          "Delete Chat"
        ) do
          if @call.call({
               "op"   => JSON::Any.new("delete-chat"),
               "chat" => JSON::Any.new(chat_id),
             })
            @on_chat_deleted.call(chat_id)
            reload
          end
        end
      end

      private def descendant?(
        candidate : String,
        folder_id : String,
      ) : Bool
        current = @folder_parents[candidate]?
        while current
          return true if current == folder_id
          current = @folder_parents[current]?
        end
        false
      end

      private def folder_path(folder_id : String) : String
        names = [] of String
        current : String? = folder_id
        while current
          names.unshift(@folder_names[current])
          current = @folder_parents[current]?
        end
        names.join(" / ")
      end

      private def clear(box : Gtk::Box) : Nil
        while child = box.first_child
          box.remove(child)
        end
      end
    end
  end
end
