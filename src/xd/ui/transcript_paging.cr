module Xd
  module UI
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
