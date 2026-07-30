require "../agent/environment"

module Xd
  module UI
    module HostLaunch
      extend self

      def open_uri(uri : String) : Nil
        {% if flag?(:win32) || flag?(:darwin) %}
          Gio::AppInfo.launch_default_for_uri(uri, nil)
        {% else %}
          process = Process.new(
            ["xdg-open", uri],
            env: Agent::Environment.host,
            clear_env: true,
            input: Process::Redirect::Close,
            output: Process::Redirect::Close,
            error: Process::Redirect::Close
          )
          spawn do
            process.wait
          rescue RuntimeError
          end
        {% end %}
      rescue File::Error | IO::Error | GLib::Error
      end
    end
  end
end
