require "../app_paths"
require "../daemon/client"
require "../daemon/engine"
require "../daemon/server"
require "../storage/store"
require "../workspace/service"

module Xd
  module UI
    # Desktop-owned daemon lifetime.
    #
    # Existing daemon wins. Otherwise desktop starts exact same Engine and
    # Server used by `xd serve`, then talks to it through Unix IPC.
    class Runtime
      getter client : Daemon::Client

      @store : Storage::Store?
      @engine : Daemon::Engine?
      @server : Daemon::Server?

      def initialize
        @store = nil
        @engine = nil
        @server = nil
        @client = connect
      end

      def close : Nil
        @client.close
        @server.try(&.close)
        @engine.try(&.close)
        @store.try(&.close)
        @server = nil
        @engine = nil
        @store = nil
      end

      private def connect : Daemon::Client
        return Daemon::Client.local(AppPaths.local_socket)
      rescue Daemon::Client::Error
        start_local_daemon
      end

      private def start_local_daemon : Daemon::Client
        store = Storage::Store.new(AppPaths.database)
        workspaces = Workspace::Service.new(AppPaths.workspaces, store)
        engine = Daemon::Engine.new(store, workspaces)
        server = Daemon::Server.new(engine)

        begin
          store.clear_daemon_working
          server.listen_local(AppPaths.local_socket)
          client = Daemon::Client.local(AppPaths.local_socket)
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
            Daemon::Client.local(AppPaths.local_socket)
          rescue Daemon::Client::Error
            raise error
          end
        end
      end
    end
  end
end
