require "openssl"
require "socket"
require "./session"

module Xd
  module Daemon
    class Server
      getter remote_port : Int32?
      getter local_path : String?

      @local_listener : UNIXServer?
      @remote_listener : TCPServer?
      @tls_context : OpenSSL::SSL::Context::Server?
      @clients = [] of IO
      @lock = Mutex.new
      @closed = false

      def initialize(@engine : Engine)
        @session = Session.new(@engine)
      end

      def listen_local(path : String) : Nil
        if info = File.info?(path, follow_symlinks: false)
          unless info.type.socket?
            raise IO::Error.new(
              "Refusing to replace non-socket local endpoint: #{path}"
            )
          end
        end

        directory = Path[path].dirname
        Dir.mkdir_p(directory, 0o700) unless directory.to_s == "."
        listener = UNIXServer.new(path)
        File.chmod(path, 0o600)

        @lock.synchronize do
          raise IO::Error.new("Server is closed") if @closed
          if @local_listener
            listener.close
            raise IO::Error.new("Local listener is already running")
          end
          @local_listener = listener
          @local_path = path
        end

        spawn accept_local(listener)
      end

      def listen_remote(
        host : String,
        port : Int,
        certificate_path : String,
        private_key_path : String,
      ) : Int32
        context = OpenSSL::SSL::Context::Server.new
        context.certificate_chain = certificate_path
        context.private_key = private_key_path
        context.disable_session_resume_tickets
        listener = TCPServer.new(host, port)
        actual_port = listener.local_address
          .as(Socket::IPAddress)
          .port

        @lock.synchronize do
          raise IO::Error.new("Server is closed") if @closed
          if @remote_listener
            listener.close
            raise IO::Error.new("Remote listener is already running")
          end
          @tls_context = context
          @remote_listener = listener
          @remote_port = actual_port
        end

        spawn accept_remote(listener, context)
        actual_port
      rescue error
        listener.try(&.close)
        raise error
      end

      def close : Nil
        local : UNIXServer? = nil
        remote : TCPServer? = nil
        clients = [] of IO

        @lock.synchronize do
          return if @closed
          @closed = true
          local = @local_listener
          remote = @remote_listener
          @local_listener = nil
          @remote_listener = nil
          clients = @clients.dup
          @clients.clear
        end

        local.try(&.close)
        remote.try(&.close)
        clients.each do |client|
          begin
            client.close
          rescue IO::Error
          end
        end
      end

      private def accept_local(listener : UNIXServer) : Nil
        while client = listener.accept?
          spawn serve_client(client, Transport::Local)
        end
      rescue IO::Error
      end

      private def accept_remote(
        listener : TCPServer,
        context : OpenSSL::SSL::Context::Server,
      ) : Nil
        while client = listener.accept?
          spawn serve_remote(client, context)
        end
      rescue IO::Error
      end

      private def serve_remote(
        client : TCPSocket,
        context : OpenSSL::SSL::Context::Server,
      ) : Nil
        tls = OpenSSL::SSL::Socket::Server.new(
          client,
          context,
          sync_close: true
        )
        serve_client(tls, Transport::Remote)
      rescue OpenSSL::Error | IO::Error
        client.close unless client.closed?
      end

      private def serve_client(client : IO, transport : Transport) : Nil
        return client.close unless register(client)

        begin
          @session.run(client, client, transport)
        ensure
          unregister(client)
          client.close unless client.closed?
        end
      end

      private def register(client : IO) : Bool
        @lock.synchronize do
          return false if @closed
          @clients << client
          true
        end
      end

      private def unregister(client : IO) : Nil
        @lock.synchronize { @clients.delete(client) }
      end
    end
  end
end
