require "../app_paths"
require "../daemon/certificate"
require "../daemon/client"
require "../daemon/engine"
require "../daemon/execution_host"
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
      @daemon_host : Daemon::ExecutionHost?

      def initialize(@connect_timeout : Time::Span = CONNECT_TIMEOUT)
        @store = nil
        @engine = nil
        @server = nil
        @daemon_host = nil
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
        stop_local_daemon
        @server = nil
        @engine = nil
        @store = nil
        @daemon_host = nil
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
        host = Daemon::ExecutionHost.new
        started = Channel({Storage::Store?, Daemon::Engine?, Daemon::Server?, Exception?}).new(1)
        host.spawn(name: "xd embedded daemon startup") do
          store : Storage::Store? = nil
          engine : Daemon::Engine? = nil
          server : Daemon::Server? = nil

          begin
            store = Storage::Store.new(AppPaths.database)
            workspaces = Workspace::Service.new(
              AppPaths.workspaces,
              store
            )
            engine = Daemon::Engine.new(store, workspaces)
            server = Daemon::Server.new(engine)
            engine.peer_listener = ->(bind : String, port : Int32) {
              Daemon::Certificate.ensure_pair(
                AppPaths.certificate,
                AppPaths.private_key
              )
              actual_port = server.not_nil!.listen_remote(
                bind,
                port,
                AppPaths.certificate,
                AppPaths.private_key
              )
              store.not_nil!.save_remote_listener(bind, actual_port)
              actual_port
            }
            store.clear_daemon_working
            server.listen_local(AppPaths.local_socket)
            if listener = store.remote_listener
              bind, port = listener
              begin
                Daemon::Certificate.ensure_pair(
                  AppPaths.certificate,
                  AppPaths.private_key
                )
                server.listen_remote(
                  bind,
                  port,
                  AppPaths.certificate,
                  AppPaths.private_key
                )
              rescue error
                STDERR.puts(
                  "xd: cannot restore remote listener: #{error.message}"
                )
              end
            end
            started.send({store, engine, server, nil})
          rescue error
            server.try(&.close)
            engine.try(&.close)
            store.try(&.close)
            started.send({nil, nil, nil, error})
          end
        end

        store, engine, server, startup_error = started.receive
        unless store && engine && server
          begin
            return connect_existing
          rescue Daemon::Client::Error
            raise startup_error || RuntimeError.new(
              "Cannot start the local daemon."
            )
          end
        end

        begin
          client = connect_existing
          @store = store
          @engine = engine
          @server = server
          @daemon_host = host
          client
        rescue error
          stop_daemon(host, store, engine, server)

          # Another desktop may have won startup between first connect and
          # bind. Its endpoint is authoritative.
          begin
            connect_existing
          rescue Daemon::Client::Error
            raise error
          end
        end
      end

      private def stop_local_daemon : Nil
        host = @daemon_host
        store = @store
        engine = @engine
        server = @server
        return unless host && store && engine && server

        stop_daemon(host, store, engine, server)
      end

      private def stop_daemon(
        host : Daemon::ExecutionHost,
        store : Storage::Store,
        engine : Daemon::Engine,
        server : Daemon::Server,
      ) : Nil
        stopped = Channel(Nil).new(1)
        host.spawn(name: "xd embedded daemon shutdown") do
          begin
            server.close
          rescue error
            STDERR.puts "xd: cannot close embedded server: #{error.message}"
          end
          begin
            engine.close
          rescue error
            STDERR.puts "xd: cannot close embedded engine: #{error.message}"
          end
          begin
            store.close
          rescue error
            STDERR.puts "xd: cannot close embedded store: #{error.message}"
          ensure
            stopped.send(nil)
          end
        end
        stopped.receive
      end
    end
  end
end
