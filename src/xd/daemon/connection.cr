module Xd
  module Daemon
    enum Transport
      Local
      Remote
    end

    class Connection
      getter transport : Transport

      @lock = Mutex.new
      @authenticated : Bool
      @device_id : String?
      @closed : Bool
      @revoked : Bool
      @on_close : Proc(Nil)?

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

      def authenticated : Bool
        @lock.synchronize { @authenticated }
      end

      def authenticated=(value : Bool) : Bool
        @lock.synchronize { @authenticated = value }
      end

      def device_id : String?
        @lock.synchronize { @device_id }
      end

      def closed : Bool
        @lock.synchronize { @closed }
      end

      def revoked : Bool
        @lock.synchronize { @revoked }
      end

      def on_close=(callback : Proc(Nil)?) : Proc(Nil)?
        call_now = @lock.synchronize do
          if @closed
            true
          else
            @on_close = callback
            false
          end
        end
        callback.try(&.call) if call_now
        callback
      end

      def authenticate(device_id : String? = nil) : Nil
        @lock.synchronize do
          return if @closed || @revoked

          @authenticated = true
          @device_id = device_id
        end
      end

      def revoke : Nil
        already_closed = @lock.synchronize do
          if @closed
            true
          else
            @authenticated = false
            @revoked = true
            false
          end
        end
        close unless already_closed
      end

      def close : Nil
        callback = @lock.synchronize do
          next nil if @closed

          @closed = true
          @authenticated = false
          @on_close
        end
        callback.try(&.call)
      end
    end
  end
end
