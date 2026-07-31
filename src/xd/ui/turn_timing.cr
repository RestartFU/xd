module Xd
  module UI
    module TurnTiming
      extend self

      def format(verb : String, seconds : Int64) : String
        if seconds >= 3600
          hours = seconds // 3600
          minutes = (seconds % 3600) // 60
          "#{verb} for #{hours}h #{minutes.to_s.rjust(2, '0')}m"
        elsif seconds >= 60
          minutes = seconds // 60
          remainder = seconds % 60
          "#{verb} for #{minutes}m #{remainder.to_s.rjust(2, '0')}s"
        else
          "#{verb} for #{seconds}s"
        end
      end
    end
  end
end
