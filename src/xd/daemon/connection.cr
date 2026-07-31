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
        # Local IPC is authenticated by Unix permissions or the per-user
        # Windows endpoint token. Remote transport must pair and present a
        # device token. Both run the same handlers after this transport gate.
        @authenticated = @transport.local?
      end
    end
  end
end
