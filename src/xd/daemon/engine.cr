require "digest/sha256"
require "random/secure"
require "../agent/authentication"
require "../agent/manager"
require "../agent/secrets"
require "../agent/cli_versions"
require "../protocol/message"
require "../storage/workflow_state"
require "../version"
require "../workspace/clone"
require "../workspace/service"
require "../workspace/worktrees"
require "./connection"
require "./errors"
require "./event_bus"
require "./filesystem"
require "./images"
require "./network_address"
require "./repository"
require "./repository_monitor"
require "./search"
require "./self_update"
require "./terminals"
require "./voice_jobs"
require "./workspace_monitor"

module Xd
  module Daemon
    PROTOCOL_VERSION = 1_i64
    MAX_DRAFT_BYTES  = 1024 * 1024
    PAIRING_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"

    private record Pairing,
      code : String,
      expires_at : Time::Instant

    # Sole application command dispatcher.
    #
    # Socket, platform-local IPC, and future in-process test transports stop
    # at this boundary. Every state-changing command is serialized here so
    # local and remote clients cannot diverge or race separate implementations.
    class Engine
      alias PeerListener = Proc(String, Int32, Int32)

      getter events : EventBus

      @pairing : Pairing?
      @command_mutex = Mutex.new
      @command_events = [] of Protocol::Event
      @after_write : Proc(Nil)?
      @peer_listener : PeerListener?
      @active_connections = {} of UInt64 => Connection

      def initialize(
        @store : Storage::Store,
        workspaces : Workspace::Service? = nil,
        @clock : Proc(Time::Instant) = -> { Time.instant },
        @token_generator : Proc(String) = -> { Random::Secure.base64(32) },
        launcher : Agent::Launcher? = nil,
        authentication_resolver : Agent::Authentication::Resolver? = nil,
        authentication_environment : Hash(String, String)? = nil,
        authentication_timeout : Time::Span = Agent::Authentication::COMMAND_TIMEOUT,
        agent_authorizer : Agent::Manager::Authorizer? = nil,
        cli_version_resolver : Agent::CliVersions::Resolver? = nil,
        cli_version_environment : Hash(String, String)? = nil,
        cli_version_timeout : Time::Span = Agent::CliVersions::CHECK_TIMEOUT,
        workspace_monitor_interval : Time::Span = WorkspaceMonitor::INTERVAL,
        voice_model_factory : VoiceJobs::ModelFactory? = nil,
        voice_transcriber_factory : VoiceJobs::TranscriberFactory? = nil,
        workflow_status_resolver : Agent::WorkflowRun::StatusCache::Resolver? = nil,
        @peer_host : Proc(String) = -> { NetworkAddress.local },
      )
        @workspaces = workspaces || Workspace::Service.new(
          File.join(Path[@store.path].dirname, "Workspaces"),
          @store
        )
        @events = EventBus.new
        @workspace_monitor = WorkspaceMonitor.new(
          -> { @workspaces.tree_signature },
          -> {
            publish_async_event(
              "tree",
              {} of String => JSON::Any
            )
          },
          workspace_monitor_interval
        )
        @filesystem = Filesystem.new(@store, @workspaces)
        @images = Images.new
        @search = Search.new(@store)
        @workflow_statuses = if resolver = workflow_status_resolver
                               Agent::WorkflowRun::StatusCache.new(
                                 resolver,
                                 clock: @clock
                               )
                             else
                               Agent::WorkflowRun::StatusCache.new(
                                 clock: @clock
                               )
                             end
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
          environment: authentication_environment,
          command_timeout: authentication_timeout
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
          environment: cli_version_environment,
          check_timeout: cli_version_timeout
        )
        @self_update = SelfUpdate.new(
          ->(name : String, fields : Hash(String, JSON::Any)) {
            publish_async_event(name, fields)
          }
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
          arm_pairing_unlocked(ttl)
        end
      end

      # Sessions call this when their transport closes so a device stops being
      # reported as connected without leaving an in-memory reference behind.
      def connection_closed(connection : Connection) : Nil
        @command_mutex.synchronize do
          @active_connections.delete(connection.object_id)
        end
      end

      # Server ownership stays outside Engine, but local clients may ask that
      # server to expose this exact Engine over TLS. Remote clients cannot
      # open listeners or mint credentials for more devices.
      def peer_listener=(listener : PeerListener) : PeerListener
        @peer_listener = listener
        listener
      end

      def dispatch(connection : Connection, line : String) : Protocol::Response
        outcome = process(connection, line)
        outcome.after_write.try(&.call)
        outcome.response
      end

      def process(connection : Connection, line : String) : Protocol::Outcome
        request = Protocol::Request.parse(line)

        if connection.revoked || connection.closed
          return failed_outcome("Device connection is no longer authorized.")
        end

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

          if request.operation.local_only? && !connection.transport.local?
            return Protocol::Outcome.new(
              Protocol::Response.error(
                "Device management is only available on the daemon machine."
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
      rescue error : SelfUpdate::Error
        failed_outcome(error.message || "Daemon update error")
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
             Protocol::Operation::AgentAuth,
             Protocol::Operation::AgentAuthCancel,
             Protocol::Operation::AgentClis,
             Protocol::Operation::WorkflowStatus,
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
        connections = @command_mutex.synchronize do
          values = @active_connections.values
          @active_connections.clear
          values
        end
        connections.each(&.close)
        @workspace_monitor.close
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
        when Protocol::Operation::PeerPairing
          peer_pairing(connection, request)
        when Protocol::Operation::Devices
          devices
        when Protocol::Operation::RenameDevice
          rename_device(request)
        when Protocol::Operation::RevokeDevice
          revoke_device(request)
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
        when Protocol::Operation::AgentCatalog
          agent_catalog
        when Protocol::Operation::WorkflowStatus
          workflow_status(request)
        when Protocol::Operation::DaemonUpdate
          daemon_update(request)
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
        when Protocol::Operation::Shortcuts
          shortcuts(request)
        when Protocol::Operation::SetShortcuts
          set_shortcuts(request)
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
        when Protocol::Operation::MoveChat
          move_chat(request)
        when Protocol::Operation::DeleteChat
          delete_chat(request)
        when Protocol::Operation::Chat
          chat(request)
        when Protocol::Operation::SetDraft
          set_draft(request)
        when Protocol::Operation::SetOption
          set_option(request)
        when Protocol::Operation::RemoveWorktree
          remove_worktree(request)
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
        when Protocol::Operation::GitDraft
          git_draft(request)
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
        when Protocol::Operation::VoiceStreamStart
          voice_stream_start(connection, request)
        when Protocol::Operation::VoiceStreamChunk
          voice_stream_chunk(connection, request)
        when Protocol::Operation::VoiceStreamFinish
          voice_stream_finish(connection, request)
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
        name = DeviceStore.normalize_name(
          request.string("name", "pair needs a device name.")
        )
        @pairing = nil

        token = @token_generator.call
        token_hash = token_hash(token)
        @store.add_device(token_hash, name)
        connection.authenticate(token_hash)
        @active_connections[connection.object_id] = connection

        Protocol::Response.ok({
          "token"  => JSON::Any.new(token),
          "device" => JSON::Any.new(name),
        })
      end

      private def peer_pairing(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        unless connection.transport.local?
          return Protocol::Response.error(
            "Pairing codes can only be created on the daemon machine."
          )
        end

        listener = @peer_listener
        unless listener
          return Protocol::Response.error(
            "This daemon cannot accept remote devices."
          )
        end

        bind = request.string?("bind") || "::"
        requested_port = request.int?("port") || 4001_i64
        unless 0 <= requested_port <= UInt16::MAX
          raise Protocol::Error.new("Port must be from 0 to 65535.")
        end

        port = begin
          listener.call(bind, requested_port.to_i32)
        rescue error
          return Protocol::Response.error(
            "Cannot accept remote devices: " \
            "#{error.message || error.class.name}"
          )
        end
        code = arm_pairing_unlocked(5.minutes)
        Protocol::Response.ok({
          "code"       => JSON::Any.new(code),
          "host"       => JSON::Any.new(@peer_host.call),
          "port"       => JSON::Any.new(port.to_i64),
          "expires_in" => JSON::Any.new(300_i64),
        })
      end

      private def arm_pairing_unlocked(ttl : Time::Span) : String
        code = String.build do |io|
          8.times do |index|
            io << '-' if index == 4
            io << PAIRING_ALPHABET[Random::Secure.rand(PAIRING_ALPHABET.size)]
          end
        end
        @pairing = Pairing.new(code, @clock.call + ttl)
        code
      end

      private def hello(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        token = request.string?("token")
        return Protocol::Response.error("hello needs a token") unless token

        name = @store.device_name(token_hash(token))
        return Protocol::Response.error(UNKNOWN_DEVICE_ERROR) unless name

        connection.authenticate(token_hash(token))
        @active_connections[connection.object_id] = connection
        Protocol::Response.ok({
          "device"  => JSON::Any.new(name),
          "version" => JSON::Any.new(PROTOCOL_VERSION),
        })
      end

      private def devices : Protocol::Response
        values = @store.list_devices.map do |device|
          connected = @active_connections.values.any? do |connection|
            !connection.closed && connection.device_id == device.id
          end
          JSON::Any.new({
            "id"         => JSON::Any.new(device.id),
            "name"       => JSON::Any.new(device.name),
            "created_at" => JSON::Any.new(device.created_at),
            "last_seen"  => JSON::Any.new(device.last_seen),
            "connected"  => JSON::Any.new(connected),
          })
        end
        Protocol::Response.ok({
          "devices" => JSON::Any.new(values),
        })
      end

      private def rename_device(
        request : Protocol::Request,
      ) : Protocol::Response
        id = request.string?(
          "device"
        ) || request.string("id", "rename-device needs a device id.")
        name = request.string(
          "name",
          "rename-device needs a device name."
        )
        @store.rename_device(id, name)
        Protocol::Response.ok
      end

      private def revoke_device(
        request : Protocol::Request,
      ) : Protocol::Response
        id = request.string?(
          "device"
        ) || request.string("id", "revoke-device needs a device id.")
        @store.revoke_device(id)
        connections = @active_connections.values.select do |connection|
          connection.device_id == id
        end
        connections.each(&.revoke)
        Protocol::Response.ok
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

      # Updates this daemon's own installation.
      #
      # A paired device can see the machine is behind but cannot do anything
      # about it without a shell there. Installing and restarting are separate
      # actions: replacing files is safe while turns run, restarting drops
      # every connection and loses the turn, so only the caller decides.
      private def daemon_update(
        request : Protocol::Request,
      ) : Protocol::Response
        case request.string?("action") || "status"
        when "status"  then nil
        when "check"   then @self_update.check
        when "install" then @self_update.install
        when "restart" then @self_update.restart
        else
          raise Protocol::Error.new("No such daemon-update action.")
        end

        Protocol::Response.ok(@self_update.snapshot)
      end

      # The assistants and models this daemon can actually run.
      #
      # The desktop reads Agent::Catalog directly because it ships in the same
      # binary. A separately released client cannot, and hard-coding the list
      # would drift the moment a model is added or retired -- set-option
      # validates the id, so a stale client would simply be refused.
      private def agent_catalog : Protocol::Response
        backends = Agent::Catalog.all.map do |backend|
          models = backend.models.map do |model|
            JSON::Any.new({
              "id"             => JSON::Any.new(model.id),
              "name"           => JSON::Any.new(model.display_name),
              "context_window" => JSON::Any.new(model.context_window.to_i64),
            })
          end
          efforts = backend.efforts.map { |effort| JSON::Any.new(effort.wire_name) }

          JSON::Any.new({
            "id"            => JSON::Any.new(backend.id),
            "name"          => JSON::Any.new(backend.display_name),
            "default_model" => JSON::Any.new(backend.default_model),
            "models"        => JSON::Any.new(models),
            "efforts"       => JSON::Any.new(efforts),
          })
        end

        Protocol::Response.ok({
          "backends" => JSON::Any.new(backends),
        })
      end

      private def agent_cli_snapshots : JSON::Any
        values = @cli_versions.snapshots.map do |snapshot|
          JSON::Any.new(snapshot.wire_fields)
        end
        JSON::Any.new(values)
      end

      private def workflow_status(
        request : Protocol::Request,
      ) : Protocol::Response
        content = request.string(
          "text",
          "Workflow status needs the captured run marker."
        )
        run = Agent::WorkflowRun.parse(content)
        return Protocol::Response.error("Invalid workflow run marker.") unless run

        status = @workflow_statuses.fetch(run)
        jobs = status.jobs.map do |job|
          fields = {
            "id"    => JSON::Any.new(job.id),
            "name"  => JSON::Any.new(job.name),
            "state" => JSON::Any.new(job.state),
          }
          fields["conclusion"] = JSON::Any.new(job.conclusion) if job.conclusion
          fields["log"] = JSON::Any.new(job.log) if job.log
          # Sent as instants rather than as an elapsed count: the phone polls
          # on its own clock and counts up between replies.
          if started_at = job.started_at
            fields["started_at"] = JSON::Any.new(started_at.to_unix)
          end
          if completed_at = job.completed_at
            fields["completed_at"] = JSON::Any.new(completed_at.to_unix)
          end
          JSON::Any.new(fields)
        end
        fields = {
          "name"  => JSON::Any.new(status.name),
          "state" => JSON::Any.new(status.state),
          "jobs"  => JSON::Any.new(jobs),
        }
        fields["conclusion"] = JSON::Any.new(status.conclusion) if status.conclusion
        if started_at = status.started_at
          fields["started_at"] = JSON::Any.new(started_at.to_unix)
        end
        if completed_at = status.completed_at
          fields["completed_at"] = JSON::Any.new(completed_at.to_unix)
        end
        Protocol::Response.ok(fields)
      rescue error : Agent::WorkflowRun::StatusError
        Protocol::Response.error(error.message || "Workflow status unavailable.")
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
        @workspace_monitor.acknowledge

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
        # Checked before the folder exists, so a mistyped address leaves
        # nothing behind to clean up.
        url = Workspace::Clone.normalize(request.string?("repo_url"))
        id = @workspaces.create_folder(
          request.string?("parent"),
          name,
          request.string?("repo")
        )
        fields = {"id" => JSON::Any.new(id)}
        if url
          start_clone(id, url)
          fields["cloning"] = JSON::Any.new(url)
        end
        Protocol::Response.ok(fields)
      end

      # Clones in the background: a repository of any size takes longer than a
      # request should, and every command here is serialized behind one lock.
      # The folder is already there and usable; clients follow the events.
      private def start_clone(folder_id : String, url : String) : Nil
        destination = @workspaces.find_folder(folder_id)
        publish_async_event("folder-clone", {
          "folder" => JSON::Any.new(folder_id),
          "url"    => JSON::Any.new(url),
          "state"  => JSON::Any.new("cloning"),
        })

        Fiber::ExecutionContext::Isolated.new("xd clone #{folder_id}") do
          trouble : String? = nil
          begin
            Workspace::Clone.run(url, destination)
          rescue error : Workspace::Clone::Error
            trouble = error.message
          end
          finish_clone(folder_id, url, destination, trouble)
        end
      rescue error : RuntimeError
        publish_async_event("folder-clone", {
          "folder" => JSON::Any.new(folder_id),
          "url"    => JSON::Any.new(url),
          "state"  => JSON::Any.new("failed"),
          "error"  => JSON::Any.new(error.message || "Cannot clone."),
        })
      end

      private def finish_clone(
        folder_id : String,
        url : String,
        destination : String,
        trouble : String?,
      ) : Nil
        unless trouble
          # Same lock every other write to the tree takes: this one arrives
          # from the clone's own thread.
          begin
            @command_mutex.synchronize do
              settings = @workspaces.folder_settings(folder_id)
              @workspaces.set_folder_settings(
                folder_id,
                settings.backend,
                settings.model,
                settings.workdir,
                settings.repo || destination
              )
            end
          rescue error : Workspace::Error
            trouble = error.message
          end
        end

        fields = {
          "folder" => JSON::Any.new(folder_id),
          "url"    => JSON::Any.new(url),
          "state"  => JSON::Any.new(trouble ? "failed" : "ready"),
        }
        fields["error"] = JSON::Any.new(trouble) if trouble
        publish_async_event("folder-clone", fields)
        publish_async_event("tree", {} of String => JSON::Any)
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

      private def shortcuts(request : Protocol::Request) : Protocol::Response
        folder_id = request.member?("folder") ? request.string(
          "folder",
          "folder must be a workspace id."
        ) : nil
        Protocol::Response.ok(shortcut_fields(folder_id))
      end

      private def set_shortcuts(
        request : Protocol::Request,
      ) : Protocol::Response
        nodes = request.body["shortcuts"]?.try(&.as_a?) ||
                raise Protocol::Error.new(
                  "set-shortcuts needs a shortcuts array."
                )
        prompts = nodes.map do |node|
          node.as_s? || raise Protocol::Error.new(
            "Every shortcut must be a text prompt."
          )
        end
        folder_id = request.member?("folder") ? request.string(
          "folder",
          "folder must be a workspace id."
        ) : nil
        if folder_id
          @workspaces.set_workspace_shortcuts(folder_id, prompts)
        else
          @workspaces.set_global_shortcuts(prompts)
        end
        Protocol::Response.ok(shortcut_fields(folder_id))
      end

      private def shortcut_fields(
        folder_id : String?,
      ) : Hash(String, JSON::Any)
        global = @workspaces.global_shortcuts
        workspace = if folder_id
                      @workspaces.workspace_shortcuts(folder_id)
                    else
                      [] of String
                    end
        effective = if folder_id
                      @workspaces.resolve_shortcuts(folder_id)
                    else
                      global
                    end
        {
          "global"    => json_any(global),
          "workspace" => json_any(workspace),
          "effective" => json_any(effective),
        }
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

      private def move_chat(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string(
          "chat",
          "move-chat needs a chat id"
        )
        folder_id = request.string(
          "folder",
          "move-chat needs a folder"
        )
        @store.get_chat(chat_id)
        @workspaces.find_folder(folder_id)
        @store.set_chat_folder(chat_id, folder_id)
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
        auth_provider = if stored.backend == "codex" && stored.claude_mode
                          "claude-mode"
                        else
                          stored.backend
                        end
        authentication = @authentication.snapshot(auth_provider)
        if authentication.state.unknown?
          @authentication.refresh(auth_provider)
          authentication = @authentication.snapshot(auth_provider)
        end
        fields["auth_state"] = JSON::Any.new(
          authentication.state.wire_name
        )
        if detail = authentication.detail
          fields["auth_detail"] = JSON::Any.new(detail)
        end
        fields["commands"] = json_any(@agents.commands(chat_id))
        fields["plan"] = JSON::Any.new(stored.plan)
        fields["fast"] = JSON::Any.new(
          stored.backend == "codex" && stored.fast
        )
        fields["claude_mode"] = JSON::Any.new(
          stored.backend == "codex" && stored.claude_mode
        )
        fields["queued"] = JSON::Any.new(stored.queue.first) unless stored.queue.empty?
        fields["queue"] = json_any(stored.queue)
        fields["draft"] = JSON::Any.new(stored.draft)
        fields["draft_revision"] = JSON::Any.new(stored.draft_revision)
        fields["draft_attachments"] = JSON.parse(stored.draft_attachments)
        shortcuts = begin
          @workspaces.resolve_shortcuts(stored.folder_id)
        rescue Workspace::Error
          # Chats remain readable after their workspace is removed. Global
          # buttons still apply; only the missing workspace layer is skipped.
          @workspaces.global_shortcuts
        end
        fields["shortcuts"] = json_any(shortcuts)
        active_turn = @agents.active_turn(chat_id)
        fields["working"] = JSON::Any.new(
          !active_turn.nil? || stored.daemon_working
        )
        if turn = active_turn
          fields["label"] = JSON::Any.new(turn.label)
          fields["turn_id"] = JSON::Any.new(turn.id)
          fields["turn_sequence"] = JSON::Any.new(turn.sequence)
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
        effective_effort = stored.effort
        unless effective_effort
          effective_effort = Agent::Catalog.lookup(stored.backend)
            .try { |backend| backend.default_effort.wire_name }
        end
        fields["effort"] = JSON::Any.new(
          effective_effort || Agent::Effort::High.wire_name
        )
        fields["access"] = JSON::Any.new(stored.access) if stored.access

        if usage = @store.get_context_usage(
             stored.id,
             auth_provider,
             stored.model
           )
          fields["context_used"] = JSON::Any.new(usage.used.to_i64)
          fields["context_window"] = JSON::Any.new(usage.window.to_i64)
        end

        fields["new_worktree"] = JSON::Any.new(stored.new_worktree)
        has_messages = @store.last_message_id(stored.id) > 0
        fields["has_messages"] = JSON::Any.new(has_messages)
        resolved_workdir : String? = nil
        begin
          state = @git_worktrees.state(stored)
          resolved_workdir = state.workdir
          fields["workdir"] = JSON::Any.new(state.workdir)
          fields["linked_worktree"] = JSON::Any.new(state.linked)
          fields["worktrees"] = worktrees_json(state.worktrees)
          if !has_messages && !stored.new_worktree &&
             stored.original_workdir && state.linked
            fields["selected_worktree"] = JSON::Any.new(state.workdir)
          end
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

      private def set_draft(
        request : Protocol::Request,
      ) : Protocol::Response
        message = "set-draft needs a chat and text."
        chat_id = request.string("chat", message)
        text = request.string("text", message)
        if text.bytesize > MAX_DRAFT_BYTES
          raise Protocol::Error.new("A message draft is too large.")
        end

        attachments : String? = nil
        if node = request.body["attachments"]?
          validated = @images.validate_attachments(node, allow_empty: true)
          attachments = validated.map do |attachment|
            {
              "name" => attachment.name,
              "mime" => "image/png",
              "data" => attachment.encoded,
            }
          end.to_json
        end
        state = @store.set_draft(chat_id, text, attachments)
        fields = {
          "draft"          => JSON::Any.new(state.text),
          "draft_revision" => JSON::Any.new(state.revision),
        }
        if attachments
          fields["draft_attachments"] = JSON.parse(state.attachments)
        end
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
            if previous.effort &&
               !selected.supports_effort?(
                 Agent::Effort.from_wire(previous.effort)
               )
              @store.set_effort(chat_id, nil)
            end
            @store.set_fast(chat_id, false) if previous.fast && backend != "codex"
            if previous.claude_mode && backend != "codex"
              @store.set_claude_mode(chat_id, false)
            end
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
          if value
            effort = Agent::Effort.from_wire(value)
            selected = Agent::Catalog.lookup(
              @store.get_chat(chat_id).backend
            ) || Agent::Catalog::CLAUDE
            chat = @store.get_chat(chat_id)
            unless effort.wire_name == value &&
                   selected.supports_effort?(effort) &&
                   !(chat.claude_mode && effort.ultra?)
              raise Protocol::Error.new(
                "That reasoning effort is not available for this assistant."
              )
            end
          end
          @store.set_effort(chat_id, value)
        when "access"
          @store.set_access(chat_id, value)
        when "plan"
          @store.set_plan(chat_id, value == "true")
        when "fast"
          fast = value == "true"
          if fast && @store.get_chat(chat_id).backend != "codex"
            raise Protocol::Error.new(
              "Fast mode is only available for Codex."
            )
          end
          @store.set_fast(chat_id, fast)
        when "claude-mode"
          enabled = value == "true"
          chat = @store.get_chat(chat_id)
          if enabled && chat.backend != "codex"
            raise Protocol::Error.new(
              "Claude mode is only available for Codex."
            )
          end
          @store.set_claude_mode(chat_id, enabled)
          if enabled && chat.effort == Agent::Effort::Ultra.wire_name
            @store.set_effort(chat_id, Agent::Effort::Max.wire_name)
          end
        when "backend"
          backend = value || raise Protocol::Error.new(
            "A backend value is required."
          )
          unless Agent::Catalog.lookup(backend)
            raise Protocol::Error.new("No such assistant.")
          end
          @store.set_backend(chat_id, backend)
          @store.set_fast(chat_id, false) unless backend == "codex"
          @store.set_claude_mode(chat_id, false) unless backend == "codex"
        when "new-worktree"
          @store.set_new_worktree(chat_id, value == "true")
        when "workspace"
          @git_worktrees.select(@store.get_chat(chat_id), value)
        else
          raise Protocol::Error.new("No such option.")
        end

        Protocol::Response.ok
      end

      private def remove_worktree(
        request : Protocol::Request,
      ) : Protocol::Response
        chat_id = request.string(
          "chat",
          "remove-worktree needs a chat and worktree path."
        )
        requested_path = request.string(
          "worktree",
          "remove-worktree needs a chat and worktree path."
        )
        @git_worktrees.remove(@store.get_chat(chat_id), requested_path)
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
        pull_request_title = request.string?("title")
        pull_request_body = request.string?("body")
        if action == "commit" && commit_message.try(&.strip).to_s.empty?
          raise Protocol::Error.new("Write a commit message first.")
        end
        if action == "create-pr" && pull_request_title.try(&.strip).to_s.empty?
          raise Protocol::Error.new("Write a pull request title first.")
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
                commit_message,
                pull_request_title,
                pull_request_body
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

      private def git_draft(
        request : Protocol::Request,
      ) : Protocol::Response
        message = "git-draft needs a chat id and kind."
        chat_id = request.string("chat", message)
        kind = request.string("kind", message)
        unless {"commit", "pull-request"}.includes?(kind)
          raise Protocol::Error.new("No such Git draft kind.")
        end
        backend = request.string?("backend")
        model = request.string?("model")
        request_id = request.string?("request")
        @store.get_chat(chat_id)

        @after_write = -> {
          spawn do
            begin
              context = @repository.draft_context(chat_id, kind)
              prompt = Agent::GitDrafts.prompt(kind, context)
              @agents.generate(
                chat_id,
                backend,
                model,
                prompt,
                Agent::GitDrafts::SYSTEM_PROMPT
              ) do |success, text, error|
                if success
                  begin
                    draft = Agent::GitDrafts.parse(text || "")
                    publish_git_draft(
                      chat_id,
                      kind,
                      request_id,
                      draft: draft
                    )
                  rescue parse_error : Agent::GitDrafts::Error
                    publish_git_draft(
                      chat_id,
                      kind,
                      request_id,
                      error: parse_error.message
                    )
                  end
                else
                  publish_git_draft(
                    chat_id,
                    kind,
                    request_id,
                    error: error || "Assistant could not write a Git draft."
                  )
                end
              end
            rescue error
              publish_git_draft(
                chat_id,
                kind,
                request_id,
                error: error.message || "Cannot prepare a Git draft."
              )
            end
          end
          nil
        }
        Protocol::Response.ok
      end

      private def publish_git_draft(
        chat_id : String,
        kind : String,
        request_id : String?,
        draft : Agent::GitDraft? = nil,
        error : String? = nil,
      ) : Nil
        fields = {
          "chat"    => JSON::Any.new(chat_id),
          "kind"    => JSON::Any.new(kind),
          "success" => JSON::Any.new(!draft.nil?),
        }
        fields["request"] = JSON::Any.new(request_id) if request_id
        if value = draft
          fields["title"] = JSON::Any.new(value.title)
          fields["body"] = JSON::Any.new(value.body)
        elsif message = error
          fields["error"] = JSON::Any.new(message)
        end
        publish_async_event("git-draft-finished", fields)
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

      private def voice_stream_start(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        voice_chat(request, "voice-stream-start")
        token = voice_token(request, "voice-stream-start")
        @voice.start_stream(connection.object_id, token)
        Protocol::Response.ok
      end

      private def voice_stream_chunk(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        voice_chat(request, "voice-stream-chunk")
        token = voice_token(request, "voice-stream-chunk")
        audio = request.string(
          "audio",
          "voice-stream-chunk needs audio."
        )
        @voice.append_stream(connection.object_id, token, audio)
        Protocol::Response.ok
      end

      private def voice_stream_finish(
        connection : Connection,
        request : Protocol::Request,
      ) : Protocol::Response
        voice_chat(request, "voice-stream-finish")
        token = voice_token(request, "voice-stream-finish")
        audio = request.string(
          "audio",
          "voice-stream-finish needs audio."
        )
        @voice.finish_stream(connection.object_id, token, audio)
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
             Protocol::Operation::SetFolderContext,
             Protocol::Operation::SetFolderSettings,
             Protocol::Operation::NewChat,
             Protocol::Operation::RenameChat,
             Protocol::Operation::MoveChat,
             Protocol::Operation::DeleteChat
          @workspace_monitor.acknowledge
          [protocol_event("tree")]
        when Protocol::Operation::SetShortcuts
          @workspace_monitor.acknowledge
          fields = {} of String => JSON::Any
          if folder_id = request.string?("folder")
            fields["folder"] = JSON::Any.new(folder_id)
          end
          [protocol_event("shortcuts-changed", fields)]
        when Protocol::Operation::SetOption
          fields = {} of String => JSON::Any
          if chat_id = request.string?("chat")
            fields["chat"] = JSON::Any.new(chat_id)
          end
          [protocol_event("changed", fields)]
        when Protocol::Operation::RemoveWorktree
          chat_id = request.string?("chat")
          return [] of Protocol::Event unless chat_id

          fields = {
            "chat" => JSON::Any.new(chat_id),
          }
          [
            protocol_event("changed", fields),
            protocol_event("worktrees-changed"),
          ]
        when Protocol::Operation::SetDraft
          chat_id = request.string?("chat")
          return [] of Protocol::Event unless chat_id

          stored = @store.get_chat(chat_id)
          fields = {
            "chat"           => JSON::Any.new(chat_id),
            "draft"          => JSON::Any.new(stored.draft),
            "draft_revision" => JSON::Any.new(stored.draft_revision),
          }
          if request.member?("attachments")
            fields["draft_attachments"] = JSON.parse(
              stored.draft_attachments
            )
          end
          [protocol_event("draft", fields)]
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
        Protocol::Event.new(name, 0_i64, fields, audience)
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
