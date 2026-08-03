module Xd
  module Agent
    enum AssistantSectionKind
      Normal
      Analysis
    end

    record AssistantSection,
      kind : AssistantSectionKind,
      text : String

    # Client-side presentation wrappers emitted by some assistant responses.
    #
    # These are deliberately narrower than HTML parsing. Only the exact tags
    # on their own lines are structural, and fenced code always wins, so an
    # assistant can still show tag examples literally.
    module AssistantSections
      extend self

      OPEN_ANALYSIS  = "<analysis>"
      CLOSE_ANALYSIS = "</analysis>"
      OPEN_SUMMARY   = "<summary>"
      CLOSE_SUMMARY  = "</summary>"

      private enum StreamMode
        Normal
        Analysis
        Summary
      end

      TAGS = [OPEN_ANALYSIS, CLOSE_ANALYSIS, OPEN_SUMMARY, CLOSE_SUMMARY]

      private record Block,
        start_line : Int32,
        finish_line : Int32,
        kind : AssistantSectionKind

      def parse(text : String) : Array(AssistantSection)
        return [] of AssistantSection if text.empty?

        lines = text.split('\n')
        blocks = [] of Block
        active : {Int32, AssistantSectionKind}? = nil
        in_fence = false

        lines.each_with_index do |line, index|
          marker = line.rstrip('\r').strip
          if marker.starts_with?("```")
            in_fence = !in_fence
            next
          end
          next if in_fence

          case marker
          when OPEN_ANALYSIS
            if active
              # Nested or mismatched wrappers are left literal.
              active = nil
            else
              active = {index, AssistantSectionKind::Analysis}
            end
          when OPEN_SUMMARY
            if active
              active = nil
            else
              active = {index, AssistantSectionKind::Normal}
            end
          when CLOSE_ANALYSIS
            if current = active
              if current[1].analysis?
                blocks << Block.new(current[0], index, current[1])
              end
              active = nil
            end
          when CLOSE_SUMMARY
            if current = active
              if current[1].normal?
                blocks << Block.new(current[0], index, current[1])
              end
              active = nil
            end
          end
        end

        return [AssistantSection.new(AssistantSectionKind::Normal, text)] if blocks.empty?

        blocks.sort_by!(&.start_line)
        sections = [] of AssistantSection
        cursor = 0
        blocks.each do |block|
          # A malformed block can overlap a later valid block only when a
          # mismatched marker interrupted it. Keep such source literal.
          next if block.start_line < cursor

          append_normal(sections, lines[cursor...block.start_line].join('\n'))
          body = lines[(block.start_line + 1)...block.finish_line].join('\n')
          sections << AssistantSection.new(block.kind, body)
          cursor = block.finish_line + 1
        end
        append_normal(sections, lines[cursor..].join('\n')) if cursor <= lines.size
        sections
      end

      # Projection for a live plain-text label. Analysis is withheld until the
      # final response is rendered as a disclosure; summary content is shown.
      def stream(text : String) : String
        return text if text.empty?

        lines = text.split('\n')
        output = [] of String
        mode = StreamMode::Normal
        in_fence = false

        lines.each_with_index do |line, index|
          marker = line.rstrip('\r').strip
          if marker.starts_with?("```")
            in_fence = !in_fence
            output << line unless mode.analysis?
            next
          end

          unless in_fence
            if index == lines.size - 1 && partial_tag?(marker)
              next
            end

            case mode
            when StreamMode::Normal
              case marker
              when OPEN_ANALYSIS
                mode = StreamMode::Analysis
                next
              when OPEN_SUMMARY
                mode = StreamMode::Summary
                next
              end
            when StreamMode::Analysis
              mode = StreamMode::Normal if marker == CLOSE_ANALYSIS
              next
            when StreamMode::Summary
              if marker == CLOSE_SUMMARY
                mode = StreamMode::Normal
                next
              end
            end
          end

          output << line unless mode.analysis?
        end

        output.join('\n')
      end

      private def partial_tag?(marker : String) : Bool
        return false if marker.empty?

        TAGS.any? { |tag| tag.starts_with?(marker) }
      end

      private def append_normal(
        sections : Array(AssistantSection),
        text : String,
      ) : Nil
        text = text.strip
        return if text.empty?

        sections << AssistantSection.new(AssistantSectionKind::Normal, text)
      end
    end
  end
end
