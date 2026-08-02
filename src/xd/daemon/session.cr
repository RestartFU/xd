require "./engine"
require "./event_bus"
require "../protocol/frame"

module Xd
  module Daemon
    # One newline-delimited protocol session.
    #
    # Both platform-local IPC and TLS sockets call this exact runner. Transport
    # only controls initial authentication; it never selects app handlers.
    class Session
      EVENT_QUEUE_SIZE = 256

      private class Writer
        @lock = Mutex.new

        def initialize(@output : IO)
        end

        def write(message) : Nil
          @lock.synchronize do
            message.to_json(@output)
            @output << '\n'
            @output.flush
          end
        end
      end

      # Engine publishers must never inherit socket backpressure from one
      # client. Queue events per session and disconnect a client that cannot
      # consume a bounded backlog; responses retain the same serialized writer.
      private class OutboundEvents
        @queue = Channel(Protocol::Event).new(EVENT_QUEUE_SIZE)
        @lock = Mutex.new
        @closed = false

        def initialize(@writer : Writer, @output : IO)
          spawn run
        end

        def enqueue(event : Protocol::Event) : Nil
          overflow = @lock.synchronize do
            next false if @closed

            select
            when @queue.send(event)
              false
            else
              @closed = true
              true
            end
          end
          shutdown if overflow
        rescue Channel::ClosedError
        end

        def close : Nil
          should_close = @lock.synchronize do
            next false if @closed

            @closed = true
            true
          end
          shutdown if should_close
        end

        private def run : Nil
          while event = @queue.receive?
            @writer.write(event)
          end
        rescue IO::Error | Channel::ClosedError
        ensure
          close
        end

        private def shutdown : Nil
          @queue.close
          @output.close
        rescue IO::Error
        end
      end

      def initialize(@engine : Engine, @events : EventBus? = nil)
      end

      def run(
        input : IO,
        output : IO,
        transport : Transport,
      ) : Nil
        connection = Connection.new(transport)
        connection.on_close = -> {
          begin
            input.close
          rescue IO::Error
          end
          begin
            output.close
          rescue IO::Error
          end
        }
        writer = Writer.new(output)
        outbound = OutboundEvents.new(writer, output)
        subscription = @events.try do |events|
          events.subscribe do |event|
            next unless connection.authenticated
            next if (audience = event.audience) &&
                    audience != connection.object_id

            outbound.enqueue(event)
          end
        end

        begin
          while line = Protocol.read_frame(
                  input,
                  connection.authenticated ?
                    Protocol::FRAME_LIMIT :
                    Protocol::AUTH_FRAME_LIMIT
                )
            next if line.empty?

            if request_id = request_id(line)
              spawn process_request(
                connection,
                line,
                request_id,
                writer
              )
            else
              # Old clients receive FIFO replies and therefore remain
              # serialized. New clients opt into multiplexing with an id.
              process_request(connection, line, nil, writer)
            end
            Fiber.yield
          end
        ensure
          @engine.connection_closed(connection)
          connection.close
          if id = subscription
            @events.try(&.unsubscribe(id))
          end
          outbound.close
        end
      rescue IO::Error
        # EOF, disconnect, and a listener shutdown all end only this session.
      end

      private def process_request(
        connection : Connection,
        line : String,
        request_id : Int64?,
        writer : Writer,
      ) : Nil
        outcome = @engine.process(connection, line)
        if id = request_id
          outcome.response.body[Protocol::REQUEST_ID] = JSON::Any.new(id)
        end
        begin
          writer.write(outcome.response)
          if events = @events
            outcome.events.each { |event| events.publish(event) }
          end
        ensure
          outcome.after_write.try(&.call)
        end
      rescue IO::Error
      end

      private def request_id(line : String) : Int64?
        JSON.parse(line).as_h?
          .try(&.[Protocol::REQUEST_ID]?)
          .try(&.as_i64?)
      rescue JSON::ParseException
        nil
      end
    end
  end
end
