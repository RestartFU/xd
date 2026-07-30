require "../storage/messages"

module Xd
  module Agent
    module Conversation
      extend self

      HANDOVER_LIMIT_BYTES = 12_000
      TITLE_LENGTH         =     48
      HANDOVER_OPEN        =
        "[Part of this conversation happened with a different assistant, " \
        "so you have not seen it. It is reproduced below verbatim. Treat it " \
        "as part of the conversation you are already in: continue from it, " \
        "and do not greet the user again or re-introduce yourself.]"
      HANDOVER_CLOSE =
        "[End of earlier conversation. The user's new message follows.]"

      def handover(
        store : Storage::Store,
        chat_id : String,
        last_seen : Int64,
      ) : String?
        messages = store.list_messages_since(chat_id, last_seen)
        return nil if messages.size < 2

        previous = messages[0...(messages.size - 1)].select do |message|
          message.role == "user" || message.role == "assistant"
        end
        return nil if previous.empty?

        first = previous.size
        budget = 0
        while first > 0
          message = previous[first - 1]
          budget += message.content.bytesize + 16
          break if budget > HANDOVER_LIMIT_BYTES
          first -= 1
        end

        transcript = String.build do |text|
          text << HANDOVER_OPEN << "\n\n"
          previous[first...].each do |message|
            who = message.role == "user" ? "User" : "Assistant"
            text << who << ": " << message.content << "\n\n"
          end
          text << HANDOVER_CLOSE
        end
        transcript
      end

      def join(handover : String?, prompt : String) : String
        return prompt unless handover && !handover.empty?

        command?(prompt) ? "#{prompt}\n\n#{handover}" : "#{handover}\n\n#{prompt}"
      end

      def title(prompt : String) : String?
        first_line = prompt.lines.first?.try(&.strip) || ""
        return nil if first_line.empty?

        characters = first_line.chars
        characters.size > TITLE_LENGTH ? "#{characters.first(TITLE_LENGTH).join}…" : first_line
      end

      private def command?(prompt : String) : Bool
        match = /\A\/[[:alnum:]_:-]+(?:\s|\z)/.match(prompt)
        !match.nil?
      end
    end
  end
end
