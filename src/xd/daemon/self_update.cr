require "json"
require "../agent/environment"
require "../bundle_environment"
require "../update_channel"
require "../version"

module Xd
  module Daemon
    # Updates the daemon's own installation, when a client asks.
    #
    # A paired device can see the machine is behind but has no way to do
    # anything about it without a shell there. This is that button.
    #
    # Installing and restarting are deliberately separate. Replacing files on
    # disk is safe while turns run -- the process keeps its mapped binary --
    # but restarting drops every connection and loses whatever the agent was
    # doing. Only the caller can know that is acceptable, so only the caller
    # asks for it.
    class SelfUpdate
      class Error < Exception
      end

      enum State
        Idle
        Checking
        Installing
        Installed
        Failed
      end

      alias Publisher = Proc(String, Hash(String, JSON::Any), Nil)

      @state = State::Idle
      @latest : String?
      @trouble : String?
      @lock = Mutex.new

      def initialize(@publish : Publisher)
      end

      # Only a Linux bundle install can replace itself with the installer.
      # macOS and Windows ship through their own packaging, and a build run
      # from a source tree has nothing to replace.
      def self.install_directory : String?
        {% if flag?(:linux) && flag?(:x86_64) %}
          executable = Process.executable_path
          return unless executable
          directory = File.dirname(File.dirname(executable))
          expected = File.join(Path.home, ".local", "opt", DATA_NAME)
          launcher = File.join(directory, "xd.sh")
          directory == expected && File::Info.executable?(launcher) ? directory : nil
        {% else %}
          nil
        {% end %}
      rescue File::Error
        nil
      end

      def supported? : Bool
        !self.class.install_directory.nil?
      end

      def snapshot : Hash(String, JSON::Any)
        @lock.synchronize do
          fields = {
            "version"   => JSON::Any.new(Xd.version_string),
            "channel"   => JSON::Any.new(UpdateChannel.current.to_s.downcase),
            "supported" => JSON::Any.new(supported?),
            "state"     => JSON::Any.new(@state.to_s.downcase),
            "available" => JSON::Any.new(
              UpdateChannel.newer?(UpdateChannel.current, @latest)
            ),
          }
          if latest = @latest
            fields["latest"] = JSON::Any.new(latest)
          end
          if trouble = @trouble
            fields["error"] = JSON::Any.new(trouble)
          end
          fields
        end
      end

      # Asks the release API what the newest build is.
      def check : Nil
        return unless begin_work(State::Checking)

        run_isolated("xd daemon update check") do
          output = IO::Memory.new
          status = Process.run(
            curl,
            [
              "-fsSL", "--max-time", "20",
              "-H", "Accept: application/vnd.github+json",
              UpdateChannel.check_url(UpdateChannel.current),
            ],
            env: Agent::Environment.host,
            clear_env: true,
            input: Process::Redirect::Close,
            output: output,
            error: Process::Redirect::Close
          )
          latest = if status.success?
                     UpdateChannel.latest_from_reply(
                       UpdateChannel.current,
                       output.to_s
                     )
                   end
          finish(State::Idle, latest: latest, trouble: latest ? nil : "Could not reach the release feed.")
        end
      end

      # Replaces the installed bundle. The running daemon keeps serving from
      # the binary it already mapped until it is restarted.
      def install : Nil
        unless supported?
          raise Error.new("This daemon's installation cannot update itself.")
        end
        return unless begin_work(State::Installing)

        run_isolated("xd daemon update install") do
          errors = IO::Memory.new
          environment = Agent::Environment.host
          environment["XD_CURL"] = curl
          status = Process.run(
            ["sh", "-c", UpdateChannel.install_command(UpdateChannel.current, curl)],
            env: environment,
            clear_env: true,
            input: Process::Redirect::Close,
            output: Process::Redirect::Close,
            error: errors
          )
          if status.success?
            finish(State::Installed)
          else
            finish(
              State::Failed,
              trouble: errors.to_s.strip.presence || "The installer failed."
            )
          end
        end
      end

      # Replaces this process with the freshly installed one.
      #
      # Every connection drops and any running turn is lost, which is why this
      # is never automatic.
      def restart : Nil
        directory = self.class.install_directory
        unless directory
          raise Error.new("This daemon's installation cannot restart itself.")
        end

        launcher = File.join(directory, "xd.sh")
        publish("daemon-update")
        # Let the reply reach the client before the socket dies with us.
        spawn do
          sleep 250.milliseconds
          begin
            Process.exec(
              launcher,
              ["serve"],
              env: Agent::Environment.host,
              clear_env: true
            )
          rescue error : File::Error | IO::Error
            STDERR.puts "xd: cannot restart after update: #{error.message}"
          end
        end
      end

      private def begin_work(state : State) : Bool
        started = @lock.synchronize do
          next false if @state.checking? || @state.installing?

          @state = state
          @trouble = nil
          true
        end
        publish("daemon-update") if started
        started
      end

      private def finish(
        state : State,
        latest : String? = nil,
        trouble : String? = nil,
      ) : Nil
        @lock.synchronize do
          @state = state
          @latest = latest if latest
          @trouble = trouble
        end
        publish("daemon-update")
      end

      private def publish(name : String) : Nil
        @publish.call(name, snapshot)
      end

      private def curl : String
        BundleEnvironment.executable("curl") || "curl"
      end

      private def run_isolated(name : String, &block : -> Nil) : Nil
        Fiber::ExecutionContext::Isolated.new(name) do
          begin
            block.call
          rescue error : File::Error | IO::Error
            finish(State::Failed, trouble: error.message)
          end
        end
      rescue error : RuntimeError
        finish(State::Failed, trouble: error.message)
      end
    end
  end
end
