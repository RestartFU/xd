require "../protocol/message"

module Xd
  module Daemon
    class EventBus
      @subscribers = {} of Int64 => Proc(Protocol::Event, Nil)
      @next_id = 0_i64
      @next_event_id = 0_i64
      @lock = Mutex.new
      @publish_lock = Mutex.new

      def subscribe(&subscriber : Protocol::Event ->) : Int64
        @lock.synchronize do
          @next_id += 1
          @subscribers[@next_id] = subscriber
          @next_id
        end
      end

      def unsubscribe(id : Int64) : Nil
        @lock.synchronize { @subscribers.delete(id) }
      end

      def publish(event : Protocol::Event) : Nil
        @publish_lock.synchronize do
          @next_event_id += 1
          event.assign_id(@next_event_id)
          subscribers = @lock.synchronize { @subscribers.values.dup }
          subscribers.each do |subscriber|
            begin
              subscriber.call(event)
            rescue error
              STDERR.puts(
                "xd: daemon event subscriber failed: #{error.message}"
              )
            end
          end
        end
      end
    end
  end
end
