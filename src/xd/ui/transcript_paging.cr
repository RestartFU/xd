module Xd
  module UI
    class TranscriptBatch(T)
      DEFAULT_SIZE = 4

      getter position : Int32

      def initialize(
        @items : Array(T),
        start : Int = 0,
        @batch_size : Int32 = DEFAULT_SIZE,
      )
        raise ArgumentError.new("batch size must be positive") unless @batch_size > 0

        @position = Math.min(
          Math.max(start, 0),
          @items.size
        ).to_i32
      end

      def next_batch : Array({Int32, T})
        batch = [] of {Int32, T}
        finish = Math.min(
          @position.to_i64 + @batch_size,
          @items.size.to_i64
        ).to_i32
        while @position < finish
          batch << {@position, @items[@position]}
          @position += 1
        end
        batch
      end

      def done? : Bool
        @position >= @items.size
      end
    end

    class TranscriptPaging
      PAGE_SIZE = 100

      getter limit : Int32

      def initialize(@limit = PAGE_SIZE)
        raise ArgumentError.new("limit must be positive") unless @limit > 0
      end

      def query_limit : Int32
        @limit < Int32::MAX ? @limit + 1 : @limit
      end

      def start(fetched : Int) : Int32
        fetched > @limit ? fetched.to_i32 - @limit : 0
      end

      def displayed(fetched : Int) : Int32
        fetched.to_i32 - start(fetched)
      end

      def hidden(total : Int64, fetched : Int) : Int64
        Math.max(total - displayed(fetched), 0_i64)
      end

      def earlier_count(total : Int64, fetched : Int) : Int32
        Math.min(hidden(total, fetched), PAGE_SIZE).to_i32
      end

      def earlier_label(total : Int64, fetched : Int) : String?
        count = earlier_count(total, fetched)
        return if count == 0

        suffix = count == 1 ? "" : "s"
        "Load #{count} earlier message#{suffix}"
      end

      # A raw-message page can land in the middle of an assistant turn when it
      # contains many tool calls. Grow it before rendering so the prompt that
      # those calls answer is not hidden behind the history button.
      def extend_to_turn_start(
        total : Int64,
        fetched : Int,
        first_role : String?,
      ) : Bool
        return false if start(fetched) == 0
        return false if hidden(total, fetched) == 0
        return false if first_role == "user"

        before = @limit
        load_earlier
        @limit > before
      end

      def load_earlier : Int32
        @limit = Math.min(
          @limit.to_i64 + PAGE_SIZE,
          Int32::MAX.to_i64
        ).to_i32
      end
    end

    class TranscriptLru
      CACHE_SIZE = 4

      getter keys = [] of String

      def touch(key : String) : String?
        @keys.delete(key)
        @keys << key
        @keys.size > CACHE_SIZE ? @keys.shift : nil
      end

      def delete(key : String) : Nil
        @keys.delete(key)
      end
    end
  end
end
