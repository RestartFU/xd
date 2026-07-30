require "./version"

module Xd
  module AppPaths
    extend self

    def data_name : String
      ENV["XD_DATA_NAME"]? || DATA_NAME
    end

    def data_dir : String
      path = File.join(data_home, data_name)
      Dir.mkdir_p(path, 0o700)
      path
    end

    def database : String
      File.join(data_dir, "chats.db")
    end

    def workspaces : String
      path = File.join(data_dir, "Workspaces")
      Dir.mkdir_p(path, 0o700)
      path
    end

    def local_socket : String
      File.join(data_dir, "daemon.sock")
    end

    def certificate : String
      File.join(data_dir, "server-cert.pem")
    end

    def private_key : String
      File.join(data_dir, "server-key.pem")
    end

    def agent_secrets : String
      ENV["XD_AGENT_SECRETS_FILE"]? ||
        File.join(data_dir, "agent-secrets.json")
    end

    def remote_credentials : String
      ENV["XD_REMOTE_CREDENTIALS_FILE"]? ||
        File.join(data_dir, "remote.json")
    end

    def remote_pastes : String
      path = File.join(cache_home, data_name, "remote-pasted")
      Dir.mkdir_p(path, 0o700)
      path
    end

    private def data_home : String
      {% if flag?(:win32) %}
        ENV["LOCALAPPDATA"]? || File.join(Path.home, "AppData", "Local")
      {% else %}
        ENV["XDG_DATA_HOME"]? || File.join(Path.home, ".local", "share")
      {% end %}
    end

    private def cache_home : String
      {% if flag?(:win32) %}
        ENV["LOCALAPPDATA"]? || File.join(Path.home, "AppData", "Local")
      {% else %}
        ENV["XDG_CACHE_HOME"]? || File.join(Path.home, ".cache")
      {% end %}
    end
  end
end
