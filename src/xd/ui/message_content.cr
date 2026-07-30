require "../markdown"

module Xd
  module UI
    enum MessagePartKind
      Prose
      Code
      Diff
      Table
    end

    record MessagePart,
      kind : MessagePartKind,
      text : String

    module MessageContent
      extend self

      def parse(text : String) : Array(MessagePart)
        return [] of MessagePart if text.empty?
        unless text.includes?("```") ||
               (text.includes?('|') && text.includes?('\n'))
          return [MessagePart.new(MessagePartKind::Prose, text)]
        end

        parts = [] of MessagePart
        chunk = [] of String
        in_fence = false
        diff_fence = false

        text.split('\n').each do |line|
          if line.starts_with?("```")
            append_chunk(parts, chunk.join('\n'), in_fence, diff_fence)
            chunk.clear
            in_fence = !in_fence
            diff_fence = in_fence && line.lchop("```") == "diff"
            next
          end

          chunk << line
        end
        append_chunk(parts, chunk.join('\n'), in_fence, diff_fence)
        parts
      end

      private def append_chunk(
        parts : Array(MessagePart),
        chunk : String,
        in_fence : Bool,
        diff_fence : Bool,
      ) : Nil
        return if chunk.empty?

        if in_fence
          kind = diff_fence ? MessagePartKind::Diff : MessagePartKind::Code
          parts << MessagePart.new(kind, chunk)
        else
          append_markdown_chunk(parts, chunk)
        end
      end

      private def append_markdown_chunk(
        parts : Array(MessagePart),
        text : String,
      ) : Nil
        lines = text.split('\n')
        prose = [] of String
        line = 0

        while line < lines.size
          if blank?(lines[line])
            prose << lines[line]
            line += 1
            next
          end

          finish = line
          paragraph = [] of String
          while finish < lines.size && !blank?(lines[finish])
            paragraph << lines[finish]
            finish += 1
          end

          paragraph_text = paragraph.join('\n')
          grid = if paragraph_text.includes?('|') &&
                    paragraph_text.includes?('\n')
                   Markdown.table_to_text(paragraph_text)
                 end
          unless grid
            while line < finish
              prose << lines[line]
              line += 1
            end
            next
          end

          append_prose(parts, prose.join('\n'))
          prose.clear
          parts << MessagePart.new(MessagePartKind::Table, grid)
          line = finish
          while line < lines.size && blank?(lines[line])
            line += 1
          end
        end

        append_prose(parts, prose.join('\n'))
      end

      private def append_prose(
        parts : Array(MessagePart),
        prose : String,
      ) : Nil
        prose = prose.rstrip('\n')
        return if prose.empty?

        parts << MessagePart.new(MessagePartKind::Prose, prose)
      end

      private def append_line(text : String, line : String) : String
        text.empty? ? line : "#{text}\n#{line}"
      end

      private def blank?(line : String) : Bool
        line.each_byte.all? do |byte|
          byte == 0x09 || byte == 0x0a || byte == 0x0b ||
            byte == 0x0c || byte == 0x0d || byte == 0x20
        end
      end
    end
  end
end
