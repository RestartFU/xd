module Xd
  module Agent
    class ImageReference
      record Part,
        text : String?,
        path : String?

      getter remainder : String
      getter paths : Array(String)
      getter parts : Array(Part)

      private def initialize(
        @remainder : String,
        @paths : Array(String),
        @parts : Array(Part),
      )
      end

      def self.parse(text : String) : self?
        paths = [] of String
        parts = [] of Part
        prose = ""

        text.split('\n').each do |raw_line|
          line = raw_line.ends_with?('\r') ? raw_line[..-2] : raw_line
          match = line.match(/\A\[image: (.+)\]\z/)
          if match
            unless prose.empty?
              parts << Part.new(prose, nil)
              prose = ""
            end
            path = match[1]
            paths << path
            parts << Part.new(nil, path)
          else
            prose += '\n' unless prose.empty?
            prose += line
          end
        end

        return if paths.empty?
        parts << Part.new(prose, nil) unless prose.empty?
        remainder = parts.compact_map(&.text).join('\n').strip
        new(remainder, paths, parts)
      end
    end
  end
end
