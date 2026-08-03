module Xd
  module UI
    # FIFO retries for saturated background rendering. The caller drives this
    # queue from one shared timer, avoiding one hot timer per transcript row.
    class RenderRetryQueue
      @items = Deque(Proc(Bool)).new

      getter size : Int32

      def initialize
        @size = 0
      end

      def push(&retry : -> Bool) : Nil
        @items << retry
        @size += 1
      end

      # Successful/stale work leaves the queue. A saturated front item stays
      # in place so later rows cannot overtake it or create more retry work.
      def drain : Bool
        while retry = @items.first?
          break unless retry.call

          @items.shift
          @size -= 1
        end
        !@items.empty?
      end
    end
  end
end
