require "digest/sha256"
require "json"
require "uuid"
require "../app_paths"

module Xd
  module Agent
    class Secrets
      MAX_ENTRIES = 256

      class Error < Exception
      end

      private record Document,
        version : Int32,
        secrets : Hash(String, String) do
        include JSON::Serializable
      end

      getter path : String

      def initialize(
        @path : String,
        @values = {} of String => String,
      )
      end

      def self.load(path : String = AppPaths.agent_secrets) : self
        secrets = new(path)
        return secrets unless File.exists?(path)

        root = JSON.parse(File.read(path)).as_h?
        unless root
          raise Error.new("#{path} does not contain a JSON object")
        end
        values = root["secrets"]?.try(&.as_h?)
        unless values
          raise Error.new("#{path} has no secrets object")
        end
        if values.size > MAX_ENTRIES
          raise Error.new(
            "#{path} contains more than #{MAX_ENTRIES} secrets"
          )
        end

        values.each do |name, node|
          value = node.as_s?
          unless valid_name?(name) && value && !value.empty?
            raise Error.new("#{path} contains an invalid secret entry")
          end
          secrets.@values[name] = value
        end
        File.chmod(path, 0o600)
        secrets
      rescue error : JSON::ParseException
        raise Error.new("Cannot read #{path}: #{error.message}")
      rescue error : File::Error
        raise Error.new("Cannot read #{path}: #{error.message}")
      end

      def self.for_folder(
        folder_id : String,
        global_path : String = AppPaths.agent_secrets,
      ) : self
        raise Error.new("A folder id cannot be empty") if folder_id.empty?
        load(folder_path(folder_id, global_path))
      end

      def self.effective(
        folder_ids : Enumerable(String),
        global_path : String = AppPaths.agent_secrets,
      ) : self
        merged = load(global_path)
        folder_ids.each do |folder_id|
          scoped = for_folder(folder_id, global_path)
          scoped.@values.each do |name, value|
            if !merged.@values.has_key?(name) &&
               merged.@values.size >= MAX_ENTRIES
              raise Error.new(
                "Effective secret set exceeds #{MAX_ENTRIES} entries."
              )
            end
            merged.@values[name] = value
          end
        end
        merged
      end

      def self.valid_name?(name : String?) : Bool
        return false unless name
        return false if name.empty?

        first = name.byte_at(0)
        return false unless ascii_letter?(first) || first == '_'.ord

        name.each_byte.skip(1).all? do |byte|
          ascii_letter?(byte) || ascii_digit?(byte) || byte == '_'.ord
        end
      end

      def names : Array(String)
        @values.keys.sort
      end

      def includes?(name : String) : Bool
        @values.has_key?(name)
      end

      def set(name : String, value : String) : Nil
        unless self.class.valid_name?(name)
          raise Error.new(
            "Secret names must use letters, numbers and underscores, " \
            "and cannot start with a number."
          )
        end
        if value.empty?
          raise Error.new("A new secret needs a value.")
        end
        if !@values.has_key?(name) && @values.size >= MAX_ENTRIES
          raise Error.new("At most #{MAX_ENTRIES} secrets can be stored.")
        end
        @values[name] = value
      end

      def remove(name : String) : Nil
        @values.delete(name)
      end

      def environment(
        base : Hash(String, String) = ENV.to_h,
      ) : Hash(String, String)
        result = base.dup
        @values.each { |name, value| result[name] = value }
        result
      end

      def prompt : String?
        list = names
        return nil if list.empty?

        "[Agent secrets available as environment variables: " \
        "#{list.join(", ")}. Their values are not included in this prompt. " \
        "Use them when needed, and never print or expose their values.]"
      end

      def save : Nil
        parent = File.dirname(@path)
        Dir.mkdir_p(parent, 0o700)
        temporary = "#{@path}.#{UUID.random}.tmp"
        document = Document.new(1, @values)

        begin
          File.open(temporary, "w", perm: 0o600) do |file|
            document.to_pretty_json(file)
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
      rescue error : File::Error | IO::Error
        raise Error.new("Cannot save #{@path}: #{error.message}")
      end

      private def self.folder_path(
        folder_id : String,
        global_path : String,
      ) : String
        directory = "#{global_path}.d"
        digest = Digest::SHA256.hexdigest(folder_id)
        File.join(directory, "#{digest}.json")
      end

      private def self.ascii_letter?(byte : UInt8) : Bool
        (byte >= 'A'.ord && byte <= 'Z'.ord) ||
          (byte >= 'a'.ord && byte <= 'z'.ord)
      end

      private def self.ascii_digit?(byte : UInt8) : Bool
        byte >= '0'.ord && byte <= '9'.ord
      end
    end
  end
end
