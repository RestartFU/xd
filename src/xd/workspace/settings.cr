require "json"
require "uuid"

module Xd
  module Workspace
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

      def initialize(
        @id : String? = nil,
        @backend : String? = nil,
        @model : String? = nil,
        @workdir : String? = nil,
        @repo : String? = nil,
        @instructions : String? = nil,
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

      def self.ensure(folder_path : String) : Settings
        path = path_for(folder_path)
        settings = if File.file?(path)
                     load?(folder_path) || Settings.new
                   else
                     Settings.new
                   end

        if settings.id.nil?
          settings.id = UUID.random.to_s
          save(settings, folder_path)
        elsif !File.file?(path)
          save(settings, folder_path)
        end
        settings
      end

      def self.save(settings : Settings, folder_path : String) : Nil
        raise Error.new("Folder settings need an id") unless settings.id

        path = path_for(folder_path)
        temporary = File.tempfile(".xd-settings-", dir: folder_path)
        temporary_path : String? = temporary.path
        begin
          File.chmod(temporary.path, 0o600)
          temporary << settings.to_pretty_json << '\n'
          temporary.flush
          temporary.fsync
          temporary.close
          File.rename(temporary.path, path)
          temporary_path = nil
        ensure
          temporary.close unless temporary.closed?
          if pending = temporary_path
            File.delete?(pending)
          end
        end
      rescue error : File::Error | IO::Error
        raise Error.new("Cannot save folder settings: #{error.message}")
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
