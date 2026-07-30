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

    record PreparedMessagePart,
      kind : MessagePartKind,
      text : String,
      markup : String?

    module MessageContent
      extend self

      RENDER_CHUNK_BYTES = 64 * 1024

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

      # Pure response preparation. Call off GTK: Markdown parsing and large
      # block splitting otherwise run in one main-loop callback when a turn
      # finishes.
      def prepare(
        text : String,
        chunk_bytes : Int32 = RENDER_CHUNK_BYTES,
      ) : Array(PreparedMessagePart)
        prepared = [] of PreparedMessagePart
        parse(text).each do |part|
          each_chunk(part.text, chunk_bytes) do |chunk|
            markup = if part.kind.prose?
                       Markdown.to_pango(chunk)
                     end
            prepared << PreparedMessagePart.new(
              part.kind,
              chunk,
              markup
            )
          end
        end
        prepared
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

      private def each_chunk(
        text : String,
        limit : Int32,
        &block : String ->
      ) : Nil
        return if text.empty?
        raise ArgumentError.new("chunk limit must be positive") if limit <= 0

        offset = 0
        bytes = text.to_slice
        while offset < bytes.size
          take = Math.min(limit, bytes.size - offset)
          if offset + take < bytes.size
            while take > 0 &&
                  (bytes[offset + take] & 0xc0) == 0x80
              take -= 1
            end

            minimum = take // 2
            newline = take - 1
            while newline >= minimum && bytes[offset + newline] != '\n'.ord
              newline -= 1
            end
            take = newline + 1 if newline >= minimum
          end

          # A valid UTF-8 string cannot have a continuation-free boundary of
          # zero bytes, but retain progress for arbitrary tool text.
          take = Math.min(limit, bytes.size - offset) if take == 0
          chunk = text.byte_slice(offset, take)
          until chunk.valid_encoding?
            chunk = chunk.byte_slice(0, chunk.bytesize - 1)
          end
          break if chunk.empty?

          yield chunk
          offset += chunk.bytesize
        end
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
