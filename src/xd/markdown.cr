require "markd"
require "pango"

module Xd
  module Markdown
    extend self

    private OPTIONS = Markd::Options.new

    def to_pango(text : String?) : String
      return "" unless text
      return "" if text.empty?

      document = Markd::Parser.parse(text, OPTIONS)
      markup = render_block(document, 0)
      valid_markup?(markup) ? markup : escape(text)
    rescue
      text ? escape(text) : ""
    end

    def table_to_text(text : String?) : String?
      return unless text
      return if text.empty?

      document = Markd::Parser.parse(text, OPTIONS)
      paragraph = document.first_child?
      return unless paragraph
      return if paragraph.next?
      return unless paragraph.type == Markd::Node::Type::Paragraph

      render_table(paragraph, markup: false)
    rescue
      nil
    end

    def urls_to_pango(text : String?) : String
      autolink_text(text || "", true)
    end

    private def render_block(
      node : Markd::Node,
      depth : Int32,
    ) : String
      case node.type
      when Markd::Node::Type::Document
        render_block_children(node, depth, "\n\n")
      when Markd::Node::Type::Paragraph
        render_table(node, markup: true) ||
          render_inline_children(node, true)
      when Markd::Node::Type::Heading
        level = node.data["level"]?.try(&.as?(Int32)) || 1
        size = level <= 2 ? "large" : "medium"
        %(<span size="#{size}"><b>) +
          render_inline_children(node, true) +
          "</b></span>"
      when Markd::Node::Type::BlockQuote
        contents = render_block_children(node, depth, "\n\n")
        prefix_lines(contents, "│ ", "│ ")
      when Markd::Node::Type::List
        render_list(node, depth)
      when Markd::Node::Type::Item
        render_block_children(node, depth + 1, "\n")
      when Markd::Node::Type::CodeBlock
        literal = chomp_one_newline(node.text)
        %(<tt><span background="#181818">) +
          escape(literal) +
          "</span></tt>"
      when Markd::Node::Type::HTMLBlock
        escape(chomp_one_newline(node.text))
      when Markd::Node::Type::ThematicBreak
        "────────────"
      when Markd::Node::Type::CustomBlock
        render_block_children(node, depth, "\n\n")
      else
        render_inline(node, true)
      end
    end

    private def render_block_children(
      parent : Markd::Node,
      depth : Int32,
      separator : String,
    ) : String
      blocks = [] of String
      child = parent.first_child?
      while child
        block = render_block(child, depth)
        blocks << block unless block.empty?
        child = child.next?
      end
      blocks.join(separator)
    end

    private def render_inline(
      node : Markd::Node,
      autolink : Bool,
    ) : String
      case node.type
      when Markd::Node::Type::Text
        autolink_text(node.text, autolink)
      when Markd::Node::Type::SoftBreak,
           Markd::Node::Type::LineBreak
        "\n"
      when Markd::Node::Type::Code
        "<tt>#{escape(node.text)}</tt>"
      when Markd::Node::Type::HTMLInline
        escape(node.text)
      when Markd::Node::Type::Emphasis
        "<i>#{render_inline_children(node, autolink)}</i>"
      when Markd::Node::Type::Strong
        "<b>#{render_inline_children(node, autolink)}</b>"
      when Markd::Node::Type::Link
        render_link(node)
      when Markd::Node::Type::Image
        render_image(node)
      when Markd::Node::Type::CustomInLine
        render_inline_children(node, autolink)
      else
        render_inline_children(node, autolink)
      end
    end

    private def render_inline_children(
      parent : Markd::Node,
      autolink : Bool,
    ) : String
      String.build do |io|
        child = parent.first_child?
        while child
          if child.type == Markd::Node::Type::Text
            text = String.build do |plain|
              while child && child.type == Markd::Node::Type::Text
                plain << child.text
                child = child.next?
              end
            end
            io << autolink_text(text, autolink)
          else
            io << render_inline(child, autolink)
            child = child.next?
          end
        end
      end
    end

    private def render_link(node : Markd::Node) : String
      url = node.data["destination"]?.try(&.as?(String)) || ""
      contents = render_inline_children(node, false)
      return contents unless safe_link?(url)

      %(<a href="#{escape(url)}">#{contents}</a>)
    end

    private def render_image(node : Markd::Node) : String
      url = node.data["destination"]?.try(&.as?(String)) || ""
      contents = render_inline_children(node, false)
      return "Image: #{contents}" unless safe_link?(url)

      %(Image: <a href="#{escape(url)}">#{contents}</a>)
    end

    private def render_list(
      node : Markd::Node,
      depth : Int32,
    ) : String
      ordered = node.data["type"]?.try(&.as?(String)) == "ordered"
      number = ordered ? (
        node.data["start"]?.try(&.as?(Int32)) || 1
      ) : 1
      rows = [] of String
      item = node.first_child?

      while item
        contents = render_block_children(item, depth + 1, "\n")
        unless contents.empty?
          indent = " " * (depth * 2)
          marker = ordered ? "#{number}. " : "• "
          first = indent + marker
          rest = " " * (indent.bytesize + marker.size)
          rows << prefix_lines(contents, first, rest)
        end
        number += 1
        item = item.next?
      end
      rows.join('\n')
    end

    private def prefix_lines(
      text : String,
      first : String,
      rest : String,
    ) : String
      String.build do |io|
        io << first
        text.each_char do |char|
          io << char
          io << rest if char == '\n'
        end
      end
    end

    private def render_table(
      paragraph : Markd::Node,
      markup : Bool,
    ) : String?
      plain = inline_plain_children(paragraph)
      lines = plain.split('\n')
      return if lines.size < 2

      header = split_table_row(lines[0])
      delimiter = split_table_row(lines[1])
      return unless header && delimiter
      return if delimiter.empty? || header.size != delimiter.size
      return unless delimiter.all? { |cell| table_delimiter?(cell) }

      rows = [header]
      lines.skip(2).each do |line|
        row = split_table_row(line)
        return unless row && row.size == delimiter.size
        rows << row
      end

      widths = Array.new(delimiter.size, 0)
      rows.each do |row|
        row.each_with_index do |cell, column|
          widths[column] = Math.max(widths[column], cell.size)
        end
      end

      String.build do |io|
        io << "<tt>" if markup
        rows.each_with_index do |row, row_index|
          io << '\n' if row_index > 0
          io << "<b>" if row_index == 0 && markup
          row.each_with_index do |cell, column|
            io << " │ " if column > 0
            io << (markup ? escape(cell) : cell)
            io << (" " * (widths[column] - cell.size))
          end
          next unless row_index == 0

          io << "</b>" if markup
          io << '\n'
          widths.each_with_index do |width, column|
            io << "─┼─" if column > 0
            io << ("─" * width)
          end
        end
        io << "</tt>" if markup
      end
    end

    private def inline_plain_children(parent : Markd::Node) : String
      String.build do |io|
        child = parent.first_child?
        while child
          io << inline_plain(child)
          child = child.next?
        end
      end
    end

    private def inline_plain(node : Markd::Node) : String
      case node.type
      when Markd::Node::Type::Text,
           Markd::Node::Type::Code,
           Markd::Node::Type::HTMLInline
        node.text
      when Markd::Node::Type::SoftBreak,
           Markd::Node::Type::LineBreak
        "\n"
      else
        inline_plain_children(node)
      end
    end

    private def split_table_row(line : String) : Array(String)?
      contents = line.strip
      return unless contents.includes?('|')

      contents = contents[1..] if contents.starts_with?('|')
      contents = contents[...-1] if contents.ends_with?('|')
      contents.split('|').map(&.strip)
    end

    private def table_delimiter?(cell : String) : Bool
      !!cell.match(/\A:?-{3,}:?\z/)
    end

    private def autolink_text(text : String, enabled : Bool) : String
      String.build do |io|
        offset = 0
        plain_start = 0
        while offset < text.bytesize
          if enabled && starts_url_at?(text, offset)
            length = url_length(text, offset)
            if length > "http://".bytesize
              append_escaped(io, text, plain_start, offset - plain_start)
              io << %(<a href=")
              append_escaped(io, text, offset, length)
              io << %(">)
              append_escaped(io, text, offset, length)
              io << "</a>"
              offset += length
              plain_start = offset
              next
            end
          end

          offset += 1
        end
        append_escaped(io, text, plain_start, offset - plain_start)
      end
    end

    private def starts_url?(text : String) : Bool
      text.starts_with?("https://") || text.starts_with?("http://")
    end

    private def starts_url_at?(text : String, offset : Int32) : Bool
      starts_with_at?(text, offset, "https://") ||
        starts_with_at?(text, offset, "http://")
    end

    private def starts_with_at?(
      text : String,
      offset : Int32,
      prefix : String,
    ) : Bool
      return false if offset + prefix.bytesize > text.bytesize
      text.to_slice[offset, prefix.bytesize] == prefix.to_slice
    end

    private def safe_link?(url : String) : Bool
      starts_url?(url) || url.starts_with?("mailto:")
    end

    private def url_length(text : String, start : Int32) : Int32
      length = 0
      while start + length < text.bytesize
        byte = text.byte_at(start + length)
        break if ascii_whitespace?(byte)
        break if {'<', '>', '"', '\''}.includes?(byte.chr)

        length += 1
      end

      while length > 0
        last = text.byte_at(start + length - 1).chr
        if {'.', ',', ';', ':', '!', '?'}.includes?(last)
          length -= 1
          next
        end

        if (last == ')' &&
           count_byte(text, start, length, ')') >
             count_byte(text, start, length, '(')) ||
           (last == ']' &&
           count_byte(text, start, length, ']') >
             count_byte(text, start, length, '[')) ||
           (last == '}' &&
           count_byte(text, start, length, '}') >
             count_byte(text, start, length, '{'))
          length -= 1
          next
        end
        break
      end
      length
    end

    private def count_byte(
      text : String,
      start : Int32,
      length : Int32,
      wanted : Char,
    ) : Int32
      count = 0
      length.times do |index|
        count += 1 if text.byte_at(start + index) == wanted.ord
      end
      count
    end

    private def append_escaped(
      io : String::Builder,
      text : String,
      start : Int32,
      length : Int32,
    ) : Nil
      return if length <= 0

      finish = start + length
      segment = start
      position = start
      while position < finish
        entity = case text.byte_at(position)
                 when '&'.ord  then "&amp;"
                 when '<'.ord  then "&lt;"
                 when '>'.ord  then "&gt;"
                 when '"'.ord  then "&quot;"
                 when '\''.ord then "&apos;"
                 else               nil
                 end
        if entity
          io.write(text.to_slice[segment, position - segment]) if position > segment
          io << entity
          segment = position + 1
        end
        position += 1
      end
      io.write(text.to_slice[segment, finish - segment]) if segment < finish
    end

    private def ascii_whitespace?(byte : UInt8) : Bool
      byte == 0x09 || byte == 0x0a || byte == 0x0b ||
        byte == 0x0c || byte == 0x0d || byte == 0x20
    end

    private def chomp_one_newline(text : String) : String
      text.ends_with?('\n') ? text.byte_slice(0, text.bytesize - 1) : text
    end

    private def escape(text : String) : String
      String.build do |io|
        text.each_char do |char|
          case char
          when '&'  then io << "&amp;"
          when '<'  then io << "&lt;"
          when '>'  then io << "&gt;"
          when '"'  then io << "&quot;"
          when '\'' then io << "&apos;"
          else           io << char
          end
        end
      end
    end

    private def valid_markup?(markup : String) : Bool
      check = markup.gsub(/<a href="[^"]*">|<\/a>/, "")
      Pango.parse_markup(check, -1, '\0')
    rescue Pango::PangoError
      false
    end
  end
end
