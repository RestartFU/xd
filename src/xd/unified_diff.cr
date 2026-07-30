module Xd
  enum DiffLineKind
    File
    Context
    Added
    Removed
    Hunk
    Meta

    def background : String?
      case self
      when File    then "#2a2a2d"
      when Added   then "#183522"
      when Removed then "#3a1d1b"
      else              nil
      end
    end
  end

  record DiffLine,
    kind : DiffLineKind,
    text : String,
    old_line : UInt32 = 0_u32,
    new_line : UInt32 = 0_u32

  record ParsedDiff,
    lines : Array(DiffLine),
    additions : UInt32,
    deletions : UInt32

  module UnifiedDiff
    extend self

    HUNK_START = /\A@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/

    def parse(patch : String?) : ParsedDiff
      lines = [] of DiffLine
      old_line = 0_u32
      new_line = 0_u32
      additions = 0_u32
      deletions = 0_u32
      in_hunk = false

      (patch || "").split('\n').each do |raw|
        if raw.starts_with?("diff --git ")
          lines << DiffLine.new(DiffLineKind::File, file_title(raw))
          in_hunk = false
          next
        end

        if start = hunk_start(raw)
          old_line, new_line = start
          lines << DiffLine.new(
            DiffLineKind::Hunk,
            raw,
            old_line,
            new_line
          )
          in_hunk = true
          next
        end

        unless in_hunk
          next if raw.empty? || plumbing_header?(raw)
          lines << DiffLine.new(DiffLineKind::Meta, raw)
          next
        end

        case raw.byte_at?(0)
        when '+'.ord
          lines << DiffLine.new(
            DiffLineKind::Added,
            raw.byte_slice(1),
            new_line: new_line
          )
          new_line &+= 1
          additions &+= 1
        when '-'.ord
          lines << DiffLine.new(
            DiffLineKind::Removed,
            raw.byte_slice(1),
            old_line: old_line
          )
          old_line &+= 1
          deletions &+= 1
        when ' '.ord
          lines << DiffLine.new(
            DiffLineKind::Context,
            raw.byte_slice(1),
            old_line,
            new_line
          )
          old_line &+= 1
          new_line &+= 1
        when nil
          nil
        else
          lines << DiffLine.new(DiffLineKind::Meta, raw)
        end
      end

      ParsedDiff.new(lines, additions, deletions)
    end

    def display_rows(
      lines : Array(DiffLine),
      show_file_headers : Bool,
    ) : Int32
      return lines.size if show_file_headers
      lines.count { |line| !line.kind.file? }
    end

    private def hunk_start(line : String) : Tuple(UInt32, UInt32)?
      match = HUNK_START.match(line)
      return nil unless match

      {
        clamped_line(match[1]),
        clamped_line(match[2]),
      }
    end

    private def clamped_line(value : String) : UInt32
      parsed = value.to_u64? || 0_u64
      Math.min(parsed, UInt32::MAX).to_u32
    end

    private def plumbing_header?(line : String) : Bool
      line.starts_with?("index ") ||
        line.starts_with?("--- ") ||
        line.starts_with?("+++ ")
    end

    private def file_title(line : String) : String
      if at = line.rindex(" b/")
        return line.byte_slice(at + 3)
      end

      if at = line.rindex(" \"b/")
        title = line.byte_slice(at + 4)
        return title.ends_with?('"') ? title.rchop : title
      end

      line.lchop("diff --git ")
    end
  end
end
