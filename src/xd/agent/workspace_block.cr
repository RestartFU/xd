module Xd
  module Agent
    # Control block emitted after an agent moves into another Git worktree.
    module WorkspaceBlock
      extend self

      OPEN  = "<workspace>"
      CLOSE = "</workspace>"

      record Parsed, path : String, remainder : String

      # Removes every valid standalone block and returns the last reported path.
      # Inline mentions and multiline bodies remain ordinary prose.
      def parse(text : String) : Parsed?
        path : String? = nil
        prose = [] of String

        text.split('\n', remove_empty: false).each do |line|
          content = line.ends_with?('\r') ? line[0...-1] : line
          if content.starts_with?(OPEN) && content.ends_with?(CLOSE)
            body_size = content.bytesize - OPEN.bytesize - CLOSE.bytesize
            candidate = content.byte_slice(OPEN.bytesize, body_size).strip
            unless candidate.empty?
              path = candidate
              next
            end
          end
          prose << line
        end

        reported = path
        return unless reported
        Parsed.new(reported, prose.join("\n").strip)
      end

      # Hides a complete or partial opening marker only when it starts a line.
      def visible_bytes(text : String) : Int32
        offset = 0
        while at = text.byte_index('<', offset)
          if at == 0 || text.byte_at(at - 1) == '\n'.ord
            remaining = text.bytesize - at
            compared = Math.min(remaining, OPEN.bytesize)
            if text.byte_slice(at, compared) == OPEN.byte_slice(0, compared)
              return at
            end
          end
          offset = at + 1
        end
        text.bytesize
      end
    end
  end
end
