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

            outcome = @engine.process(connection, line)
            begin
              writer.write(outcome.response)
              if events = @events
                outcome.events.each { |event| events.publish(event) }
              end
            ensure
              outcome.after_write.try(&.call)
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
    end
  end
end
