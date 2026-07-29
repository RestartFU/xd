module Xd
  module Agent
    module Executable
      extend self

      def resolve(name : String) : String
        key = "XD_#{name.upcase}_EXECUTABLE"
        if configured = ENV[key]?
          return configured unless configured.empty?
        end

        if executable = Process.executable_path
          filename = {% if flag?(:win32) %}
                       "#{name}.exe"
                     {% else %}
                       name
                     {% end %}
          bundled = File.expand_path(
            File.join(
              File.dirname(executable),
              "..",
              "libexec",
              filename
            )
          )
          if info = File.info?(bundled)
            return bundled if info.type.file? &&
                              info.permissions.owner_execute?
          end
        end

        Process.find_executable(name) || name
      end
    end
  end
end
