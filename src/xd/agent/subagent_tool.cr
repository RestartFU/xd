module Xd
  module Agent
    module SubagentTool
      extend self

      PREFIX = "subagent\n"

      # Which agent a record is about, when the backend names one.
      #
      # Codex reports a spawned agent again every time its state changes, so
      # without a key the client cannot tell "the same agent, further along"
      # from "another agent": every update became another card, and a long
      # fan-out ended as a run of near-identical cards the transcript had to
      # lay out again on each one. Keyed, a repeat is a label change inside a
      # card that is already there. Empty when the backend names no agent --
      # Claude's Task calls are one card each, whatever they were asked.
      record Delegation, key : String, identity : String, task : String

      def build(
        identity : String?,
        task : String?,
        key : String? = nil,
      ) : String
        head = one_line(key, 120, "")
        # No key, no key line: a backend that names no agent writes exactly
        # what it wrote before, and every reader of these records stays put.
        head = "#{head}\n" unless head.empty?
        "#{PREFIX}#{head}" \
        "#{one_line(identity, 80, "General")}\n" \
        "#{one_line(task, 320, "Delegated task")}"
      end

      def parse(message : String?) : Delegation?
        return nil unless message
        return nil unless message.starts_with?(PREFIX)

        parts = message[PREFIX.size..].split('\n')
        case parts.size
        when 2
          # An agent the backend did not name, and every record written before
          # cards carried a key.
          return nil if parts.any?(&.empty?)
          Delegation.new("", parts[0], parts[1])
        when 3
          return nil if parts.any?(&.empty?)
          Delegation.new(parts[0], parts[1], parts[2])
        else
          nil
        end
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
