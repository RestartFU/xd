require "openssl"
require "socket"
require "./local_ipc"
require "./session"

module Xd
  module Daemon
    class Server
      getter remote_port : Int32?
      getter local_path : String?

      {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
        alias LocalListener = TCPServer
      {% else %}
        alias LocalListener = UNIXServer
      {% end %}

      @local_listener : LocalListener?
      @local_token : String?
      @local_lock_path : String?
      @remote_listener : TCPServer?
      @remote_host : String?
      @remote_requested_port : Int32?
      @tls_context : OpenSSL::SSL::Context::Server?
      @clients = [] of IO
      @lock = Mutex.new
      @closed = false

      def initialize(@engine : Engine)
        @events = @engine.events
        @session = Session.new(@engine, @events)
      end

      def listen_local(path : String) : Nil
        {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
          listen_local_loopback(path)
        {% else %}
          if info = File.info?(path, follow_symlinks: false)
            unless info.type.socket?
              raise IO::Error.new(
                "Refusing to replace non-socket local endpoint: #{path}"
              )
            end

            live = begin
              probe = UNIXSocket.new(path)
              probe.close
              true
            rescue IO::Error
              false
            end
            if live
              raise IO::Error.new("Local daemon is already running at #{path}")
            end
            File.delete(path)
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
        {% end %}
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
        listener : TCPServer? = nil
        actual_port = @lock.synchronize do
          raise IO::Error.new("Server is closed") if @closed
          if @remote_listener
            if @remote_host == host &&
               (@remote_requested_port == port || @remote_port == port)
              return @remote_port.not_nil!
            end
            raise IO::Error.new("Remote listener is already running")
          end

          listener = TCPServer.new(host, port)
          bound_port = listener.not_nil!.local_address
            .as(Socket::IPAddress)
            .port
          @tls_context = context
          @remote_listener = listener
          @remote_host = host
          @remote_requested_port = port.to_i32
          @remote_port = bound_port
          bound_port
        end

        spawn accept_remote(listener.not_nil!, context)
        actual_port
      rescue error
        listener.try(&.close)
        raise error
      end

      def close : Nil
        local : LocalListener? = nil
        remote : TCPServer? = nil
        clients = [] of IO
        local_path : String? = nil
        local_token : String? = nil
        local_lock_path : String? = nil

        @lock.synchronize do
          return if @closed
          @closed = true
          local = @local_listener
          remote = @remote_listener
          @local_listener = nil
          @remote_listener = nil
          @remote_host = nil
          @remote_requested_port = nil
          local_path = @local_path
          @local_path = nil
          local_token = @local_token
          @local_token = nil
          local_lock_path = @local_lock_path
          @local_lock_path = nil
          clients = @clients.dup
          @clients.clear
        end

        local.try(&.close)
        remote.try(&.close)
        clients.each do |client|
          begin
            client.close
          rescue IO::Error | OpenSSL::Error
          end
        end
        if path = local_path
          {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
            if token = local_token
              LocalIPC.remove_if_owned(path, token)
            end
          {% else %}
            info = File.info?(path, follow_symlinks: false)
            File.delete?(path) if info.try(&.type.socket?)
          {% end %}
        end
        LocalIPC.release(local_lock_path.not_nil!) if local_lock_path
      end

      {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
        private def listen_local_loopback(path : String) : Nil
          lock_path = LocalIPC.claim(path)
          token = Random::Secure.hex(LocalIPC::TOKEN_BYTES)
          listener : TCPServer? = nil
          published = false
          owned = false

          begin
            LocalIPC.prepare(path)
            listener = TCPServer.new("127.0.0.1", 0)
            port = listener.local_address.as(Socket::IPAddress).port
            LocalIPC.publish(path, port, token)
            published = true

            active = listener.not_nil!
            @lock.synchronize do
              raise IO::Error.new("Server is closed") if @closed
              if @local_listener
                raise IO::Error.new("Local listener is already running")
              end
              @local_listener = active
              @local_path = path
              @local_token = token
              @local_lock_path = lock_path
              owned = true
            end

            spawn accept_local(active, token)
          rescue error
            listener.try(&.close)
            LocalIPC.remove_if_owned(path, token) if published && !owned
            LocalIPC.release(lock_path) unless owned
            raise error
          end
        end

        private def accept_local(
          listener : TCPServer,
          token : String,
        ) : Nil
          while client = listener.accept?
            spawn serve_local(client, token)
          end
        rescue IO::Error
        end

        private def serve_local(client : TCPSocket, token : String) : Nil
          unless LocalIPC.authenticate(client, token)
            return client.close
          end
          serve_client(client, Transport::Local)
        rescue IO::Error
          client.close unless client.closed?
        end
      {% else %}
        private def accept_local(listener : UNIXServer) : Nil
          while client = listener.accept?
            spawn serve_client(client, Transport::Local)
          end
        rescue IO::Error
        end
      {% end %}

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
