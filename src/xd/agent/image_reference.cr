module Xd
  module Agent
    class ImageReference
      getter remainder : String
      getter paths : Array(String)

      private def initialize(
        @remainder : String,
        @paths : Array(String),
      )
      end

      def self.parse(text : String) : self?
        paths = [] of String
        prose = [] of String

        text.split('\n').each do |raw_line|
          line = raw_line.ends_with?('\r') ? raw_line[..-2] : raw_line
          match = line.match(/\A\[image: (.+)\]\z/)
          if match
            paths << match[1]
          else
            prose << line
          end
        end

        return if paths.empty?
        new(prose.join('\n').strip, paths)
      end
    end
  end
end
