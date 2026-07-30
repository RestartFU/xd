module Xd
  module Daemon
    # Publishes workspace tree changes made outside xd.
    #
    # Polling one compact signature keeps behavior identical on Linux, macOS,
    # Windows, local IPC, and paired TLS without platform watcher edge cases.
    class WorkspaceMonitor
      INTERVAL = 1.second

      @lock = Mutex.new
      @closed = false
      @initialized = false
      @previous : String?

      def initialize(
        @signature : Proc(String),
        @changed : Proc(Nil),
        @interval : Time::Span = INTERVAL,
      )
        @previous = nil
        spawn run
      end

      # An xd command already publishes its own tree event. Adopt that state so
      # polling does not publish a second event for the same mutation.
      def acknowledge : Nil
        current = read_signature
        return unless current

        @lock.synchronize do
          return if @closed

          @previous = current
          @initialized = true
        end
      end

      def close : Nil
        @lock.synchronize do
          @closed = true
          @previous = nil
        end
      end

      private def run : Nil
        loop do
          sleep @interval
          break if @lock.synchronize { @closed }

          current = read_signature
          next unless current

          notify = @lock.synchronize do
            next false if @closed

            previous = @previous
            initialized = @initialized
            @previous = current
            @initialized = true
            initialized && previous != current
          end
          next unless notify

          @changed.call
        rescue error
          STDERR.puts(
            "xd: workspace monitor callback failed: #{error.message}"
          )
        end
      end

      private def read_signature : String?
        @signature.call
      rescue
        nil
      end
    end
  end
end
