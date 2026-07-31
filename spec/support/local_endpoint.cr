require "socket"
require "random/secure"
require "../../src/xd/daemon/local_ipc"

module XdSpec
  module LocalEndpoint
    extend self

    {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
      alias Connection = TCPSocket
    {% else %}
      alias Connection = UNIXSocket
    {% end %}

    def connect(path : String) : Connection
      {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
        Xd::Daemon::LocalIPC.connect(path)
      {% else %}
        UNIXSocket.new(path)
      {% end %}
    end

    def leave_stale(path : String) : Nil
      {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
        listener = TCPServer.new("127.0.0.1", 0)
        port = listener.local_address.as(Socket::IPAddress).port
        listener.close
        token = "0" * (Xd::Daemon::LocalIPC::TOKEN_BYTES * 2)
        Xd::Daemon::LocalIPC.publish(path, port, token)
      {% else %}
        listener = UNIXServer.new(path)
        listener.close
      {% end %}
    end

    class Server
      {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
        @listener : TCPServer? = nil
      {% else %}
        @listener : UNIXServer? = nil
      {% end %}
      @token : String?

      def initialize(@path : String)
        Dir.mkdir_p(File.dirname(@path))
        {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
          listener = TCPServer.new("127.0.0.1", 0)
          @listener = listener
          @token = Random::Secure.hex(Xd::Daemon::LocalIPC::TOKEN_BYTES)
          port = listener.local_address.as(Socket::IPAddress).port
          Xd::Daemon::LocalIPC.publish(@path, port, @token.not_nil!)
        {% else %}
          @listener = UNIXServer.new(@path)
          @token = nil
        {% end %}
      end

      def accept : Connection
        {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
          loop do
            socket = @listener.not_nil!.accept
            return socket if Xd::Daemon::LocalIPC.authenticate(
              socket,
              @token.not_nil!
            )
            socket.close
          end
        {% else %}
          @listener.not_nil!.accept
        {% end %}
      end

      def close : Nil
        @listener.try(&.close)
        {% if flag?(:win32) || flag?(:darwin) || flag?(:xd_loopback_local) %}
          if token = @token
            Xd::Daemon::LocalIPC.remove_if_owned(@path, token)
          end
        {% end %}
      rescue IO::Error
      end
    end
  end
end
