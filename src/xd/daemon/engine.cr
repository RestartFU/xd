require "digest/sha256"
require "random/secure"
require "../protocol/message"
require "./connection"
require "./device_store"

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
        @devices : DeviceStore,
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
        @devices.add_device(token_hash(token), name)
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

        name = @devices.device_name(token_hash(token))
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
    end
  end
end
