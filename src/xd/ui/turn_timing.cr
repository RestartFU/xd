module Xd
  module UI
    module TurnTiming
      extend self

      def format(verb : String, seconds : Int64) : String
        "#{verb} for #{duration(seconds)}"
      end

      # The same reading without the verb, for places that already say what
      # they are timing: a workflow card names its run beside the count.
      def duration(seconds : Int64) : String
        safe = Math.max(seconds, 0_i64)
        if safe >= 3600
          hours = safe // 3600
          minutes = (safe % 3600) // 60
          "#{hours}h #{minutes.to_s.rjust(2, '0')}m"
        elsif safe >= 60
          "#{safe // 60}m #{(safe % 60).to_s.rjust(2, '0')}s"
        else
          "#{safe}s"
        end
      end
    end
  end
end
