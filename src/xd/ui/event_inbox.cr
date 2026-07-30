require "json"

module Xd
  module UI
    # One bounded bridge from daemon fibers into GTK.
    #
    # A fast CLI can emit thousands of tiny text deltas. Scheduling one GLib
    # idle source per delta starves input and redraws even though socket reads
    # themselves yield. Model downloads can likewise emit progress faster than
    # GTK paints it. Keep one source active, merge replaceable events, and let
    # GTK regain control between bounded batches.
    class EventInbox(T)
      BATCH_SIZE         = 32
      TEXT_COALESCE_LIMIT = 16 * 1024

      @items = Deque({T, Hash(String, JSON::Any)}).new
      @scheduled = false
      @lock = Mutex.new

      # True means the caller must create the single drain source.
      def push(target : T, event : Hash(String, JSON::Any)) : Bool
        @lock.synchronize do
          @items << {target, event} unless coalesce(target, event)
          unless @scheduled
            @scheduled = true
            return true
          end
          false
        end
      end

      def drain(
        limit : Int = BATCH_SIZE,
      ) : {Array({T, Hash(String, JSON::Any)}), Bool}
        raise ArgumentError.new("limit must be positive") unless limit > 0

        @lock.synchronize do
          batch = [] of {T, Hash(String, JSON::Any)}
          Math.min(limit, @items.size).times do
            batch << @items.shift
          end
          more = !@items.empty?
          @scheduled = false unless more
          {batch, more}
        end
      end

      def clear : Nil
        @lock.synchronize do
          @items.clear
          @scheduled = false
        end
      end

      private def coalesce(
        target : T,
        event : Hash(String, JSON::Any),
      ) : Bool
        return true if coalesce_voice_progress(target, event)
        return false unless event["event"]?.try(&.as_s?) == "text"
        previous = @items.last?
        return false unless previous && previous[0] == target

        before = previous[1]
        return false unless before["event"]?.try(&.as_s?) == "text"
        return false unless before["chat"]?.try(&.as_s?) ==
                              event["chat"]?.try(&.as_s?)

        old_text = before["text"]?.try(&.as_s?) || ""
        new_text = event["text"]?.try(&.as_s?) || ""
        return false if old_text.bytesize + new_text.bytesize >
                        TEXT_COALESCE_LIMIT

        merged = before.dup
        merged["text"] = JSON::Any.new(old_text + new_text)
        @items.pop
        @items << {target, merged}
        true
      end

      private def coalesce_voice_progress(
        target : T,
        event : Hash(String, JSON::Any),
      ) : Bool
        return false unless event["event"]?.try(&.as_s?) == "voice"
        return false unless event["state"]?.try(&.as_s?) == "downloading"
        previous = @items.last?
        return false unless previous && previous[0] == target

        before = previous[1]
        return false unless before["event"]?.try(&.as_s?) == "voice"
        return false unless before["state"]?.try(&.as_s?) == "downloading"
        return false unless before["request"]?.try(&.as_s?) ==
                              event["request"]?.try(&.as_s?)

        @items.pop
        @items << {target, event}
        true
      end
    end
  end
end
