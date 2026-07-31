require "../agent/environment"

module Xd
  module UI
    module HostLaunch
      extend self

      def open_uri(uri : String) : Nil
        command = {% if flag?(:win32) %}
                    ["rundll32.exe", "url.dll,FileProtocolHandler", uri]
                  {% elsif flag?(:darwin) %}
                    ["open", uri]
                  {% else %}
                    ["xdg-open", uri]
                  {% end %}

        Fiber::ExecutionContext::Isolated.new("xd host launch") do
          process = Process.new(
            command,
            env: Agent::Environment.host,
            clear_env: true,
            input: Process::Redirect::Close,
            output: Process::Redirect::Close,
            error: Process::Redirect::Close
          )
          process.wait
        rescue File::Error | IO::Error | RuntimeError
        end
      rescue RuntimeError
      end
    end
  end
end
