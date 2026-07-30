require "json"
require "openssl"
require "socket"
require "../protocol/message"
require "./endpoint"
require "./local_ipc"

module Xd
  module Daemon
    record ClientAnswer,
      body : Hash(String, JSON::Any)?,
      error : String?

    record RemotePairing,
      client : Client,
      token : String,
      fingerprint : String

    # One multiplexed client for both local IPC and remote TLS.
    #
    # Current daemons echo the private request id, so a slow repository read
    # cannot hold a cancel, terminal input, or model-status call behind it.
    # The FIFO remains as a compatibility path for older daemons.
    class Client < Endpoint
      class Error < Exception
      end

      @pending = {} of Int64 => Channel(ClientAnswer)
      @pending_order = Deque(Int64).new
      @subscribers = {} of Int64 => Proc(Hash(String, JSON::Any), Nil)
      @disconnect_subscribers = {} of Int64 => Proc(String, Nil)
      @next_subscriber = 0_i64
      @next_request = 0_i64
      @lock = Mutex.new
      @closed = false
      @disconnect_message : String?

      getter fingerprint : String?

      private def initialize(@io : IO, @fingerprint : String? = nil)
        spawn read_loop
      end

      def self.local(path : String) : self
        {% if flag?(:win32) || flag?(:xd_loopback_local) %}
          new(LocalIPC.connect(path))
        {% else %}
          new(UNIXSocket.new(path))
        {% end %}
      rescue error : IO::Error
        raise Error.new("Cannot connect to local daemon: #{error.message}")
      end

      def self.remote(
        host : String,
        port : Int,
        token : String,
        fingerprint : String,
      ) : self
        io, actual = remote_socket(host, port, fingerprint)
        client = new(io, actual)
        begin
          client.call({
            "op"    => JSON::Any.new("hello"),
            "token" => JSON::Any.new(token),
          })
          client
        rescue error
          client.close
          raise error
        end
      end

      def self.pair_remote(
        host : String,
        port : Int,
        code : String,
        name : String = System.hostname,
      ) : RemotePairing
        io, fingerprint = remote_socket(host, port, nil)
        client = new(io, fingerprint)
        begin
          response = client.call({
            "op"   => JSON::Any.new("pair"),
            "code" => JSON::Any.new(code),
            "name" => JSON::Any.new(name),
          })
          token = response["token"]?.try(&.as_s?) ||
                  raise Error.new("Pairing returned no device token.")
          RemotePairing.new(client, token, fingerprint)
        rescue error
          client.close
          raise error
        end
      end

      def call(
        request : Hash(String, JSON::Any),
      ) : Hash(String, JSON::Any)
        answer = Channel(ClientAnswer).new(1)

        begin
          @lock.synchronize do
            raise Error.new("Daemon connection is closed.") if @closed

            @next_request &+= 1
            request_id = @next_request
            body = request.dup
            body[Protocol::REQUEST_ID] = JSON::Any.new(request_id)
            @pending[request_id] = answer
            @pending_order << request_id
            @io << body.to_json << '\n'
            @io.flush
          end
        rescue error : IO::Error
          disconnect("Daemon connection failed: #{error.message}")
        end

        result = answer.receive
        if message = result.error
          raise Error.new(message)
        end
        result.body.not_nil!
      end

      def subscribe(
        &subscriber : Hash(String, JSON::Any) ->
      ) : Int64
        @lock.synchronize do
          @next_subscriber += 1
          @subscribers[@next_subscriber] = subscriber
          @next_subscriber
        end
      end

      def unsubscribe(id : Int64) : Nil
        @lock.synchronize do
          @subscribers.delete(id)
          @disconnect_subscribers.delete(id)
        end
      end

      def on_disconnect(&subscriber : String ->) : Int64
        message : String? = nil
        id = @lock.synchronize do
          @next_subscriber += 1
          if @closed
            message = @disconnect_message ||
                      "Daemon connection is closed."
          else
            @disconnect_subscribers[@next_subscriber] = subscriber
          end
          @next_subscriber
        end
        subscriber.call(message.not_nil!) if message
        id
      end

      def closed? : Bool
        @lock.synchronize { @closed }
      end

      def close : Nil
        disconnect("Daemon connection is closed.")
      end

      private def self.remote_socket(
        host : String,
        port : Int,
        expected_fingerprint : String?,
      ) : {IO, String}
        socket = TCPSocket.new(host, port)
        context = OpenSSL::SSL::Context::Client.insecure
        tls = OpenSSL::SSL::Socket::Client.new(
          socket,
          context,
          sync_close: true
        )
        fingerprint = tls.peer_certificate.digest("SHA256").hexstring
        if expected = expected_fingerprint
          normalized = expected.downcase.delete(':')
          unless secure_compare(fingerprint, normalized)
            tls.close
            raise Error.new(
              "#{host} is not the paired daemon: its certificate changed."
            )
          end
        end
        {tls.as(IO), fingerprint}
      rescue error : Error
        raise error
      rescue error : IO::Error | OpenSSL::Error
        socket.try(&.close)
        raise Error.new("Cannot connect to #{host}:#{port}: #{error.message}")
      end

      private def self.secure_compare(left : String, right : String) : Bool
        return false unless left.bytesize == right.bytesize

        difference = 0_u8
        left.to_slice.zip(right.to_slice) do |a, b|
          difference |= a ^ b
        end
        difference == 0
      end

      private def read_loop : Nil
        while line = @io.gets
          next if line.empty?

          root = JSON.parse(line).as_h?
          unless root
            return disconnect("Daemon sent a non-object response.")
          end

          if root.has_key?("event")
            publish(root)
          else
            answer = @lock.synchronize do
              if request_id = root[Protocol::REQUEST_ID]?.try(&.as_i64?)
                @pending_order.delete(request_id)
                @pending.delete(request_id)
              elsif oldest = @pending_order.shift?
                @pending.delete(oldest)
              end
            end
            unless answer
              return disconnect("Daemon sent an unanswerable response.")
            end

            if root["ok"]?.try(&.as_bool?) == true
              answer.send(ClientAnswer.new(root, nil))
            else
              message = root["error"]?.try(&.as_s?) ||
                        "Daemon refused the request."
              answer.send(ClientAnswer.new(nil, message))
            end
          end
          Fiber.yield
        end
        disconnect("Daemon closed the connection.")
      rescue error : JSON::ParseException
        disconnect("Daemon sent invalid JSON.")
      rescue error : IO::Error
        disconnect("Daemon connection failed: #{error.message}")
      rescue error
        disconnect("Daemon client failed: #{error.message}")
      end

      private def publish(event : Hash(String, JSON::Any)) : Nil
        subscribers = @lock.synchronize { @subscribers.values.dup }
        subscribers.each do |subscriber|
          begin
            subscriber.call(event)
          rescue error
            STDERR.puts(
              "xd: client event subscriber failed: #{error.message}"
            )
          end
        end
      end

      private def disconnect(message : String) : Nil
        pending = [] of Channel(ClientAnswer)
        subscribers = [] of Proc(String, Nil)
        should_close = @lock.synchronize do
          unless @closed
            @closed = true
            @disconnect_message = message
            pending = @pending.values
            @pending.clear
            @pending_order.clear
            subscribers = @disconnect_subscribers.values.dup
            true
          else
            false
          end
        end
        return unless should_close

        begin
          @io.close
        rescue IO::Error
        end
        pending.each do |answer|
          answer.send(ClientAnswer.new(nil, message))
        end
        subscribers.each do |subscriber|
          begin
            subscriber.call(message)
          rescue error
            STDERR.puts(
              "xd: disconnect subscriber failed: #{error.message}"
            )
          end
        end
      end
    end
  end
end
