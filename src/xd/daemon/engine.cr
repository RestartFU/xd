require "digest/sha256"
require "random/secure"
require "../agent/manager"
require "../agent/secrets"
require "../protocol/message"
require "../storage/workflow_state"
require "../version"
require "../workspace/service"
require "./connection"
require "./event_bus"
require "./filesystem"
require "./images"

module Xd
  module Daemon
    PROTOCOL_VERSION = 1_i64
    PAIRING_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"

    private record Pairing, code : String, expires_at : Time::Instant

    # Sole application command dispatcher.
    #
    # Socket, Unix-domain IPC, and future in-process test transports stop at
    # this boundary. Every state-changing command is serialized here so local
    # and remote clients cannot diverge or race separate implementations.
    class Engine
      getter events : EventBus

      @pairing : Pairing?
      @command_mutex = Mutex.new
      @event_mutex = Mutex.new
      @next_event_id = 0_i64

      def initialize(
        @store : Storage::Store,
        workspaces : Workspace::Service? = nil,
        @clock : Proc(Time::Instant) = -> { Time.instant },
        @token_generator : Proc(String) = -> { Random::Secure.base64(32) },
        launcher : Agent::Launcher? = nil,
      )
        @workspaces = workspaces || Workspace::Service.new(
          File.join(Path[@store.path].dirname, "Workspaces"),
          @store
        )
        @events = EventBus.new
        @filesystem = Filesystem.new(@store, @workspaces)
        @images = Images.new
        @agents = Agent::Manager.new(
          @store,
          @workspaces,
          launcher || Agent::ProcessLauncher.new(VERSION),
          ->(name : String, fields : Hash(String, JSON::Any)) {
            publish_agent_event(name, fields)
          }
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
        process(connection, line).response
      end

      def process(connection : Connection, line : String) : Protocol::Outcome
        request = Protocol::Request.parse(line)

        @command_mutex.synchronize do
          if request.operation.authentication_required? && !connection.authenticated
            return Protocol::Outcome.new(
              Protocol::Response.error(
                "Not authenticated. Say hello first."
              ),
              [] of Protocol::Event
            )
          end

          response = dispatch_request(connection, request)
          events = response.success? ? events_for(request) : [] of Protocol::Event
          Protocol::Outcome.new(response, events)
        end
      rescue error : Protocol::Error
        failed_outcome(error.message || "Invalid request")
      rescue error : DeviceStoreError
        failed_outcome(error.message || "Storage error")
      rescue error : Workspace::Error
        failed_outcome(error.message || "Workspace error")
      rescue error : Agent::Secrets::Error
        failed_outcome(error.message || "Agent secrets error")
      rescue error : Agent::Manager::Error
        failed_outcome(error.message || "Agent error")
      rescue error : Filesystem::Error
        failed_outcome(error.message || "Filesystem error")
      rescue error : Images::Error
        failed_outcome(error.message || "Image error")
      end

      def close : Nil
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
        when Protocol::Operation::NewChat
          new_chat(request)
        when Protocol::Operation::Messages
          messages(request)
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

        if requested > 0
          page = @store.list_recent_messages(
            chat_id,
            Math.min(requested, Int32::MAX).to_i
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
            @store.last_message_id(chat_id)
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
        Protocol::Response.ok(@images.read(request.string?("path")))
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
        @store.delete_chat(chat_id)
        Protocol::Response.ok
      end

      private def chat(request : Protocol::Request) : Protocol::Response
        chat_id = request.string("chat", "chat needs a chat id")
        stored = @store.get_chat(chat_id)
        fields = {} of String => JSON::Any

        fields["title"] = json_any(stored.title)
        fields["backend"] = JSON::Any.new(stored.backend)
        fields["commands"] = json_any(@agents.commands(chat_id))
        fields["plan"] = JSON::Any.new(stored.plan)
        fields["queued"] = JSON::Any.new(stored.queue.first) unless stored.queue.empty?
        fields["queue"] = json_any(stored.queue)
        fields["working"] = JSON::Any.new(stored.daemon_working)
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
        begin
          fields["workdir"] = JSON::Any.new(
            @workspaces.resolve_workdir(stored.folder_id, stored.workdir)
          )
        rescue Workspace::Error
          # Orphaned chats remain readable even when their folder disappeared.
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
          @store.set_model(chat_id, value)
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
      ) : Protocol::Event
        @event_mutex.synchronize do
          @next_event_id += 1
          Protocol::Event.new(name, @next_event_id, fields)
        end
      end

      private def publish_agent_event(
        name : String,
        fields : Hash(String, JSON::Any),
      ) : Nil
        @events.publish(protocol_event(name, fields))
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
