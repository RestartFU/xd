module Xd
  module Agent
    module SubagentTool
      extend self

      PREFIX = "subagent\n"

      def build(identity : String?, task : String?) : String
        "#{PREFIX}#{one_line(identity, 80, "General")}\n" \
        "#{one_line(task, 320, "Delegated task")}"
      end

      def parse(message : String?) : Tuple(String, String)?
        return nil unless message
        return nil unless message.starts_with?(PREFIX)

        parts = message[PREFIX.size..].split('\n')
        return nil unless parts.size == 2
        return nil if parts.any?(&.empty?)
        {parts[0], parts[1]}
      end

      private def one_line(
        text : String?,
        limit : Int32,
        fallback : String,
      ) : String
        return fallback unless text

        normalized = text.split.join(" ")
        return fallback if normalized.empty?
        return normalized if normalized.size <= limit

        normalized[0, limit] + "…"
      end
    end
  end
end
