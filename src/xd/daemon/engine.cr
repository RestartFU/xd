require "digest/sha256"
require "random/secure"
require "../protocol/message"
require "../storage/workflow_state"
require "./connection"

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
      @pairing : Pairing?
      @mutex = Mutex.new

      def initialize(
        @store : Storage::Store,
        @clock : Proc(Time::Instant) = -> { Time.instant },
        @token_generator : Proc(String) = -> { Random::Secure.base64(32) },
      )
      end

      def arm_pairing(ttl : Time::Span) : String
        @mutex.synchronize do
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
        request = Protocol::Request.parse(line)

        @mutex.synchronize do
          if request.operation.authentication_required? && !connection.authenticated
            return Protocol::Response.error("Not authenticated. Say hello first.")
          end

          case request.operation
          when Protocol::Operation::Pair
            pair(connection, request)
          when Protocol::Operation::Hello
            hello(connection, request)
          when Protocol::Operation::Messages
            messages(request)
          when Protocol::Operation::RenameChat
            rename_chat(request)
          when Protocol::Operation::DeleteChat
            delete_chat(request)
          when Protocol::Operation::Chat
            chat(request)
          when Protocol::Operation::SetOption
            set_option(request)
          when Protocol::Operation::Queue
            queue(request)
          when Protocol::Operation::DropQueue
            drop_queue(request)
          when Protocol::Operation::EditQueue
            edit_queue(request)
          when Protocol::Operation::SteerQueue
            steer_queue(request)
          when Protocol::Operation::Ping
            Protocol::Response.ok
          else
            Protocol::Response.error("Operation not migrated yet")
          end
        end
      rescue error : Protocol::Error
        Protocol::Response.error(error.message || "Invalid request")
      rescue error : DeviceStoreError
        Protocol::Response.error(error.message || "Storage error")
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

      private def token_hash(token : String) : String
        Digest::SHA256.hexdigest(token)
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
          "total_messages" => JSON::Any.new(total),
          "last_message_id" => JSON::Any.new(
            @store.last_message_id(chat_id)
          ),
          "messages" => messages_json(rows),
        })
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
        fields["commands"] = json_any([] of String)
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
        fields["workdir"] = JSON::Any.new(stored.workdir) if stored.workdir

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
    end
  end
end
