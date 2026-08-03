module Xd
  module UI
    class TextReveal
      # Updating a wrapping label relays out all streamed text. Ten frames per
      # second stays visibly live without turning long responses into a CPU
      # benchmark on lower-power laptops.
      FRAME_MILLISECONDS =         100
      INITIAL_DELAY_USEC =  80_000_i64
      TAIL_DELAY_USEC    = 100_000_i64
      TRAILING_CHARS     =           2

      # Caught up means the current buffer has gone quiet. It is not a final
      # boundary: another agent delta can still extend the same segment.
      record Frame, shown : Int32, caught_up : Bool

      getter shown = 0

      @reveal_after = 0_i64
      @last_append = 0_i64

      def reset : Nil
        @shown = 0
        @reveal_after = 0_i64
        @last_append = 0_i64
      end

      def note_append(now : Int64) : Nil
        if @reveal_after == 0
          @reveal_after = now + INITIAL_DELAY_USEC
        end
        @last_append = now
      end

      def advance(text : String, now : Int64) : Frame
        total = text.size
        @shown = Math.min(@shown, total)
        quiet = @last_append == 0 ||
                now - @last_append >= TAIL_DELAY_USEC

        if @reveal_after != 0 && now < @reveal_after
          return Frame.new(@shown, false)
        end

        target = if quiet
                   total
                 elsif total > TRAILING_CHARS
                   total - TRAILING_CHARS
                 else
                   0
                 end
        pending = target > @shown ? target - @shown : 0

        if pending > 0
          step = (pending + 2) // 3
          step = pending if quiet && pending <= 4
          @shown += step
        end

        Frame.new(@shown, quiet && @shown == total)
      end

      def sync(text : String) : Nil
        @shown = text.size
      end

      def self.prefix(text : String, characters : Int) : String
        text[0, Math.min(characters, text.size)]
      end
    end
  end
end
