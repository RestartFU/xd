require "./engine"
require "./event_bus"

module Xd
  module Daemon
    # One newline-delimited protocol session.
    #
    # Both platform-local IPC and TLS sockets call this exact runner. Transport
    # only controls initial authentication; it never selects app handlers.
    class Session
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

      def initialize(@engine : Engine, @events : EventBus? = nil)
      end

      def run(
        input : IO,
        output : IO,
        transport : Transport,
      ) : Nil
        connection = Connection.new(transport)
        writer = Writer.new(output)
        subscription = @events.try do |events|
          events.subscribe do |event|
            next unless connection.authenticated
            next if (audience = event.audience) &&
                    audience != connection.object_id

            begin
              writer.write(event)
            rescue IO::Error
            end
          end
        end

        begin
          while line = input.gets
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
          if id = subscription
            @events.try(&.unsubscribe(id))
          end
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
