module Xd
  module UI
    module ContextUsage
      extend self

      enum Severity
        Normal
        Warning
        Error
      end

      record Meter,
        fraction : Float64,
        label : String,
        tooltip : String,
        severity : Severity

      def format_tokens(tokens : UInt64) : String
        if tokens >= 1_000_000
          return "#{tokens // 1_000_000}M" if tokens % 1_000_000 == 0

          "%.1fM" % (tokens / 1_000_000.0)
        elsif tokens >= 1_000
          return "#{tokens // 1_000}k" if tokens % 1_000 == 0

          "%.1fk" % (tokens / 1_000.0)
        else
          tokens.to_s
        end
      end

      def meter(used : UInt64, window : UInt64) : Meter?
        return if used == 0 || window == 0

        fraction = Math.min(used.to_f64 / window, 1.0)
        severity = if fraction >= 0.9
                     Severity::Error
                   elsif fraction >= 0.75
                     Severity::Warning
                   else
                     Severity::Normal
                   end
        label = "#{format_tokens(used)} / #{format_tokens(window)}"
        tooltip = "Context window: #{used} of #{window} tokens " \
                  "(#{(fraction * 100).round.to_i}%)"
        Meter.new(fraction, label, tooltip, severity)
      end
    end
  end
end
