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

    # Round-robin GTK work with an explicit per-frame mutation budget. A task
    # returns true while it has another small step to perform.
    class FrameRenderQueue
      @items = Deque(Proc(Bool)).new

      getter size : Int32

      def initialize
        @size = 0
      end

      def push(&render : -> Bool) : Nil
        @items << render
        @size += 1
      end

      def drain(limit : Int) : Bool
        raise ArgumentError.new("limit must be positive") unless limit > 0

        rendered = 0
        while rendered < limit
          render = @items.shift?
          break unless render

          @size -= 1
          if render.call
            @items << render
            @size += 1
          end
          rendered += 1
        end
        !@items.empty?
      end
    end
  end
end
