module Xd
  module UI
    # Mutable FIFO drained in fixed slices. Items queued by the drain callback
    # remain ordered and may be consumed by the same slice without exceeding
    # its work bound.
    class IdleQueue(T)
      getter size : Int32

      def initialize
        @items = Deque(T).new
        @size = 0
      end

      def <<(item : T) : self
        @items << item
        @size += 1
        self
      end

      def empty? : Bool
        @items.empty?
      end

      def drain(limit : Int, & : T ->) : Int32
        raise ArgumentError.new("limit must be positive") unless limit > 0

        count = 0
        while count < limit
          item = @items.shift?
          break unless item

          @size -= 1
          yield item
          count += 1
        end
        count.to_i32
      end
    end
  end
end
