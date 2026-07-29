require "./engine"

module Xd
  module Daemon
    # One newline-delimited protocol session.
    #
    # Both Unix IPC and TLS sockets call this exact runner. Transport only
    # controls initial authentication; it never selects application handlers.
    class Session
      def initialize(@engine : Engine)
      end

      def run(
        input : IO,
        output : IO,
        transport : Transport,
      ) : Nil
        connection = Connection.new(transport)

        while line = input.gets
          next if line.empty?

          @engine.dispatch(connection, line).to_json(output)
          output << '\n'
          output.flush
        end
      rescue IO::Error
        # EOF, disconnect, and a listener shutdown all end only this session.
      end
    end
  end
end
