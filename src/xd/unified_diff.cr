require "html"
require "./syntax"

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

  record DiffMarkup,
    markup : String,
    row_kinds : Array(DiffLineKind)

  record DiffFileSection,
    path : String,
    start : Int32,
    finish : Int32,
    additions : UInt32,
    deletions : UInt32

  module UnifiedDiff
    extend self

    HUNK_START         = /\A@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/
    DISPLAY_LINE_BYTES = 1024

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

    # The same patch with each file named rather than pathed.
    #
    # A patch built from a tool payload carries whatever path the tool used,
    # which is usually absolute: inline in a transcript that is a line of home
    # directory and worktree before the part worth reading, and the part worth
    # reading is what falls off the end. The repository pane keeps whole paths,
    # where telling apart two files of the same name is the point.
    def name_only(lines : Array(DiffLine)) : Array(DiffLine)
      return lines unless lines.any?(&.kind.file?)

      lines.map do |line|
        next line unless line.kind.file?

        name = file_name(line.text)
        name == line.text ? line : DiffLine.new(DiffLineKind::File, name)
      end
    end

    def display_rows(
      lines : Array(DiffLine),
      show_file_headers : Bool,
    ) : Int32
      return lines.size if show_file_headers
      lines.count { |line| !line.kind.file? }
    end

    def file_sections(
      lines : Array(DiffLine),
    ) : Array(DiffFileSection)
      return [] of DiffFileSection if lines.empty?

      starts = [] of Int32
      lines.each_with_index do |line, index|
        starts << index if line.kind.file?
      end
      starts << 0 if starts.empty?

      starts.map_with_index do |start, index|
        finish = starts[index + 1]? || lines.size
        first = lines[start]
        additions = 0_u32
        deletions = 0_u32
        (start...finish).each do |position|
          kind = lines[position].kind
          additions &+= 1 if kind.added?
          deletions &+= 1 if kind.removed?
        end
        DiffFileSection.new(
          first.kind.file? ? first.text : "Changes",
          start,
          finish,
          additions,
          deletions
        )
      end
    end

    def markup(
      lines : Array(DiffLine),
      show_file_headers : Bool,
      limit : Int32 = 0,
    ) : DiffMarkup
      builder = MarkupBuilder.new
      total = display_rows(lines, show_file_headers)

      lines.each_with_index do |line, position|
        break if limit > 0 && builder.rendered >= limit
        render_line(builder, lines, position, line, show_file_headers)
      end

      if builder.rendered < total
        rendered = builder.rendered
        builder.start_row(DiffLineKind::Meta)
        builder.append(
          %(<span foreground="#9a9996">Showing first #{rendered} of #{total} rows</span>)
        )
      end
      builder.finish
    end

    def markup_slice(
      lines : Array(DiffLine),
      show_file_headers : Bool,
      start : Int32,
      finish : Int32,
    ) : DiffMarkup
      finish = Math.min(Math.max(finish, 0), lines.size)
      start = Math.min(Math.max(start, 0), finish)
      builder = MarkupBuilder.new
      prime_slice(builder, lines, start)

      (start...finish).each do |position|
        render_line(
          builder,
          lines,
          position,
          lines[position],
          show_file_headers
        )
      end
      builder.finish
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

    # Windows tools report paths with their own separator.
    private def file_name(title : String) : String
      at = title.rindex('/') || title.rindex('\\')
      return title unless at

      name = title[(at + 1)..]
      name.empty? ? title : name
    end

    private def render_line(
      builder : MarkupBuilder,
      lines : Array(DiffLine),
      position : Int32,
      line : DiffLine,
      show_file_headers : Bool,
    ) : Nil
      case line.kind
      when .file?
        builder.begin_file(line.text)
        append_file_markup(builder, lines, position, line) if show_file_headers
      when .hunk?
        builder.begin_hunk
        append_full_line_markup(builder, line)
      when .meta?
        append_full_line_markup(builder, line)
      else
        builder.append_code_line(line)
      end
    end

    private def append_file_markup(
      builder : MarkupBuilder,
      lines : Array(DiffLine),
      position : Int32,
      line : DiffLine,
    ) : Nil
      additions = 0
      deletions = 0
      ((position + 1)...lines.size).each do |scan|
        within = lines[scan]
        break if within.kind.file?
        additions += 1 if within.kind.added?
        deletions += 1 if within.kind.removed?
      end

      builder.start_row(line.kind)
      builder.append(
        %(<span foreground="#ffbe6f" weight="bold">#{escaped_display_text(line.text)}</span>  <span foreground="#57e389">+#{additions}</span>  <span foreground="#f66151">−#{deletions}</span>)
      )
    end

    private def append_full_line_markup(
      builder : MarkupBuilder,
      line : DiffLine,
    ) : Nil
      colour = line.kind.hunk? ? "#78aeed" : "#9a9996"
      builder.start_row(line.kind)
      builder.append(
        %(<span foreground="#{colour}">#{escaped_display_text(line.text)}</span>)
      )
    end

    private def prime_slice(
      builder : MarkupBuilder,
      lines : Array(DiffLine),
      start : Int32,
    ) : Nil
      (start - 1).downto(0) do |position|
        line = lines[position]
        if line.kind.file?
          builder.begin_file(line.text)
          break
        end
      end
      return if builder.language.none?

      resume = 0
      (start - 1).downto(0) do |position|
        line = lines[position]
        if line.kind.hunk? || line.kind.file?
          resume = position + 1
          break
        end
      end

      (resume...start).each do |position|
        line = lines[position]
        if line.kind.context? || line.kind.added? || line.kind.removed?
          builder.advance_code_line(line)
        end
      end
    end

    def display_text(text : String) : String
      valid = text.scrub
      return valid if valid.bytesize <= DISPLAY_LINE_BYTES

      cut = DISPLAY_LINE_BYTES
      while cut > 0 && valid.byte_at(cut) & 0xc0 == 0x80
        cut -= 1
      end
      valid.byte_slice(0, cut) + "…"
    end

    private def escaped_display_text(text : String) : String
      HTML.escape(display_text(text))
    end

    class MarkupBuilder
      getter rendered : Int32
      getter language : SyntaxLanguage

      @markup : String::Builder
      @row_kinds : Array(DiffLineKind)
      @old_state : SyntaxState
      @new_state : SyntaxState

      def initialize
        @markup = String::Builder.new
        @row_kinds = [] of DiffLineKind
        @rendered = 0
        @language = SyntaxLanguage::None
        @old_state = SyntaxState.new
        @new_state = SyntaxState.new
      end

      def start_row(kind : DiffLineKind) : Nil
        @markup << '\n' if @rendered > 0
        @row_kinds << kind
        @rendered += 1
      end

      def append(text : String) : Nil
        @markup << text
      end

      def begin_file(path : String) : Nil
        @language = Syntax.language_for_path(path)
        reset_states
      end

      def begin_hunk : Nil
        reset_states
      end

      def append_code_line(line : DiffLine) : Nil
        shown = UnifiedDiff.display_text(line.text)
        old_line = line.old_line > 0 ? "%4d" % line.old_line.to_i64 : "    "
        new_line = line.new_line > 0 ? "%4d" % line.new_line.to_i64 : "    "
        marker = " "
        colour = "#9a9996"
        changed = !line.kind.background.nil?
        if line.kind.added?
          marker = "+"
          colour = "#57e389"
        elsif line.kind.removed?
          marker = "−"
          colour = "#f66151"
        end

        start_row(line.kind)
        @markup << %(<span foreground=")
        @markup << (changed ? "#c0bfbc" : "#77767b")
        @markup << %(">#{old_line} #{new_line}</span> <span foreground="#{colour}" weight="bold">#{marker}</span> )

        if @language.none?
          escaped = HTML.escape(shown)
          if changed
            @markup << %(<span foreground="#{colour}">#{escaped}</span>)
          else
            @markup << escaped
          end
          return
        end

        pieces = scan_code_line(line, shown)
        pieces.each do |piece|
          escaped = HTML.escape(piece.text)
          if token_colour = piece.token.colour
            @markup << %(<span foreground="#{token_colour}">#{escaped}</span>)
          else
            @markup << escaped
          end
        end
      end

      def advance_code_line(line : DiffLine) : Nil
        scan_code_line(line, UnifiedDiff.display_text(line.text))
      end

      def finish : DiffMarkup
        DiffMarkup.new(@markup.to_s, @row_kinds)
      end

      private def scan_code_line(
        line : DiffLine,
        shown : String,
      ) : Array(SyntaxPiece)
        if line.kind.context?
          Syntax.scan_line(@language, shown, @old_state)
        end
        state = line.kind.removed? ? @old_state : @new_state
        Syntax.scan_line(@language, shown, state)
      end

      private def reset_states : Nil
        @old_state = SyntaxState.new
        @new_state = SyntaxState.new
      end
    end
  end
end
