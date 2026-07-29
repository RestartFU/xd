module Xd
  module Daemon
    enum Transport
      Local
      Remote
    end

    class Connection
      getter transport : Transport
      property authenticated : Bool

      def initialize(@transport)
        # Local IPC is authenticated by its filesystem/OS boundary. Remote
        # transport must pair and present a token. Both run the same handlers
        # after this transport-only gate.
        @authenticated = @transport.local?
      end
    end
  end
end
