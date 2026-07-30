module Xd
  module Daemon
    # Watches repository HEAD state for recently active chats.
    #
    # Monitoring belongs beside the repository on the daemon: Unix and TLS
    # clients receive the same event, and neither UI probes a local checkout.
    class RepositoryMonitor
      MAX_CHATS = 8
      INTERVAL  = 1.second

      private record Entry, signature : String?, touched : Int64

      @entries = {} of String => Entry
      @lock = Mutex.new
      @closed = false
      @clock = 0_i64

      def initialize(
        @signature : Proc(String, String),
        @changed : Proc(String, Nil),
        @interval : Time::Span = INTERVAL,
      )
        spawn run
      end

      def watch(chat_id : String) : Nil
        @lock.synchronize do
          return if @closed

          @clock += 1
          signature = @entries[chat_id]?.try(&.signature)
          @entries[chat_id] = Entry.new(signature, @clock)
          if @entries.size > MAX_CHATS
            oldest = @entries.min_by { |_id, entry| entry.touched }[0]
            @entries.delete(oldest)
          end
        end
      end

      def reset(chat_id : String) : Nil
        @lock.synchronize do
          return if @closed
          return unless entry = @entries[chat_id]?

          @entries[chat_id] = Entry.new(nil, entry.touched)
        end
      end

      def close : Nil
        @lock.synchronize do
          @closed = true
          @entries.clear
        end
      end

      private def run : Nil
        loop do
          sleep @interval
          break if @lock.synchronize { @closed }

          chats = @lock.synchronize { @entries.keys }

          chats.each do |chat_id|
            signature = begin
              @signature.call(chat_id)
            rescue
              ""
            end
            changed = @lock.synchronize do
              entry = @entries[chat_id]?
              next false unless entry

              previous = entry.signature
              @entries[chat_id] = Entry.new(signature, entry.touched)
              !previous.nil? && previous != signature
            end
            @changed.call(chat_id) if changed
          end
        end
      end
    end
  end
end
