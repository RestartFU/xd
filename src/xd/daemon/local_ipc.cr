require "base64"
require "json"
require "random/secure"
require "socket"

{% if flag?(:win32) %}
  @[Link("crypt32")]
  lib LibLocalDPAPI
    CRYPTPROTECT_UI_FORBIDDEN = 1_u32

    struct Blob
      size : LibC::DWORD
      data : UInt8*
    end

    fun protect = CryptProtectData(
      input : Blob*,
      description : UInt16*,
      entropy : Blob*,
      reserved : Void*,
      prompt : Void*,
      flags : LibC::DWORD,
      output : Blob*,
    ) : Int32
    fun unprotect = CryptUnprotectData(
      input : Blob*,
      description : UInt16**,
      entropy : Blob*,
      reserved : Void*,
      prompt : Void*,
      flags : LibC::DWORD,
      output : Blob*,
    ) : Int32
  end
{% end %}

module Xd
  module Daemon
    # Windows local IPC uses an authenticated loopback socket because Crystal
    # does not expose named pipes as IO. The endpoint file lives under the
    # user's private data directory and contains an unguessable session token.
    module LocalIPC
      extend self

      VERSION             = 1_i64
      TOKEN_BYTES         =    32
      AUTH_LINE_LIMIT     =   512
      ENDPOINT_SIZE_LIMIT = 4_096
      STARTUP_GRACE       = 2.seconds
      RETRY_DELAY         = 20.milliseconds

      record Descriptor,
        port : Int32,
        token : String

      class Error < IO::Error
      end

      class InvalidEndpoint < Error
      end

      class InUse < Error
      end

      def claim(
        path : String,
        startup_grace : Time::Span = STARTUP_GRACE,
      ) : String
        parent = File.dirname(path)
        Dir.mkdir_p(parent, 0o700) unless parent == "."
        lock_path = "#{path}.lock"
        deadline = Time.instant + startup_grace

        loop do
          begin
            Dir.mkdir(lock_path, 0o700)
            return lock_path
          rescue error : File::AlreadyExistsError
          rescue error : File::Error
            raise Error.new(
              "Cannot claim local endpoint #{path}: #{error.message}"
            )
          end

          if descriptor = descriptor?(path)
            if live?(descriptor)
              raise InUse.new("Local daemon is already running at #{path}")
            end
          end

          if Time.instant >= deadline
            reclaim_lock(lock_path, path)
            deadline = Time.instant + startup_grace
          else
            sleep RETRY_DELAY
          end
        end
      end

      def prepare(path : String) : Nil
        info = File.info?(path, follow_symlinks: false)
        return unless info
        unless info.type.file?
          raise InvalidEndpoint.new(
            "Refusing to replace non-file local endpoint: #{path}"
          )
        end

        descriptor = read(path)
        if live?(descriptor)
          raise InUse.new("Local daemon is already running at #{path}")
        end
        File.delete(path)
      rescue error : InvalidEndpoint
        raise InvalidEndpoint.new(
          "Refusing to replace invalid local endpoint: #{path}"
        )
      rescue error : File::Error
        raise Error.new(
          "Cannot prepare local endpoint #{path}: #{error.message}"
        )
      end

      def publish(
        path : String,
        port : Int32,
        token : String,
      ) : Nil
        validate(port, token)
        encoded_token = encode_token(token)
        temporary = "#{path}.#{Random::Secure.hex(8)}.tmp"

        begin
          File.open(temporary, "w", perm: 0o600) do |file|
            JSON.build(file) do |json|
              json.object do
                json.field "version", VERSION
                json.field "port", port
                json.field "token", encoded_token
              end
            end
            file << '\n'
            file.flush
            file.fsync
          end
          {% if flag?(:win32) %}
            File.delete?(path)
          {% end %}
          File.rename(temporary, path)
          File.chmod(path, 0o600)
        ensure
          File.delete?(temporary)
        end
      rescue error : File::Error | IO::Error
        raise Error.new(
          "Cannot publish local endpoint #{path}: #{error.message}"
        )
      end

      def read(path : String) : Descriptor
        info = File.info?(path, follow_symlinks: false)
        unless info.try(&.type.file?)
          raise InvalidEndpoint.new("Local endpoint is not a file: #{path}")
        end
        if info.not_nil!.size > ENDPOINT_SIZE_LIMIT
          raise InvalidEndpoint.new("Local endpoint is too large: #{path}")
        end

        root = JSON.parse(File.read(path)).as_h?
        unless root
          raise InvalidEndpoint.new("Local endpoint is not an object: #{path}")
        end
        version = root["version"]?.try(&.as_i?)
        port = root["port"]?.try(&.as_i?)
        encoded_token = root["token"]?.try(&.as_s?)
        unless version == VERSION && port && encoded_token
          raise InvalidEndpoint.new("Local endpoint is incomplete: #{path}")
        end
        unless port >= 1 && port <= UInt16::MAX
          raise InvalidEndpoint.new("Local endpoint port is invalid: #{path}")
        end
        token = decode_token(encoded_token)
        validate(port.to_i32, token)
        Descriptor.new(port.to_i32, token)
      rescue error : JSON::ParseException
        raise InvalidEndpoint.new("Local endpoint is invalid JSON: #{path}")
      rescue error : File::Error
        raise InvalidEndpoint.new(
          "Cannot read local endpoint #{path}: #{error.message}"
        )
      end

      def connect(path : String) : TCPSocket
        descriptor = read(path)
        socket : TCPSocket? = nil
        begin
          socket = TCPSocket.new("127.0.0.1", descriptor.port)
          socket.read_timeout = STARTUP_GRACE
          socket.write_timeout = STARTUP_GRACE
          socket << {
            "token" => descriptor.token,
          }.to_json << '\n'
          socket.flush

          line = bounded_line(socket)
          accepted = line.try do |value|
            JSON.parse(value)["ok"]?.try(&.as_bool?) == true
          rescue JSON::ParseException
            false
          end
          unless accepted
            raise Error.new("Local daemon rejected endpoint authentication.")
          end

          socket.read_timeout = nil
          socket.write_timeout = nil
          socket
        rescue error : Error
          socket.try(&.close)
          raise error
        rescue error : IO::Error
          socket.try(&.close)
          raise Error.new(
            "Cannot connect to local daemon at #{path}: #{error.message}"
          )
        end
      end

      def authenticate(socket : TCPSocket, expected_token : String) : Bool
        socket.read_timeout = STARTUP_GRACE
        socket.write_timeout = STARTUP_GRACE
        line = bounded_line(socket)
        token = line.try do |value|
          JSON.parse(value)["token"]?.try(&.as_s?)
        rescue JSON::ParseException
          nil
        end
        accepted = !!token && secure_compare(token, expected_token)
        socket << %({"ok":#{accepted}}) << '\n'
        socket.flush
        if accepted
          socket.read_timeout = nil
          socket.write_timeout = nil
        end
        accepted
      rescue IO::Error
        false
      end

      def live?(descriptor : Descriptor) : Bool
        socket = TCPSocket.new("127.0.0.1", descriptor.port)
        true
      rescue IO::Error
        false
      ensure
        socket.try(&.close)
      end

      def remove_if_owned(path : String, token : String) : Nil
        descriptor = descriptor?(path)
        File.delete?(path) if descriptor && secure_compare(
                                descriptor.token,
                                token
                              )
      rescue File::Error
      end

      def release(lock_path : String) : Nil
        Dir.delete(lock_path)
      rescue File::Error
      end

      private def descriptor?(path : String) : Descriptor?
        read(path)
      rescue InvalidEndpoint
        nil
      end

      private def reclaim_lock(lock_path : String, path : String) : Nil
        stale = "#{lock_path}.#{Random::Secure.hex(8)}.stale"
        begin
          unless Dir.empty?(lock_path)
            raise Error.new(
              "Cannot reclaim non-empty local endpoint lock: #{path}"
            )
          end
          File.rename(lock_path, stale)
          Dir.delete(stale)
        rescue error : File::NotFoundError
        rescue error : File::Error
          raise Error.new(
            "Cannot reclaim local endpoint lock #{path}: #{error.message}"
          )
        ensure
          begin
            Dir.delete(stale) if Dir.exists?(stale)
          rescue File::Error
          end
        end
      end

      private def validate(port : Int32, token : String) : Nil
        valid_token = token.bytesize == TOKEN_BYTES * 2 &&
                      token.each_byte.all? do |byte|
                        (byte >= '0'.ord && byte <= '9'.ord) ||
                          (byte >= 'a'.ord && byte <= 'f'.ord)
                      end
        unless port >= 1 && port <= UInt16::MAX && valid_token
          raise InvalidEndpoint.new("Invalid local endpoint descriptor.")
        end
      end

      private def bounded_line(io : IO) : String?
        line = io.gets('\n', AUTH_LINE_LIMIT, chomp: false)
        return nil unless line
        return nil if line.bytesize >= AUTH_LINE_LIMIT
        return nil unless line.ends_with?('\n')
        line
      end

      private def encode_token(token : String) : String
        {% if flag?(:win32) %}
          protect_token(token)
        {% else %}
          token
        {% end %}
      end

      private def decode_token(token : String) : String
        {% if flag?(:win32) %}
          unprotect_token(token)
        {% else %}
          token
        {% end %}
      end

      {% if flag?(:win32) %}
        private def protect_token(token : String) : String
          bytes = token.to_slice
          input = LibLocalDPAPI::Blob.new(
            size: bytes.size.to_u32,
            data: bytes.to_unsafe
          )
          output = LibLocalDPAPI::Blob.new(
            size: 0_u32,
            data: Pointer(UInt8).null
          )
          success = LibLocalDPAPI.protect(
            pointerof(input),
            Pointer(UInt16).null,
            Pointer(LibLocalDPAPI::Blob).null,
            Pointer(Void).null,
            Pointer(Void).null,
            LibLocalDPAPI::CRYPTPROTECT_UI_FORBIDDEN,
            pointerof(output)
          )
          unless success != 0 && !output.data.null?
            LibC.LocalFree(output.data.as(Void*)) unless output.data.null?
            raise InvalidEndpoint.new(
              "Windows could not protect the local endpoint token."
            )
          end

          begin
            Base64.strict_encode(Slice.new(output.data, output.size.to_i))
          ensure
            LibC.LocalFree(output.data.as(Void*))
          end
        end

        private def unprotect_token(encoded : String) : String
          bytes = Base64.decode(encoded)
          input = LibLocalDPAPI::Blob.new(
            size: bytes.size.to_u32,
            data: bytes.to_unsafe
          )
          output = LibLocalDPAPI::Blob.new(
            size: 0_u32,
            data: Pointer(UInt8).null
          )
          success = LibLocalDPAPI.unprotect(
            pointerof(input),
            Pointer(Pointer(UInt16)).null,
            Pointer(LibLocalDPAPI::Blob).null,
            Pointer(Void).null,
            Pointer(Void).null,
            LibLocalDPAPI::CRYPTPROTECT_UI_FORBIDDEN,
            pointerof(output)
          )
          unless success != 0 && !output.data.null?
            LibC.LocalFree(output.data.as(Void*)) unless output.data.null?
            raise InvalidEndpoint.new(
              "Windows could not read the local endpoint token."
            )
          end

          begin
            String.new(output.data, output.size.to_i)
          ensure
            LibC.LocalFree(output.data.as(Void*))
          end
        rescue Base64::Error
          raise InvalidEndpoint.new(
            "Windows local endpoint token is not valid base64."
          )
        end
      {% end %}

      private def secure_compare(left : String, right : String) : Bool
        return false unless left.bytesize == right.bytesize

        difference = 0_u8
        left.to_slice.zip(right.to_slice) do |a, b|
          difference |= a ^ b
        end
        difference == 0
      end
    end
  end
end
