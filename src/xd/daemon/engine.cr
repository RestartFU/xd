require "digest/sha256"
require "random/secure"
require "../agent/authentication"
require "../agent/manager"
require "../agent/secrets"
require "../agent/cli_versions"
require "../protocol/message"
require "../storage/workflow_state"
require "../version"
require "../workspace/service"
require "../workspace/worktrees"
require "./connection"
require "./event_bus"
require "./filesystem"
require "./images"
require "./repository"
require "./repository_monitor"
require "./search"
require "./terminals"
require "./voice_jobs"

module Xd
  module Daemon
    PROTOCOL_VERSION = 1_i64
    PAIRING_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"

    private record Pairing, code : String, expires_at : Time::Instant

    # Sole application command dispatcher.
    #
    # Socket, platform-local IPC, and future in-process test transports stop
    # at this boundary. Every state-changing command is serialized here so
    # local and remote clients cannot diverge or race separate implementations.
    class Engine
      getter events : EventBus

      @pairing : Pairing?
      @command_mutex = Mutex.new
      @event_mutex = Mutex.new
      @next_event_id = 0_i64
      @command_events = [] of Protocol::Event
      @after_write : Proc(Nil)?

      def initialize(
        @store : Storage::Store,
        workspaces : Workspace::Service? = nil,
        @clock : Proc(Time::Instant) = -> { Time.instant },
        @token_generator : Proc(String) = -> { Random::Secure.base64(32) },
        launcher : Agent::Launcher? = nil,
        authentication_resolver : Agent::Authentication::Resolver? = nil,
        authentication_environment : Hash(String, String)? = nil,
        agent_authorizer : Agent::Manager::Authorizer? = nil,
        cli_version_resolver : Agent::CliVersions::Resolver? = nil,
        cli_version_environment : Hash(String, String)? = nil,
        voice_model_factory : VoiceJobs::ModelFactory? = nil,
        voice_transcriber_factory : VoiceJobs::TranscriberFactory? = nil,
      )
        @workspaces = workspaces || Workspace::Service.new(
          File.join(Path[@store.path].dirname, "Workspaces"),
          @store
        )
        @events = EventBus.new
        @filesystem = Filesystem.new(@store, @workspaces)
        @images = Images.new
        @search = Search.new(@store)
        @git_worktrees = Workspace::Worktrees.new(@store, @workspaces)
        @repository = Repository.new(@store, @workspaces, @filesystem)
        @repository_monitor = RepositoryMonitor.new(
          ->(chat_id : String) { @repository.head_signature(chat_id) },
          ->(chat_id : String) {
            publish_async_event("repository-changed", {
              "chat" => JSON::Any.new(chat_id),
            })
          }
        )
        @terminals = Terminals.new(
          @filesystem,
          ->(name : String, fields : Hash(String, JSON::Any)) {
            publish_async_event(name, fields)
          }
        )
        @authentication = Agent::Authentication.new(
          ->(name : String, fields : Hash(String, JSON::Any)) {
            publish_async_event(name, fields)
          },
          resolver: authentication_resolver,
          environment: authentication_environment
        )
        authorizer = agent_authorizer || ->(provider : String) {
          @authentication.authorization_error(provider)
        }
        @agents = Agent::Manager.new(
          @store,
          @workspaces,
          launcher || Agent::ProcessLauncher.new(VERSION),
          ->(name : String, fields : Hash(String, JSON::Any)) {
            publish_async_event(name, fields)
          },
          @git_worktrees,
          clock: @clock,
          authorizer: authorizer
        )
        @cli_versions = Agent::CliVersions.new(
          ->(name : String, fields : Hash(String, JSON::Any)) {
            publish_async_event(name, fields)
          },
          resolver: cli_version_resolver,
          environment: cli_version_environment
        )
        @voice = VoiceJobs.new(
          ->(name : String, fields : Hash(String, JSON::Any), audience : UInt64) {
            publish_async_event(name, fields, audience)
          },
          model_factory: voice_model_factory ||
                         -> { Voice::Model.new },
          transcriber_factory: voice_transcriber_factory ||
                               -> { Voice::Transcriber.new }
        )
      end

      def arm_pairing(ttl : Time::Span) : String
        @command_mutex.synchronize do
          code = String.build do |io|
            8.times do |index|
              io << '-' if index == 4
              io << PAIRING_ALPHABET[Random::Secure.rand(PAIRING_ALPHABET.size)]
            end
          end
          @pairing = Pairing.new(code, @clock.call + ttl)
          code
        end
      end

      def dispatch(connection : Connection, line : String) : Protocol::Response
        outcome = process(connection, line)
        outcome.after_write.try(&.call)
        outcome.response
      end

      def process(connection : Connection, line : String) : Protocol::Outcome
        request = Protocol::Request.parse(line)

        if control_operation?(request.operation)
          return process_control(connection, request)
        end

        @command_mutex.synchronize do
          @command_events.clear
          @after_write = nil
          if request.operation.authentication_required? && !connection.authenticated
            return Protocol::Outcome.new(
              Protocol::Response.error(
                "Not authenticated. Say hello first."
              ),
              [] of Protocol::Event
            )
          end

          response = dispatch_request(connection, request)
          events = if response.success?
                     events_for(request) + @command_events
                   else
                     [] of Protocol::Event
                   end
          after_write = response.success? ? @after_write : nil
          Protocol::Outcome.new(response, events, after_write)
        end
      rescue error : Protocol::Error
        failed_outcome(error.message || "Invalid request")
      rescue error : DeviceStoreError
        failed_outcome(error.message || "Storage error")
      rescue error : Workspace::Error
        failed_outcome(error.message || "Workspace error")
      rescue error : Workspace::Worktrees::Error
        failed_outcome(error.message || "Worktree error")
      rescue error : Agent::Secrets::Error
        failed_outcome(error.message || "Agent secrets error")
      rescue error : Agent::Authentication::Error
        failed_outcome(error.message || "Agent authentication error")
      rescue error : Agent::CliVersions::Error
        failed_outcome(error.message || "Assistant version error")
      rescue error : Agent::Manager::Error
        failed_outcome(error.message || "Agent error")
      rescue error : Filesystem::Error
        failed_outcome(error.message || "Filesystem error")
      rescue error : Images::Error
        failed_outcome(error.message || "Image error")
      rescue error : Repository::Error
        failed_outcome(error.message || "Repository error")
      rescue error : Terminals::Error
        failed_outcome(error.message || "Terminal error")
      rescue error : VoiceJobs::Error
        failed_outcome(error.message || "Voice input error")
      rescue error
        STDERR.puts(
          "xd: unexpected daemon request failure: " \
          "#{error.class.name}: #{error.message}"
        )
        failed_outcome("Internal daemon error.")
      end

      # These operations must remain available while a repository read,
      # installer, or other serialized command is slow. Their services own
      # their own locks and none use per-command event scratch state.
      private def control_operation?(operation : Protocol::Operation) : Bool
        case operation
        when Protocol::Operation::Cancel,
             Protocol::Operation::VoiceCancel,
             Protocol::Operation::AgentAuthCancel,
             Protocol::Operation::Ping
          true
        else
          false
        end
      end

      private def process_control(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Outcome
        if request.operation.authentication_required? &&
           !connection.authenticated
          return Protocol::Outcome.new(
            Protocol::Response.error(
              "Not authenticated. Say hello first."
            ),
            [] of Protocol::Event
          )
        end

        Protocol::Outcome.new(
          dispatch_request(connection, request),
          [] of Protocol::Event
        )
      end

      def close : Nil
        @repository_monitor.close
        @terminals.close
        @voice.close
        @cli_versions.close
        @authentication.close
        @agents.close
      end

      private def dispatch_request(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        case request.operation
        when Protocol::Operation::Pair
          pair(connection, request)
        when Protocol::Operation::Hello
          hello(connection, request)
        when Protocol::Operation::AgentSecrets
          agent_secrets(request)
        when Protocol::Operation::SetAgentSecrets
          set_agent_secrets(request)
        when Protocol::Operation::AgentAuth
          agent_auth
        when Protocol::Operation::AgentAuthStart
          agent_auth_start(request)
        when Protocol::Operation::AgentAuthInput
          agent_auth_input(request)
        when Protocol::Operation::AgentAuthCancel
          agent_auth_cancel(request)
        when Protocol::Operation::AgentAuthLogout
          agent_auth_logout(request)
        when Protocol::Operation::AgentClis
          agent_clis
        when Protocol::Operation::Tree
          tree
        when Protocol::Operation::NewFolder
          new_folder(request)
        when Protocol::Operation::RenameFolder
          rename_folder(request)
        when Protocol::Operation::MoveFolder
          move_folder(request)
        when Protocol::Operation::TrashFolder
          trash_folder(request)
        when Protocol::Operation::FolderContext
          folder_context(request)
        when Protocol::Operation::SetFolderContext
          set_folder_context(request)
        when Protocol::Operation::FolderSettings
          folder_settings(request)
        when Protocol::Operation::SetFolderSettings
          set_folder_settings(request)
        when Protocol::Operation::NewChat
          new_chat(request)
        when Protocol::Operation::Messages
          messages(request)
        when Protocol::Operation::Search
          search(request)
        when Protocol::Operation::ImageRead
          image_read(request)
        when Protocol::Operation::ListDir
          list_dir(request)
        when Protocol::Operation::FileBrowse
          file_browse(request)
        when Protocol::Operation::RenameChat
          rename_chat(request)
        when Protocol::Operation::DeleteChat
          delete_chat(request)
        when Protocol::Operation::Chat
          chat(request)
        when Protocol::Operation::SetOption
          set_option(request)
        when Protocol::Operation::Send
          send_message(request)
        when Protocol::Operation::Queue
          queue(request)
        when Protocol::Operation::DropQueue
          drop_queue(request)
        when Protocol::Operation::EditQueue
          edit_queue(request)
        when Protocol::Operation::SteerQueue
          steer_queue(request)
        when Protocol::Operation::Cancel
          cancel(request)
        when Protocol::Operation::DiffRead
          diff_read(request)
        when Protocol::Operation::GitState
          git_state(request)
        when Protocol::Operation::GitAction
          git_action(request)
        when Protocol::Operation::TerminalList
          terminal_list(request)
        when Protocol::Operation::TerminalOpen
          terminal_open(request)
        when Protocol::Operation::TerminalInput
          terminal_input(request)
        when Protocol::Operation::TerminalResize
          terminal_resize(request)
        when Protocol::Operation::TerminalKill
          terminal_kill(request)
        when Protocol::Operation::VoiceModel
          voice_model(request)
        when Protocol::Operation::VoiceModelDownload
          voice_model_download(connection, request)
        when Protocol::Operation::VoiceTranscribe
          voice_transcribe(connection, request)
        when Protocol::Operation::VoiceCancel
          voice_cancel(connection, request)
        when Protocol::Operation::Ping
          Protocol::Response.ok
        else
          Protocol::Response.error("Operation not migrated yet")
        end
      end

      private def pair(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        pairing = @pairing
        code = request.string?("code")

        unless pairing && @clock.call <= pairing.expires_at && code == pairing.code
          return Protocol::Response.error(
            "No such pairing code. Run the server with --pair."
          )
        end

        # Spend valid code before storage. Retrying a half-completed pair must
        # never mint several permanent credentials.
        @pairing = nil

        token = @token_generator.call
        name = request.string?("name") || "Unknown device"
        @store.add_device(token_hash(token), name)
        connection.authenticated = true

        Protocol::Response.ok({
          "token" => JSON::Any.new(token),
        })
      end

      private def hello(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        token = request.string?("token")
        return Protocol::Response.error("hello needs a token") unless token

        name = @store.device_name(token_hash(token))
        return Protocol::Response.error("Unknown device. Pair first.") unless name

        connection.authenticated = true
        Protocol::Response.ok({
          "device"  => JSON::Any.new(name),
          "version" => JSON::Any.new(PROTOCOL_VERSION),
        })
      end

      private def agent_secrets(
        request : Protocol::Request,
      ) : Protocol::Response
        secrets = secrets_for(request)
        Protocol::Response.ok({
          "names" => json_any(secrets.names),
        })
      end

      private def set_agent_secrets(
        request : Protocol::Request,
      ) : Protocol::Response
        entries = request.body["entries"]?.try(&.as_a?)
        unless entries
          raise Protocol::Error.new(
            "set-agent-secrets needs an entries array."
          )
        end

        secrets = secrets_for(request)
        desired = {} of String => String?

        entries.each do |node|
          entry = node.as_h?
          unless entry
            raise Protocol::Error.new(
              "Every secret entry must be an object."
            )
          end

          name = entry["name"]?.try(&.as_s?)
          unless Agent::Secrets.valid_name?(name)
            raise Protocol::Error.new(
              "A secret has an invalid environment name."
            )
          end
          name = name.not_nil!
          if desired.has_key?(name)
            raise Protocol::Error.new("Secret names must be unique.")
          end

          if entry.has_key?("value")
            value = entry["value"].as_s?
            unless value
              raise Protocol::Error.new(
                "A secret value must be text."
              )
            end
            if value.empty?
              raise Protocol::Error.new(
                "A replacement secret needs a value."
              )
            end
            desired[name] = value
          else
            unless secrets.includes?(name)
              raise Protocol::Error.new(
                "A new secret needs a value."
              )
            end
            desired[name] = nil
          end
        end

        secrets.names.each do |name|
          secrets.remove(name) unless desired.has_key?(name)
        end
        desired.each do |name, value|
          secrets.set(name, value) if value
        end
        secrets.save
        Protocol::Response.ok
      end

      private def agent_auth : Protocol::Response
        @authentication.refresh
        Protocol::Response.ok({
          "providers" => agent_auth_snapshots,
        })
      end

      private def agent_auth_start(
        request : Protocol::Request,
      ) : Protocol::Response
        @authentication.login(agent_auth_provider(request))
        Protocol::Response.ok
      end

      private def agent_auth_input(
        request : Protocol::Request,
      ) : Protocol::Response
        input = request.string(
          "input",
          "agent-auth-input needs text."
        )
        @authentication.input(agent_auth_provider(request), input)
        Protocol::Response.ok
      end

      private def agent_auth_cancel(
        request : Protocol::Request,
      ) : Protocol::Response
        @authentication.cancel(agent_auth_provider(request))
        Protocol::Response.ok
      end

      private def agent_auth_logout(
        request : Protocol::Request,
      ) : Protocol::Response
        @authentication.logout(agent_auth_provider(request))
        Protocol::Response.ok
      end

      private def agent_auth_provider(
        request : Protocol::Request,
      ) : String
        request.string(
          "provider",
          "Agent authentication needs a provider."
        )
      end

      private def agent_auth_snapshots : JSON::Any
        values = @authentication.snapshots.map do |snapshot|
          JSON::Any.new(snapshot.wire_fields)
        end
        JSON::Any.new(values)
      end

      private def agent_clis : Protocol::Response
        @cli_versions.refresh
        Protocol::Response.ok({
          "providers" => agent_cli_snapshots,
        })
      end

      private def agent_cli_snapshots : JSON::Any
        values = @cli_versions.snapshots.map do |snapshot|
          JSON::Any.new(snapshot.wire_fields)
        end
        JSON::Any.new(values)
      end

      private def secrets_for(
        request : Protocol::Request,
      ) : Agent::Secrets
        if folder_id = request.string?("folder")
          @workspaces.find_folder(folder_id)
          Agent::Secrets.for_folder(folder_id)
        else
          Agent::Secrets.load
        end
      end

      private def token_hash(token : String) : String
        Digest::SHA256.hexdigest(token)
      end

      private def tree : Protocol::Response
        snapshot = @workspaces.snapshot

        folders = snapshot.folders.map do |folder|
          fields = {
            "id"   => JSON::Any.new(folder.id),
            "name" => JSON::Any.new(folder.name),
          }
          if parent = folder.parent
            fields["parent"] = JSON::Any.new(parent)
          end
          JSON::Any.new(fields)
        end
        chats = snapshot.chats.map do |chat|
          JSON::Any.new({
            "id"      => JSON::Any.new(chat.id),
            "folder"  => JSON::Any.new(chat.folder),
            "title"   => json_any(chat.title),
            "backend" => JSON::Any.new(chat.backend),
            "working" => JSON::Any.new(chat.working),
          })
        end

        Protocol::Response.ok({
          "folders" => JSON::Any.new(folders),
          "chats"   => JSON::Any.new(chats),
        })
      end

      private def new_folder(
        request : Protocol::Request,
      ) : Protocol::Response
        name = request.string(
          "name",
          "A folder name cannot be empty or hidden, or contain a path separator."
        )
        id = @workspaces.create_folder(request.string?("parent"), name)
        Protocol::Response.ok({"id" => JSON::Any.new(id)})
      end

      private def rename_folder(
        request : Protocol::Request,
      ) : Protocol::Response
        folder_id = request.string(
          "folder",
          "That request needs a folder."
        )
        name = request.string(
          "name",
          "A folder name cannot be empty or hidden, or contain a path separator."
        )
        @workspaces.rename_folder(folder_id, name)
        Protocol::Response.ok
      end

      private def move_folder(
        request : Protocol::Request,
      ) : Protocol::Response
        folder_id = request.string(
          "folder",
          "That request needs a folder."
        )
        @workspaces.move_folder(folder_id, request.string?("parent"))
        Protocol::Response.ok
      end

      private def trash_folder(
        request : Protocol::Request,
      ) : Protocol::Response
        folder_id = request.string(
          "folder",
          "That request needs a folder."
        )
        @workspaces.trash_folder(folder_id)
        Protocol::Response.ok
      end

      private def folder_context(
        request : Protocol::Request,
      ) : Protocol::Response
        folder_id = request.string(
          "folder",
          "That request needs a folder."
        )
        Protocol::Response.ok({
          "context" => json_any(@workspaces.folder_context(folder_id)),
        })
      end

      private def set_folder_context(
        request : Protocol::Request,
      ) : Protocol::Response
        folder_id = request.string(
          "folder",
          "That request needs a folder."
        )
        unless request.member?("context")
          raise Protocol::Error.new("set-folder-context needs context.")
        end

        node = request.body["context"]
        context = node.as_s?
        unless context || node.raw.nil?
          raise Protocol::Error.new("Folder context must be text or null.")
        end

        @workspaces.set_folder_context(folder_id, context)
        Protocol::Response.ok
      end

      private def folder_settings(
        request : Protocol::Request,
      ) : Protocol::Response
        folder_id = request.string(
          "folder",
          "That request needs a folder."
        )
        settings = @workspaces.folder_settings(folder_id)
        effective = @workspaces.resolve(folder_id)
        inherited = @workspaces.inherited_settings(folder_id)
        Protocol::Response.ok({
          "backend"                => json_any(settings.backend),
          "model"                  => json_any(settings.model),
          "workdir"                => json_any(settings.workdir),
          "repo"                   => json_any(settings.repo),
          "effective_backend"      => JSON::Any.new(effective.backend),
          "effective_model"        => json_any(effective.model),
          "effective_workdir"      => JSON::Any.new(effective.workdir),
          "effective_repo"         => json_any(effective.repo),
          "inherited_backend"      => JSON::Any.new(inherited.backend),
          "inherited_model"        => json_any(inherited.model),
          "inherited_workdir"      => json_any(inherited.workdir),
          "inherited_repo"         => json_any(inherited.repo),
          "inherited_backend_from" => json_any(inherited.backend_from),
          "inherited_model_from"   => json_any(inherited.model_from),
          "inherited_workdir_from" => json_any(inherited.workdir_from),
          "inherited_repo_from"    => json_any(inherited.repo_from),
        })
      end

      private def set_folder_settings(
        request : Protocol::Request,
      ) : Protocol::Response
        folder_id = request.string(
          "folder",
          "That request needs a folder."
        )
        names = {"backend", "model", "workdir", "repo"}
        unless names.all? { |name| request.member?(name) }
          raise Protocol::Error.new(
            "set-folder-settings needs backend, model, workdir, and repo."
          )
        end

        backend = nullable_text(request, "backend")
        if backend && Agent::Catalog.lookup(backend).nil?
          raise Protocol::Error.new("No such agent backend.")
        end
        @workspaces.set_folder_settings(
          folder_id,
          backend,
          nullable_text(request, "model"),
          nullable_text(request, "workdir"),
          nullable_text(request, "repo")
        )
        Protocol::Response.ok
      end

      private def nullable_text(
        request : Protocol::Request,
        name : String,
      ) : String?
        node = request.body[name]
        return nil if node.raw.nil?

        node.as_s? || raise Protocol::Error.new(
          "#{name} must be text or null."
        )
      end

      private def new_chat(
        request : Protocol::Request,
      ) : Protocol::Response
        folder_id = request.string(
          "folder",
          "That request needs a folder."
        )
        settings = @workspaces.resolve(folder_id)
        chat_id = @store.create_chat(
          folder_id,
          request.string?("title") || "New Chat",
          settings.backend,
          settings.model,
          nil,
          request.string?("workdir")
        )
        Protocol::Response.ok({"id" => JSON::Any.new(chat_id)})
      end

      private def messages(request : Protocol::Request) : Protocol::Response
        chat_id = request.string("chat", "messages needs a chat id")
        requested = request.int?("limit") || 0_i64
        transcript_id = @agents.transcript_message_id(chat_id)

        if transcript_id || requested > 0
          limit = requested > 0 ? Math.min(requested, Int32::MAX).to_i : Int32::MAX
          page = @store.list_recent_messages_through(
            chat_id,
            transcript_id || Int64::MAX,
            limit
          )
          rows = page.messages
          total = page.total
        else
          rows = @store.list_messages(chat_id)
          total = rows.size.to_i64
        end

        Protocol::Response.ok({
          "total_messages"  => JSON::Any.new(total),
          "last_message_id" => JSON::Any.new(
            transcript_id || @store.last_message_id(chat_id)
          ),
          "messages" => messages_json(rows),
        })
      end

      private def list_dir(
        request : Protocol::Request,
      ) : Protocol::Response
        Protocol::Response.ok(
          @filesystem.list_directory(request.string?("path"))
        )
      end

      private def image_read(
        request : Protocol::Request,
      ) : Protocol::Response
        preview = request.body["preview"]?.try(&.as_bool?) || false
        Protocol::Response.ok(
          @images.read(request.string?("path"), preview)
        )
      end

      private def search(
        request : Protocol::Request,
      ) : Protocol::Response
        query = request.string?("query") || ""
        hits = @search.call(query).map do |hit|
          JSON::Any.new({
            "id"      => JSON::Any.new(hit.message_id),
            "chat"    => JSON::Any.new(hit.chat_id),
            "title"   => JSON::Any.new(hit.title),
            "role"    => JSON::Any.new(hit.role),
            "snippet" => JSON::Any.new(hit.snippet),
          })
        end
        Protocol::Response.ok({
          "results" => JSON::Any.new(hits),
        })
      end

      private def file_browse(
        request : Protocol::Request,
      ) : Protocol::Response
        message = "file-browse needs a chat and action."
        chat_id = request.string("chat", message)
        action = request.string("action", message)
        Protocol::Response.ok(@filesystem.browse(
          chat_id,
          action,
          request.string?("path"),
          request.string?("content")
        ))
      end

      private def rename_chat(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string(
          "chat",
          "A chat needs an id and a title."
        )
        title = request.string(
          "title",
          "A chat needs an id and a title."
        )
        if title.empty?
          raise Protocol::Error.new("A chat needs an id and a title.")
        end

        @store.set_chat_title(chat_id, title)
        Protocol::Response.ok
      end

      private def delete_chat(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string(
          "chat",
          "delete-chat needs a chat id"
        )
        @agents.forget(chat_id)
        @terminals.kill_chat(chat_id)
        @store.delete_chat(chat_id)
        Protocol::Response.ok
      end

      private def chat(request : Protocol::Request) : Protocol::Response
        chat_id = request.string("chat", "chat needs a chat id")
        stored = @store.get_chat(chat_id)
        fields = {} of String => JSON::Any

        fields["title"] = json_any(stored.title)
        fields["backend"] = JSON::Any.new(stored.backend)
        authentication = @authentication.snapshot(stored.backend)
        if authentication.state.unknown?
          @authentication.refresh(stored.backend)
          authentication = @authentication.snapshot(stored.backend)
        end
        fields["auth_state"] = JSON::Any.new(
          authentication.state.wire_name
        )
        if detail = authentication.detail
          fields["auth_detail"] = JSON::Any.new(detail)
        end
        fields["commands"] = json_any(@agents.commands(chat_id))
        fields["plan"] = JSON::Any.new(stored.plan)
        fields["queued"] = JSON::Any.new(stored.queue.first) unless stored.queue.empty?
        fields["queue"] = json_any(stored.queue)
        active_turn = @agents.active_turn(chat_id)
        fields["working"] = JSON::Any.new(
          !active_turn.nil? || stored.daemon_working
        )
        if turn = active_turn
          fields["label"] = JSON::Any.new(turn.label)
          fields["working_for"] = JSON::Any.new(turn.working_for)
          fields["segment"] = JSON::Any.new(turn.segment)
          fields["items"] = JSON::Any.new(turn.items.map do |item|
            JSON::Any.new({
              "text" => JSON::Any.new(item.text),
              "tool" => JSON::Any.new(item.tool),
            })
          end)
        end
        fields["model"] = JSON::Any.new(stored.model) if stored.model
        fields["effort"] = JSON::Any.new(stored.effort) if stored.effort
        fields["access"] = JSON::Any.new(stored.access) if stored.access

        if usage = @store.get_context_usage(
             stored.id,
             stored.backend,
             stored.model
           )
          fields["context_used"] = JSON::Any.new(usage.used.to_i64)
          fields["context_window"] = JSON::Any.new(usage.window.to_i64)
        end

        fields["new_worktree"] = JSON::Any.new(stored.new_worktree)
        fields["has_messages"] = JSON::Any.new(
          @store.last_message_id(stored.id) > 0
        )
        resolved_workdir : String? = nil
        begin
          state = @git_worktrees.state(stored)
          resolved_workdir = state.workdir
          fields["workdir"] = JSON::Any.new(state.workdir)
          fields["linked_worktree"] = JSON::Any.new(state.linked)
          fields["worktrees"] = worktrees_json(state.worktrees)
        rescue Workspace::Error | Workspace::Worktrees::Error
          # Orphaned chats remain readable even when their folder disappeared.
          begin
            resolved_workdir = @git_worktrees.resolve(stored)
            fields["workdir"] = JSON::Any.new(resolved_workdir.not_nil!)
          rescue Workspace::Error
          end
        end
        context = @git_worktrees.describe(resolved_workdir)
        context = "New worktree from #{context}" if stored.new_worktree
        fields["context"] = JSON::Any.new(context)

        Protocol::Response.ok(fields)
      end

      private def set_option(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string(
          "chat",
          "set-option needs a chat and an option."
        )
        option = request.string(
          "option",
          "set-option needs a chat and an option."
        )
        value = request.string?("value")

        case option
        when "model"
          if backend = request.string?("backend")
            model = value || raise Protocol::Error.new(
              "A model value is required."
            )
            unless selected = Agent::Catalog.lookup(backend)
              raise Protocol::Error.new("No such assistant.")
            end
            unless selected.models.any?(&.id.==(model))
              raise Protocol::Error.new("No such model.")
            end
            previous = @store.get_chat(chat_id)
            @store.set_model_selection(chat_id, backend, model)
            if previous.backend != backend || previous.model != model
              @store.append_message(
                chat_id,
                "event",
                "Switched to #{selected.model_label(model)}"
              )
            end
          else
            @store.set_model(chat_id, value)
          end
        when "effort"
          @store.set_effort(chat_id, value)
        when "access"
          @store.set_access(chat_id, value)
        when "plan"
          @store.set_plan(chat_id, value == "true")
        when "backend"
          backend = value || raise Protocol::Error.new(
            "A backend value is required."
          )
          @store.set_backend(chat_id, backend)
        when "new-worktree"
          @store.set_new_worktree(chat_id, value == "true")
        when "workspace"
          @git_worktrees.select(@store.get_chat(chat_id), value)
        else
          raise Protocol::Error.new("No such option.")
        end

        Protocol::Response.ok
      end

      private def queue(request : Protocol::Request) : Protocol::Response
        chat_id = request.string(
          "chat",
          "A queued message needs a chat and text."
        )
        text = request.string(
          "text",
          "A queued message needs a chat and text."
        )
        if text.empty?
          raise Protocol::Error.new(
            "A queued message needs a chat and text."
          )
        end

        @store.queue_append(chat_id, text)
        Protocol::Response.ok
      end

      private def send_message(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string(
          "chat",
          "A message needs a chat and something to say."
        )
        text = request.string?("text") || ""
        if text.empty? && !request.member?("attachments")
          raise Protocol::Error.new(
            "A message needs a chat and something to say."
          )
        end

        message = @images.materialize(request.body, text)
        result = @agents.send(chat_id, message)
        Protocol::Response.ok({
          "queued" => JSON::Any.new(result.queued?),
        })
      end

      private def cancel(request : Protocol::Request) : Protocol::Response
        chat_id = request.string("chat", "cancel needs a chat id")
        @agents.cancel(chat_id)
        Protocol::Response.ok
      end

      private def diff_read(
        request : Protocol::Request,
      ) : Protocol::Response
        message = "diff-read needs a chat and read type."
        chat_id = request.string("chat", message)
        kind = request.string("read", message)
        output = @repository.read(
          chat_id,
          kind,
          request.string?("path"),
          request.string?("base")
        )
        Protocol::Response.ok({"output" => JSON::Any.new(output)})
      end

      private def git_state(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string("chat", "git-state needs a chat id.")
        refresh_id = request.string?("request")
        @store.get_chat(chat_id)
        @repository_monitor.watch(chat_id)
        @after_write = -> {
          spawn do
            fields = repository_state_fields(
              chat_id,
              @repository.state(chat_id)
            )
            if id = refresh_id
              fields["request"] = JSON::Any.new(id)
            end
            publish_async_event("git-state", fields)
          end
          nil
        }
        Protocol::Response.ok
      end

      private def git_action(
        request : Protocol::Request,
      ) : Protocol::Response
        message = "git-action needs a chat id and action."
        chat_id = request.string("chat", message)
        action = request.string("action", message)
        unless {"commit", "push", "create-pr", "view-pr"}.includes?(action)
          raise Protocol::Error.new("No such Git action.")
        end
        commit_message = request.string?("message")
        if action == "commit" && commit_message.try(&.strip).to_s.empty?
          raise Protocol::Error.new("Write a commit message first.")
        end
        @store.get_chat(chat_id)

        @after_write = -> {
          spawn do
            fields = {
              "chat"    => JSON::Any.new(chat_id),
              "action"  => JSON::Any.new(action),
              "success" => JSON::Any.new(false),
            }
            if id = request.string?("request")
              fields["request"] = JSON::Any.new(id)
            end
            begin
              result = @repository.perform(
                chat_id,
                action,
                commit_message
              )
              @repository_monitor.reset(chat_id)
              repository_state_fields(
                chat_id,
                result.state
              ).each { |name, value| fields[name] = value }
              fields["success"] = JSON::Any.new(true)
              if url = result.url
                fields["url"] = JSON::Any.new(url)
              end
            rescue error : Repository::Error
              fields["error"] = JSON::Any.new(
                error.message || "Git refused the request."
              )
            end
            publish_async_event("git-action-finished", fields)
          end
          nil
        }
        Protocol::Response.ok
      end

      private def repository_state_fields(
        chat_id : String,
        state : Repository::State,
      ) : Hash(String, JSON::Any)
        fields = {
          "chat"    => JSON::Any.new(chat_id),
          "visible" => JSON::Any.new(state.visible),
          "action"  => JSON::Any.new(state.action),
          "label"   => JSON::Any.new(state.label),
          "enabled" => JSON::Any.new(state.enabled),
        }
        if url = state.url
          fields["url"] = JSON::Any.new(url)
        end
        fields
      end

      private def terminal_list(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string(
          "chat",
          "terminal-list needs a chat id."
        )
        Protocol::Response.ok({
          "terminals" => JSON::Any.new(@terminals.list(chat_id)),
        })
      end

      private def terminal_open(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string(
          "chat",
          "terminal-open needs a chat id."
        )
        columns = request.int?("columns") || Terminal::DEFAULT_COLUMNS
        rows = request.int?("rows") || Terminal::DEFAULT_ROWS
        reuse = request.body["reuse"]?.try(&.as_bool?) || false
        opened = @terminals.open(chat_id, columns, rows, reuse)
        terminal = opened.terminal

        if opened.created
          @command_events << protocol_event("terminal-opened", {
            "chat"     => JSON::Any.new(terminal.chat_id),
            "terminal" => JSON::Any.new(terminal.id),
            "title"    => JSON::Any.new(terminal.title),
            "columns"  => JSON::Any.new(terminal.columns.to_i64),
            "rows"     => JSON::Any.new(terminal.rows.to_i64),
          })
          @after_write = -> { @terminals.start(terminal.id) }
        end

        Protocol::Response.ok({
          "id" => JSON::Any.new(terminal.id),
        })
      end

      private def terminal_input(
        request : Protocol::Request,
      ) : Protocol::Response
        terminal_id = request.string(
          "terminal",
          "A terminal id is required."
        )
        data = request.string(
          "data",
          "terminal-input needs data."
        )
        @terminals.input(terminal_id, data)
        Protocol::Response.ok
      end

      private def terminal_resize(
        request : Protocol::Request,
      ) : Protocol::Response
        terminal_id = request.string(
          "terminal",
          "A terminal id is required."
        )
        columns = request.int?("columns") || Terminal::DEFAULT_COLUMNS
        rows = request.int?("rows") || Terminal::DEFAULT_ROWS
        terminal, width, height = @terminals.resize(
          terminal_id,
          columns,
          rows
        )
        @command_events << protocol_event("terminal-resized", {
          "chat"     => JSON::Any.new(terminal.chat_id),
          "terminal" => JSON::Any.new(terminal.id),
          "columns"  => JSON::Any.new(width.to_i64),
          "rows"     => JSON::Any.new(height.to_i64),
        })
        Protocol::Response.ok
      end

      private def terminal_kill(
        request : Protocol::Request,
      ) : Protocol::Response
        terminal_id = request.string(
          "terminal",
          "A terminal id is required."
        )
        @terminals.kill(terminal_id)
        Protocol::Response.ok
      end

      private def voice_model(
        request : Protocol::Request,
      ) : Protocol::Response
        voice_chat(request, "voice-model")
        Protocol::Response.ok({
          "available" => JSON::Any.new(@voice.model_available?),
        })
      end

      private def voice_model_download(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        voice_chat(request, "voice-model-download")
        token = voice_token(request, "voice-model-download")
        @voice.download(connection.object_id, token)
        Protocol::Response.ok
      end

      private def voice_transcribe(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        voice_chat(request, "voice-transcribe")
        token = voice_token(request, "voice-transcribe")
        audio = request.string(
          "audio",
          "voice-transcribe needs audio."
        )
        @voice.transcribe(connection.object_id, token, audio)
        Protocol::Response.ok
      end

      private def voice_cancel(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        token = voice_token(request, "voice-cancel")
        Protocol::Response.ok({
          "cancelled" => JSON::Any.new(
            @voice.cancel(connection.object_id, token)
          ),
        })
      end

      private def voice_chat(
        request : Protocol::Request,
        operation : String,
      ) : String
        chat_id = request.string(
          "chat",
          "#{operation} needs a chat id."
        )
        @store.get_chat(chat_id)
        chat_id
      end

      private def voice_token(
        request : Protocol::Request,
        operation : String,
      ) : String
        request.string(
          "request",
          "#{operation} needs a request token."
        )
      end

      private def drop_queue(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string(
          "chat",
          "drop-queue needs a chat id"
        )
        if request.member?("index")
          @store.queue_remove(chat_id, request.int?("index") || 0_i64)
        else
          @store.set_queue(chat_id, [] of String)
        end
        Protocol::Response.ok
      end

      private def edit_queue(
        request : Protocol::Request,
      ) : Protocol::Response
        message = "edit-queue needs a chat id, queue index, and text."
        chat_id = request.string("chat", message)
        old_text = request.string("old-text", message)
        text = request.string("text", message)
        index = request.int?("index") || raise Protocol::Error.new(message)
        raise Protocol::Error.new(message) if text.empty? || index < 0

        @store.queue_replace(chat_id, index, old_text, text)
        Protocol::Response.ok
      end

      private def steer_queue(
        request : Protocol::Request,
      ) : Protocol::Response
        message = "steer-queue needs a chat id, queue index, and text."
        chat_id = request.string("chat", message)
        text = request.string("text", message)
        index = request.int?("index") || raise Protocol::Error.new(message)
        raise Protocol::Error.new(message) if index < 0

        stored = @store.get_chat(chat_id)
        if index >= stored.queue.size || stored.queue[index] != text
          raise Protocol::Error.new(
            "That queued message changed; try again."
          )
        end

        @store.queue_promote(chat_id, index)
        @agents.cancel(chat_id, publish_queue_event: false)
        Protocol::Response.ok
      end

      private def messages_json(
        rows : Array(Storage::Message),
      ) : JSON::Any
        values = rows.map do |message|
          fields = {
            "role"    => JSON::Any.new(message.role),
            "content" => JSON::Any.new(message.content),
            "at"      => JSON::Any.new(message.created_at),
          }
          if label = message.label
            fields["label"] = JSON::Any.new(label)
          end
          JSON::Any.new(fields)
        end
        JSON::Any.new(values)
      end

      private def worktrees_json(
        rows : Array(Workspace::Worktree),
      ) : JSON::Any
        values = rows.map do |item|
          fields = {
            "path"     => JSON::Any.new(item.path),
            "detached" => JSON::Any.new(item.detached),
            "main"     => JSON::Any.new(item.main),
            "current"  => JSON::Any.new(item.current),
          }
          if branch = item.branch
            fields["branch"] = JSON::Any.new(branch)
          end
          JSON::Any.new(fields)
        end
        JSON::Any.new(values)
      end

      private def json_any(value) : JSON::Any
        JSON.parse(value.to_json)
      end

      private def events_for(
        request : Protocol::Request,
      ) : Array(Protocol::Event)
        case request.operation
        when Protocol::Operation::NewFolder,
             Protocol::Operation::RenameFolder,
             Protocol::Operation::MoveFolder,
             Protocol::Operation::TrashFolder,
             Protocol::Operation::NewChat,
             Protocol::Operation::RenameChat,
             Protocol::Operation::DeleteChat
          [protocol_event("tree")]
        when Protocol::Operation::SetOption
          fields = {} of String => JSON::Any
          if chat_id = request.string?("chat")
            fields["chat"] = JSON::Any.new(chat_id)
          end
          [protocol_event("changed", fields)]
        when Protocol::Operation::Queue,
             Protocol::Operation::DropQueue,
             Protocol::Operation::EditQueue,
             Protocol::Operation::SteerQueue
          chat_id = request.string?("chat")
          return [] of Protocol::Event unless chat_id

          queued = @store.get_chat(chat_id).queue
          fields = {
            "chat"  => JSON::Any.new(chat_id),
            "queue" => json_any(queued),
          }
          fields["text"] = JSON::Any.new(queued.first) unless queued.empty?
          [protocol_event("queued", fields)]
        else
          [] of Protocol::Event
        end
      end

      private def protocol_event(
        name : String,
        fields = {} of String => JSON::Any,
        audience : UInt64? = nil,
      ) : Protocol::Event
        @event_mutex.synchronize do
          @next_event_id += 1
          Protocol::Event.new(name, @next_event_id, fields, audience)
        end
      end

      private def publish_async_event(
        name : String,
        fields : Hash(String, JSON::Any),
        audience : UInt64? = nil,
      ) : Nil
        @events.publish(protocol_event(name, fields, audience))
      end

      private def failed_outcome(message : String) : Protocol::Outcome
        Protocol::Outcome.new(
          Protocol::Response.error(message),
          [] of Protocol::Event
        )
      end
    end
  end
end
