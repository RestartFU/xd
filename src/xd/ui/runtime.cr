require "../app_paths"
require "../daemon/client"
require "../daemon/engine"
require "../daemon/local_connection"
require "../daemon/server"
require "../remote/connection"
require "../storage/store"
require "../workspace/service"

module Xd
  module UI
    # Desktop-owned daemon lifetime.
    #
    # Existing daemon wins. Otherwise desktop starts exact same Engine and
    # Server used by `xd serve`, then talks to it through platform-local IPC.
    class Runtime
      CONNECT_TIMEOUT = 2.seconds

      getter client : Daemon::LocalConnection
      getter remote : Remote::Connection

      @store : Storage::Store?
      @engine : Daemon::Engine?
      @server : Daemon::Server?

      def initialize(@connect_timeout : Time::Span = CONNECT_TIMEOUT)
        @store = nil
        @engine = nil
        @server = nil
        initial = connect
        @client = Daemon::LocalConnection.new(
          AppPaths.local_socket,
          initial_client: initial,
          connector: ->(_path : String) { connect_existing }
        )
        @remote = Remote::Connection.new
      end

      def close : Nil
        @remote.close
        @client.close
        @server.try(&.close)
        @engine.try(&.close)
        @store.try(&.close)
        @server = nil
        @engine = nil
        @store = nil
      end

      private def connect : Daemon::Client
        return connect_existing
      rescue Daemon::Client::Error
        start_local_daemon
      end

      private def connect_existing : Daemon::Client
        client = Daemon::Client.local(
          AppPaths.local_socket,
          request_timeout: @connect_timeout
        )
        begin
          client.call({"op" => JSON::Any.new("ping")})
          client
        rescue error
          client.close
          raise error
        end
      end

      private def start_local_daemon : Daemon::Client
        store = Storage::Store.new(AppPaths.database)
        workspaces = Workspace::Service.new(AppPaths.workspaces, store)
        engine = Daemon::Engine.new(store, workspaces)
        server = Daemon::Server.new(engine)

        begin
          store.clear_daemon_working
          server.listen_local(AppPaths.local_socket)
          client = connect_existing
          @store = store
          @engine = engine
          @server = server
          client
        rescue error
          server.close
          engine.close
          store.close

          # Another desktop may have won startup between first connect and
          # bind. Its endpoint is authoritative.
          begin
            connect_existing
          rescue Daemon::Client::Error
            raise error
          end
        end
      end
    end
  end
end
