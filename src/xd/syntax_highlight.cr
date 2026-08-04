require "html"
require "./syntax"

module Xd
  record HighlightSpan,
    token : SyntaxToken,
    start : Int32,
    finish : Int32

  module SyntaxHighlight
    extend self

    LINE_LIMIT = 8000

    def prepare(
      path : String,
      text : String,
      line_limit : Int = LINE_LIMIT,
    ) : Array(HighlightSpan)
      raise ArgumentError.new("line_limit must be positive") unless line_limit > 0

      language = Syntax.language_for_path(path)
      return [] of HighlightSpan if language.none?

      spans = [] of HighlightSpan
      state = SyntaxState.new
      offset = 0
      lines = text.split('\n', remove_empty: false)
      lines.first(line_limit).each_with_index do |line, index|
        Syntax.scan_line(language, line, state).each do |piece|
          finish = offset + piece.text.size
          if piece.token.colour
            spans << HighlightSpan.new(piece.token, offset, finish)
          end
          offset = finish
        end
        offset += 1 if index < lines.size - 1
      end
      spans
    end

    def markup(
      language : SyntaxLanguage,
      text : String,
      state : SyntaxState = SyntaxState.new,
    ) : String?
      return if language.none?

      rendered = String::Builder.new
      lines = text.split('\n', remove_empty: false)
      lines.each_with_index do |line, index|
        rendered << '\n' if index > 0
        Syntax.scan_line(language, line, state).each do |piece|
          escaped = HTML.escape(piece.text)
          if colour = piece.token.colour
            rendered << %(<span foreground="#{colour}">#{escaped}</span>)
          else
            rendered << escaped
          end
        end
      end
      rendered.to_s
    end
  end
end
