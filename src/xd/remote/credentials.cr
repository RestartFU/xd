require "json"
require "uuid"
require "../app_paths"

module Xd
  module Remote
    class Credentials
      class Error < Exception
      end

      include JSON::Serializable

      getter version : Int32
      getter host : String
      getter port : Int32
      getter token : String
      getter fingerprint : String

      def initialize(
        @host : String,
        @port : Int32,
        @token : String,
        fingerprint : String,
        @version : Int32 = 1,
      )
        @host = @host.strip
        @fingerprint = fingerprint.downcase.delete(':')
        validate
      end

      def validate : Nil
        raise Error.new("Remote host cannot be empty.") if @host.empty?
        unless 1 <= @port <= 65_535
          raise Error.new("Remote port must be from 1 to 65535.")
        end
        raise Error.new("Remote device token cannot be empty.") if @token.empty?
        unless @fingerprint.size == 64 &&
               @fingerprint.each_byte.all? { |byte| hex_digit?(byte) }
          raise Error.new("Remote certificate fingerprint is invalid.")
        end
        unless @version == 1
          raise Error.new("Remote credentials version is unsupported.")
        end
      end

      private def hex_digit?(byte : UInt8) : Bool
        (48_u8 <= byte <= 57_u8) || (97_u8 <= byte <= 102_u8)
      end
    end

    class CredentialsFile
      class Error < Exception
      end

      getter path : String

      def initialize(@path : String = AppPaths.remote_credentials)
      end

      def load : Credentials?
        return nil unless File.exists?(@path)

        credentials = Credentials.from_json(File.read(@path))
        credentials.validate
        File.chmod(@path, 0o600)
        credentials
      rescue error : Credentials::Error
        raise Error.new("Cannot use remote credentials: #{error.message}")
      rescue error : File::Error | JSON::SerializableError
        raise Error.new("Cannot read remote credentials: #{error.message}")
      end

      def save(credentials : Credentials) : Nil
        credentials.validate
        parent = File.dirname(@path)
        Dir.mkdir_p(parent, 0o700)
        temporary = "#{@path}.#{UUID.random}.tmp"

        begin
          File.open(temporary, "w", perm: 0o600) do |file|
            credentials.to_pretty_json(file)
            file << '\n'
            file.flush
            file.fsync
          end
          {% if flag?(:win32) %}
            File.delete?(@path)
          {% end %}
          File.rename(temporary, @path)
          File.chmod(@path, 0o600)
        ensure
          File.delete?(temporary)
        end
      rescue error : Credentials::Error
        raise Error.new("Cannot save remote credentials: #{error.message}")
      rescue error : File::Error | IO::Error
        raise Error.new("Cannot save remote credentials: #{error.message}")
      end

      def clear : Nil
        File.delete?(@path)
      rescue error : File::Error
        raise Error.new("Cannot remove remote credentials: #{error.message}")
      end
    end
  end
end
