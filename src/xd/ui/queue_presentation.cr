require "json"

module Xd
  module UI
    module QueuePresentation
      extend self

      MAX_ROWS = 50

      record Plan,
        rows : Array(String),
        hidden : Int32

      def prepare(queue : Array(JSON::Any)) : Plan
        visible = Math.min(queue.size, MAX_ROWS)
        rows = Array(String).new(visible)
        visible.times { |index| rows << queue[index].as_s }
        Plan.new(rows, Math.max(queue.size - visible, 0).to_i32)
      end
    end
  end
end
