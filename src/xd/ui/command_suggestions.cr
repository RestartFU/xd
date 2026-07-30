module Xd
  module UI
    module CommandSuggestions
      extend self

      def matches(commands : Array(String), text : String) : Array(String)
        return [] of String unless text.starts_with?('/')

        query = text[1..]
        return [] of String if query.each_char.any?(&.ascii_whitespace?)

        lowered = query.downcase
        commands.select do |command|
          lowered.empty? || command.downcase.starts_with?(lowered)
        end
      end
    end
  end
end
