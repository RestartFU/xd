require "../bundle_environment"

module Xd
  module Agent
    module Executable
      extend self

      def resolve(name : String) : String
        key = "XD_#{name.upcase.gsub('-', '_')}_EXECUTABLE"
        if configured = ENV[key]?
          return configured unless configured.empty?
        end

        if bundled = BundleEnvironment.executable(name)
          return bundled
        end

        Process.find_executable(name) || name
      end
    end
  end
end
