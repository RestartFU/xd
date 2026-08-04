require "json"

module Xd
  module Workspace
    # Legacy on-disk metadata. Workspace::Service consumes these files into
    # SQLite and removes them; nothing writes sidecars anymore.
    SETTINGS_FILE             = ".xd.json"
    LEGACY_SETTINGS_FILE      = ".hy.json"
    WORKTREE_CONTAINER_MARKER = ".xd-worktrees"

    class Error < Exception
    end

    class Settings
      include JSON::Serializable

      property id : String?
      property backend : String?
      property model : String?
      property workdir : String?
      property repo : String?
      property instructions : String?
      property shortcuts : Array(String)?

      def initialize(
        @id : String? = nil,
        @backend : String? = nil,
        @model : String? = nil,
        @workdir : String? = nil,
        @repo : String? = nil,
        @instructions : String? = nil,
        @shortcuts : Array(String)? = nil,
      )
      end
    end

    class SettingsFile
      def self.load(folder_path : String) : Settings
        Settings.from_json(File.read(path_for(folder_path)))
      rescue error : File::Error | JSON::SerializableError
        raise Error.new("Cannot read folder settings: #{error.message}")
      end

      def self.load?(folder_path : String) : Settings?
        load(folder_path)
      rescue Error
        nil
      end

      def self.managed?(folder_path : String) : Bool
        File.file?(File.join(folder_path, SETTINGS_FILE)) ||
          File.file?(File.join(folder_path, LEGACY_SETTINGS_FILE))
      end

      def self.path_for(folder_path : String) : String
        current = File.join(folder_path, SETTINGS_FILE)
        legacy = File.join(folder_path, LEGACY_SETTINGS_FILE)
        !File.exists?(current) && File.exists?(legacy) ? legacy : current
      end

      def self.remove(folder_path : String) : Nil
        File.delete?(File.join(folder_path, SETTINGS_FILE))
        File.delete?(File.join(folder_path, LEGACY_SETTINGS_FILE))
      rescue error : File::Error
        raise Error.new("Cannot remove imported folder settings: #{error.message}")
      end
    end

    record EffectiveSettings,
      backend : String,
      model : String?,
      workdir : String,
      repo : String?,
      instructions : String?

    record SettingsInheritance,
      backend : String,
      model : String?,
      workdir : String?,
      repo : String?,
      backend_from : String?,
      model_from : String?,
      workdir_from : String?,
      repo_from : String?
  end
end
