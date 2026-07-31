require "json"

module Xd
  module UI
    module CommandSuggestions
      extend self

      MAX_COMMANDS = 200
      MAX_MATCHES  =  40

      def normalize(nodes : Array(JSON::Any)) : Array(String)
        visible = Math.min(nodes.size, MAX_COMMANDS)
        commands = Array(String).new(visible)
        visible.times do |index|
          command = nodes[index].as_s?.try(&.lchop("/"))
          commands << command unless command.nil? || command.empty?
        end
        commands
      end

      def matches(commands : Array(String), text : String) : Array(String)
        return [] of String unless text.starts_with?('/')

        query = text[1..]
        return [] of String if query.each_char.any?(&.ascii_whitespace?)

        lowered = query.downcase
        matches = [] of String
        commands.first(MAX_COMMANDS).each do |command|
          next unless lowered.empty? ||
                      command.downcase.starts_with?(lowered)

          matches << command
          break if matches.size >= MAX_MATCHES
        end
        matches
      end
    end
  end
end
