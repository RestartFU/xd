module Xd
  module Daemon
    enum Transport
      Local
      Remote
    end

    class Connection
      getter transport : Transport
      property authenticated : Bool
      getter device_id : String?
      getter closed : Bool
      getter revoked : Bool
      property on_close : Proc(Nil)?

      def initialize(@transport)
        # Local IPC is authenticated by Unix permissions or the per-user
        # Windows endpoint token. Remote transport must pair and present a
        # device token. Both run the same handlers after this transport gate.
        @authenticated = @transport.local?
        @device_id = nil
        @closed = false
        @revoked = false
        @on_close = nil
      end

      def authenticate(device_id : String? = nil) : Nil
        @authenticated = true
        @device_id = device_id
      end

      def revoke : Nil
        return if @closed

        @authenticated = false
        @revoked = true
        close
      end

      def close : Nil
        return if @closed

        @closed = true
        @authenticated = false
        @on_close.try(&.call)
      end
    end
  end
end
