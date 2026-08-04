require "json"

module Xd
  module UI
    module QueuePresentation
      extend self

      MAX_ROWS           = 50
      PREVIEW_CHAR_LIMIT = 240

      record Plan,
        rows : Array(String),
        hidden : Int32

      def prepare(queue : Array(JSON::Any)) : Plan
        visible = Math.min(queue.size, MAX_ROWS)
        rows = Array(String).new(visible)
        visible.times { |index| rows << queue[index].as_s }
        Plan.new(rows, Math.max(queue.size - visible, 0).to_i32)
      end

      def event_queue(event : Hash(String, JSON::Any)) : Array(JSON::Any)?
        event["queue"]?.try(&.as_a?)
      end

      def reload_after_send?(response : Hash(String, JSON::Any)) : Bool
        response["queued"]?.try(&.as_bool?) != true
      end

      def preview(text : String) : String
        compact = text.gsub(/\s+/, " ").strip
        return compact if compact.size <= PREVIEW_CHAR_LIMIT

        "#{compact[0, PREVIEW_CHAR_LIMIT - 1]}…"
      end
    end
  end
end
