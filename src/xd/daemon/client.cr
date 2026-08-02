require "json"
require "openssl"
require "set"
require "socket"
require "../protocol/frame"
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
      REQUEST_TIMEOUT        = 30.seconds
      LONG_REQUEST_TIMEOUT   = 5.minutes
      MAX_ABANDONED_REQUESTS = 256
      MAX_PENDING_EVENTS     = 65_536

      class Error < Exception
      end

      class AuthorizationError < Error
      end

      class TimeoutError < Error
      end

      @pending = {} of Int64 => Channel(ClientAnswer)
      @pending_order = Deque(Int64).new
      @abandoned = Set(Int64).new
      @event_queue = Deque(Hash(String, JSON::Any)).new
      @event_ready = Channel(Nil).new(1)
      @subscribers = {} of Int64 => Proc(Hash(String, JSON::Any), Nil)
      @disconnect_subscribers = {} of Int64 => Proc(String, Nil)
      @next_subscriber = 0_i64
      @next_request = 0_i64
      @lock = Mutex.new
      @write_ready = Channel(Nil).new(1)
      @closed = false
      @disconnect_message : String?

      getter fingerprint : String?

      private def initialize(
        @io : IO,
        @fingerprint : String? = nil,
        @request_timeout : Time::Span = REQUEST_TIMEOUT,
      )
        @write_ready.send(nil)
        set_write_timeout(@request_timeout)
        spawn publish_loop
        spawn read_loop
      end

      def self.local(
        path : String,
        request_timeout : Time::Span = REQUEST_TIMEOUT,
      ) : self
        {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
          new(LocalIPC.connect(path), request_timeout: request_timeout)
        {% else %}
          new(UNIXSocket.new(path), request_timeout: request_timeout)
        {% end %}
      rescue error : IO::Error
        raise Error.new("Cannot connect to local daemon: #{error.message}")
      end

      def self.remote(
        host : String,
        port : Int,
        token : String,
        fingerprint : String,
        request_timeout : Time::Span = REQUEST_TIMEOUT,
      ) : self
        io, actual = remote_socket(host, port, fingerprint)
        client = new(io, actual, request_timeout)
        begin
          client.call({
            "op"    => JSON::Any.new("hello"),
            "token" => JSON::Any.new(token),
          })
          client
        rescue error : Error
          client.close
          if error.message == "Unknown device. Pair first."
            raise AuthorizationError.new(error.message)
          end
          raise error
        rescue error
          client.close
          raise error
        end
      end

      # The connecting client supplies its automatic name; the daemon stores it
      # and the local owner may rename it later.
      def self.pair_remote(
        host : String,
        port : Int,
        code : String,
        name : String = System.hostname,
        request_timeout : Time::Span = REQUEST_TIMEOUT,
      ) : RemotePairing
        io, fingerprint = remote_socket(host, port, nil)
        client = new(io, fingerprint, request_timeout)
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
        request_id = 0_i64
        write_deadline = Time.instant + @request_timeout

        select
        when @write_ready.receive
        when timeout(@request_timeout)
          disconnect("Daemon request timed out while waiting to write.")
          raise TimeoutError.new("Daemon request timed out.")
        end

        begin
          remaining = write_deadline - Time.instant
          if remaining <= 0.seconds
            disconnect("Daemon request timed out while waiting to write.")
            raise TimeoutError.new("Daemon request timed out.")
          end
          set_write_timeout(remaining)

          encoded = @lock.synchronize do
            raise Error.new("Daemon connection is closed.") if @closed

            @next_request &+= 1
            request_id = @next_request
            body = request.dup
            body[Protocol::REQUEST_ID] = JSON::Any.new(request_id)
            @pending[request_id] = answer
            @pending_order << request_id
            "#{body.to_json}\n"
          end
          # Socket backpressure must not hold the state lock: the reader
          # needs it to deliver replies and disconnect a failed transport.
          @io << encoded
          @io.flush
        rescue error : IO::Error | OpenSSL::Error
          disconnect("Daemon connection failed: #{error.message}")
        ensure
          @write_ready.send(nil)
        end

        wait = request["op"]?.try(&.as_s?) == "git-action" ? LONG_REQUEST_TIMEOUT : @request_timeout
        result = select
        when response = answer.receive
          response
        when timeout(wait)
          timed_out, overflow = abandon(request_id)
          if timed_out
            if overflow
              disconnect(
                "Too many daemon requests timed out."
              )
            end
            raise TimeoutError.new("Daemon request timed out.")
          end
          answer.receive
        end
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

      private def set_write_timeout(value : Time::Span) : Nil
        case io = @io
        when Socket
          io.write_timeout = value
        when IO::FileDescriptor
          io.write_timeout = value
        when OpenSSL::SSL::Socket
          io.write_timeout = value
        end
      end

      private def read_loop : Nil
        while line = Protocol.read_frame(@io)
          next if line.empty?

          root = JSON.parse(line).as_h?
          unless root
            return disconnect("Daemon sent a non-object response.")
          end

          if root.has_key?("event")
            unless enqueue_event(root)
              return disconnect("Daemon sent too many pending events.")
            end
          else
            answer, abandoned = @lock.synchronize do
              if request_id = root[Protocol::REQUEST_ID]?.try(&.as_i64?)
                @pending_order.delete(request_id)
                ignored = @abandoned.delete(request_id)
                {@pending.delete(request_id), ignored}
              elsif oldest = @pending_order.shift?
                ignored = @abandoned.delete(oldest)
                {@pending.delete(oldest), ignored}
              else
                {nil, false}
              end
            end
            next if abandoned
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
      rescue Protocol::FrameTooLarge
        disconnect("Daemon sent an oversized response.")
      rescue error : JSON::ParseException
        disconnect("Daemon sent invalid JSON.")
      rescue error : IO::Error
        disconnect("Daemon connection failed: #{error.message}")
      rescue error
        disconnect("Daemon client failed: #{error.message}")
      end

      private def enqueue_event(event : Hash(String, JSON::Any)) : Bool
        signal = @lock.synchronize do
          return false if @closed || @event_queue.size >= MAX_PENDING_EVENTS

          was_empty = @event_queue.empty?
          @event_queue << event
          was_empty
        end
        @event_ready.send(nil) if signal
        true
      rescue Channel::ClosedError
        false
      end

      private def publish_loop : Nil
        loop do
          @event_ready.receive
          while event = @lock.synchronize { @event_queue.shift? }
            publish_event(event)
            Fiber.yield
          end
        end
      rescue Channel::ClosedError
      end

      private def publish_event(event : Hash(String, JSON::Any)) : Nil
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

      private def abandon(request_id : Int64) : {Bool, Bool}
        @lock.synchronize do
          unless @pending.delete(request_id)
            next {false, false}
          end

          @abandoned << request_id
          {true, @abandoned.size > MAX_ABANDONED_REQUESTS}
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
            @abandoned.clear
            @event_queue.clear
            subscribers = @disconnect_subscribers.values.dup
            true
          else
            false
          end
        end
        return unless should_close

        @event_ready.close
        begin
          @io.close
        rescue error
          STDERR.puts "xd: daemon connection close failed: #{error.message}"
        ensure
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
end
