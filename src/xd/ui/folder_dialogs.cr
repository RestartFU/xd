require "json"
require "gtk4"
require "./adw"
require "./context_dialog"
require "./directory_browser"
require "./panel_call"
require "./secrets_dialog"

module Xd
  module UI
    class FolderDialogs
      def initialize(
        @parent : Gtk::Window,
        @request : PanelCall,
        @on_error : Proc(String, Nil),
        @remote : Bool,
      )
      end

      def settings(folder_id : String, folder_name : String) : Nil
        request_async({
          "op"     => JSON::Any.new("folder-settings"),
          "folder" => JSON::Any.new(folder_id),
        }) do |result|
          if error = result.error
            @on_error.call(error)
          elsif state = result.body
            present_settings(folder_id, folder_name, state)
          else
            @on_error.call("Daemon returned no folder settings.")
          end
        end
      end

      private def present_settings(
        folder_id : String,
        folder_name : String,
        state : Hash(String, JSON::Any),
      ) : Nil
        inherited_backend =
          state["inherited_backend"]?.try(&.as_s?) ||
            state["effective_backend"].as_s
        inherited_model =
          state["inherited_model"]?.try(&.as_s?) ||
            state["effective_model"]?.try(&.as_s?)
        inherited_workdir =
          state["inherited_workdir"]?.try(&.as_s?) ||
            state["effective_workdir"]?.try(&.as_s?)
        inherited_repo =
          state["inherited_repo"]?.try(&.as_s?) ||
            state["effective_repo"]?.try(&.as_s?)

        dialog = Adw::PreferencesDialog.new
        dialog.title = folder_name
        page = Adw::PreferencesPage.new

        assistant = Adw::PreferencesGroup.new
        assistant.title = "Assistant"
        assistant.description =
          "Chats started in this folder use these unless they say otherwise."

        backend_ids = [nil, "claude", "codex"] of String?
        backend_names = Gtk::StringList.new([
          "Inherit (#{inherited_backend})",
          "Claude Code",
          "Codex",
        ])
        backend_row = Adw::ComboRow.new
        backend_row.title = "Backend"
        backend_row.model = backend_names
        backend_row.selected = (
          backend_ids.index(state["backend"].as_s?) || 0
        ).to_u32

        model_row = Adw::EntryRow.new
        model_row.title = inherited_model ? "Model (blank inherits)" : "Model (blank: CLI default)"
        model_row.text = state["model"].as_s? || ""

        assistant.add(backend_row)
        assistant.add(model_row)
        page.add(assistant)

        project = Adw::PreferencesGroup.new
        project.title = "Project"
        project.description =
          "Where the assistant runs. The code does not have to live inside " \
          "the workspace tree."

        workdir = state["workdir"].as_s?
        repo = state["repo"].as_s?
        workdir_row, choose_workdir, clear_workdir =
          build_path_row("Working Directory")
        repo_row, choose_repo, clear_repo =
          build_path_row("Repository")

        refresh_paths = -> {
          update_path_row(
            workdir_row,
            workdir,
            inherited_workdir,
            state["inherited_workdir_from"]?.try(&.as_s?)
          )
          update_path_row(
            repo_row,
            repo,
            inherited_repo,
            state["inherited_repo_from"]?.try(&.as_s?)
          )
        }
        choose_workdir.clicked_signal.connect do
          DirectoryBrowser.present(
            @parent,
            @request,
            workdir || inherited_workdir
          ) do |chosen|
            if chosen
              workdir = chosen
              refresh_paths.call
            end
          end
        end
        choose_repo.clicked_signal.connect do
          DirectoryBrowser.present(
            @parent,
            @request,
            repo || inherited_repo || inherited_workdir
          ) do |chosen|
            if chosen
              repo = chosen
              refresh_paths.call
            end
          end
        end
        clear_workdir.clicked_signal.connect do
          workdir = nil
          refresh_paths.call
        end
        clear_repo.clicked_signal.connect do
          repo = nil
          refresh_paths.call
        end

        project.add(workdir_row)
        project.add(repo_row)
        page.add(project)
        refresh_paths.call

        dialog.add(page)
        dialog.closed_signal.connect do
          save_async({
            "op"      => JSON::Any.new("set-folder-settings"),
            "folder"  => JSON::Any.new(folder_id),
            "backend" => nullable(backend_ids[backend_row.selected.to_i]),
            "model"   => nullable(clean(model_row.text)),
            "workdir" => nullable(workdir),
            "repo"    => nullable(repo),
          })
        end
        dialog.present(@parent)
      end

      private def build_path_row(
        title : String,
      ) : {Adw::ActionRow, Gtk::Button, Gtk::Button}
        row = Adw::ActionRow.new
        row.title = title
        row.subtitle_lines = 2

        choose = Gtk::Button.new_from_icon_name("folder-open-symbolic")
        choose.add_css_class("flat")
        choose.valign = :center
        choose.tooltip_text = "Choose…"

        clear = Gtk::Button.new_from_icon_name("edit-clear-symbolic")
        clear.add_css_class("flat")
        clear.valign = :center
        clear.tooltip_text = "Inherit from the parent folder"

        buttons = Gtk::Box.new(:horizontal, 6)
        buttons.append(choose)
        buttons.append(clear)
        row.add_suffix(buttons)
        {row, choose, clear}
      end

      private def update_path_row(
        row : Adw::ActionRow,
        value : String?,
        inherited : String?,
        inherited_from : String?,
      ) : Nil
        row.subtitle = if value && !value.empty?
                         value
                       elsif inherited && inherited_from
                         "#{inherited} — inherited from #{inherited_from}"
                       else
                         inherited || "Not set"
                       end
      end

      private def save_async(
        request : Hash(String, JSON::Any),
      ) : Nil
        spawn do
          result = @request.call(request)
          if error = result.error
            GLib.idle_add do
              @on_error.call(error)
              false
            end
          end
        end
      end

      private def request_async(
        request : Hash(String, JSON::Any),
        &complete : PanelCallResult -> Nil
      ) : Nil
        spawn do
          result = @request.call(request)
          GLib.idle_add do
            complete.call(result)
            false
          end
        end
      end

      def context(folder_id : String, folder_name : String) : Nil
        ContextDialog.new(
          @parent,
          @request,
          folder_id,
          folder_name
        ).present
      end

      def secrets(
        folder_id : String? = nil,
        folder_name : String? = nil,
      ) : Nil
        SecretsDialog.new(
          @parent,
          @request,
          @remote,
          folder_id,
          folder_name
        ).present
      end

      private def clean(value : String) : String?
        stripped = value.strip
        stripped unless stripped.empty?
      end

      private def nullable(value : String?) : JSON::Any
        JSON::Any.new(value)
      end
    end
  end
end
